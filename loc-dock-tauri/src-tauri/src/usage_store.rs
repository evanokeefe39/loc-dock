use crate::source_adapter::{FileDiscoverer, SourceKind, SourceManager};
use crate::types::{CostBreakdown, SourceStats, TokenTotals};
use chrono::{DateTime, Utc};
use duckdb::Connection;
use log::{info, warn};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const RETENTION_DAYS: f64 = 7.0;
const DAY_SECS: f64 = 86400.0;
const DB_NAME: &str = "usage_cache.db";
const MARKER: &str = "usage_cache.db.reset";
const SCHEMA_VERSION: &str = "6";  // V2: SQL silver ingest, no Rust parsers / file_tracker

/// Files ingested per silver INSERT. Caps transient JSON-parse memory on cold
/// rebuilds (Spike 4b: 16 files + 512 MB guard rail peaks ~150 MB).
const INGEST_BATCH_FILES: usize = 16;
const MAX_OBJECT_SIZE: u64 = 64 * 1024 * 1024;  // 64 MB; default 16 MB too small (edge case)

// ponytail: kept for future use; per-source ETL in data loop doesn't need this
#[allow(dead_code)]
#[derive(Default, Debug)]
pub struct EtlResult {
    pub total_entries: usize,
    pub claude_new: usize,
    pub pi_new: usize,
}

/// File-backed DuckDB store that ingests normalized session usage entries
/// from multiple sources (Claude, Pi) via a single-phase ETL pipeline:
///
/// 1. **File discovery** finds all session files within the 7-day retention window
/// 2. **Parsing** extracts entries from JSONL files
/// 3. **INSERT OR IGNORE** with a UNIQUE constraint prevents duplicates
///
/// Query results are cached via `generation` counter — re-queried only when
/// the DB row count changes (PERF-003 + PERF-004).
pub struct UsageStore {
    con: Connection,
    source_manager: SourceManager,
    initialized: bool,
    /// Incremented when new rows are inserted; invalidates query cache.
    generation: u64,
    /// Row count at last check — used to detect new data.
    last_row_count: u64,
    /// Cached query results keyed by (query_type, since_str).
    /// Valid only when stored generation matches `self.generation`.
    cache: RefCell<QueryCache>,
}

#[derive(Default)]
struct QueryCache {
    token_totals: HashMap<String, (u64, TokenTotals)>,
    cost_breakdowns: HashMap<String, (u64, CostBreakdown)>,
    cost_timelines: HashMap<String, (u64, Vec<(f64, f64)>)>,
    token_timelines: HashMap<String, (u64, Vec<(f64, i64, i64, i64, i64)>)>,
    session_counts: HashMap<(String, String), (u64, (i64, i64))>,
    source_breakdowns: HashMap<(String, String), (u64, Vec<SourceStats>)>,
}

impl QueryCache {
    fn clear(&mut self) {
        self.token_totals.clear();
        self.cost_breakdowns.clear();
        self.cost_timelines.clear();
        self.token_timelines.clear();
        self.session_counts.clear();
        self.source_breakdowns.clear();
    }
}

impl UsageStore {
    pub fn new(source_manager: SourceManager, cache_dir: &Path) -> Self {
        let _ = std::fs::create_dir_all(cache_dir);

        let marker = cache_dir.join(MARKER);
        let db_path = cache_dir.join(DB_NAME);
        if marker.exists() {
            let _ = std::fs::remove_file(&db_path);
            let _ = std::fs::remove_file(&marker);
            info!("Usage cache reset via marker file");
        }

        let con = Connection::open(&db_path).expect("failed to open usage_cache.db");

        // Guard rails for the SQL JSON-ingest path (Spike 4b): the read_ndjson
        // parse path holds non-spillable buffers, so a tight memory_limit acts as
        // a high ceiling, not a spill trigger. 512 MB + micro-batching keeps cold
        // rebuilds bounded; preserve_insertion_order=false lowers peak further.
        let _ = con.execute_batch(
            "SET memory_limit='512MB'; SET preserve_insertion_order=false;",
        );

        // Ensure meta table exists (check schema version)
        con.execute_batch(
            "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        )
        .expect("failed to create meta table");

        // Check schema version — force reset if outdated
        let stored_ver: String = con
            .prepare("SELECT value FROM meta WHERE key = 'schema_version'")
            .ok()
            .and_then(|mut stmt| stmt.query_row([], |row| row.get::<_, String>(0)).ok())
            .unwrap_or_default();

        if stored_ver != SCHEMA_VERSION {
            let _ = con.execute_batch(
                "DROP TABLE IF EXISTS entries; \
                 DROP TABLE IF EXISTS watermarks; \
                 DROP TABLE IF EXISTS file_tracker;"
            );
            info!("Schema version {} → {}: reset tables", stored_ver, SCHEMA_VERSION);
        }

        // Ensure +E5 tables exist (/w correct schema)
        let _ = con.execute_batch(
            "CREATE TABLE IF NOT EXISTS entries (
                 source            TEXT NOT NULL,
                 session_id        TEXT NOT NULL,
                 ts                TIMESTAMP NOT NULL,
                 model             TEXT,
                 provider          TEXT,
                 role              TEXT,
                 input_tokens      BIGINT NOT NULL DEFAULT 0,
                 output_tokens     BIGINT NOT NULL DEFAULT 0,
                 cache_creation_input_tokens BIGINT NOT NULL DEFAULT 0,
                 cache_read_input_tokens     BIGINT NOT NULL DEFAULT 0,
                 input_cost        DOUBLE NOT NULL DEFAULT 0.0,
                 output_cost       DOUBLE NOT NULL DEFAULT 0.0,
                 cache_write_cost  DOUBLE NOT NULL DEFAULT 0.0,
                 cache_read_cost   DOUBLE NOT NULL DEFAULT 0.0,
                 total_cost        DOUBLE NOT NULL DEFAULT 0.0,
                 file_path         TEXT NOT NULL,
                 UNIQUE(source, session_id, ts, file_path)
             );
"
        );

        // v5: daily_aggregates — materialized per-date, per-source rollups
        // Pre-computed at ETL time for O(1) frontend queries.
        // loc_added/loc_deleted populated separately by git pipeline (Phase 2).
        let _ = con.execute_batch(
            "CREATE TABLE IF NOT EXISTS daily_aggregates (
                 date             DATE NOT NULL,
                 source           TEXT NOT NULL,
                 input_tokens     BIGINT NOT NULL DEFAULT 0,
                 output_tokens    BIGINT NOT NULL DEFAULT 0,
                 cache_creation_input_tokens BIGINT NOT NULL DEFAULT 0,
                 cache_read_input_tokens     BIGINT NOT NULL DEFAULT 0,
                 input_cost       DOUBLE NOT NULL DEFAULT 0.0,
                 output_cost      DOUBLE NOT NULL DEFAULT 0.0,
                 cache_write_cost DOUBLE NOT NULL DEFAULT 0.0,
                 cache_read_cost  DOUBLE NOT NULL DEFAULT 0.0,
                 total_cost       DOUBLE NOT NULL DEFAULT 0.0,
                 session_count    BIGINT NOT NULL DEFAULT 0,
                 loc_added        BIGINT NOT NULL DEFAULT 0,
                 loc_deleted      BIGINT NOT NULL DEFAULT 0,
                 UNIQUE(date, source)
             );
"
        );

        let row_count: i64 = con
            .prepare("SELECT COUNT(*) FROM entries")
            .and_then(|mut stmt| stmt.query_row([], |row| row.get(0)))
            .unwrap_or(0);
        let initialized = row_count > 0;

        if initialized {
            info!("Usage cache loaded from disk ({} rows)", row_count);
        }

        // Persist current schema version
        let _ = con.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', ?)",
            duckdb::params![SCHEMA_VERSION],
        );

        UsageStore {
            con,
            source_manager,
            initialized,
            generation: 0,
            last_row_count: row_count as u64,
            cache: RefCell::new(QueryCache::default()),
        }
    }

    pub fn reset(cache_dir: &Path) -> Result<(), String> {
        let marker = cache_dir.join(MARKER);
        std::fs::write(&marker, "").map_err(|e| e.to_string())
    }

    // ── ETL pipeline ───────────────────────────────────────────────────

    /// Source names (e.g. "claude", "pi") for per-source ETL iteration.
    pub fn source_names(&self) -> Vec<String> {
        self.source_manager.pairs.iter().map(|p| p.1.name().to_string()).collect()
    }

    /// Process one source's files. Returns number of entries inserted.
    /// Call this per-source so the UI can emit incremental updates.
    pub fn process_source_named(&mut self, name: &str) -> Result<usize, String> {
        let now_ts = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let cutoff = now_ts - RETENTION_DAYS * DAY_SECS;

        for pair in &self.source_manager.pairs {
            if pair.1.name() == name {
                let n = process_source(&self.con, &*pair.0, pair.1, cutoff)?;
                self.initialized = true;
                return Ok(n);
            }
        }
        Ok(0)
    }

    /// Finalize ETL: refresh aggregates, prune old data, bump generation.
    /// Call once after all sources have been processed.
    pub fn finalize_etl(&mut self) {
        let _ = self.con.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', ?)",
            duckdb::params![SCHEMA_VERSION],
        );

        let current_count: i64 = self.con
            .prepare("SELECT COUNT(*) FROM entries")
            .and_then(|mut stmt| stmt.query_row([], |row| row.get(0)))
            .unwrap_or(0);
        if (current_count as u64) > self.last_row_count {
            self.generation += 1;
            self.last_row_count = current_count as u64;
            self.cache.borrow_mut().clear();
            if let Err(e) = self.refresh_aggregates() {
                warn!("Failed to refresh aggregates: {}", e);
            }
        }

        let retention_cutoff = (Utc::now() - chrono::Duration::days(RETENTION_DAYS as i64))
            .format("%Y-%m-%d 00:00:00")
            .to_string();
        if let Err(e) = self.con.execute(
            "DELETE FROM entries WHERE ts < ?::TIMESTAMP",
            duckdb::params![retention_cutoff],
        ) {
            warn!("Retention prune entries: {}", e);
        }
        if let Err(e) = self.con.execute(
            "DELETE FROM daily_aggregates WHERE date < ?::DATE",
            duckdb::params![retention_cutoff],
        ) {
            warn!("Retention prune aggregates: {}", e);
        }
    }

    /// Run single-phase ETL over the 7-day retention window (all sources).
    /// Convenience wrapper — prefer per-source emits via process_source_named.
    #[allow(dead_code)]
    pub fn run_etl(&mut self) -> Result<EtlResult, String> {
        let names = self.source_names();
        let mut result = EtlResult::default();
        for name in &names {
            let n = self.process_source_named(name)?;
            if name == "claude" { result.claude_new += n; } else { result.pi_new += n; }
            result.total_entries += n;
        }
        self.finalize_etl();
        info!("ETL: processed {} entries (claude:{} pi:{})", result.total_entries, result.claude_new, result.pi_new);
        Ok(result)
    }
}

/// Process one source: discover files within the retention window, then ingest
/// them into the silver `entries` table via DuckDB SQL (bronze `read_ndjson_objects`
/// + per-source extraction). No Rust JSON parsing, no byte seek/tail tracking.
///
/// Files are micro-batched (Spike 4b) to cap the non-spillable JSON-parse memory
/// on cold rebuilds. Discovery stays in Rust so the read is bounded to the 7-day
/// window — globbing in DuckDB would read full history (Spike 4 memory regression).
fn process_source(
    con: &Connection,
    discoverer: &dyn FileDiscoverer,
    kind: SourceKind,
    cutoff: f64,
) -> Result<usize, String> {
    let (all_files, _max_mtime) = match discoverer.discover_files(cutoff) {
        Ok(r) => r,
        Err(e) => { warn!("ETL '{}': discover failed: {}", kind.name(), e); return Ok(0); }
    };

    if all_files.is_empty() {
        return Ok(0);
    }

    info!("ETL '{}': {} files in window (cutoff={:.0}h ago)", kind.name(), all_files.len(),
        (SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs_f64() - cutoff) / 3600.0
    );

    // Pi carry-forward (model_change → assistant rows) needs intra-file row order,
    // so Pi ingests one file per statement under preserve_insertion_order=true.
    // Claude has no window functions → larger batches, order-independent.
    let batch_files = match kind {
        SourceKind::Claude => INGEST_BATCH_FILES,
        SourceKind::Pi => 1,
    };

    let mut total = 0usize;
    for chunk in all_files.chunks(batch_files) {
        total += ingest_files(con, kind, chunk)?;
    }

    info!("ETL '{}': {} entries from {} files", kind.name(), total, all_files.len());
    Ok(total)
}

/// Build the bronze→silver SQL for a batch of files and execute it.
/// Returns the number of rows inserted (INSERT OR IGNORE skips duplicates).
fn ingest_files(con: &Connection, kind: SourceKind, paths: &[PathBuf]) -> Result<usize, String> {
    if paths.is_empty() {
        return Ok(0);
    }
    let paths_array = paths_to_sql_array(paths);
    let sql = match kind {
        SourceKind::Claude => claude_silver_sql(&paths_array),
        SourceKind::Pi => pi_silver_sql(&paths_array),
    };

    if kind == SourceKind::Pi {
        // Carry-forward relies on file line order from the scan.
        let _ = con.execute_batch("SET preserve_insertion_order=true;");
    }
    let result = con.execute(&sql, []);
    if kind == SourceKind::Pi {
        let _ = con.execute_batch("SET preserve_insertion_order=false;");
    }

    match result {
        Ok(n) => Ok(n),
        Err(e) => { warn!("ETL '{}': ingest insert: {}", kind.name(), e); Ok(0) }
    }
}

/// Render a slice of paths as a DuckDB array literal: `'a/b.jsonl','c/d.jsonl'`.
/// Backslashes are normalized to `/` (DuckDB accepts `/` on Windows and the
/// `entries.file_path` key is stored forward-slash for cross-platform stability).
/// Single quotes are doubled per SQL string-literal escaping.
fn paths_to_sql_array(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| {
            let s = p.to_string_lossy().replace('\\', "/").replace('\'', "''");
            format!("'{}'", s)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Claude silver extraction. Assistant messages carry usage; cost is flat-priced
/// in SQL. The `input>0 OR output>0` guard replicates `fill_costs` exactly (incl.
/// its cache-only-row zero-cost gap — a latent bug kept for parity; see plan).
fn claude_silver_sql(paths_array: &str) -> String {
    let ip = crate::pricing::INPUT_PRICE;
    let op = crate::pricing::OUTPUT_PRICE;
    let cwp = crate::pricing::CACHE_WRITE_PRICE;
    let crp = crate::pricing::CACHE_READ_PRICE;
    format!(
        "INSERT OR IGNORE INTO entries
           (source, session_id, ts, model, provider, role,
            input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens,
            input_cost, output_cost, cache_write_cost, cache_read_cost, total_cost, file_path)
         WITH bronze AS (
           SELECT json AS j, replace(filename, '\\', '/') AS file_path
           FROM read_ndjson_objects([{paths}],
                  filename = true, ignore_errors = true, maximum_object_size = {mos})
         ),
         ex AS (
           SELECT
             COALESCE(json_extract_string(j, '$.sessionId'),
                      regexp_extract(file_path, '([^/]+)\\.jsonl$', 1)) AS session_id,
             TRY_CAST(json_extract_string(j, '$.timestamp') AS TIMESTAMP) AS ts,
             json_extract_string(j, '$.message.model') AS model,
             COALESCE(TRY_CAST(json_extract_string(j, '$.message.usage.input_tokens')  AS BIGINT), 0) AS input_tokens,
             COALESCE(TRY_CAST(json_extract_string(j, '$.message.usage.output_tokens') AS BIGINT), 0) AS output_tokens,
             COALESCE(TRY_CAST(json_extract_string(j, '$.message.usage.cache_creation_input_tokens') AS BIGINT), 0) AS cache_creation_input_tokens,
             COALESCE(TRY_CAST(json_extract_string(j, '$.message.usage.cache_read_input_tokens')     AS BIGINT), 0) AS cache_read_input_tokens,
             file_path
           FROM bronze
           WHERE json_extract_string(j, '$.type') = 'assistant'
             AND json_extract(j, '$.message.usage') IS NOT NULL
         )
         SELECT
           'claude', session_id, ts, model, 'anthropic', 'assistant',
           input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens,
           CASE WHEN input_tokens > 0 OR output_tokens > 0 THEN input_tokens / 1e6 * {ip} ELSE 0 END,
           CASE WHEN input_tokens > 0 OR output_tokens > 0 THEN output_tokens / 1e6 * {op} ELSE 0 END,
           CASE WHEN input_tokens > 0 OR output_tokens > 0 THEN cache_creation_input_tokens / 1e6 * {cwp} ELSE 0 END,
           CASE WHEN input_tokens > 0 OR output_tokens > 0 THEN cache_read_input_tokens / 1e6 * {crp} ELSE 0 END,
           CASE WHEN input_tokens > 0 OR output_tokens > 0
                THEN input_tokens / 1e6 * {ip} + output_tokens / 1e6 * {op}
                   + cache_creation_input_tokens / 1e6 * {cwp} + cache_read_input_tokens / 1e6 * {crp}
                ELSE 0 END,
           file_path
         FROM ex
         WHERE ts IS NOT NULL",
        paths = paths_array, mos = MAX_OBJECT_SIZE, ip = ip, op = op, cwp = cwp, crp = crp,
    )
}

/// Pi silver extraction. Pi uses camelCase token fields and carries its own cost
/// nested under `usage.cost`. `model_change` events set the active model/provider,
/// carried forward to subsequent assistant rows via a window LAST_VALUE. Flat
/// pricing is applied only when Pi supplied no total cost (matches `fill_costs`).
fn pi_silver_sql(paths_array: &str) -> String {
    let ip = crate::pricing::INPUT_PRICE;
    let op = crate::pricing::OUTPUT_PRICE;
    let cwp = crate::pricing::CACHE_WRITE_PRICE;
    let crp = crate::pricing::CACHE_READ_PRICE;
    format!(
        "INSERT OR IGNORE INTO entries
           (source, session_id, ts, model, provider, role,
            input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens,
            input_cost, output_cost, cache_write_cost, cache_read_cost, total_cost, file_path)
         WITH bronze AS (
           SELECT json AS j, replace(filename, '\\', '/') AS file_path,
                  row_number() OVER () AS rn
           FROM read_ndjson_objects([{paths}],
                  filename = true, ignore_errors = true, maximum_object_size = {mos})
         ),
         carried AS (
           SELECT *,
             LAST_VALUE(CASE WHEN json_extract_string(j, '$.type') = 'model_change'
                             THEN json_extract_string(j, '$.modelId') END IGNORE NULLS)
               OVER (ORDER BY rn ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS carried_model,
             LAST_VALUE(CASE WHEN json_extract_string(j, '$.type') = 'model_change'
                             THEN json_extract_string(j, '$.provider') END IGNORE NULLS)
               OVER (ORDER BY rn ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS carried_provider
           FROM bronze
         ),
         ex AS (
           SELECT
             split_part(regexp_extract(file_path, '([^/]+)\\.jsonl$', 1), '_', 2) AS session_id,
             COALESCE(
               CASE WHEN TRY_CAST(json_extract_string(j, '$.message.timestamp') AS BIGINT) IS NOT NULL
                    THEN to_timestamp(TRY_CAST(json_extract_string(j, '$.message.timestamp') AS BIGINT) / 1000.0) AT TIME ZONE 'UTC'
               END,
               TRY_CAST(json_extract_string(j, '$.timestamp') AS TIMESTAMP)
             ) AS ts,
             COALESCE(json_extract_string(j, '$.message.model'), carried_model) AS model,
             COALESCE(json_extract_string(j, '$.message.provider'), carried_provider) AS provider,
             COALESCE(TRY_CAST(json_extract_string(j, '$.message.usage.input')      AS BIGINT), 0) AS input_tokens,
             COALESCE(TRY_CAST(json_extract_string(j, '$.message.usage.output')     AS BIGINT), 0) AS output_tokens,
             COALESCE(TRY_CAST(json_extract_string(j, '$.message.usage.cacheWrite') AS BIGINT), 0) AS cache_creation_input_tokens,
             COALESCE(TRY_CAST(json_extract_string(j, '$.message.usage.cacheRead')  AS BIGINT), 0) AS cache_read_input_tokens,
             COALESCE(TRY_CAST(json_extract_string(j, '$.message.usage.cost.input')      AS DOUBLE), 0) AS p_input_cost,
             COALESCE(TRY_CAST(json_extract_string(j, '$.message.usage.cost.output')     AS DOUBLE), 0) AS p_output_cost,
             COALESCE(TRY_CAST(json_extract_string(j, '$.message.usage.cost.cacheWrite') AS DOUBLE), 0) AS p_cache_write_cost,
             COALESCE(TRY_CAST(json_extract_string(j, '$.message.usage.cost.cacheRead')  AS DOUBLE), 0) AS p_cache_read_cost,
             COALESCE(TRY_CAST(json_extract_string(j, '$.message.usage.cost.total')      AS DOUBLE), 0) AS p_total_cost,
             file_path
           FROM carried
           WHERE json_extract_string(j, '$.type') = 'message'
             AND json_extract_string(j, '$.message.role') = 'assistant'
             AND json_extract(j, '$.message.usage') IS NOT NULL
         )
         SELECT
           'pi', session_id, ts, model, provider, 'assistant',
           input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens,
           CASE WHEN p_total_cost = 0 AND (input_tokens > 0 OR output_tokens > 0)
                THEN input_tokens / 1e6 * {ip} ELSE p_input_cost END,
           CASE WHEN p_total_cost = 0 AND (input_tokens > 0 OR output_tokens > 0)
                THEN output_tokens / 1e6 * {op} ELSE p_output_cost END,
           CASE WHEN p_total_cost = 0 AND (input_tokens > 0 OR output_tokens > 0)
                THEN cache_creation_input_tokens / 1e6 * {cwp} ELSE p_cache_write_cost END,
           CASE WHEN p_total_cost = 0 AND (input_tokens > 0 OR output_tokens > 0)
                THEN cache_read_input_tokens / 1e6 * {crp} ELSE p_cache_read_cost END,
           CASE WHEN p_total_cost = 0 AND (input_tokens > 0 OR output_tokens > 0)
                THEN input_tokens / 1e6 * {ip} + output_tokens / 1e6 * {op}
                   + cache_creation_input_tokens / 1e6 * {cwp} + cache_read_input_tokens / 1e6 * {crp}
                ELSE p_total_cost END,
           file_path
         FROM ex
         WHERE ts IS NOT NULL",
        paths = paths_array, mos = MAX_OBJECT_SIZE, ip = ip, op = op, cwp = cwp, crp = crp,
    )
}

impl UsageStore {

        /// Recompute daily_aggregates from the entries table.
    /// Called after each ETL cycle that inserted new rows.
    /// Uses REPLACE semantics (ON CONFLICT DO UPDATE SET = EXCLUDED.*)
    /// since this is a full recompute of the retention window.
    fn refresh_aggregates(&self) -> Result<(), String> {
        let now_ts = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let cutoff = now_ts - RETENTION_DAYS * DAY_SECS;
        let cutoff_dt = DateTime::from_timestamp(cutoff as i64, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| {
                "2000-01-01 00:00:00".to_string()
            });

        self.con.execute_batch(
            &format!(
                "INSERT INTO daily_aggregates (
                     date, source,
                     input_tokens, output_tokens,
                     cache_creation_input_tokens, cache_read_input_tokens,
                     input_cost, output_cost, cache_write_cost, cache_read_cost,
                     total_cost, session_count,
                     loc_added, loc_deleted
                 )
                 SELECT
                     date_trunc('day', ts)::DATE AS date,
                     source,
                     COALESCE(SUM(input_tokens), 0)::BIGINT,
                     COALESCE(SUM(output_tokens), 0)::BIGINT,
                     COALESCE(SUM(cache_creation_input_tokens), 0)::BIGINT,
                     COALESCE(SUM(cache_read_input_tokens), 0)::BIGINT,
                     COALESCE(SUM(input_cost), 0),
                     COALESCE(SUM(output_cost), 0),
                     COALESCE(SUM(cache_write_cost), 0),
                     COALESCE(SUM(cache_read_cost), 0),
                     COALESCE(SUM(total_cost), 0),
                     COUNT(DISTINCT session_id)::BIGINT,
                     0, 0  -- loc_added/loc_deleted from git pipeline
                 FROM entries
                 WHERE ts >= '{}'::TIMESTAMP
                 GROUP BY date_trunc('day', ts)::DATE, source
                 ON CONFLICT(date, source) DO UPDATE SET
                     input_tokens = EXCLUDED.input_tokens,
                     output_tokens = EXCLUDED.output_tokens,
                     cache_creation_input_tokens = EXCLUDED.cache_creation_input_tokens,
                     cache_read_input_tokens = EXCLUDED.cache_read_input_tokens,
                     input_cost = EXCLUDED.input_cost,
                     output_cost = EXCLUDED.output_cost,
                     cache_write_cost = EXCLUDED.cache_write_cost,
                     cache_read_cost = EXCLUDED.cache_read_cost,
                     total_cost = EXCLUDED.total_cost,
                     session_count = EXCLUDED.session_count
                 ",
                cutoff_dt
            )
        )
        .map_err(|e| format!("Refresh aggregates: {}", e))?;

        Ok(())
    }

// ── Query methods ─────────────────────────────────────────────────

    /// Query aggregates for a date range. Returns (total_cost, CostBreakdown, TokenTotals, sessions).
    /// Unlike query_since (which scans entries table), this reads from pre-computed daily_aggregates.
    /// O(number of days in range) vs O(number of entries in range).
    pub fn query_aggregates(&self, since_str: &str) -> (f64, CostBreakdown, TokenTotals, i64) {
        if !self.initialized {
            return (0.0, CostBreakdown::default(), TokenTotals::default(), 0);
        }
        let result = match self.con.prepare(
            "SELECT
                COALESCE(SUM(total_cost), 0),
                COALESCE(SUM(input_cost), 0),
                COALESCE(SUM(output_cost), 0),
                COALESCE(SUM(cache_write_cost), 0),
                COALESCE(SUM(cache_read_cost), 0),
                COALESCE(SUM(input_tokens), 0)::BIGINT,
                COALESCE(SUM(output_tokens), 0)::BIGINT,
                COALESCE(SUM(cache_creation_input_tokens), 0)::BIGINT,
                COALESCE(SUM(cache_read_input_tokens), 0)::BIGINT,
                COALESCE(SUM(session_count), 0)::BIGINT
            FROM daily_aggregates WHERE date >= ?::DATE",
        ) {
            Ok(mut stmt) => match stmt.query_row([since_str], |row| {
                Ok((
                    row.get::<_, f64>(0)?,
                    CostBreakdown {
                        input: row.get(1)?,
                        output: row.get(2)?,
                        cache_write: row.get(3)?,
                        cache_read: row.get(4)?,
                    },
                    TokenTotals {
                        input_tokens: row.get(5)?,
                        output_tokens: row.get(6)?,
                        cache_creation_input_tokens: row.get(7)?,
                        cache_read_input_tokens: row.get(8)?,
                    },
                    row.get::<_, i64>(9)?,
                ))
            }) {
                Ok(r) => r,
                Err(e) => {
                    warn!("query_aggregates failed: {}", e);
                    (0.0, CostBreakdown::default(), TokenTotals::default(), 0)
                }
            },
            Err(e) => {
                warn!("query_aggregates prepare: {}", e);
                (0.0, CostBreakdown::default(), TokenTotals::default(), 0)
            }
        };
        result
    }

    /// Query source breakdown from daily_aggregates.
    pub fn query_aggregate_source_breakdown(&self, since_str: &str) -> Vec<SourceStats> {
        if !self.initialized {
            return Vec::new();
        }
        match self.con.prepare(
            "SELECT
                source,
                COALESCE(SUM(session_count), 0)::BIGINT,
                COALESCE(SUM(input_tokens), 0)::BIGINT,
                COALESCE(SUM(output_tokens), 0)::BIGINT,
                COALESCE(SUM(cache_creation_input_tokens), 0)::BIGINT,
                COALESCE(SUM(cache_read_input_tokens), 0)::BIGINT,
                COALESCE(SUM(total_cost), 0)
            FROM daily_aggregates WHERE date >= ?::DATE
            GROUP BY source ORDER BY source",
        ) {
            Ok(mut stmt) => match stmt.query_map([since_str], |row| {
                Ok(SourceStats {
                    source: row.get(0)?,
                    sessions_total: row.get(1)?,
                    sessions_active: 0,  // active sessions will be handled separately
                    tokens: TokenTotals {
                        input_tokens: row.get(2)?,
                        output_tokens: row.get(3)?,
                        cache_creation_input_tokens: row.get(4)?,
                        cache_read_input_tokens: row.get(5)?,
                    },
                    cost_total: row.get(6)?,
                })
            }) {
                Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
                Err(e) => {
                    warn!("aggregate source breakdown: {}", e);
                    Vec::new()
                }
            },
            Err(e) => {
                warn!("aggregate source breakdown prepare: {}", e);
                Vec::new()
            }
        }
    }

    #[allow(dead_code)] // ponytail: unused since build_all_stats switched to query_aggregates
    pub fn query_since(&self, since_str: &str) -> TokenTotals {
        if !self.initialized {
            return TokenTotals::default();
        }
        let gen = self.generation;
        {
            let cache = self.cache.borrow();
            if let Some((cg, val)) = cache.token_totals.get(since_str) {
                if *cg == gen { return val.clone(); }
            }
        }
        let key = since_str.to_string();
        let result = match self.con.prepare(
            "SELECT
                COALESCE(SUM(input_tokens), 0)::BIGINT,
                COALESCE(SUM(output_tokens), 0)::BIGINT,
                COALESCE(SUM(cache_creation_input_tokens), 0)::BIGINT,
                COALESCE(SUM(cache_read_input_tokens), 0)::BIGINT
            FROM entries WHERE ts >= ?::TIMESTAMP",
        ) {
            Ok(mut stmt) => match stmt.query_row([since_str], |row| {
                Ok(TokenTotals {
                    input_tokens: row.get(0)?,
                    output_tokens: row.get(1)?,
                    cache_creation_input_tokens: row.get(2)?,
                    cache_read_input_tokens: row.get(3)?,
                })
            }) {
                Ok(t) => t,
                Err(e) => { warn!("query_since failed: {}", e); TokenTotals::default() }
            },
            Err(e) => { warn!("query_since prepare: {}", e); TokenTotals::default() }
        };
        self.cache.borrow_mut().token_totals.insert(key, (gen, result.clone()));
        result
    }

    pub fn query_cost_timeline(&self, since_str: &str) -> Vec<(f64, f64)> {
        if !self.initialized { return Vec::new(); }
        let gen = self.generation;
        {
            let cache = self.cache.borrow();
            if let Some((cg, val)) = cache.cost_timelines.get(since_str) {
                if *cg == gen { return val.clone(); }
            }
        }
        let key = since_str.to_string();
        let result: Vec<(f64, f64)> = match self.con.prepare(
            "SELECT epoch(ts)::DOUBLE, total_cost FROM entries WHERE ts >= ?::TIMESTAMP ORDER BY ts",
        ) {
            Ok(mut stmt) => match stmt.query_map([since_str], |row| {
                Ok((row.get::<_, f64>(0)?, row.get::<_, f64>(1)?))
            }) {
                Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
                Err(e) => { warn!("Cost timeline: {}", e); Vec::new() }
            },
            Err(e) => { warn!("Cost timeline prepare: {}", e); Vec::new() }
        };
        self.cache.borrow_mut().cost_timelines.insert(key, (gen, result.clone()));
        result
    }

    #[allow(dead_code)] // ponytail: unused since build_all_stats switched to query_aggregates
    pub fn query_cost_breakdown(&self, since_str: &str) -> CostBreakdown {
        if !self.initialized { return CostBreakdown::default(); }
        let gen = self.generation;
        {
            let cache = self.cache.borrow();
            if let Some((cg, val)) = cache.cost_breakdowns.get(since_str) {
                if *cg == gen { return val.clone(); }
            }
        }
        let key = since_str.to_string();
        let result = match self.con.prepare(
            "SELECT
                COALESCE(SUM(input_cost), 0),
                COALESCE(SUM(output_cost), 0),
                COALESCE(SUM(cache_write_cost), 0),
                COALESCE(SUM(cache_read_cost), 0)
            FROM entries WHERE ts >= ?::TIMESTAMP",
        ) {
            Ok(mut stmt) => match stmt.query_row([since_str], |row| {
                Ok(CostBreakdown {
                    input: row.get(0)?,
                    output: row.get(1)?,
                    cache_write: row.get(2)?,
                    cache_read: row.get(3)?,
                })
            }) {
                Ok(cb) => cb,
                Err(e) => { warn!("Cost breakdown: {}", e); CostBreakdown::default() }
            },
            Err(e) => { warn!("Cost breakdown prepare: {}", e); CostBreakdown::default() }
        };
        self.cache.borrow_mut().cost_breakdowns.insert(key, (gen, result.clone()));
        result
    }

    pub fn query_token_timeline(&self, since_str: &str) -> Vec<(f64, i64, i64, i64, i64)> {
        if !self.initialized { return Vec::new(); }
        let gen = self.generation;
        {
            let cache = self.cache.borrow();
            if let Some((cg, val)) = cache.token_timelines.get(since_str) {
                if *cg == gen { return val.clone(); }
            }
        }
        let key = since_str.to_string();
        let result = match self.con.prepare(
            "SELECT epoch(ts)::DOUBLE, input_tokens, output_tokens,
                    cache_creation_input_tokens, cache_read_input_tokens
             FROM entries WHERE ts >= ?::TIMESTAMP ORDER BY ts",
        ) {
            Ok(mut stmt) => match stmt.query_map([since_str], |row| {
                Ok((
                    row.get::<_, f64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            }) {
                Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
                Err(e) => { warn!("Token timeline: {}", e); Vec::new() }
            },
            Err(e) => { warn!("Token timeline prepare: {}", e); Vec::new() }
        };
        self.cache.borrow_mut().token_timelines.insert(key, (gen, result.clone()));
        result
    }

    pub fn count_sessions(&self, since_str: &str, active_str: &str) -> (i64, i64) {
        if !self.initialized { return (0, 0); }
        let gen = self.generation;
        let cache_key = (since_str.to_string(), active_str.to_string());
        {
            let cache = self.cache.borrow();
            if let Some((cg, val)) = cache.session_counts.get(&cache_key) {
                if *cg == gen { return *val; }
            }
        }
        let result = match self.con.prepare(
            "SELECT
                COUNT(DISTINCT session_id) FILTER (WHERE ts >= ?::TIMESTAMP),
                COUNT(DISTINCT session_id) FILTER (WHERE ts >= ?::TIMESTAMP)
            FROM entries",
        ) {
            Ok(mut stmt) => match stmt.query_row([since_str, active_str], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            }) {
                Ok(c) => c,
                Err(e) => { warn!("Session count: {}", e); (0, 0) }
            },
            Err(e) => { warn!("Session count prepare: {}", e); (0, 0) }
        };
        self.cache.borrow_mut().session_counts.insert(cache_key, (gen, result));
        result
    }

    #[allow(dead_code)] // ponytail: unused since build_all_stats switched to query_aggregates
    pub fn query_source_breakdown(&self, since_str: &str, active_str: &str) -> Vec<SourceStats> {
        if !self.initialized { return Vec::new(); }
        let gen = self.generation;
        let cache_key = (since_str.to_string(), active_str.to_string());
        {
            let cache = self.cache.borrow();
            if let Some((cg, val)) = cache.source_breakdowns.get(&cache_key) {
                if *cg == gen { return val.clone(); }
            }
        }
        let result = match self.con.prepare(
            "SELECT
                source,
                COUNT(DISTINCT session_id) FILTER (WHERE ts >= ?1::TIMESTAMP),
                COUNT(DISTINCT session_id) FILTER (WHERE ts >= ?2::TIMESTAMP),
                COALESCE(SUM(input_tokens), 0)::BIGINT,
                COALESCE(SUM(output_tokens), 0)::BIGINT,
                COALESCE(SUM(cache_creation_input_tokens), 0)::BIGINT,
                COALESCE(SUM(cache_read_input_tokens), 0)::BIGINT,
                COALESCE(SUM(total_cost), 0)
            FROM entries WHERE ts >= ?1::TIMESTAMP
            GROUP BY source ORDER BY source",
        ) {
            Ok(mut stmt) => match stmt.query_map([since_str, active_str], |row| {
                Ok(SourceStats {
                    source: row.get(0)?,
                    sessions_total: row.get(1)?,
                    sessions_active: row.get(2)?,
                    tokens: TokenTotals {
                        input_tokens: row.get(3)?,
                        output_tokens: row.get(4)?,
                        cache_creation_input_tokens: row.get(5)?,
                        cache_read_input_tokens: row.get(6)?,
                    },
                    cost_total: row.get(7)?,
                })
            }) {
                Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
                Err(e) => { warn!("Source breakdown: {}", e); Vec::new() }
            },
            Err(e) => { warn!("Source breakdown prepare: {}", e); Vec::new() }
        };
        self.cache.borrow_mut().source_breakdowns.insert(cache_key, (gen, result.clone()));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pricing::{CACHE_READ_PRICE, CACHE_WRITE_PRICE, INPUT_PRICE, OUTPUT_PRICE};

    const ENTRIES_DDL: &str = "CREATE TABLE entries (
        source TEXT NOT NULL, session_id TEXT NOT NULL, ts TIMESTAMP NOT NULL,
        model TEXT, provider TEXT, role TEXT,
        input_tokens BIGINT NOT NULL DEFAULT 0, output_tokens BIGINT NOT NULL DEFAULT 0,
        cache_creation_input_tokens BIGINT NOT NULL DEFAULT 0,
        cache_read_input_tokens BIGINT NOT NULL DEFAULT 0,
        input_cost DOUBLE NOT NULL DEFAULT 0.0, output_cost DOUBLE NOT NULL DEFAULT 0.0,
        cache_write_cost DOUBLE NOT NULL DEFAULT 0.0, cache_read_cost DOUBLE NOT NULL DEFAULT 0.0,
        total_cost DOUBLE NOT NULL DEFAULT 0.0, file_path TEXT NOT NULL,
        UNIQUE(source, session_id, ts, file_path)
    );";

    fn test_con() -> Connection {
        let con = Connection::open_in_memory().unwrap();
        con.execute_batch(ENTRIES_DDL).unwrap();
        con
    }

    /// Write a fixture JSONL file under a unique temp dir; returns its path.
    fn write_fixture(name: &str, content: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "locdock_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    fn approx(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "expected {b}, got {a}");
    }

    #[test]
    fn claude_silver_parity_and_idempotency() {
        let con = test_con();
        // 2 valid assistant rows (one with io tokens, one cache-only), plus rows that
        // must be skipped: user type, assistant w/o usage, and a malformed line.
        let content = "\
{\"type\":\"assistant\",\"sessionId\":\"s1\",\"timestamp\":\"2024-01-02T03:04:05.000Z\",\"message\":{\"model\":\"claude-x\",\"usage\":{\"input_tokens\":100,\"output_tokens\":50,\"cache_creation_input_tokens\":10,\"cache_read_input_tokens\":5}}}
{\"type\":\"user\",\"sessionId\":\"s1\",\"timestamp\":\"2024-01-02T03:04:06.000Z\",\"message\":{\"role\":\"user\"}}
{\"type\":\"assistant\",\"sessionId\":\"s1\",\"timestamp\":\"2024-01-02T03:04:07.000Z\",\"message\":{\"model\":\"claude-x\"}}
{\"type\":\"assistant\",\"sessionId\":\"s1\",\"timestamp\":\"2024-01-02T03:04:08.000Z\",\"message\":{\"model\":\"claude-x\",\"usage\":{\"input_tokens\":0,\"output_tokens\":0,\"cache_read_input_tokens\":1000}}}
{not valid json
";
        let path = write_fixture("claude.jsonl", content);

        let n = ingest_files(&con, SourceKind::Claude, std::slice::from_ref(&path)).unwrap();
        assert_eq!(n, 2, "only the 2 assistant-with-usage rows ingest");

        let count: i64 = con.query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 2);

        // Token sums
        let (it, ot, cc, cr): (i64, i64, i64, i64) = con
            .query_row(
                "SELECT SUM(input_tokens), SUM(output_tokens), SUM(cache_creation_input_tokens), SUM(cache_read_input_tokens) FROM entries",
                [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            ).unwrap();
        assert_eq!((it, ot, cc, cr), (100, 50, 10, 1005));

        // io row: flat-priced. cache-only row: zero cost (fill_costs guard parity).
        let io_total: f64 = con
            .query_row("SELECT total_cost FROM entries WHERE input_tokens = 100", [], |r| r.get(0)).unwrap();
        approx(io_total,
            100.0 / 1e6 * INPUT_PRICE + 50.0 / 1e6 * OUTPUT_PRICE
            + 10.0 / 1e6 * CACHE_WRITE_PRICE + 5.0 / 1e6 * CACHE_READ_PRICE);
        let cacheonly_total: f64 = con
            .query_row("SELECT total_cost FROM entries WHERE cache_read_input_tokens = 1000", [], |r| r.get(0)).unwrap();
        approx(cacheonly_total, 0.0);

        // session_id, model, provider, role, file_path normalization
        let (sid, model, prov, role): (String, String, String, String) = con
            .query_row("SELECT session_id, model, provider, role FROM entries WHERE input_tokens = 100",
                [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap();
        assert_eq!((sid.as_str(), model.as_str(), prov.as_str(), role.as_str()),
                   ("s1", "claude-x", "anthropic", "assistant"));

        // Idempotency: re-ingest inserts nothing.
        let n2 = ingest_files(&con, SourceKind::Claude, std::slice::from_ref(&path)).unwrap();
        assert_eq!(n2, 0, "re-ingest is a no-op");
        let count2: i64 = con.query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0)).unwrap();
        assert_eq!(count2, 2);

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn pi_silver_model_carryforward_and_cost() {
        let con = test_con();
        // model_change(alpha) → assistant (no model, no cost → carries alpha, flat-priced)
        // model_change(beta)  → assistant (explicit model gamma, parsed cost 0.5 → kept)
        // a user message is skipped.
        let content = "\
{\"type\":\"model_change\",\"modelId\":\"alpha\",\"provider\":\"prov1\"}
{\"type\":\"message\",\"timestamp\":\"2023-11-14T00:00:00Z\",\"message\":{\"role\":\"assistant\",\"timestamp\":1700000000000,\"usage\":{\"input\":10,\"output\":20}}}
{\"type\":\"model_change\",\"modelId\":\"beta\",\"provider\":\"prov2\"}
{\"type\":\"message\",\"message\":{\"role\":\"user\",\"timestamp\":1700000001000,\"text\":\"hi\"}}
{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"model\":\"gamma\",\"timestamp\":1700000002000,\"usage\":{\"input\":5,\"output\":5,\"cost\":{\"total\":0.5}}}}
";
        let path = write_fixture("agent_sessABC_2023.jsonl", content);

        let n = ingest_files(&con, SourceKind::Pi, std::slice::from_ref(&path)).unwrap();
        assert_eq!(n, 2, "two assistant messages ingest");

        // session_id derived from filename: split on '_' → element 2.
        let sid: String = con.query_row("SELECT DISTINCT session_id FROM entries", [], |r| r.get(0)).unwrap();
        assert_eq!(sid, "sessABC");

        // model carry-forward + explicit override, ordered by ts.
        let models: Vec<String> = {
            let mut stmt = con.prepare("SELECT model FROM entries ORDER BY ts").unwrap();
            let rows = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
            rows.filter_map(|r| r.ok()).collect()
        };
        assert_eq!(models, vec!["alpha".to_string(), "gamma".to_string()]);

        // first row: no parsed cost → flat-priced. second: parsed total kept.
        let alpha_total: f64 = con
            .query_row("SELECT total_cost FROM entries WHERE model = 'alpha'", [], |r| r.get(0)).unwrap();
        approx(alpha_total, 10.0 / 1e6 * INPUT_PRICE + 20.0 / 1e6 * OUTPUT_PRICE);
        let gamma_total: f64 = con
            .query_row("SELECT total_cost FROM entries WHERE model = 'gamma'", [], |r| r.get(0)).unwrap();
        approx(gamma_total, 0.5);

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
