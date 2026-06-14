use crate::git_cache::GitCache;
use chrono::{DateTime, FixedOffset};
use log::{info, warn};
use std::collections::HashSet;
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

fn get_head_sha(path: &Path) -> Option<String> {
    let mut cmd = Command::new("git");
    cmd.args(["rev-parse", "HEAD"])
        .current_dir(path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    configure_no_window(&mut cmd);
    cmd.output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

fn parse_git_log(stdout: &str) -> Vec<GitPoint> {
    let mut points = Vec::new();
    let mut current_time: Option<DateTime<FixedOffset>> = None;

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with(|c: char| c.is_ascii_digit()) && line.contains('\t') {
            if let Some(ts) = current_time {
                let parts: Vec<&str> = line.split('\t').collect();
                if parts.len() >= 2 && parts[0] != "-" && parts[1] != "-" {
                    if let (Ok(a), Ok(d)) = (parts[0].parse::<i64>(), parts[1].parse::<i64>()) {
                        points.push(GitPoint { ts, added: a, deleted: d });
                    }
                }
            }
        } else if let Ok(ts) = DateTime::parse_from_rfc3339(line) {
            current_time = Some(ts);
        } else {
            current_time = DateTime::parse_from_str(line, "%Y-%m-%dT%H:%M:%S%z").ok();
        }
    }
    points
}

fn run_git_log(path: &Path, since: &str) -> Vec<GitPoint> {
    let mut cmd = Command::new("git");
    cmd.args(["log", &format!("--since={}", since), "--format=%aI", "--numstat"])
        .current_dir(path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    configure_no_window(&mut cmd);

    match cmd.output() {
        Ok(o) if o.status.success() => parse_git_log(&String::from_utf8_lossy(&o.stdout)),
        _ => Vec::new(),
    }
}

/// Scan git repos under `repos_dir`, using `cache` for incremental updates.
/// Returns all points since `since_iso`, sorted ascending by timestamp.
pub fn get_git_loc_timeline(repos_dir: &Path, since_iso: &str, cache: &GitCache) -> Vec<GitPoint> {
    if !check_git() || !repos_dir.exists() {
        return cache.query_since(since_iso);
    }

    let entries = match std::fs::read_dir(repos_dir) {
        Ok(e) => e,
        Err(_) => return cache.query_since(since_iso),
    };

    let mut disk_repos = HashSet::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() || !path.join(".git").exists() {
            continue;
        }
        let repo_name = match path.file_name() {
            Some(n) => n.to_string_lossy().to_string(),
            None => continue,
        };
        disk_repos.insert(repo_name.clone());

        let current_sha = match get_head_sha(&path) {
            Some(sha) => sha,
            None => continue,
        };

        let cached_sha = cache.get_head_sha(&repo_name);
        let sha_matches = cached_sha.as_deref() == Some(&current_sha);

        if !sha_matches {
            if cached_sha.is_some() {
                info!("Git cache: SHA mismatch for {}, purging", repo_name);
            }
            cache.purge_repo(&repo_name);
            let new_points = run_git_log(&path, since_iso);
            cache.insert_points(&repo_name, &new_points);
            cache.set_head_sha(&repo_name, &current_sha);
        } else {
            let effective_since = cache.latest_ts(&repo_name).unwrap_or_else(|| since_iso.to_string());
            let new_points = run_git_log(&path, &effective_since);
            if !new_points.is_empty() {
                cache.insert_points(&repo_name, &new_points);
                cache.set_head_sha(&repo_name, &current_sha);
            }
        }
    }

    for stale in cache.cached_repos().difference(&disk_repos) {
        info!("Git cache: repo {} removed from disk, purging", stale);
        cache.purge_repo(stale);
    }

    cache.prune_before(since_iso);
    cache.query_since(since_iso)
}
