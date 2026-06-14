use crate::git::GitPoint;
use chrono::{DateTime, FixedOffset};
use duckdb::Connection;
use log::info;
use std::collections::HashSet;
use std::path::Path;

const DB_NAME: &str = "git_cache.db";
const MARKER: &str = "git_cache.db.reset";

pub struct GitCache {
    con: Connection,
}

impl GitCache {
    pub fn new(cache_dir: &Path) -> Self {
        let _ = std::fs::create_dir_all(cache_dir);

        let marker = cache_dir.join(MARKER);
        let db_path = cache_dir.join(DB_NAME);
        if marker.exists() {
            let _ = std::fs::remove_file(&db_path);
            let _ = std::fs::remove_file(&marker);
            info!("Git cache reset via marker file");
        }

        let con = Connection::open(&db_path).expect("failed to open git_cache.db");
        con.execute_batch(
            "CREATE TABLE IF NOT EXISTS git_points (
                repo TEXT NOT NULL,
                ts TEXT NOT NULL,
                added BIGINT NOT NULL,
                deleted BIGINT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS repo_meta (
                repo TEXT PRIMARY KEY,
                head_sha TEXT NOT NULL
            );",
        )
        .expect("failed to create git cache tables");

        GitCache { con }
    }

    pub fn get_head_sha(&self, repo: &str) -> Option<String> {
        self.con
            .prepare("SELECT head_sha FROM repo_meta WHERE repo = ?")
            .ok()
            .and_then(|mut stmt| {
                stmt.query_row(duckdb::params![repo], |row| row.get::<_, String>(0))
                    .ok()
            })
    }

    pub fn set_head_sha(&self, repo: &str, sha: &str) {
        let _ = self.con.execute(
            "INSERT OR REPLACE INTO repo_meta (repo, head_sha) VALUES (?, ?)",
            duckdb::params![repo, sha],
        );
    }

    pub fn latest_ts(&self, repo: &str) -> Option<String> {
        self.con
            .prepare("SELECT MAX(ts) FROM git_points WHERE repo = ?")
            .ok()
            .and_then(|mut stmt| {
                stmt.query_row(duckdb::params![repo], |row| row.get::<_, String>(0))
                    .ok()
            })
    }

    pub fn insert_points(&self, repo: &str, points: &[GitPoint]) {
        if points.is_empty() {
            return;
        }
        let _ = self.con.execute("BEGIN TRANSACTION", []);
        let mut stmt = match self.con.prepare(
            "INSERT INTO git_points (repo, ts, added, deleted) VALUES (?, ?, ?, ?)",
        ) {
            Ok(s) => s,
            Err(_) => {
                let _ = self.con.execute("ROLLBACK", []);
                return;
            }
        };
        for p in points {
            let ts_str = p.ts.to_rfc3339();
            let _ = stmt.execute(duckdb::params![repo, ts_str, p.added, p.deleted]);
        }
        let _ = self.con.execute("COMMIT", []);
    }

    pub fn query_since(&self, since_iso: &str) -> Vec<GitPoint> {
        let mut stmt = match self
            .con
            .prepare("SELECT ts, added, deleted FROM git_points WHERE ts >= ? ORDER BY ts")
        {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        match stmt.query_map(duckdb::params![since_iso], |row| {
            let ts_str: String = row.get(0)?;
            let added: i64 = row.get(1)?;
            let deleted: i64 = row.get(2)?;
            Ok((ts_str, added, deleted))
        }) {
            Ok(rows) => rows
                .filter_map(|r| r.ok())
                .filter_map(|(ts_str, added, deleted)| {
                    DateTime::parse_from_rfc3339(&ts_str)
                        .or_else(|_| DateTime::parse_from_str(&ts_str, "%Y-%m-%dT%H:%M:%S%z"))
                        .ok()
                        .map(|ts: DateTime<FixedOffset>| GitPoint { ts, added, deleted })
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn purge_repo(&self, repo: &str) {
        let _ = self
            .con
            .execute("DELETE FROM git_points WHERE repo = ?", duckdb::params![repo]);
        let _ = self
            .con
            .execute("DELETE FROM repo_meta WHERE repo = ?", duckdb::params![repo]);
    }

    pub fn prune_before(&self, since_iso: &str) {
        let _ = self
            .con
            .execute("DELETE FROM git_points WHERE ts < ?", duckdb::params![since_iso]);
    }

    pub fn cached_repos(&self) -> HashSet<String> {
        let mut stmt = match self.con.prepare("SELECT DISTINCT repo FROM git_points") {
            Ok(s) => s,
            Err(_) => return HashSet::new(),
        };
        match stmt.query_map([], |row| row.get::<_, String>(0)) {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(_) => HashSet::new(),
        }
    }

    pub fn reset(cache_dir: &Path) -> Result<(), String> {
        let marker = cache_dir.join(MARKER);
        std::fs::write(&marker, "").map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_point(ts_str: &str, added: i64, deleted: i64) -> GitPoint {
        let ts = DateTime::parse_from_rfc3339(ts_str).unwrap();
        GitPoint { ts, added, deleted }
    }

    #[test]
    fn insert_and_query() {
        let dir = TempDir::new().unwrap();
        let cache = GitCache::new(dir.path());

        let points = vec![
            make_point("2026-06-10T10:00:00+00:00", 10, 2),
            make_point("2026-06-11T12:00:00+00:00", 5, 1),
        ];
        cache.insert_points("repo-a", &points);

        let result = cache.query_since("2026-06-10T00:00:00+00:00");
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].added, 10);
        assert_eq!(result[1].added, 5);
    }

    #[test]
    fn query_since_filters() {
        let dir = TempDir::new().unwrap();
        let cache = GitCache::new(dir.path());

        cache.insert_points("r", &[
            make_point("2026-06-09T10:00:00+00:00", 1, 0),
            make_point("2026-06-11T10:00:00+00:00", 2, 0),
        ]);

        let result = cache.query_since("2026-06-10T00:00:00+00:00");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].added, 2);
    }

    #[test]
    fn purge_repo() {
        let dir = TempDir::new().unwrap();
        let cache = GitCache::new(dir.path());

        cache.insert_points("keep", &[make_point("2026-06-10T10:00:00+00:00", 1, 0)]);
        cache.insert_points("remove", &[make_point("2026-06-10T10:00:00+00:00", 2, 0)]);
        cache.set_head_sha("keep", "aaa");
        cache.set_head_sha("remove", "bbb");

        cache.purge_repo("remove");

        assert_eq!(cache.query_since("2026-06-01T00:00:00+00:00").len(), 1);
        assert!(cache.get_head_sha("remove").is_none());
        assert_eq!(cache.get_head_sha("keep").unwrap(), "aaa");
    }

    #[test]
    fn prune_before() {
        let dir = TempDir::new().unwrap();
        let cache = GitCache::new(dir.path());

        cache.insert_points("r", &[
            make_point("2026-06-01T10:00:00+00:00", 1, 0),
            make_point("2026-06-10T10:00:00+00:00", 2, 0),
        ]);

        cache.prune_before("2026-06-05T00:00:00+00:00");
        let result = cache.query_since("2026-06-01T00:00:00+00:00");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].added, 2);
    }

    #[test]
    fn head_sha_roundtrip() {
        let dir = TempDir::new().unwrap();
        let cache = GitCache::new(dir.path());

        assert!(cache.get_head_sha("repo").is_none());
        cache.set_head_sha("repo", "abc123");
        assert_eq!(cache.get_head_sha("repo").unwrap(), "abc123");
        cache.set_head_sha("repo", "def456");
        assert_eq!(cache.get_head_sha("repo").unwrap(), "def456");
    }

    #[test]
    fn cached_repos() {
        let dir = TempDir::new().unwrap();
        let cache = GitCache::new(dir.path());

        cache.insert_points("alpha", &[make_point("2026-06-10T10:00:00+00:00", 1, 0)]);
        cache.insert_points("beta", &[make_point("2026-06-10T10:00:00+00:00", 2, 0)]);

        let repos = cache.cached_repos();
        assert!(repos.contains("alpha"));
        assert!(repos.contains("beta"));
        assert_eq!(repos.len(), 2);
    }

    #[test]
    fn latest_ts() {
        let dir = TempDir::new().unwrap();
        let cache = GitCache::new(dir.path());

        assert!(cache.latest_ts("r").is_none());
        cache.insert_points("r", &[
            make_point("2026-06-10T10:00:00+00:00", 1, 0),
            make_point("2026-06-12T10:00:00+00:00", 2, 0),
        ]);
        let ts = cache.latest_ts("r").unwrap();
        assert!(ts.contains("2026-06-12"));
    }

    #[test]
    fn reset_marker_clears_db() {
        let dir = TempDir::new().unwrap();
        let cache = GitCache::new(dir.path());
        cache.insert_points("r", &[make_point("2026-06-10T10:00:00+00:00", 1, 0)]);
        drop(cache);

        GitCache::reset(dir.path()).unwrap();
        assert!(dir.path().join(MARKER).exists());

        let cache2 = GitCache::new(dir.path());
        assert!(!dir.path().join(MARKER).exists());
        assert_eq!(cache2.query_since("2026-06-01T00:00:00+00:00").len(), 0);
    }

    #[test]
    fn persists_across_instances() {
        let dir = TempDir::new().unwrap();
        {
            let cache = GitCache::new(dir.path());
            cache.insert_points("r", &[make_point("2026-06-10T10:00:00+00:00", 5, 3)]);
            cache.set_head_sha("r", "sha1");
        }
        {
            let cache = GitCache::new(dir.path());
            let points = cache.query_since("2026-06-01T00:00:00+00:00");
            assert_eq!(points.len(), 1);
            assert_eq!(points[0].added, 5);
            assert_eq!(cache.get_head_sha("r").unwrap(), "sha1");
        }
    }
}
