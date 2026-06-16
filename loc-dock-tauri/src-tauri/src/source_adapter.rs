use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

// ── Common normalized schema ───────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct NormalizedEntry {
    pub source: String,             // "claude" | "pi"
    pub session_id: String,         // UUID of the session file
    pub ts: DateTime<Utc>,          // timestamp of the entry
    pub model: Option<String>,      // model name
    pub provider: Option<String>,   // provider name
    pub role: Option<String>,       // "user" | "assistant" | "tool_result"
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_input_tokens: i64,
    pub cache_read_input_tokens: i64,
    pub input_cost: f64,
    pub output_cost: f64,
    pub cache_write_cost: f64,
    pub cache_read_cost: f64,
    pub total_cost: f64,
    pub file_path: String,          // source file path
}

// ── File discovery trait ───────────────────────────────────────────────────

/// Discovers session files on disk.
pub trait FileDiscoverer: Send {
    /// Find all session files newer than `cutoff` (unix epoch seconds).
    /// Returns (file_paths, max_mtime_seen).
    fn discover_files(&self, cutoff: f64) -> Result<(Vec<PathBuf>, f64), String>;
}

// ── Shared glob-based file discoverer ──────────────────────────────────────

pub struct GlobFileDiscoverer {
    root_dir: PathBuf,
    glob_pattern: String,
    skip_subdirs: Vec<String>,
}

impl GlobFileDiscoverer {
    /// `skip_subdirs` — path components to skip (e.g. `["subagents"]`).
    pub fn new(root_dir: PathBuf, skip_subdirs: Vec<String>) -> Self {
        let pattern = root_dir.join("**/*.jsonl");
        let glob_pattern = pattern.to_string_lossy().replace('\\', "/");
        Self { root_dir, glob_pattern, skip_subdirs }
    }
}

impl FileDiscoverer for GlobFileDiscoverer {
    fn discover_files(&self, cutoff: f64) -> Result<(Vec<PathBuf>, f64), String> {
        let mut files = Vec::new();
        let mut max_mtime = 0.0;

        if !self.root_dir.exists() {
            return Ok((files, max_mtime));
        }

        for entry in glob::glob(&self.glob_pattern).unwrap_or_else(|_| glob::glob("").unwrap()) {
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
                // Skip files in excluded subdirectories
                if self.skip_subdirs.iter().any(|dir| {
                    p.components().any(|c| c.as_os_str() == dir.as_str())
                }) {
                    continue;
                }
                files.push(p);
            }
        }

        Ok((files, max_mtime))
    }
}

// ── Session parser trait ───────────────────────────────────────────────────

/// Parses a single session JSONL file into normalized entries.
pub trait SessionParser: Send + Sync {
    fn name(&self) -> &str;
    /// Parse content that has already been read from disk.
    /// Used by append-only file tracking to avoid re-reading unchanged bytes.
    fn parse_content(&self, path: &Path, content: &str) -> Vec<NormalizedEntry>;
}

// ── Claude parser ──────────────────────────────────────────────────────────

pub struct ClaudeParser;

impl ClaudeParser {
    pub fn new() -> Self { Self }
}

impl Default for ClaudeParser {
    fn default() -> Self { Self }
}

impl SessionParser for ClaudeParser {
    fn name(&self) -> &str { "claude" }

    fn parse_content(&self, path: &Path, content: &str) -> Vec<NormalizedEntry> {
        let filename = path.to_string_lossy().replace('\\', "/");
        let session_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        let mut entries = Vec::new();

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() { continue; }

            let raw: serde_json::Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };

            // Only process assistant type entries (they carry usage data)
            if raw.get("type").and_then(|v| v.as_str()).unwrap_or("") != "assistant" {
                continue;
            }

            let ts_str = raw.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");
            let ts = parse_iso_timestamp(ts_str);

            let session_id_val = raw
                .get("sessionId")
                .and_then(|v| v.as_str())
                .unwrap_or(&session_id)
                .to_string();

            let msg = match raw.get("message") {
                Some(m) => m,
                None => continue,
            };

            let model = msg.get("model").and_then(|v| v.as_str()).map(|s| s.to_string());

            let usage = match msg.get("usage") {
                Some(u) => u,
                None => continue,
            };

            // Extract tokens only. Cost is filled by SourceManager post-process.
            entries.push(NormalizedEntry {
                source: "claude".to_string(),
                session_id: session_id_val,
                ts,
                model,
                provider: Some("anthropic".to_string()),
                role: Some("assistant".to_string()),
                input_tokens: usage.get("input_tokens").and_then(|v| v.as_i64()).unwrap_or(0),
                output_tokens: usage.get("output_tokens").and_then(|v| v.as_i64()).unwrap_or(0),
                cache_creation_input_tokens: usage.get("cache_creation_input_tokens").and_then(|v| v.as_i64()).unwrap_or(0),
                cache_read_input_tokens: usage.get("cache_read_input_tokens").and_then(|v| v.as_i64()).unwrap_or(0),
                // Costs left as 0 — filled by fill_flat_pricing post-process
                input_cost: 0.0,
                output_cost: 0.0,
                cache_write_cost: 0.0,
                cache_read_cost: 0.0,
                total_cost: 0.0,
                file_path: filename.clone(),
            });
        }

        entries
    }
}

// ── Pi parser ──────────────────────────────────────────────────────────────

pub struct PiParser;

impl PiParser {
    pub fn new() -> Self { Self }
}

impl Default for PiParser {
    fn default() -> Self { Self }
}

impl SessionParser for PiParser {
    fn name(&self) -> &str { "pi" }

    fn parse_content(&self, path: &Path, content: &str) -> Vec<NormalizedEntry> {
        let filename = path.to_string_lossy().replace('\\', "/");
        let session_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.split('_').nth(1))
            .unwrap_or("unknown")
            .to_string();

        let mut entries = Vec::new();
        let mut current_model: Option<String> = None;
        let mut current_provider: Option<String> = None;

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() { continue; }

            let raw: serde_json::Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let entry_type = raw.get("type").and_then(|v| v.as_str()).unwrap_or("");

            // Track model/provider changes across the session
            if entry_type == "model_change" {
                current_model = raw.get("modelId").and_then(|v| v.as_str()).map(|s| s.to_string());
                current_provider = raw.get("provider").and_then(|v| v.as_str()).map(|s| s.to_string());
                continue;
            }

            if entry_type != "message" { continue; }

            let msg = match raw.get("message") {
                Some(m) => m,
                None => continue,
            };

            if msg.get("role").and_then(|v| v.as_str()).unwrap_or("") != "assistant" {
                continue;
            }

            // Timestamp: prefer message-level unix ms, fallback to top-level ISO
            let ts = msg
                .get("timestamp")
                .and_then(|v| v.as_i64())
                .map(|ms| {
                    let secs = ms / 1000;
                    let nsecs = ((ms % 1000) * 1_000_000) as u32;
                    DateTime::from_timestamp(secs, nsecs).unwrap_or(Utc::now())
                })
                .or_else(|| {
                    raw.get("timestamp")
                        .and_then(|v| v.as_str())
                        .map(parse_iso_timestamp)
                })
                .unwrap_or(Utc::now());

            let model = msg.get("model")
                .and_then(|v| v.as_str()).map(|s| s.to_string())
                .or_else(|| current_model.clone());

            let provider = msg.get("provider")
                .and_then(|v| v.as_str()).map(|s| s.to_string())
                .or_else(|| current_provider.clone());

            let usage = match msg.get("usage") {
                Some(u) => u,
                None => continue,
            };

            // Pi uses camelCase field names
            let input_tokens = usage.get("input").and_then(|v| v.as_i64()).unwrap_or(0);
            let output_tokens = usage.get("output").and_then(|v| v.as_i64()).unwrap_or(0);
            let cache_write = usage.get("cacheWrite").and_then(|v| v.as_i64()).unwrap_or(0);
            let cache_read = usage.get("cacheRead").and_then(|v| v.as_i64()).unwrap_or(0);

            // Pi has cost nested inside usage
            let input_cost = usage.get("cost").and_then(|c| c.get("input")).and_then(|v| v.as_f64()).unwrap_or(0.0);
            let output_cost = usage.get("cost").and_then(|c| c.get("output")).and_then(|v| v.as_f64()).unwrap_or(0.0);
            let cache_write_cost = usage.get("cost").and_then(|c| c.get("cacheWrite")).and_then(|v| v.as_f64()).unwrap_or(0.0);
            let cache_read_cost = usage.get("cost").and_then(|c| c.get("cacheRead")).and_then(|v| v.as_f64()).unwrap_or(0.0);
            let total_cost = usage.get("cost").and_then(|c| c.get("total")).and_then(|v| v.as_f64()).unwrap_or(0.0);

            entries.push(NormalizedEntry {
                source: "pi".to_string(),
                session_id: session_id.clone(),
                ts,
                model,
                provider,
                role: Some("assistant".to_string()),
                input_tokens,
                output_tokens,
                cache_creation_input_tokens: cache_write,
                cache_read_input_tokens: cache_read,
                input_cost,
                output_cost,
                cache_write_cost,
                cache_read_cost,
                total_cost,
                file_path: filename.clone(),
            });
        }

        entries
    }
}

// ── SourceManager: orchestrates discovery + parsing + cost filling ─────────

pub struct SourceManager {
    pub pairs: Vec<(Box<dyn FileDiscoverer>, Box<dyn SessionParser>)>,
}

impl SourceManager {
    pub fn with_discoverers(
        pairs: Vec<(Box<dyn FileDiscoverer>, Box<dyn SessionParser>)>,
    ) -> Self {
        Self { pairs }
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn parse_iso_timestamp(s: &str) -> DateTime<Utc> {
    s.parse::<DateTime<Utc>>()
        .unwrap_or_else(|_| DateTime::parse_from_rfc3339(s).map(|d| d.to_utc()).unwrap_or(Utc::now()))
}
