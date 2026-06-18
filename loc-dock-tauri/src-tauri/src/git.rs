use chrono::{DateTime, FixedOffset};
use log::warn;
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

/// Per-commit metadata with aggregated LOC and commit message.
/// One row per (repo, sha) — added/deleted are summed across all files.
pub struct GitCommit {
    pub sha: String,
    pub ts: DateTime<FixedOffset>,
    pub msg: String,
    pub added: i64,
    pub deleted: i64,
    pub file_count: usize,
}

/// Commits collected from one repo.
pub struct RepoCommits {
    pub repo: String,
    pub head_sha: String,
    pub commits: Vec<GitCommit>,
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

// ── Per-repo commit scanning with SHA + message ─────────────────────────────

/// Run `git log` on one repo with the given `--after` timestamp.
/// Returns a `RepoCommits` with per-commit aggregates and the HEAD SHA.
fn scan_one_repo(path: &Path, since_iso: &str) -> Option<RepoCommits> {
    let repo = path.file_name()?.to_string_lossy().to_string();

    let mut cmd = Command::new("git");
    // ponytail: single git log call with both header and numstat.
    // Format: %H|<ts>|<subject> interleaved with numstat lines.
    cmd.args([
        "log",
        &format!("--after={}", since_iso),
        "--format=%H|%aI|%s",
        "--numstat",
    ])
    .current_dir(path)
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::null());
    configure_no_window(&mut cmd);

    let output = cmd.output().ok().filter(|o| o.status.success())?;
    let commits = parse_git_commits(&String::from_utf8_lossy(&output.stdout));
    // ponytail: head_sha not used by insert_commits, but carried for future summary tracking.
    let head_sha = commits.first().map(|c| c.sha.clone()).unwrap_or_default();

    Some(RepoCommits { repo, head_sha, commits })
}

/// Parse `git log --format="%H|%aI|%s" --numstat` output.
/// Returns one `GitCommit` per commit with aggregated LOC.
fn parse_git_commits(stdout: &str) -> Vec<GitCommit> {
    let mut commits: Vec<GitCommit> = Vec::new();
    let mut sha: Option<String> = None;
    let mut ts: Option<DateTime<FixedOffset>> = None;
    let mut msg: Option<String> = None;
    let mut added: i64 = 0;
    let mut deleted: i64 = 0;
    let mut file_count: usize = 0;

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Detect header line: SHA|<ts>|<subject>  (SHA is 40 hex chars)
        // Numeric numstat lines start with digits and contain tabs.
        if line.starts_with(|c: char| c.is_ascii_digit()) && line.contains('\t') {
            // Numstat line
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() >= 2 && fields[0] != "-" && fields[1] != "-" {
                if let (Ok(a), Ok(d)) = (fields[0].parse::<i64>(), fields[1].parse::<i64>()) {
                    added += a;
                    deleted += d;
                    file_count += 1;
                }
            }
        } else if let Some((s, rest)) = line.split_once('|') {
            // Commit header candidate: hex SHA followed by |
            if s.len() >= 7 && s.chars().all(|c| c.is_ascii_hexdigit()) {
                // Finalize previous commit
                if let (Some(prev_sha), Some(prev_ts), Some(prev_msg)) = (sha.take(), ts.take(), msg.take()) {
                    commits.push(GitCommit {
                        sha: prev_sha,
                        ts: prev_ts,
                        msg: prev_msg,
                        added,
                        deleted,
                        file_count,
                    });
                }
                let (ts_str, rest2) = rest.split_once('|').unwrap_or((rest, ""));
                sha = Some(s.to_string());
                ts = DateTime::parse_from_rfc3339(ts_str)
                    .or_else(|_| DateTime::parse_from_str(ts_str, "%Y-%m-%dT%H:%M:%S%z"))
                    .ok();
                msg = Some(rest2.to_string());
                added = 0;
                deleted = 0;
                file_count = 0;
            }
        }
    }

    // Finalize last commit
    if let (Some(s), Some(t), Some(m)) = (sha, ts, msg) {
        commits.push(GitCommit {
            sha: s,
            ts: t,
            msg: m,
            added,
            deleted,
            file_count,
        });
    }

    commits
}

/// Scan all repos under `repos_dir` for commits newer than `since_iso`.
/// Returns one `RepoCommits` per repo that has new commits (empty repos excluded).
/// Caller stores results in DuckDB for incremental tracking.
pub fn collect_new_commits(repos_dir: &Path, since_iso: &str) -> Vec<RepoCommits> {
    if !check_git() || !repos_dir.exists() {
        return Vec::new();
    }

    let entries = match std::fs::read_dir(repos_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut results = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() || !path.join(".git").exists() {
            continue;
        }
        if let Some(rc) = scan_one_repo(&path, since_iso) {
            if !rc.commits.is_empty() {
                results.push(rc);
            }
        }
    }

    // Sort by earliest commit ts for consistent insertion order
    results.sort_by(|a, b| {
        a.commits
            .first()
            .map(|c| &c.ts)
            .cmp(&b.commits.first().map(|c| &c.ts))
    });
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_git_commits_basic() {
        let output = "\
abc123def4567890123456789012345678901234|2024-01-15T10:30:00+01:00|Add user auth
1\t1\tsrc/auth.rs
2\t0\tsrc/login.rs

fedcba9876543210987654321098765432109876|2024-01-15T11:00:00+01:00|Fix payment bug
5\t3\tsrc/pay.rs
0\t1\tsrc/refund.rs
";
        let commits = parse_git_commits(output);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].sha, "abc123def4567890123456789012345678901234");
        assert_eq!(commits[0].msg, "Add user auth");
        assert_eq!(commits[0].added, 3);
        assert_eq!(commits[0].deleted, 1);
        assert_eq!(commits[0].file_count, 2);
        assert_eq!(commits[1].sha, "fedcba9876543210987654321098765432109876");
        assert_eq!(commits[1].added, 5);
        assert_eq!(commits[1].deleted, 4);
        assert_eq!(commits[1].file_count, 2);
    }

    #[test]
    fn test_parse_git_commits_no_commits() {
        let commits = parse_git_commits("");
        assert!(commits.is_empty());
    }

    #[test]
    fn test_parse_git_commits_single_commit() {
        let output = "\
abcd1234abcd1234abcd1234abcd1234abcd1234|2024-06-01T00:00:00Z|Initial commit
10\t0\tsrc/main.rs
";
        let commits = parse_git_commits(output);
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].added, 10);
        assert_eq!(commits[0].deleted, 0);
        assert_eq!(commits[0].file_count, 1);
    }

    #[test]
    fn real_git_repo_scan() {
        // Create a real git repo with a commit, then scan it.
        let dir = std::env::temp_dir().join(format!(
            "locdock_git_test_{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        // Init git repo
        let init = Command::new("git")
            .args(["init"])
            .current_dir(&dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(init.status.success(), "git init failed");

        // Configure user (required for commit on some systems)
        let _ = Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(&dir)
            .output();
        let _ = Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(&dir)
            .output();

        // Create a file and commit it
        let file_path = dir.join("test.rs");
        std::fs::write(&file_path, "fn main() {}\nfn new_func() {}\n").unwrap();

        let add = Command::new("git")
            .args(["add", "test.rs"])
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(add.status.success(), "git add failed");

        let commit = Command::new("git")
            .args(["commit", "-m", "Initial commit with test.rs"])
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(commit.status.success(), "git commit failed: {}",
            String::from_utf8_lossy(&commit.stderr));

        // Now run collect_new_commits with a wide window
        let parent = dir.parent().unwrap();
        let since_7d_ago = "1970-01-01T00:00:00+0000";
        let results = collect_new_commits(parent, since_7d_ago);

        // We should find the repo and its one commit
        let repo_name = dir.file_name().unwrap().to_string_lossy();
        let our_repo = results.iter().find(|rc| rc.repo == repo_name);
        assert!(our_repo.is_some(), "Should find repo '{}' in results. Found: {:?}",
            repo_name, results.iter().map(|r| r.repo.as_str()).collect::<Vec<_>>());

        let rc = our_repo.unwrap();
        assert_eq!(rc.commits.len(), 1, "Should have 1 commit");
        assert_eq!(rc.commits[0].msg, "Initial commit with test.rs");
        assert!(rc.commits[0].added > 0, "LOC added should be > 0, got {}", rc.commits[0].added);
        assert_eq!(rc.commits[0].file_count, 1, "Should have 1 file");
        assert_eq!(rc.commits[0].sha.len(), 40, "SHA should be 40 chars");

        // Also test with the EXACT same since_iso format used in data.rs (DateTime<Utc> format)
        use chrono::{DateTime, Utc, Duration};
        let since_dt: DateTime<Utc> = Utc::now() - Duration::days(7);
        let since_fmt = since_dt.format("%Y-%m-%dT%H:%M:%S%z").to_string();
        assert!(since_fmt.contains("+0000") || since_fmt.contains("Z"),
            "Utc format should produce +0000 or Z, got: {}", since_fmt);

        let results2 = collect_new_commits(parent, &since_fmt);
        let our_repo2 = results2.iter().find(|rc| rc.repo == repo_name);
        assert!(our_repo2.is_some(), "Should find repo with data.rs format '{}'", since_fmt);
        assert_eq!(our_repo2.unwrap().commits.len(), 1, "Should have 1 commit with data.rs format");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_parse_git_commits_msg_with_pipe() {
        // Commit message contains "|" — splitn(3, '|') should keep it.
        let output = "\
abcd1234abcd1234abcd1234abcd1234abcd1234|2024-01-15T00:00:00Z|fix | bug | again
1\t1\tsrc/lib.rs
";
        let commits = parse_git_commits(output);
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].msg, "fix | bug | again");
    }
}
