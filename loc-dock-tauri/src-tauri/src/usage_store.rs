use crate::pricing;
use crate::types::{CostBreakdown, TokenTotals};
use duckdb::Connection;
use log::{error, info, warn};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const RETENTION_DAYS: f64 = 7.0;

/// In-memory DuckDB store that ingests Claude JSONL usage logs and
/// exposes aggregation queries for tokens, cost, and session counts.
pub struct UsageStore {
    con: Connection,
    last_max_mtime: f64,
    initialized: bool,
    projects_dir: PathBuf,
}

impl UsageStore {
    /// Create a new store backed by an in-memory DuckDB connection.
    /// `projects_dir` is typically `~/.claude/projects`.
    pub fn new(projects_dir: &Path) -> Self {
        let con = Connection::open_in_memory().expect("failed to open in-memory DuckDB");
        UsageStore {
            con,
            last_max_mtime: 0.0,
            initialized: false,
            projects_dir: projects_dir.to_path_buf(),
        }
    }

    /// Scan `projects_dir` for JSONL files modified within the retention
    /// window, reload the DuckDB table if anything changed.
    /// Returns `true` when the table was rebuilt.
    pub fn load(&mut self) -> bool {
        if !self.projects_dir.exists() {
            return false;
        }

        let now_ts = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let cutoff = now_ts - RETENTION_DAYS * 86400.0;

        let mut files: Vec<String> = Vec::new();
        let mut max_mtime: f64 = 0.0;

        let pattern = self.projects_dir.join("**/*.jsonl");
        let pattern_str = pattern.to_string_lossy().replace('\\', "/");

        for entry in glob::glob(&pattern_str).unwrap_or_else(|_| glob::glob("").unwrap()) {
            let p = match entry {
                Ok(p) => p,
                Err(_) => continue,
            };
            let mtime = match p.metadata().and_then(|m| m.modified()) {
                Ok(t) => t
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0),
                Err(_) => continue,
            };
            if mtime > max_mtime {
                max_mtime = mtime;
            }
            if mtime >= cutoff {
                // DuckDB on Windows needs forward slashes in file paths.
                let normalized = p.to_string_lossy().replace('\\', "/");
                files.push(normalized);
            }
        }

        if max_mtime <= self.last_max_mtime && self.initialized {
            return false;
        }
        if files.is_empty() {
            self.last_max_mtime = max_mtime;
            return false;
        }

        // Build a DuckDB list literal: ['path1', 'path2', ...]
        let file_list = format!(
            "[{}]",
            files
                .iter()
                .map(|f| format!("'{}'", f.replace('\'', "''")))
                .collect::<Vec<_>>()
                .join(", ")
        );

        let _ = self.con.execute("DROP TABLE IF EXISTS entries", []);

        let sql = format!(
            r#"
            CREATE TEMP TABLE entries AS
            SELECT ts, src, input_tokens, output_tokens,
                   cache_creation_input_tokens, cache_read_input_tokens
            FROM (
                SELECT
                    try_cast(timestamp AS TIMESTAMP) AS ts,
                    filename                         AS src,
                    COALESCE(message.usage.input_tokens, 0)::BIGINT AS input_tokens,
                    COALESCE(message.usage.output_tokens, 0)::BIGINT AS output_tokens,
                    COALESCE(message.usage.cache_creation_input_tokens, 0)::BIGINT
                        AS cache_creation_input_tokens,
                    COALESCE(message.usage.cache_read_input_tokens, 0)::BIGINT
                        AS cache_read_input_tokens,
                    ROW_NUMBER() OVER (
                        PARTITION BY COALESCE(message.id, gen_random_uuid()::TEXT)
                        ORDER BY try_cast(timestamp AS TIMESTAMP) DESC
                    ) AS rn,
                FROM read_json_auto({file_list},
                    format='newline_delimited',
                    union_by_name=true,
                    ignore_errors=true,
                    filename=true)
                WHERE try_cast(timestamp AS TIMESTAMP) IS NOT NULL
            ) WHERE rn = 1
            "#
        );

        match self.con.execute(&sql, []) {
            Ok(_) => {
                self.last_max_mtime = max_mtime;
                self.initialized = true;

                let n: i64 = self
                    .con
                    .prepare("SELECT COUNT(*) FROM entries")
                    .and_then(|mut stmt| {
                        stmt.query_row([], |row| row.get(0))
                    })
                    .unwrap_or(0);

                info!("Loaded {} files, {} rows", files.len(), n);
                true
            }
            Err(e) => {
                error!("Failed to load: {}", e);
                false
            }
        }
    }

    /// Aggregate token totals for all entries since `since_str`
    /// (format: "YYYY-MM-DD HH:MM:SS").
    pub fn query_since(&self, since_str: &str) -> TokenTotals {
        if !self.initialized {
            return TokenTotals::default();
        }
        match self.con.prepare(
            r#"
            SELECT
                COALESCE(SUM(input_tokens), 0)::BIGINT,
                COALESCE(SUM(output_tokens), 0)::BIGINT,
                COALESCE(SUM(cache_creation_input_tokens), 0)::BIGINT,
                COALESCE(SUM(cache_read_input_tokens), 0)::BIGINT
            FROM entries
            WHERE ts >= ?::TIMESTAMP
            "#,
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
                Err(e) => {
                    warn!("query_since failed: {}", e);
                    TokenTotals::default()
                }
            },
            Err(e) => {
                warn!("query_since prepare failed: {}", e);
                TokenTotals::default()
            }
        }
    }

    /// Return per-entry (epoch_seconds, cost) pairs ordered by time.
    pub fn query_cost_timeline(&self, since_str: &str) -> Vec<(f64, f64)> {
        if !self.initialized {
            return Vec::new();
        }
        match self.con.prepare(
            r#"
            SELECT epoch(ts)::DOUBLE,
                (input_tokens / 1e6) * ? +
                (output_tokens / 1e6) * ? +
                (cache_creation_input_tokens / 1e6) * ? +
                (cache_read_input_tokens / 1e6) * ?
            FROM entries
            WHERE ts >= ?::TIMESTAMP
            ORDER BY ts
            "#,
        ) {
            Ok(mut stmt) => {
                match stmt.query_map(
                    duckdb::params![
                        pricing::INPUT_PRICE,
                        pricing::OUTPUT_PRICE,
                        pricing::CACHE_WRITE_PRICE,
                        pricing::CACHE_READ_PRICE,
                        since_str,
                    ],
                    |row| {
                        let epoch: f64 = row.get(0)?;
                        let cost: f64 = row.get(1)?;
                        Ok((epoch, cost))
                    },
                ) {
                    Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
                    Err(e) => {
                        warn!("Cost timeline query failed: {}", e);
                        Vec::new()
                    }
                }
            }
            Err(e) => {
                warn!("Cost timeline prepare failed: {}", e);
                Vec::new()
            }
        }
    }

    /// Return per-category dollar costs since `since_str`.
    pub fn query_cost_breakdown(&self, since_str: &str) -> CostBreakdown {
        if !self.initialized {
            return CostBreakdown::default();
        }
        match self.con.prepare(
            r#"
            SELECT
                COALESCE(SUM(input_tokens), 0) / 1e6 * ?,
                COALESCE(SUM(output_tokens), 0) / 1e6 * ?,
                COALESCE(SUM(cache_creation_input_tokens), 0) / 1e6 * ?,
                COALESCE(SUM(cache_read_input_tokens), 0) / 1e6 * ?
            FROM entries
            WHERE ts >= ?::TIMESTAMP
            "#,
        ) {
            Ok(mut stmt) => match stmt.query_row(
                duckdb::params![
                    pricing::INPUT_PRICE,
                    pricing::OUTPUT_PRICE,
                    pricing::CACHE_WRITE_PRICE,
                    pricing::CACHE_READ_PRICE,
                    since_str,
                ],
                |row| {
                    Ok(CostBreakdown {
                        input: row.get(0)?,
                        output: row.get(1)?,
                        cache_write: row.get(2)?,
                        cache_read: row.get(3)?,
                    })
                },
            ) {
                Ok(cb) => cb,
                Err(e) => {
                    warn!("Cost breakdown query failed: {}", e);
                    CostBreakdown::default()
                }
            },
            Err(e) => {
                warn!("Cost breakdown prepare failed: {}", e);
                CostBreakdown::default()
            }
        }
    }

    /// Return per-entry token breakdown ordered by time.
    /// Each tuple: (epoch_seconds, input, output, cache_write, cache_read).
    pub fn query_token_timeline(&self, since_str: &str) -> Vec<(f64, i64, i64, i64, i64)> {
        if !self.initialized {
            return Vec::new();
        }
        match self.con.prepare(
            r#"
            SELECT epoch(ts)::DOUBLE, input_tokens, output_tokens,
                   cache_creation_input_tokens, cache_read_input_tokens
            FROM entries
            WHERE ts >= ?::TIMESTAMP
            ORDER BY ts
            "#,
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
                Err(e) => {
                    warn!("Token timeline query failed: {}", e);
                    Vec::new()
                }
            },
            Err(e) => {
                warn!("Token timeline prepare failed: {}", e);
                Vec::new()
            }
        }
    }

    /// Count distinct session files (sources) since `since_str`,
    /// and those active since `active_str`.
    /// Returns (total_sessions, active_sessions).
    pub fn count_sessions(&self, since_str: &str, active_str: &str) -> (i64, i64) {
        if !self.initialized {
            return (0, 0);
        }
        match self.con.prepare(
            r#"
            SELECT
                COUNT(DISTINCT src) FILTER (WHERE ts >= ?::TIMESTAMP),
                COUNT(DISTINCT src) FILTER (WHERE ts >= ?::TIMESTAMP)
            FROM entries
            "#,
        ) {
            Ok(mut stmt) => match stmt.query_row([since_str, active_str], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            }) {
                Ok(counts) => counts,
                Err(e) => {
                    warn!("Session count query failed: {}", e);
                    (0, 0)
                }
            },
            Err(e) => {
                warn!("Session count prepare failed: {}", e);
                (0, 0)
            }
        }
    }
}
