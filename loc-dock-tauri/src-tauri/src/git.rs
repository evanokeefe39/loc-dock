use chrono::{DateTime, FixedOffset};
use log::warn;
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

/// A single numstat entry from a git commit, carrying the author timestamp
/// and lines added/deleted for one file in that commit.
pub struct GitPoint {
    pub ts: DateTime<FixedOffset>,
    pub added: i64,
    pub deleted: i64,
}

static GIT_AVAILABLE: OnceLock<bool> = OnceLock::new();

/// Check (once) whether `git` is reachable on PATH.
fn check_git() -> bool {
    *GIT_AVAILABLE.get_or_init(|| {
        let mut cmd = Command::new("git");
        cmd.args(["--version"]);
        configure_no_window(&mut cmd);
        match cmd.output() {
            Ok(output) => output.status.success(),
            Err(_) => {
                warn!("git not found on PATH; LOC tracking disabled");
                false
            }
        }
    })
}

/// On Windows, set CREATE_NO_WINDOW so subprocess consoles are suppressed.
#[cfg(target_os = "windows")]
fn configure_no_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(target_os = "windows"))]
fn configure_no_window(_cmd: &mut Command) {}

/// Scan every git repository directly under `repos_dir` and return
/// per-file commit entries with timestamps, lines added, and lines deleted
/// since `since_iso` (an ISO-8601 string such as "2025-06-09T07:00:00+02:00").
///
/// Repos that time out (10 s) or fail are silently skipped.
/// Results are sorted ascending by timestamp.
pub fn get_git_loc_timeline(repos_dir: &Path, since_iso: &str) -> Vec<GitPoint> {
    let mut points = Vec::new();

    if !check_git() || !repos_dir.exists() {
        return points;
    }

    let entries = match std::fs::read_dir(repos_dir) {
        Ok(e) => e,
        Err(_) => return points,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() || !path.join(".git").exists() {
            continue;
        }

        let mut cmd = Command::new("git");
        cmd.args([
            "log",
            &format!("--since={}", since_iso),
            "--format=%aI",
            "--numstat",
        ])
        .current_dir(&path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
        configure_no_window(&mut cmd);

        let output = match cmd.output() {
            Ok(o) if o.status.success() => o,
            _ => continue,
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut current_time: Option<DateTime<FixedOffset>> = None;

        for line in stdout.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            // Numstat lines start with a digit and contain tabs:
            //   123\t45\tpath/to/file
            if line.starts_with(|c: char| c.is_ascii_digit()) && line.contains('\t') {
                if let Some(ts) = current_time {
                    let parts: Vec<&str> = line.split('\t').collect();
                    // Binary files show "-" for added/deleted; skip those.
                    if parts.len() >= 2 && parts[0] != "-" && parts[1] != "-" {
                        if let (Ok(a), Ok(d)) = (parts[0].parse::<i64>(), parts[1].parse::<i64>())
                        {
                            points.push(GitPoint {
                                ts,
                                added: a,
                                deleted: d,
                            });
                        }
                    }
                }
            } else if let Ok(ts) = DateTime::parse_from_rfc3339(line) {
                current_time = Some(ts);
            } else {
                // Fallback: some git versions emit offsets without colon (e.g. +0200).
                current_time = DateTime::parse_from_str(line, "%Y-%m-%dT%H:%M:%S%z").ok();
            }
        }
    }

    points.sort_by_key(|p| p.ts);
    points
}
