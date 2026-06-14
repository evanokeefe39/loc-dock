use chrono::Utc;
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static LOG_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn init(log_dir: &Path) {
    let _ = std::fs::create_dir_all(log_dir);
    let path = log_dir.join("jobs.log");
    *LOG_DIR.lock().unwrap() = Some(path);
}

pub fn log(job: &str, status: &str, message: &str) {
    let guard = LOG_DIR.lock().unwrap();
    let Some(path) = guard.as_ref() else { return };
    let ts = Utc::now().format("%Y-%m-%d %H:%M:%S");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "[{}] [{}] [{}] {}", ts, job, status, message);
    }
}

pub fn log_ok(job: &str, message: &str) {
    log(job, "OK", message);
}

pub fn log_err(job: &str, message: &str) {
    log(job, "ERROR", message);
}

#[derive(Serialize)]
pub struct LogEntry {
    pub timestamp: String,
    pub job: String,
    pub status: String,
    pub message: String,
}

pub fn read_logs(log_dir: &Path, limit: usize) -> Vec<LogEntry> {
    let path = log_dir.join("jobs.log");
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(limit);
    lines[start..]
        .iter()
        .rev()
        .filter_map(|line| parse_log_line(line))
        .collect()
}

fn parse_log_line(line: &str) -> Option<LogEntry> {
    let rest = line.strip_prefix('[')?;
    let (timestamp, rest) = rest.split_once("] [")?;
    let (job, rest) = rest.split_once("] [")?;
    let (status, message) = rest.split_once("] ")?;
    Some(LogEntry {
        timestamp: timestamp.to_string(),
        job: job.to_string(),
        status: status.to_string(),
        message: message.to_string(),
    })
}

pub fn clear_logs(log_dir: &Path) -> Result<(), String> {
    let path = log_dir.join("jobs.log");
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn parse_log_line_valid() {
        let entry = parse_log_line("[2026-06-14 10:00:00] [data] [OK] Refreshed in 100ms").unwrap();
        assert_eq!(entry.timestamp, "2026-06-14 10:00:00");
        assert_eq!(entry.job, "data");
        assert_eq!(entry.status, "OK");
        assert_eq!(entry.message, "Refreshed in 100ms");
    }

    #[test]
    fn parse_log_line_error() {
        let entry = parse_log_line("[2026-06-14 10:00:00] [summary] [ERROR] LLM failed: timeout").unwrap();
        assert_eq!(entry.status, "ERROR");
        assert_eq!(entry.job, "summary");
    }

    #[test]
    fn parse_log_line_invalid() {
        assert!(parse_log_line("garbage").is_none());
        assert!(parse_log_line("").is_none());
        assert!(parse_log_line("[only one bracket").is_none());
    }

    #[test]
    fn read_logs_returns_newest_first() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("jobs.log");
        std::fs::write(&path, "[2026-06-14 10:00:00] [a] [OK] first\n[2026-06-14 10:01:00] [b] [OK] second\n").unwrap();

        let entries = read_logs(dir.path(), 10);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].message, "second");
        assert_eq!(entries[1].message, "first");
    }

    #[test]
    fn read_logs_respects_limit() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("jobs.log");
        let mut content = String::new();
        for i in 0..20 {
            content.push_str(&format!("[2026-06-14 10:{:02}:00] [d] [OK] msg{}\n", i, i));
        }
        std::fs::write(&path, content).unwrap();

        let entries = read_logs(dir.path(), 5);
        assert_eq!(entries.len(), 5);
        assert_eq!(entries[0].message, "msg19");
    }

    #[test]
    fn clear_logs_removes_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("jobs.log");
        std::fs::write(&path, "data").unwrap();
        assert!(path.exists());

        clear_logs(dir.path()).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn read_logs_missing_file() {
        let dir = TempDir::new().unwrap();
        let entries = read_logs(dir.path(), 10);
        assert!(entries.is_empty());
    }
}
