use std::path::PathBuf;
use std::time::SystemTime;

// ── Source kinds ───────────────────────────────────────────────────────────

/// Identifies which silver-layer extraction SQL applies to a source's files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Claude,
    Pi,
}

impl SourceKind {
    pub fn name(self) -> &'static str {
        match self {
            SourceKind::Claude => "claude",
            SourceKind::Pi => "pi",
        }
    }
}

// ── Glob-based file discoverer ────────────────────────────────────────────

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

    /// Find all session files newer than `cutoff` (unix epoch seconds).
    /// Returns (file_paths, max_mtime_seen).
    pub fn discover_files(&self, cutoff: f64) -> Result<(Vec<PathBuf>, f64), String> {
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

// ── Source manager: pairs a discoverer with its source kind ─────────────────

pub struct SourceManager {
    pub pairs: Vec<(GlobFileDiscoverer, SourceKind)>,
}

impl SourceManager {
    pub fn with_discoverers(
        pairs: Vec<(GlobFileDiscoverer, SourceKind)>,
    ) -> Self {
        Self { pairs }
    }
}
