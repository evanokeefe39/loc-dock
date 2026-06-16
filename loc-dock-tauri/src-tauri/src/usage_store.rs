use crate::source_adapter::{FileDiscoverer, NormalizedEntry, SessionParser, SourceManager};
use crate::types::{CostBreakdown, SourceStats, TokenTotals};
use chrono::{DateTime, Utc};
use duckdb::Connection;
use log::{info, warn};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::SystemTime;

const RETENTION_DAYS: f64 = 7.0;
const DAY_SECS: f64 = 86400.0;
const DB_NAME: &str = "usage_cache.db";
const MARKER: &str = "usage_cache.db.reset";
const SCHEMA_VERSION: &str = "5";  // daily_aggregates materialized table

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
                "DROP TABLE IF EXISTS entries; DROP TABLE IF EXISTS watermarks;"
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

        // v5: file_tracker — tracks per-file state for append-only reading.
        // Avoids re-reading unchaged JSONL files every cycle.
        let _ = con.execute_batch(
            "CREATE TABLE IF NOT EXISTS file_tracker (
                 file_path        TEXT PRIMARY KEY,
                 mtime            DOUBLE NOT NULL,
                 size             BIGINT NOT NULL,
                 last_entry_ts    TIMESTAMP
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
                let n = process_source(&mut self.con, &*pair.0, &*pair.1, name, cutoff)?;
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

/// Tracked file state for append-only reading.
#[derive(Debug, Clone)]
struct FileTrackerState {
    mtime: f64,
    size: u64,
    last_entry_ts: Option<String>,  // stored as "YYYY-MM-DD HH:MM:SS"
}

/// Query the file tracker for a given path.
fn get_file_tracker(con: &Connection, path: &Path) -> Option<FileTrackerState> {
    let path_str = path.to_string_lossy().replace('\\', "/");
    con.prepare(
        "SELECT mtime, size, last_entry_ts FROM file_tracker WHERE file_path = ?"
    ).ok().and_then(|mut stmt| {
        stmt.query_row([&path_str], |row| {
            Ok(FileTrackerState {
                mtime: row.get(0)?,
                size: row.get::<_, i64>(1)? as u64,
                last_entry_ts: row.get::<_, Option<String>>(2)?,
            })
        }).ok()
    })
}

/// Update the file tracker for a given path.
fn update_file_tracker(
    con: &Connection,
    path: &Path,
    mtime: f64,
    size: u64,
    last_entry_ts: Option<String>,
) -> Result<(), String> {
    let path_str = path.to_string_lossy().replace('\\', "/");
    con.execute(
        "INSERT OR REPLACE INTO file_tracker (file_path, mtime, size, last_entry_ts)
         VALUES (?1, ?2, ?3, ?4)",
        duckdb::params![path_str, mtime, size as i64, last_entry_ts],
    ).map_err(|e| format!("Update file tracker: {}", e))?;
    Ok(())
}

/// Process one source: discover files within cutoff, stat-and-track for
/// append-only reading, parse only new/changed content, insert.
///
/// File tracking algorithm:
/// 1. Stat each discovered file — compare mtime + size against tracker
/// 2. If unchanged (mtime + size match) → skip entirely
/// 3. If size >= tracked_size → append-only: seek to tracked_size, read new bytes
/// 4. If size < tracked_size → rotated/truncated: full read from beginning
/// 5. Cold start (no tracker entry) → full read
/// 6. Update tracker after successful read
fn process_source(
    con: &mut Connection,
    discoverer: &dyn FileDiscoverer,
    parser: &dyn SessionParser,
    source_name: &str,
    cutoff: f64,
) -> Result<usize, String> {
    let (all_files, _max_mtime) = match discoverer.discover_files(cutoff) {
        Ok(r) => r,
        Err(e) => { warn!("ETL '{}': discover failed: {}", source_name, e); return Ok(0); }
    };

    if all_files.is_empty() {
        return Ok(0);
    }

    info!("ETL '{}': {} files in window (cutoff={}h ago)", source_name, all_files.len(),
        (SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs_f64() - cutoff) / 3600.0
    );

    let mut total = 0usize;
    let mut files_read = 0usize;
    let mut files_skipped = 0usize;

    for chunk in all_files.chunks(10) {
        let mut batch = Vec::with_capacity(500);
        for path in chunk {
            // Stat the file (cheap — no I/O other than metadata)
            let metadata = match fs::metadata(path) {
                Ok(m) => m,
                Err(e) => {
                    warn!("ETL '{}': stat failed for {}: {}", source_name, path.display(), e);
                    continue;
                }
            };
            let mtime = match metadata.modified() {
                Ok(t) => t
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0),
                Err(_) => 0.0,
            };
            let size = metadata.len();

            // Check tracker — skip if file is unchanged
            let tracked = get_file_tracker(con, path);
            let mut last_entry_ts: Option<String> = None;

            match tracked {
                Some(ft) if ft.mtime == mtime && ft.size == size => {
                    // File is identical to last scan — skip entirely
                    files_skipped += 1;
                    continue;
                }
                Some(ft) if size >= ft.size => {
                    // File was appended — read only new bytes from tracked position
                    let mut file = match fs::File::open(path) {
                        Ok(f) => f,
                        Err(e) => {
                            warn!("ETL '{}': open failed for {}: {}", source_name, path.display(), e);
                            continue;
                        }
                    };
                    if let Err(e) = file.seek(SeekFrom::Start(ft.size)) {
                        warn!("ETL '{}': seek failed for {}: {}", source_name, path.display(), e);
                        continue;
                    }
                    let mut new_bytes = Vec::new();
                    if let Err(e) = file.read_to_end(&mut new_bytes) {
                        warn!("ETL '{}': read failed for {}: {}", source_name, path.display(), e);
                        continue;
                    }
                    // Convert to string (new bytes only)
                    let new_content = match String::from_utf8(new_bytes) {
                        Ok(s) => s,
                        Err(e) => {
                            warn!("ETL '{}': utf8 error for {}: {}", source_name, path.display(), e);
                            continue;
                        }
                    };
                    if !new_content.is_empty() {
                        let new_entries = parser.parse_content(path, &new_content);
                        batch.extend(new_entries);
                    }
                    // Use last tracked entry timestamp if available
                    last_entry_ts = ft.last_entry_ts;
                    files_read += 1;
                }
                _ => {
                    // Cold start or file was truncated/rotated — full read
                    let content = match fs::read_to_string(path) {
                        Ok(c) => c,
                        Err(e) => {
                            warn!("ETL '{}': read failed for {}: {}", source_name, path.display(), e);
                            continue;
                        }
                    };
                    let new_entries = parser.parse_content(path, &content);
                    batch.extend(new_entries);
                    files_read += 1;
                }
            }

            // Update tracker with current mtime/size
            if let Err(e) = update_file_tracker(con, path, mtime, size, last_entry_ts) {
                warn!("ETL '{}': tracker update failed for {}: {}", source_name, path.display(), e);
            }
        }

        if batch.is_empty() { continue; }

        fill_costs(&mut batch);
        total += appender_insert(con, &batch)?;
    }

    info!("ETL '{}': {} entries from {} files ({} skipped)",
        source_name, total, files_read, files_skipped);
    Ok(total)
}

/// Bulk-insert entries using batch INSERT OR IGNORE with multi-row VALUES.
///
/// Batches of BATCH_SIZE rows per statement to avoid the per-row statement
/// overhead that made cold-start ETL take minutes over 700+ files.
/// Wrapped in a single transaction for atomicity and speed.
///
/// DuckDB's Appender API does NOT support INSERT OR IGNORE (BUG-004),
/// so we use multi-row VALUES which is the next-best option.
const BATCH_SIZE: usize = 50;  // 50 rows × 16 cols = 800 params per batch

fn appender_insert(con: &mut Connection, entries: &[NormalizedEntry]) -> Result<usize, String> {
    if entries.is_empty() {
        return Ok(0);
    }

    let tx = con.transaction().map_err(|e| format!("Begin tx: {}", e))?;
    let col_count = 16;
    let mut inserted = 0usize;

    for chunk in entries.chunks(BATCH_SIZE) {
        // Build multi-row VALUES clause: (?1,?2,...,?16), (?17,?18,...), ...
        let rows: Vec<String> = chunk.iter().enumerate().map(|(i, _)| {
            let base = i * col_count;
            let nums: Vec<String> = (1..=col_count).map(|j| {
                if j == 3 { format!("?{}::TIMESTAMP", base + j) }
                else { format!("?{}", base + j) }
            }).collect();
            format!("({})", nums.join(", "))
        }).collect();

        let sql = format!(
            "INSERT OR IGNORE INTO entries \
             (source, session_id, ts, model, provider, role, \
              input_tokens, output_tokens, \
              cache_creation_input_tokens, cache_read_input_tokens, \
              input_cost, output_cost, cache_write_cost, cache_read_cost, \
              total_cost, file_path) \
             VALUES {}",
            rows.join(", ")
        );

        let mut params: Vec<Box<dyn duckdb::types::ToSql>> = Vec::with_capacity(chunk.len() * col_count);
        for entry in chunk {
            let ts = entry.ts.format("%Y-%m-%d %H:%M:%S").to_string();
            params.push(Box::new(entry.source.clone()));
            params.push(Box::new(entry.session_id.clone()));
            params.push(Box::new(ts));
            params.push(Box::new(entry.model.clone()));
            params.push(Box::new(entry.provider.clone()));
            params.push(Box::new(entry.role.clone()));
            params.push(Box::new(entry.input_tokens as i64));
            params.push(Box::new(entry.output_tokens as i64));
            params.push(Box::new(entry.cache_creation_input_tokens as i64));
            params.push(Box::new(entry.cache_read_input_tokens as i64));
            params.push(Box::new(entry.input_cost));
            params.push(Box::new(entry.output_cost));
            params.push(Box::new(entry.cache_write_cost));
            params.push(Box::new(entry.cache_read_cost));
            params.push(Box::new(entry.total_cost));
            params.push(Box::new(entry.file_path.clone()));
        }

        let param_refs: Vec<&dyn duckdb::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
        match tx.execute(&sql, param_refs.as_slice()) {
            Ok(n) => inserted += n as usize,
            Err(e) => {
                log::warn!("Batch insert error: {}", e);
            }
        }
    }

    tx.commit().map_err(|e| format!("Commit tx: {}", e))?;
    Ok(inserted)
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

/// Fill missing cost fields using flat per-token pricing.
/// Entries that already carry costs (e.g. from Pi) are left untouched.
fn fill_costs(entries: &mut [NormalizedEntry]) {
    let input_price = crate::pricing::INPUT_PRICE;
    let output_price = crate::pricing::OUTPUT_PRICE;
    let cache_write_price = crate::pricing::CACHE_WRITE_PRICE;
    let cache_read_price = crate::pricing::CACHE_READ_PRICE;

    for e in entries.iter_mut() {
        if e.total_cost == 0.0 && (e.input_tokens > 0 || e.output_tokens > 0) {
            e.input_cost = e.input_tokens as f64 / 1_000_000.0 * input_price;
            e.output_cost = e.output_tokens as f64 / 1_000_000.0 * output_price;
            e.cache_write_cost = e.cache_creation_input_tokens as f64 / 1_000_000.0 * cache_write_price;
            e.cache_read_cost = e.cache_read_input_tokens as f64 / 1_000_000.0 * cache_read_price;
            e.total_cost = e.input_cost + e.output_cost + e.cache_write_cost + e.cache_read_cost;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn test_insert_or_ignore_duplicates() {
        let mut con = Connection::open_in_memory().unwrap();
        con.execute_batch(
            "CREATE TABLE entries (
                source TEXT NOT NULL,
                session_id TEXT NOT NULL,
                ts TIMESTAMP NOT NULL,
                model TEXT,
                provider TEXT,
                role TEXT,
                input_tokens BIGINT NOT NULL DEFAULT 0,
                output_tokens BIGINT NOT NULL DEFAULT 0,
                cache_creation_input_tokens BIGINT NOT NULL DEFAULT 0,
                cache_read_input_tokens BIGINT NOT NULL DEFAULT 0,
                input_cost DOUBLE NOT NULL DEFAULT 0.0,
                output_cost DOUBLE NOT NULL DEFAULT 0.0,
                cache_write_cost DOUBLE NOT NULL DEFAULT 0.0,
                cache_read_cost DOUBLE NOT NULL DEFAULT 0.0,
                total_cost DOUBLE NOT NULL DEFAULT 0.0,
                file_path TEXT NOT NULL,
                UNIQUE(source, session_id, ts, file_path)
            );"
        )
        .unwrap();

        let now = Utc::now();
        let entry = NormalizedEntry {
            source: "test".to_string(),
            session_id: "sess-1".to_string(),
            ts: now,
            model: None,
            provider: None,
            role: None,
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: 10,
            cache_read_input_tokens: 5,
            input_cost: 0.001,
            output_cost: 0.002,
            cache_write_cost: 0.0005,
            cache_read_cost: 0.0001,
            total_cost: 0.0036,
            file_path: "test.jsonl".to_string(),
        };

        // First insert should succeed
        let n1 = appender_insert(&mut con, &[entry.clone()]).unwrap();
        assert_eq!(n1, 1, "first insert should add 1 row");

        // Second insert (identical) should be ignored by INSERT OR IGNORE
        let n2 = appender_insert(&mut con, &[entry.clone()]).unwrap();
        assert_eq!(n2, 0, "duplicate should be ignored");

        // Different file_path should succeed (different UNIQUE key)
        let mut diff = entry.clone();
        diff.file_path = "other.jsonl".to_string();
        let n3 = appender_insert(&mut con, &[diff]).unwrap();
        assert_eq!(n3, 1, "different file_path should add 1 row");

        // Batch with mix of new and duplicate entries
        let new_entry = NormalizedEntry {
            source: "test".to_string(),
            session_id: "sess-2".to_string(),
            ts: now,
            model: None,
            provider: None,
            role: None,
            input_tokens: 200,
            output_tokens: 100,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            input_cost: 0.0,
            output_cost: 0.0,
            cache_write_cost: 0.0,
            cache_read_cost: 0.0,
            total_cost: 0.0,
            file_path: "batch.jsonl".to_string(),
        };
        let n4 = appender_insert(&mut con, &[entry.clone(), new_entry, entry.clone()]).unwrap();
        // entry is duplicate (0), new_entry is new (1), entry is duplicate (0) = 1 total
        assert_eq!(n4, 1, "batch with 2 dupes + 1 new should insert 1");

        // Verify total row count
        let count: i64 = con
            .query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 3, "total rows: 1 (first) + 1 (diff file_path) + 1 (batch new)");
    }
}
