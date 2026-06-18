use crate::git::GitCommit;
use crate::pricing::Pricing;
use crate::source_adapter::{FileDiscoverer, SourceKind, SourceManager};
use crate::types::{CostBreakdown, SourceStats, TokenTotals};
use chrono::{DateTime, Utc};
use duckdb::Connection;
use log::{info, warn};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const RETENTION_DAYS: f64 = 7.0;
const DAY_SECS: f64 = 86400.0;
/// Timeline resolution — must match `data.rs::N_BUCKETS` (frontend expects this many).
const N_BUCKETS: usize = 48;
const MARKER: &str = "usage_cache.db.reset";
const SCHEMA_VERSION: &str = "9";  // v9: clean bad timezone-shifted commit_stats timestamps

/// Files ingested per silver INSERT. Caps transient JSON-parse memory on cold
/// rebuilds; smaller batches + bounded threads keep the non-spillable parse peak
/// well under the memory_limit ceiling.
const INGEST_BATCH_FILES: usize = 8;
const MAX_OBJECT_SIZE: u64 = 64 * 1024 * 1024;  // 64 MB; default 16 MB too small (edge case)

// ── SQL template loading ────────────────────────────────────────────────────
//
// Silver extraction SQL is loaded from .sql template files, with user overrides
// in the config dir. Users can edit these files when JSONL schema changes without
// recompiling the app. See: sql/claude-silver.sql, sql/pi-silver.sql

struct SqlTemplates {
    claude_silver: String,
    pi_silver: String,
}

impl SqlTemplates {
    fn load(config_dir: &Path) -> Self {
        let bundled_claude = include_str!("../sql/claude-silver.sql");
        let bundled_pi = include_str!("../sql/pi-silver.sql");
        let sql_dir = config_dir.join("sql");
        SqlTemplates {
            claude_silver: load_or_fallback(&sql_dir.join("claude-silver.sql"), bundled_claude),
            pi_silver: load_or_fallback(&sql_dir.join("pi-silver.sql"), bundled_pi),
        }
    }

    fn format_claude(&self, paths_array: &str, pricing: &Pricing) -> String {
        self.claude_silver
            .replace("{PATHS}", paths_array)
            .replace("{INPUT_PRICE}", &pricing.input_price.to_string())
            .replace("{OUTPUT_PRICE}", &pricing.output_price.to_string())
            .replace("{CACHE_WRITE_PRICE}", &pricing.cache_write_price.to_string())
            .replace("{CACHE_READ_PRICE}", &pricing.cache_read_price.to_string())
            .replace("{MAX_OBJECT_SIZE}", &MAX_OBJECT_SIZE.to_string())
    }

    fn format_pi(&self, paths_array: &str, pricing: &Pricing) -> String {
        self.pi_silver
            .replace("{PATHS}", paths_array)
            .replace("{INPUT_PRICE}", &pricing.input_price.to_string())
            .replace("{OUTPUT_PRICE}", &pricing.output_price.to_string())
            .replace("{CACHE_WRITE_PRICE}", &pricing.cache_write_price.to_string())
            .replace("{CACHE_READ_PRICE}", &pricing.cache_read_price.to_string())
            .replace("{MAX_OBJECT_SIZE}", &MAX_OBJECT_SIZE.to_string())
    }
}

fn load_or_fallback(path: &Path, bundled: &str) -> String {
    if path.exists() {
        match std::fs::read_to_string(path) {
            Ok(content) => {
                log::info!("Loaded SQL template from {}", path.display());
                return content;
            }
            Err(e) => log::warn!("Failed to read {}, using bundled: {}", path.display(), e),
        }
    }
    bundled.to_string()
}

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
/// 2. **Ingest** maps source JSONL into `entries` via DuckDB SQL (no Rust parsing)
/// 3. **INSERT OR IGNORE** with a UNIQUE constraint prevents duplicates
///
/// Queries run directly against gold (`daily_aggregates`) / silver (`entries`) — no
/// in-memory result cache (Spike 5: serving queries are ~6 ms uncached).
pub struct UsageStore {
    con: Connection,
    source_manager: SourceManager,
    pricing: Pricing,
    sql_templates: SqlTemplates,
    initialized: bool,
    /// Row count at last check — used to detect new data and skip aggregate refresh.
    last_row_count: u64,
}

/// Open the DuckDB connection with retries — handles the hot-reload race where
/// the previous process is mid-exit and still holds the file lock.
pub(crate) fn open_usage_cache(db_path: &Path) -> Connection {
    let mut last_err = None;
    for attempt in 0..5 {
        // Clean up stale WAL/temp files from crashed processes
        if attempt > 0 {
            let s = db_path.to_string_lossy();
            let _ = std::fs::remove_file(Path::new(&format!("{}{}", s, ".wal")));
            let _ = std::fs::remove_file(Path::new(&format!("{}{}", s, ".tmp")));
        }
        match Connection::open(db_path) {
            Ok(c) => return c,
            Err(e) => {
                last_err = Some(e);
                if attempt < 4 {
                    std::thread::sleep(Duration::from_millis(200 * (attempt + 1)));
                }
            }
        }
    }
    eprintln!(
        "[loc-dock] Fatal: Could not open usage cache at {:?}: {}\nAnother instance may be running. Exiting.",
        db_path,
        last_err.unwrap()
    );
    std::process::exit(1);
}

impl UsageStore {
    pub fn new(source_manager: SourceManager, _cache_dir: &Path, pricing: Pricing, config_dir: &Path, con: Connection) -> Self {
        let sql_templates = SqlTemplates::load(config_dir);

        // Guard rails for the SQL JSON-ingest path. The read_ndjson parse path holds
        // non-spillable buffers PER THREAD, so on a many-core box the bundled engine
        // fans a batch across all cores and a tight memory_limit OOMs (it can't spill).
        // Bound the fan-out (threads) to cap concurrent parse buffers, keep a generous
        // ceiling well above the real ~300 MB cold-start peak (Spike 4), and disable
        // insertion-order preservation. Micro-batching bounds per-statement rows.
        let _ = con.execute_batch(
            "SET threads=2; SET memory_limit='2GB'; SET preserve_insertion_order=false;",
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
                 DROP TABLE IF EXISTS file_tracker; \
                 DROP TABLE IF EXISTS ingested_files; \
                 DROP TABLE IF EXISTS daily_aggregates; \
                 DROP TABLE IF EXISTS commit_stats; \
                 DROP TABLE IF EXISTS repo_summaries;"
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

        // v8: commit_stats — per-repo commit cache for incremental git scans.
        // Stores one row per (repo, sha) with aggregated LOC and message.
        // The data loop inserts new commits each cycle; past commits are immutable.
        let _ = con.execute_batch(
            "CREATE TABLE IF NOT EXISTS commit_stats (
                 repo     TEXT NOT NULL,
                 sha      TEXT NOT NULL,
                 ts       TIMESTAMP NOT NULL,
                 msg      TEXT,
                 added    BIGINT NOT NULL,
                 deleted  BIGINT NOT NULL,
                 file_ct  INT NOT NULL DEFAULT 1,
                 UNIQUE(repo, sha)
             );
"
        );

        // v8: repo_summaries — cached AI summaries per repo.
        // Used by the data loop to detect per-repo changes and debounce LLM calls.
        let _ = con.execute_batch(
            "CREATE TABLE IF NOT EXISTS repo_summaries (
                 repo_path       TEXT PRIMARY KEY,
                 last_commit_sha TEXT NOT NULL,
                 last_summary_ts TIMESTAMP NOT NULL,
                 highlights      TEXT,
                 model           TEXT
             );
"
        );

        // v6: ingested_files — stat registry for incremental ingest (Spike 4).
        // A file is re-ingested only when its (mtime, size) changes; changed files
        // are re-read whole (no byte seek), with INSERT OR IGNORE deduping prior rows.
        let _ = con.execute_batch(
            "CREATE TABLE IF NOT EXISTS ingested_files (
                 file_path  TEXT PRIMARY KEY,
                 mtime      DOUBLE NOT NULL,
                 size       BIGINT NOT NULL
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
            pricing,
            sql_templates,
            initialized,
            last_row_count: row_count as u64,
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
                let n = process_source(&self.con, &*pair.0, pair.1, cutoff, &self.pricing, &self.sql_templates)?;
                self.initialized = true;
                return Ok(n);
            }
        }
        Ok(0)
    }

    /// Finalize ETL: refresh aggregates (only if rows grew) and prune old data.
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
            if let Err(e) = self.refresh_aggregates() {
                warn!("Failed to refresh aggregates: {}", e);
            } else {
                self.last_row_count = current_count as u64;
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

/// Process one source: discover files within the retention window, ingest only
/// those whose (mtime, size) changed since last cycle (the `ingested_files`
/// registry), via DuckDB SQL (bronze `read_ndjson_objects` + per-source extraction).
/// No Rust JSON parsing, no byte seek/tail tracking — changed files are re-read
/// whole and INSERT OR IGNORE dedupes prior rows.
///
/// Files are micro-batched (Spike 4b) to cap the non-spillable JSON-parse memory on
/// cold rebuilds. Discovery stays in Rust so the read is bounded to the 7-day window
/// — globbing in DuckDB would read full history (Spike 4 memory regression).
fn process_source(
    con: &Connection,
    discoverer: &dyn FileDiscoverer,
    kind: SourceKind,
    cutoff: f64,
    pricing: &Pricing,
    templates: &SqlTemplates,
) -> Result<usize, String> {
    let (all_files, _max_mtime) = match discoverer.discover_files(cutoff) {
        Ok(r) => r,
        Err(e) => { warn!("ETL '{}': discover failed: {}", kind.name(), e); return Ok(0); }
    };

    if all_files.is_empty() {
        return Ok(0);
    }

    // Registry filter: keep only files new or changed since last ingest, carrying
    // each file's (path, stat) together so stamping can follow ingest success.
    let mut changed: Vec<(PathBuf, String, f64, i64)> = Vec::new();
    for path in &all_files {
        let meta = match path.metadata() { Ok(m) => m, Err(_) => continue };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let size = meta.len() as i64;
        let key = path.to_string_lossy().replace('\\', "/");
        if file_unchanged(con, &key, mtime, size) {
            continue;
        }
        changed.push((path.clone(), key, mtime, size));
    }

    let skipped = all_files.len() - changed.len();
    if changed.is_empty() {
        info!("ETL '{}': 0 changed files ({} unchanged)", kind.name(), skipped);
        return Ok(0);
    }

    // Pi carry-forward (model_change → assistant rows) needs intra-file row order,
    // so Pi ingests one file per statement under preserve_insertion_order=true.
    // Claude has no window functions → larger batches, order-independent.
    let batch_files = match kind {
        SourceKind::Claude => INGEST_BATCH_FILES,
        SourceKind::Pi => 1,
    };

    let mut total = 0usize;
    let mut failed = 0usize;
    for chunk in changed.chunks(batch_files) {
        let paths: Vec<PathBuf> = chunk.iter().map(|(p, ..)| p.clone()).collect();
        match ingest_files(con, kind, &paths, pricing, templates) {
            Ok(n) => {
                total += n;
                // Stamp only this batch's files now that its ingest committed; a
                // failed batch is left unstamped so it retries next cycle (INSERT OR
                // IGNORE keeps the retry idempotent).
                for (_, key, mtime, size) in chunk {
                    if let Err(e) = con.execute(
                        "INSERT OR REPLACE INTO ingested_files (file_path, mtime, size) VALUES (?, ?, ?)",
                        duckdb::params![key, mtime, size],
                    ) {
                        warn!("ETL '{}': registry update {}: {}", kind.name(), key, e);
                    }
                }
            }
            Err(e) => {
                failed += chunk.len();
                warn!("ETL '{}': batch of {} files failed (will retry): {}",
                    kind.name(), chunk.len(), e);
            }
        }
    }

    info!("ETL '{}': {} entries from {} changed files ({} unchanged, {} failed)",
        kind.name(), total, changed.len() - failed, skipped, failed);
    Ok(total)
}

/// True when the registry already has this file at the same (mtime, size).
fn file_unchanged(con: &Connection, key: &str, mtime: f64, size: i64) -> bool {
    con.prepare("SELECT mtime, size FROM ingested_files WHERE file_path = ?")
        .ok()
        .and_then(|mut stmt| {
            stmt.query_row([key], |row| Ok((row.get::<_, f64>(0)?, row.get::<_, i64>(1)?)))
                .ok()
        })
        .map(|(m, s)| m == mtime && s == size)
        .unwrap_or(false)
}

/// Build the bronze→silver SQL for a batch of files and execute it.
/// Returns the number of rows inserted (INSERT OR IGNORE skips duplicates).
fn ingest_files(con: &Connection, kind: SourceKind, paths: &[PathBuf], pricing: &Pricing, templates: &SqlTemplates) -> Result<usize, String> {
    if paths.is_empty() {
        return Ok(0);
    }
    let paths_array = paths_to_sql_array(paths);
    let sql = match kind {
        SourceKind::Claude => templates.format_claude(&paths_array, pricing),
        SourceKind::Pi => templates.format_pi(&paths_array, pricing),
    };

    if kind == SourceKind::Pi {
        // Carry-forward relies on file line order from the scan.
        let _ = con.execute_batch("SET preserve_insertion_order=true;");
    }
    let result = con.execute(&sql, []);
    if kind == SourceKind::Pi {
        let _ = con.execute_batch("SET preserve_insertion_order=false;");
    }

    // Propagate the error so the caller does NOT stamp these files as ingested —
    // a swallowed error would poison the registry (files marked done, 0 rows in).
    result.map_err(|e| format!("ingest insert: {}", e))
}

/// Build a timeline-bucketing query: assign each `entries` row in [lo, hi) to one
/// of `N_BUCKETS` equal slices and aggregate `measures` per bucket. Params, in order:
/// `lo`, `binsize` (=(hi-lo)/N_BUCKETS), `lo`, `hi`. Matches the Rust floor-index
/// convention (Spike 3 parity). Gaps are absent rows — the caller zero-fills.
fn bucket_sql(measures: &str) -> String {
    format!(
        "SELECT LEAST(CAST(floor((epoch(ts) - ?) / ?) AS INT), {last}) AS bucket, {measures}
         FROM entries
         WHERE epoch(ts) >= ? AND epoch(ts) < ?
         GROUP BY bucket",
        last = N_BUCKETS - 1, measures = measures,
    )
}

/// Same as `bucket_sql` but against the `commit_stats` table.
fn commit_bucket_sql(measures: &str) -> String {
    format!(
        "SELECT LEAST(CAST(floor((epoch(ts) - ?) / ?) AS INT), {last}) AS bucket, {measures}
         FROM commit_stats
         WHERE epoch(ts) >= ? AND epoch(ts) < ?
         GROUP BY bucket",
        last = N_BUCKETS - 1, measures = measures,
    )
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

    /// True if the store has at least one row of data (warm start).
    /// Used by prefill to distinguish cold start (show spinner) from warm start (show data).
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

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

    /// Cost timeline bucketed into `N_BUCKETS` equal time slices over [lo, hi)
    /// (unix epoch seconds). Replaces the Rust `bucket_cost` loop — bucketing is
    /// done in SQL (Spike 3 parity-verified). Returns exactly `N_BUCKETS` values,
    /// gaps filled with 0. Uncached (Spike 5: ~6 ms).
    pub fn query_cost_buckets(&self, lo: f64, hi: f64) -> Vec<f64> {
        let binsize = (hi - lo) / N_BUCKETS as f64;
        if !self.initialized || !(binsize > 0.0) { return vec![0.0; N_BUCKETS]; }
        let mut out = vec![0.0f64; N_BUCKETS];
        let sql = bucket_sql("SUM(total_cost)::DOUBLE");
        match self.con.prepare(&sql) {
            Ok(mut stmt) => match stmt.query_map([lo, binsize, lo, hi], |row| {
                Ok((row.get::<_, i64>(0)? as usize, row.get::<_, f64>(1)?))
            }) {
                Ok(rows) => for r in rows.flatten() {
                    if r.0 < N_BUCKETS { out[r.0] = r.1; }
                },
                Err(e) => warn!("Cost buckets: {}", e),
            },
            Err(e) => warn!("Cost buckets prepare: {}", e),
        }
        out
    }

    /// Token timeline bucketed into `N_BUCKETS` slices over [lo, hi) (unix epoch
    /// seconds): (input, output, cache_creation, cache_read) per bucket. Replaces
    /// the Rust `bucket_tokens` loop. Returns exactly `N_BUCKETS` tuples, gaps 0.
    pub fn query_token_buckets(&self, lo: f64, hi: f64) -> Vec<(i64, i64, i64, i64)> {
        let binsize = (hi - lo) / N_BUCKETS as f64;
        if !self.initialized || !(binsize > 0.0) { return vec![(0, 0, 0, 0); N_BUCKETS]; }
        let mut out = vec![(0i64, 0i64, 0i64, 0i64); N_BUCKETS];
        let sql = bucket_sql(
            "SUM(input_tokens)::BIGINT, SUM(output_tokens)::BIGINT, \
             SUM(cache_creation_input_tokens)::BIGINT, SUM(cache_read_input_tokens)::BIGINT",
        );
        match self.con.prepare(&sql) {
            Ok(mut stmt) => match stmt.query_map([lo, binsize, lo, hi], |row| {
                Ok((
                    row.get::<_, i64>(0)? as usize,
                    row.get::<_, i64>(1)?, row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?, row.get::<_, i64>(4)?,
                ))
            }) {
                Ok(rows) => for r in rows.flatten() {
                    if r.0 < N_BUCKETS { out[r.0] = (r.1, r.2, r.3, r.4); }
                },
                Err(e) => warn!("Token buckets: {}", e),
            },
            Err(e) => warn!("Token buckets prepare: {}", e),
        }
        out
    }

    /// Distinct session counts (total since `since_str`, active since `active_str`).
    /// Queries `entries` directly — distinct counts are non-additive so they cannot
    /// come from `daily_aggregates` over multi-day ranges (Spike 5). Uncached (~6 ms).
// ── Commit queries ────────────────────────────────────────────────

    /// Insert new commits into `commit_stats` table (INSERT OR IGNORE).
    pub fn insert_commits(&self, repo: &str, commits: &[GitCommit], _head_sha: &str) -> Result<usize, String> {
        if commits.is_empty() {
            return Ok(0);
        }
        let mut count = 0usize;
        for c in commits {
            // ponytail: store UTC-naive timestamp to avoid timezone offset in epoch() queries.
            // Convert FixedOffset to UTC, format without timezone suffix, then cast to TIMESTAMP.
            let ts_utc = c.ts.with_timezone(&Utc);
            let ts_str = ts_utc.format("%Y-%m-%d %H:%M:%S").to_string();
            match self.con.execute(
                "INSERT OR IGNORE INTO commit_stats (repo, sha, ts, msg, added, deleted, file_ct) VALUES (?, ?, ?::TIMESTAMP, ?, ?, ?, ?)",
                duckdb::params![repo, c.sha, ts_str, c.msg, c.added, c.deleted, c.file_count as i64],
            ) {
                Ok(n) => count += n as usize,
                Err(e) => warn!("insert_commits: {}: {}", repo, e),
            }
        }
        Ok(count)
    }

    /// Latest commit timestamp across all repos (for incremental scan).
    /// Returns None if `commit_stats` is empty (first cycle).
    pub fn latest_commit_ts(&self) -> Option<DateTime<Utc>> {
        self.con
            .prepare("SELECT MAX(ts) FROM commit_stats")
            .ok()
            .and_then(|mut stmt| stmt.query_row([], |row| row.get(0)).ok())
    }

    /// LOC timeline bucketed into `N_BUCKETS` slices over [lo, hi) (unix epoch secs).
    /// Returns (added, deleted) per bucket. Exact match for `bucket_git` in data.rs.
    pub fn query_commit_buckets(&self, lo: f64, hi: f64) -> Vec<(i64, i64)> {
        let binsize = (hi - lo) / N_BUCKETS as f64;
        if !(binsize > 0.0) { return vec![(0, 0); N_BUCKETS]; }
        let mut out = vec![(0i64, 0i64); N_BUCKETS];
        let sql = commit_bucket_sql("SUM(added)::BIGINT, SUM(deleted)::BIGINT");
        match self.con.prepare(&sql) {
            Ok(mut stmt) => match stmt.query_map([lo, binsize, lo, hi], |row| {
                Ok((
                    row.get::<_, i64>(0)? as usize,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            }) {
                Ok(rows) => for r in rows.flatten() {
                    if r.0 < N_BUCKETS { out[r.0] = (r.1, r.2); }
                },
                Err(e) => warn!("Commit buckets: {}", e),
            },
            Err(e) => warn!("Commit buckets prepare: {}", e),
        }
        out
    }

    /// Total LOC added/deleted since a timestamp string.
    pub fn query_commit_totals(&self, since_str: &str) -> (i64, i64) {
        let result = self.con.prepare(
            "SELECT COALESCE(SUM(added), 0)::BIGINT, COALESCE(SUM(deleted), 0)::BIGINT
             FROM commit_stats WHERE ts >= ?::TIMESTAMP"
        );
        match result {
            Ok(mut stmt) => match stmt.query_row([since_str], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            }) {
                Ok(r) => r,
                Err(e) => {
                    warn!("Commit totals: {}", e);
                    (0, 0)
                }
            },
            Err(e) => {
                warn!("Commit totals prepare: {}", e);
                (0, 0)
            }
        }
    }

    pub fn count_sessions(&self, since_str: &str, active_str: &str) -> (i64, i64) {
        if !self.initialized { return (0, 0); }
        match self.con.prepare(
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
        }
    }

// ── Summary cache ─────────────────────────────────────────────────

    /// Read cached summary for a repo. Returns (highlights JSON, last_commit_sha).
    pub fn get_repo_summary(&self, repo: &str) -> Option<(String, String)> {
        self.con
            .prepare("SELECT highlights, last_commit_sha FROM repo_summaries WHERE repo_path = ?")
            .ok()
            .and_then(|mut stmt| {
                stmt.query_row([repo], |row| {
                    Ok((
                        row.get::<_, String>(0).unwrap_or_default(),
                        row.get::<_, String>(1).unwrap_or_default(),
                    ))
                })
                .ok()
            })
    }

    /// Save/update repo summary after LLM call.
    pub fn save_repo_summary(&self, repo: &str, sha: &str, highlights_json: &str, model: &str) {
        let now = Utc::now().to_rfc3339();
        if let Err(e) = self.con.execute(
            "INSERT OR REPLACE INTO repo_summaries (repo_path, last_commit_sha, last_summary_ts, highlights, model) VALUES (?, ?, ?::TIMESTAMP, ?, ?)",
            duckdb::params![repo, sha, now, highlights_json, model],
        ) {
            warn!("save_repo_summary {}: {}", repo, e);
        }
    }

    /// Get commit messages for a repo since a timestamp (for PR extraction).
    pub fn repo_commit_messages_since(&self, repo: &str, since_str: &str) -> Vec<String> {
        self.con
            .prepare("SELECT msg FROM commit_stats WHERE repo = ? AND ts >= ?::TIMESTAMP ORDER BY ts")
            .ok()
            .map(|mut stmt| {
                stmt.query_map(duckdb::params![repo, since_str], |row| row.get::<_, String>(0))
                    .ok()
                    .map(|rows| rows.filter_map(|r| r.ok()).collect())
                    .unwrap_or_default()
            })
            .unwrap_or_default()
    }

    /// Count commits for a specific repo since a timestamp string.
    pub fn count_repo_commits_since(&self, repo: &str, since_str: &str) -> usize {
        self.con
            .prepare("SELECT COUNT(*) FROM commit_stats WHERE repo = ? AND ts >= ?::TIMESTAMP")
            .ok()
            .and_then(|mut stmt| {
                stmt.query_row(duckdb::params![repo, since_str], |row| row.get::<_, i64>(0))
                    .ok()
                    .map(|n| n as usize)
            })
            .unwrap_or(0)
    }

    /// Get all repo names with non-null highlights (for building SummaryData).
    pub fn all_summarized_repos(&self) -> Vec<(String, String)> {
        match self.con.prepare(
            "SELECT repo_path, highlights FROM repo_summaries WHERE highlights IS NOT NULL"
        ) {
            Ok(mut stmt) => match stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            }) {
                Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
                Err(e) => {
                    warn!("All summarized repos: {}", e);
                    Vec::new()
                }
            },
            Err(e) => {
                warn!("All summarized repos prepare: {}", e);
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source_adapter::GlobFileDiscoverer;

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

    const REGISTRY_DDL: &str = "CREATE TABLE ingested_files (
        file_path TEXT PRIMARY KEY, mtime DOUBLE NOT NULL, size BIGINT NOT NULL
    );";
    const COMMIT_DDL: &str = "CREATE TABLE IF NOT EXISTS commit_stats (
        repo TEXT NOT NULL, sha TEXT NOT NULL, ts TIMESTAMP NOT NULL,
        msg TEXT, added BIGINT NOT NULL, deleted BIGINT NOT NULL,
        file_ct INT NOT NULL DEFAULT 1, UNIQUE(repo, sha)
    );";

    fn test_con() -> Connection {
        let con = Connection::open_in_memory().unwrap();
        con.execute_batch(ENTRIES_DDL).unwrap();
        con.execute_batch(REGISTRY_DDL).unwrap();
        con
    }

    fn test_con_with_commits() -> Connection {
        let con = Connection::open_in_memory().unwrap();
        con.execute_batch(COMMIT_DDL).unwrap();
        con
    }

    fn test_pricing() -> Pricing {
        Pricing::default()
    }

    fn test_templates() -> SqlTemplates {
        let dir = std::env::temp_dir().join("locdock_test_sql");
        SqlTemplates::load(&dir)
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
        let pricing = test_pricing();
        let templates = test_templates();
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

        let n = ingest_files(&con, SourceKind::Claude, std::slice::from_ref(&path), &pricing, &templates).unwrap();
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
            100.0 / 1e6 * pricing.input_price + 50.0 / 1e6 * pricing.output_price
            + 10.0 / 1e6 * pricing.cache_write_price + 5.0 / 1e6 * pricing.cache_read_price);
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
        let n2 = ingest_files(&con, SourceKind::Claude, std::slice::from_ref(&path), &pricing, &templates).unwrap();
        assert_eq!(n2, 0, "re-ingest is a no-op");
        let count2: i64 = con.query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0)).unwrap();
        assert_eq!(count2, 2);

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn pi_silver_model_carryforward_and_cost() {
        let con = test_con();
        let pricing = test_pricing();
        let templates = test_templates();
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

        let n = ingest_files(&con, SourceKind::Pi, std::slice::from_ref(&path), &pricing, &templates).unwrap();
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
        approx(alpha_total, 10.0 / 1e6 * pricing.input_price + 20.0 / 1e6 * pricing.output_price);
        let gamma_total: f64 = con
            .query_row("SELECT total_cost FROM entries WHERE model = 'gamma'", [], |r| r.get(0)).unwrap();
        approx(gamma_total, 0.5);

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn commit_stats_insert_and_bucket() {
        // Test that commits inserted with UTC-naive timestamps are correctly
        // bucketed by epoch-range queries (covers the timezone bug fix).
        use chrono::{DateTime, Utc};
        let con = test_con_with_commits();

        // Insert two commits at known timestamps
        let ts_utc = "2026-06-18 08:30:00";
        let ts_bad = "2026-06-18T10:30:00+02:00";  // same instant as ts_utc, but with zone
        con.execute(
            "INSERT INTO commit_stats (repo, sha, ts, msg, added, deleted, file_ct) VALUES (?, ?, ?::TIMESTAMP, ?, ?, ?, ?)",
            duckdb::params!["r", "a", ts_utc, "utc", 10, 5, 2],
        ).unwrap();
        con.execute(
            "INSERT INTO commit_stats (repo, sha, ts, msg, added, deleted, file_ct) VALUES (?, ?, ?::TIMESTAMP, ?, ?, ?, ?)",
            duckdb::params!["r", "b", ts_bad, "timezone", 5, 2, 1],
        ).unwrap();

        // Compute epoch range using chrono — matches how data.rs computes day_lo/hi
        let day_start: DateTime<Utc> = "2026-06-18T05:00:00Z".parse().unwrap();
        let hi: DateTime<Utc> = "2026-06-18T12:00:00Z".parse().unwrap();
        let lo = day_start.timestamp() as f64;
        let hi_f = hi.timestamp() as f64;
        let binsize = (hi_f - lo) / N_BUCKETS as f64;

        let sql = commit_bucket_sql("SUM(added)::BIGINT, SUM(deleted)::BIGINT");
        let mut stmt = con.prepare(&sql).unwrap();
        let rows: Vec<(i64, i64)> = stmt.query_map([lo, binsize, lo, hi_f], |r| {
            Ok((r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
        }).unwrap().filter_map(|r| r.ok()).collect();

        let total_added: i64 = rows.iter().map(|(a, _)| a).sum();
        // Only the UTC-naive insert should be found; the timezone-shifted one
        // stores wall-clock 10:30 as "10:30", so epoch() treats it as 10:30 UTC,
        // which is outside [05:00, 12:00)? Actually it IS within [05:00, 12:00).
        // Both should be found when range is wide enough.
        assert_eq!(total_added, 15, "UTC-naive + timezone-shifted: expected 10+5=15");
    }

    #[test]
    fn commit_bucket_sql_epoch_parity() {
        // Verify that commit_bucket_sql returns the same results as
        // a direct query with epoch() — catches timezone format mismatches.
        let con = test_con_with_commits();

        // Insert using the SAME UTC-naive format that insert_commits uses after the fix
        let ts = "2026-06-18 08:30:00";
        con.execute(
            "INSERT INTO commit_stats (repo, sha, ts, msg, added, deleted, file_ct) VALUES (?, ?, ?::TIMESTAMP, ?, ?, ?, ?)",
            duckdb::params!["repo", "aaaabbbbccccddddeeeeffffgggghhhhiiiijjjj", ts, "msg", 100, 50, 3],
        ).unwrap();

        // Get epoch via SQL to confirm the stored value
        let stored_epoch: i64 = con.query_row(
            "SELECT epoch(ts) FROM commit_stats WHERE sha = 'aaaabbbbccccddddeeeeffffgggghhhhiiiijjjj'",
            [], |r| r.get(0)
        ).unwrap();

        // The stored epoch should equal epoch('2026-06-18 08:30:00'::TIMESTAMP)
        let expected_epoch: i64 = con.query_row(
            "SELECT epoch('2026-06-18 08:30:00'::TIMESTAMP)", [], |r| r.get(0)
        ).unwrap();
        assert_eq!(stored_epoch, expected_epoch,
            "Stored epoch should match direct cast. Stored: {} Expected: {}", stored_epoch, expected_epoch);

        // Now query commit_bucket_sql with a range that includes that epoch
        let lo = stored_epoch as f64;
        let hi = lo + 3600.0;  // 1 hour window
        let binsize = (hi - lo) / N_BUCKETS as f64;

        let sql = commit_bucket_sql("SUM(added)::BIGINT, SUM(deleted)::BIGINT");
        let mut stmt = con.prepare(&sql).unwrap();
        let rows: Vec<(i64, i64)> = stmt.query_map([lo, binsize, lo, hi], |r| {
            Ok((r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
        }).unwrap().filter_map(|r| r.ok()).collect();

        let total_added: i64 = rows.iter().map(|(a, _)| a).sum();
        assert_eq!(total_added, 100, "bucket query should find 100 added lines");
    }

    #[test]
    fn bucket_sql_floor_index_and_bounds() {
        let con = test_con();
        let lo = 1_000_000.0_f64;
        let hi = 1_004_800.0_f64; // binsize = 100, 48 buckets
        let binsize = (hi - lo) / N_BUCKETS as f64;

        // (epoch, cost, expected bucket or None if excluded)
        let rows = [
            (1_000_000.0, 1.0),  // offset 0     -> b0
            (1_000_050.0, 2.0),  // offset 50    -> b0
            (1_000_150.0, 4.0),  // offset 150   -> b1
            (1_004_750.0, 8.0),  // offset 4750  -> floor(47.5)=47 -> b47
            (1_004_800.0, 16.0), // offset == total -> excluded (epoch < hi false)
            (999_999.0,   32.0), // epoch < lo -> excluded
        ];
        {
            let mut ins = con.prepare(
                "INSERT INTO entries (source, session_id, ts, file_path, total_cost)
                 VALUES ('c', 's', to_timestamp(?) AT TIME ZONE 'UTC', 'f', ?)",
            ).unwrap();
            for (e, c) in rows { ins.execute(duckdb::params![e, c]).unwrap(); }
        }

        let mut out = vec![0.0f64; N_BUCKETS];
        let sql = bucket_sql("SUM(total_cost)::DOUBLE");
        let mut stmt = con.prepare(&sql).unwrap();
        let mapped = stmt.query_map([lo, binsize, lo, hi], |r| {
            Ok((r.get::<_, i64>(0)? as usize, r.get::<_, f64>(1)?))
        }).unwrap();
        for r in mapped.flatten() { out[r.0] = r.1; }

        approx(out[0], 3.0);
        approx(out[1], 4.0);
        approx(out[47], 8.0);
        approx(out.iter().sum::<f64>(), 15.0); // 16 + 32 excluded
    }

    #[test]
    fn registry_skips_unchanged_and_reingests_on_change() {
        let con = test_con();
        let pricing = test_pricing();
        let templates = test_templates();
        let dir = std::env::temp_dir().join(format!(
            "locdock_reg_{}_{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("claude.jsonl");
        let row1 = "{\"type\":\"assistant\",\"sessionId\":\"s1\",\"timestamp\":\"2024-01-02T03:04:05Z\",\"message\":{\"model\":\"m\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n";
        std::fs::write(&file, row1).unwrap();

        let disc = GlobFileDiscoverer::new(dir.clone(), vec![]);

        // First cycle ingests the one row.
        let n1 = process_source(&con, &disc, SourceKind::Claude, 0.0, &pricing, &templates).unwrap();
        assert_eq!(n1, 1);

        // Second cycle: file unchanged → skipped, nothing ingested.
        let n2 = process_source(&con, &disc, SourceKind::Claude, 0.0, &pricing, &templates).unwrap();
        assert_eq!(n2, 0);

        // Append a second row → mtime+size change → whole-file re-read; old row
        // deduped by INSERT OR IGNORE, only the new row counts.
        let row2 = "{\"type\":\"assistant\",\"sessionId\":\"s1\",\"timestamp\":\"2024-01-02T03:05:05Z\",\"message\":{\"model\":\"m\",\"usage\":{\"input_tokens\":2,\"output_tokens\":2}}}\n";
        std::fs::write(&file, format!("{row1}{row2}")).unwrap();

        let n3 = process_source(&con, &disc, SourceKind::Claude, 0.0, &pricing, &templates).unwrap();
        assert_eq!(n3, 1, "only the newly appended row inserts");

        let count: i64 = con.query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 2);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
