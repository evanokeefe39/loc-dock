use crate::config::Config;
use crate::task_queue::TaskQueue;
use crate::time_utils;
use chrono::Utc;
use chrono_tz::Tz;
use duckdb::Connection;
use log::info;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct RepoSummary {
    pub name: String,
    pub commits: usize,
    pub highlights: Vec<String>,
}

#[derive(Serialize, Clone, Default)]
pub struct SummaryData {
    pub day_repos: Vec<RepoSummary>,
    pub day_repo_count: usize,
    pub day_commits: usize,
    pub day_prs: usize,
    pub week_repos: Vec<RepoSummary>,
    pub week_repo_count: usize,
    pub week_commits: usize,
    pub week_prs: usize,
    pub loading: bool,
    pub no_api_key: bool,
}

fn count_prs(repos: &[RepoSummary]) -> usize {
    let re = regex::Regex::new(r"PR-\d+").unwrap();
    let mut seen = std::collections::HashSet::new();
    for repo in repos {
        for h in &repo.highlights {
            for m in re.find_iter(h) {
                seen.insert(m.as_str().to_string());
            }
        }
    }
    seen.len()
}

pub fn perf_log_from(config_dir: &Path, msg: &str) {
    perf_log(config_dir, msg);
}

fn perf_log(config_dir: &Path, msg: &str) {
    let path = config_dir.join("perf.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let ts = Utc::now().format("%Y-%m-%d %H:%M:%S");
        let _ = writeln!(f, "[{}] {}", ts, msg);
    }
}

struct SummaryStore {
    con: Connection,
}

impl SummaryStore {
    fn new(config_dir: &Path) -> Self {
        let db_path = config_dir.join("summaries.db");
        let con = Connection::open(db_path).expect("failed to open summaries.db");
        con.execute_batch(
            "CREATE TABLE IF NOT EXISTS summaries (
                date TEXT NOT NULL,
                scope TEXT NOT NULL,
                summary TEXT NOT NULL,
                repo_shas TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (date, scope)
            )",
        )
        .expect("failed to create summaries table");
        SummaryStore { con }
    }

    fn get_summary(&self, date: &str, scope: &str) -> Option<String> {
        self.con
            .prepare("SELECT summary FROM summaries WHERE date = ? AND scope = ?")
            .ok()
            .and_then(|mut stmt| {
                stmt.query_row(duckdb::params![date, scope], |row| {
                    row.get::<_, String>(0)
                })
                .ok()
            })
    }

    fn get_repo_shas(&self, date: &str, scope: &str) -> HashMap<String, String> {
        let json = self
            .con
            .prepare("SELECT repo_shas FROM summaries WHERE date = ? AND scope = ?")
            .ok()
            .and_then(|mut stmt| {
                stmt.query_row(duckdb::params![date, scope], |row| {
                    row.get::<_, String>(0)
                })
                .ok()
            })
            .unwrap_or_else(|| "{}".to_string());
        serde_json::from_str(&json).unwrap_or_default()
    }

    fn save_summary(
        &self,
        date: &str,
        scope: &str,
        summary: &str,
        repo_shas: &HashMap<String, String>,
    ) {
        let shas_json = serde_json::to_string(repo_shas).unwrap_or_else(|_| "{}".to_string());
        let now = Utc::now().to_rfc3339();
        let _ = self.con.execute(
            "INSERT OR REPLACE INTO summaries (date, scope, summary, repo_shas, updated_at) VALUES (?, ?, ?, ?, ?)",
            duckdb::params![date, scope, summary, shas_json, now],
        );
    }
}

// --- Platform-specific subprocess window suppression ---

/// On Windows, suppress console window for git subprocesses.
#[cfg(target_os = "windows")]
fn configure_no_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(target_os = "windows"))]
fn configure_no_window(_cmd: &mut Command) {}

fn collect_commits_filtered(repos_dir: &Path, since_iso: &str, exclude_pattern: &str) -> HashMap<String, Vec<(String, String)>> {
    let exclude_re = if exclude_pattern.is_empty() {
        None
    } else {
        match regex::Regex::new(exclude_pattern) {
            Ok(re) => Some(re),
            Err(e) => {
                log::warn!("Invalid summary_exclude_pattern '{}': {}", exclude_pattern, e);
                None
            }
        }
    };
    let mut result: HashMap<String, Vec<(String, String)>> = HashMap::new();

    let entries = match std::fs::read_dir(repos_dir) {
        Ok(e) => e,
        Err(_) => return result,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() || !path.join(".git").exists() {
            continue;
        }

        let repo_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        let mut cmd = Command::new("git");
        cmd.args(["log", &format!("--since={}", since_iso), "--format=%H|%s"])
            .current_dir(&path)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        configure_no_window(&mut cmd);

        let output = match cmd.output() {
            Ok(o) if o.status.success() => o,
            _ => continue,
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let commits: Vec<(String, String)> = stdout
            .lines()
            .filter_map(|line| {
                let parts: Vec<&str> = line.splitn(2, '|').collect();
                if parts.len() == 2 {
                    let msg = parts[1];
                    if let Some(ref re) = exclude_re {
                        if re.is_match(msg) {
                            return None;
                        }
                    }
                    Some((parts[0].to_string(), msg.to_string()))
                } else {
                    None
                }
            })
            .collect();

        if !commits.is_empty() {
            result.insert(repo_name, commits);
        }
    }

    result
}

/// Get the latest SHA per repo from collected commits.
fn latest_shas(commits: &HashMap<String, Vec<(String, String)>>) -> HashMap<String, String> {
    commits
        .iter()
        .filter_map(|(repo, cs)| cs.first().map(|(sha, _)| (repo.clone(), sha.clone())))
        .collect()
}

/// Call an OpenAI-compatible chat completions API with retry + exponential backoff.
fn call_llm(api_key: &str, endpoint: &str, model: &str, prompt: &str, content: &str) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Client build failed: {}", e))?;
    let url = format!("{}/chat/completions", endpoint.trim_end_matches('/'));
    let body = serde_json::json!({
        "model": model,
        "messages": [
            { "role": "system", "content": prompt },
            { "role": "user", "content": content }
        ],
        "temperature": 0.3,
        "max_tokens": 512
    });

    let mut last_err = String::new();
    for attempt in 0..3 {
        if attempt > 0 {
            // Exponential backoff with jitter: 1s, 3s base + random 0-1s
            let base_ms = (1000 * (1 << attempt)) as u64;
            let jitter_ms = (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_millis() as u64) % 1000;
            std::thread::sleep(std::time::Duration::from_millis(base_ms + jitter_ms));
            info!("LLM retry attempt {} after {}ms", attempt + 1, base_ms + jitter_ms);
        }

        let resp = match client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
        {
            Ok(r) => r,
            Err(e) => {
                last_err = format!("HTTP request failed: {}", e);
                continue;
            }
        };

        if resp.status().as_u16() == 429 || resp.status().is_server_error() {
            last_err = format!("API returned {}", resp.status());
            continue;
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().unwrap_or_default();
            return Err(format!("API returned {}: {}", status, text));
        }

        let json: serde_json::Value = match resp.json() {
            Ok(j) => j,
            Err(e) => {
                last_err = format!("JSON parse failed: {}", e);
                continue;
            }
        };

        return json["choices"][0]["message"]["content"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| "No content in API response".to_string());
    }

    Err(format!("All retries failed: {}", last_err))
}


const REPO_PROMPT: &str = "You receive git commit messages from a repository. \
Return a JSON array of 1-4 short highlight strings (max 12 words each). \
Order by impact: features and bug fixes first. \
Skip minor items like docs, chores, formatting, typos, and dependency bumps. \
If commit messages reference PRs (#123), rewrite as PR-123 and prepend to the highlight. \
Preserve issue keys (ENG-456, PROJ-78) as-is. \
Focus on what changed, not how. No fluff. Example: \
[\"PR-42 Added user auth with JWT tokens\",\"Fixed payment webhook retry logic\"]. \
Return ONLY the JSON array, no other text.";

/// Summarize commits per repo, returning RepoSummary structs.
fn summarize_repos(
    commits: &HashMap<String, Vec<(String, String)>>,
    api_key: &str,
    endpoint: &str,
    model: &str,
) -> Vec<RepoSummary> {
    let mut results = Vec::new();
    for (repo, cs) in commits {
        let msgs: Vec<&str> = cs.iter().map(|(_, m)| m.as_str()).collect();
        let content = msgs.join("\n");
        let highlights = match call_llm(api_key, endpoint, model, REPO_PROMPT, &content) {
            Ok(text) => {
                let cleaned = text.trim().trim_start_matches("```json").trim_end_matches("```").trim();
                serde_json::from_str::<Vec<String>>(cleaned).unwrap_or_else(|_| {
                    // Fallback: split by newline and take first few
                    cleaned.lines().take(4).map(|l| l.trim().trim_matches('"').to_string()).collect()
                })
            }
            Err(_) => {
                // Fallback: use first few commit messages as highlights
                msgs.iter().take(3).map(|m| m.to_string()).collect()
            }
        };
        results.push(RepoSummary {
            name: repo.clone(),
            commits: cs.len(),
            highlights,
        });
    }
    results.sort_by(|a, b| b.commits.cmp(&a.commits));
    results
}

/// Main summary loop — runs in its own thread.
pub fn spawn_summary_loop(app: AppHandle, config: Arc<Config>) {
    if !config.settings.summary_enabled {
        info!("Summary feature disabled");
        return;
    }

    let api_key = config
        .settings
        .llm_api_key
        .clone()
        .or_else(|| std::env::var("DEEPSEEK_API_KEY").ok())
        .filter(|k| !k.is_empty());

    let endpoint = config.settings.llm_api_endpoint.clone();
    let model = config.settings.llm_model.clone();
    let has_key = api_key.is_some();

    std::thread::spawn(move || {
        let tz: Tz = config
            .settings
            .timezone
            .parse()
            .unwrap_or(chrono_tz::Europe::Berlin);
        let store = if has_key { Some(SummaryStore::new(&config.config_dir)) } else { None };
        let debounce_secs = config.settings.summary_debounce_secs.max(60);
        let mut last_call: Option<Instant> = None;
        let queue = app.state::<TaskQueue>();

        std::thread::sleep(std::time::Duration::from_secs(3));

        loop {
            let cycle_start = Instant::now();
            let now_utc = Utc::now();
            let now_local = now_utc.with_timezone(&tz);
            let day_s = time_utils::day_start(&now_local, config.settings.day_start_hour);
            let week_s = time_utils::week_start(&now_local, config.settings.day_start_hour, config.settings.week_start_day);

            let day_date = day_s.format("%Y-%m-%d").to_string();
            let week_date = week_s.format("%Y-%m-%d").to_string();
            let day_since = day_s.format("%Y-%m-%dT%H:%M:%S%z").to_string();
            let week_since = week_s.format("%Y-%m-%dT%H:%M:%S%z").to_string();
            let exclude = &config.settings.summary_exclude_pattern;

            let collect_id = queue.start("Collecting commits");
            let _ = app.emit("tasks-changed", ());

            let git_start = Instant::now();
            let day_commits = collect_commits_filtered(&config.settings.repos_dir, &day_since, exclude);
            let week_commits = collect_commits_filtered(&config.settings.repos_dir, &week_since, exclude);
            let git_ms = git_start.elapsed().as_millis();

            let day_total_commits: usize = day_commits.values().map(|v| v.len()).sum();
            let day_total_repos = day_commits.len();
            let week_total_commits: usize = week_commits.values().map(|v| v.len()).sum();
            let week_total_repos = week_commits.len();

            queue.complete(collect_id);
            let _ = app.emit("tasks-changed", ());

            let empty_repos = |commits: &HashMap<String, Vec<(String, String)>>| -> Vec<RepoSummary> {
                commits.iter().map(|(name, cs)| RepoSummary {
                    name: name.clone(), commits: cs.len(), highlights: vec![],
                }).collect()
            };

            let day_cached: Vec<RepoSummary> = store.as_ref()
                .and_then(|s| s.get_summary(&day_date, "day"))
                .and_then(|json| serde_json::from_str(&json).ok())
                .unwrap_or_default();
            let week_cached: Vec<RepoSummary> = store.as_ref()
                .and_then(|s| s.get_summary(&week_date, "week"))
                .and_then(|json| serde_json::from_str(&json).ok())
                .unwrap_or_default();

            let day_loading = has_key && !day_commits.is_empty() && day_cached.is_empty();
            let week_loading = has_key && !week_commits.is_empty() && week_cached.is_empty();

            let _ = app.emit("summary-update", &SummaryData {
                day_repos: if day_cached.is_empty() { empty_repos(&day_commits) } else { day_cached.clone() },
                day_repo_count: day_total_repos,
                day_commits: day_total_commits,
                day_prs: count_prs(&day_cached),
                week_repos: if week_cached.is_empty() { empty_repos(&week_commits) } else { week_cached.clone() },
                week_repo_count: week_total_repos,
                week_commits: week_total_commits,
                week_prs: count_prs(&week_cached),
                loading: day_loading || week_loading,
                no_api_key: !has_key,
            });

            if let (Some(ref key), Some(ref store)) = (&api_key, &store) {
                for (scope, scope_date, commits, label) in [
                    ("day", &day_date, &day_commits, "day"),
                    ("week", &week_date, &week_commits, "week"),
                ] {
                    let current_shas = latest_shas(commits);
                    let stored_shas = store.get_repo_shas(scope_date, scope);
                    let needs_update = current_shas != stored_shas && !commits.is_empty();
                    let debounce_ok = last_call
                        .map(|t| t.elapsed().as_secs() >= debounce_secs)
                        .unwrap_or(true);

                    if needs_update && debounce_ok {
                        let total = commits.values().map(|v| v.len()).sum::<usize>();
                        let llm_id = queue.start(&format!("Generating {} summaries", label));
                        let _ = app.emit("tasks-changed", ());
                        info!("Summary ({}): {} repos, {} commits — calling LLM", label, commits.len(), total);

                        let llm_start = Instant::now();
                        let repo_summaries = summarize_repos(commits, key, &endpoint, &model);
                        let llm_ms = llm_start.elapsed().as_millis();

                        let json = serde_json::to_string(&repo_summaries).unwrap_or_default();
                        store.save_summary(scope_date, scope, &json, &current_shas);
                        if scope == "day" {
                            last_call = Some(Instant::now());
                        }

                        queue.complete(llm_id);
                        let _ = app.emit("tasks-changed", ());

                        perf_log(&config.config_dir, &format!(
                            "LLM summaries ({}): {} repos in {}ms ({} commits)",
                            label, repo_summaries.len(), llm_ms, total
                        ));
                        info!("Repo summaries ({}) updated in {}ms", label, llm_ms);
                    }
                }
            }

            let day_final: Vec<RepoSummary> = store.as_ref()
                .and_then(|s| s.get_summary(&day_date, "day"))
                .and_then(|json| serde_json::from_str(&json).ok())
                .unwrap_or_else(|| empty_repos(&day_commits));
            let week_final: Vec<RepoSummary> = store.as_ref()
                .and_then(|s| s.get_summary(&week_date, "week"))
                .and_then(|json| serde_json::from_str(&json).ok())
                .unwrap_or_else(|| empty_repos(&week_commits));

            let data = SummaryData {
                day_prs: count_prs(&day_final),
                day_repos: day_final,
                day_repo_count: day_total_repos,
                day_commits: day_total_commits,
                week_prs: count_prs(&week_final),
                week_repos: week_final,
                week_repo_count: week_total_repos,
                week_commits: week_total_commits,
                loading: false,
                no_api_key: !has_key,
            };
            let _ = app.emit("summary-update", &data);

            let total_ms = cycle_start.elapsed().as_millis();
            let timing = format!("Summary cycle: {}ms (git:{}ms)", total_ms, git_ms);
            perf_log(&config.config_dir, &timing);
            info!("{}", timing);

            std::thread::sleep(std::time::Duration::from_secs(
                config.settings.refresh_interval.max(10),
            ));
        }
    });
}

/// Tauri command to get current summary state on demand.
pub fn get_current_summary(config: &Config) -> SummaryData {
    if !config.settings.summary_enabled {
        return SummaryData::default();
    }

    let tz: Tz = config
        .settings
        .timezone
        .parse()
        .unwrap_or(chrono_tz::Europe::Berlin);
    let now_local = Utc::now().with_timezone(&tz);
    let day_s = time_utils::day_start(&now_local, config.settings.day_start_hour);
    let week_s = time_utils::week_start(&now_local, config.settings.day_start_hour, config.settings.week_start_day);
    let exclude = &config.settings.summary_exclude_pattern;

    let day_since = day_s.format("%Y-%m-%dT%H:%M:%S%z").to_string();
    let week_since = week_s.format("%Y-%m-%dT%H:%M:%S%z").to_string();
    let day_commits = collect_commits_filtered(&config.settings.repos_dir, &day_since, exclude);
    let week_commits = collect_commits_filtered(&config.settings.repos_dir, &week_since, exclude);

    let has_key = config.settings.llm_api_key.as_ref().map(|k| !k.is_empty()).unwrap_or(false)
        || std::env::var("DEEPSEEK_API_KEY").map(|k| !k.is_empty()).unwrap_or(false);

    let day_date = day_s.format("%Y-%m-%d").to_string();
    let week_date = week_s.format("%Y-%m-%d").to_string();

    let empty_repos = |commits: &HashMap<String, Vec<(String, String)>>| -> Vec<RepoSummary> {
        commits.iter().map(|(name, cs)| RepoSummary {
            name: name.clone(), commits: cs.len(), highlights: vec![],
        }).collect()
    };

    let load_cached = |date: &str, scope: &str| -> Vec<RepoSummary> {
        if !has_key { return Vec::new(); }
        let db_path = config.config_dir.join("summaries.db");
        if !db_path.exists() { return Vec::new(); }
        Connection::open(&db_path).ok()
            .and_then(|con| {
                con.prepare("SELECT summary FROM summaries WHERE date = ? AND scope = ?").ok()
                    .and_then(|mut stmt| stmt.query_row(duckdb::params![date, scope], |row| row.get::<_, String>(0)).ok())
            })
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default()
    };

    let day_cached = load_cached(&day_date, "day");
    let week_cached = load_cached(&week_date, "week");

    let day_repos = if day_cached.is_empty() { empty_repos(&day_commits) } else { day_cached };
    let week_repos = if week_cached.is_empty() { empty_repos(&week_commits) } else { week_cached };

    SummaryData {
        day_prs: count_prs(&day_repos),
        day_repos,
        day_repo_count: day_commits.len(),
        day_commits: day_commits.values().map(|v| v.len()).sum(),
        week_prs: count_prs(&week_repos),
        week_repos,
        week_repo_count: week_commits.len(),
        week_commits: week_commits.values().map(|v| v.len()).sum(),
        loading: false,
        no_api_key: !has_key,
    }
}
