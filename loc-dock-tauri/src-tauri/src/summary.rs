use crate::config::Config;
use crate::time_utils;
use chrono::Utc;
use chrono_tz::Tz;
use duckdb::Connection;
use log::{info, warn};
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;
use tauri::{AppHandle, Emitter};

#[derive(Serialize, Clone, Default)]
pub struct SummaryData {
    pub day_summary: Option<String>,
    pub week_summary: Option<String>,
    pub day_repos: usize,
    pub day_commits: usize,
    pub loading: bool,
    pub no_api_key: bool,
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

/// Collect commit (SHA, message) pairs from all repos since `since_iso`.
/// Returns map of repo_name -> Vec<(sha, message)>.
fn collect_commits(repos_dir: &Path, since_iso: &str) -> HashMap<String, Vec<(String, String)>> {
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
                    Some((parts[0].to_string(), parts[1].to_string()))
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

/// Call an OpenAI-compatible chat completions API.
fn call_llm(api_key: &str, endpoint: &str, model: &str, prompt: &str, content: &str) -> Result<String, String> {
    let client = reqwest::blocking::Client::new();
    let url = format!("{}/chat/completions", endpoint.trim_end_matches('/'));
    let body = serde_json::json!({
        "model": model,
        "messages": [
            { "role": "system", "content": prompt },
            { "role": "user", "content": content }
        ],
        "temperature": 0.3,
        "max_tokens": 1024
    });

    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .map_err(|e| format!("HTTP request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().unwrap_or_default();
        return Err(format!("API returned {}: {}", status, text));
    }

    let json: serde_json::Value = resp.json().map_err(|e| format!("JSON parse failed: {}", e))?;
    json["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "No content in API response".to_string())
}

/// Format commits for the LLM prompt.
fn format_commits_for_prompt(
    commits: &HashMap<String, Vec<(String, String)>>,
    max_messages: usize,
) -> String {
    let mut lines = Vec::new();
    let mut total = 0;

    for (repo, cs) in commits {
        lines.push(format!("\n## {}", repo));
        for (_, msg) in cs {
            if total >= max_messages {
                lines.push(format!("... and more (truncated at {})", max_messages));
                return lines.join("\n");
            }
            lines.push(format!("- {}", msg));
            total += 1;
        }
    }

    lines.join("\n")
}

const DAY_PROMPT: &str = "Summarize what was accomplished today based on these git commit messages. \
Group by theme or area of work. Be concise but preserve meaningful detail. Use plain text, no markdown headers.";

const WEEK_PROMPT: &str = "Summarize the week's accomplishments from these daily summaries. \
Highlight major themes, progress, and completed work. Be concise but don't lose important detail. \
Use plain text, no markdown headers.";

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
        .or_else(|| std::env::var("DEEPSEEK_API_KEY").ok());

    let api_key = match api_key {
        Some(k) if !k.is_empty() => k,
        _ => {
            info!("No LLM API key configured; summary feature disabled");
            return;
        }
    };

    let endpoint = config.settings.llm_api_endpoint.clone();
    let model = config.settings.llm_model.clone();

    std::thread::spawn(move || {
        let tz: Tz = config
            .settings
            .timezone
            .parse()
            .unwrap_or(chrono_tz::Europe::Berlin);
        let store = SummaryStore::new(&config.config_dir);
        let debounce_secs = config.settings.summary_debounce_secs.max(60);
        let mut last_call: Option<Instant> = None;

        loop {
            std::thread::sleep(std::time::Duration::from_secs(
                config.settings.refresh_interval.max(10),
            ));

            let now_utc = Utc::now();
            let now_local = now_utc.with_timezone(&tz);
            let day_s = time_utils::day_start(&now_local, config.settings.day_start_hour);
            let week_s = time_utils::week_start(
                &now_local,
                config.settings.day_start_hour,
                config.settings.week_start_day,
            );

            let day_date = day_s.format("%Y-%m-%d").to_string();
            let week_date = week_s.format("%Y-%m-%d").to_string();
            let since_iso = day_s.format("%Y-%m-%dT%H:%M:%S%z").to_string();

            // Collect today's commits
            let commits = collect_commits(&config.settings.repos_dir, &since_iso);
            let current_shas = latest_shas(&commits);
            let stored_shas = store.get_repo_shas(&day_date, "day");

            let total_commits: usize = commits.values().map(|v| v.len()).sum();
            let total_repos = commits.len();

            // Check if anything changed
            let needs_update = current_shas != stored_shas && !commits.is_empty();

            // Debounce: avoid calling the LLM too frequently
            let debounce_ok = last_call
                .map(|t| t.elapsed().as_secs() >= debounce_secs)
                .unwrap_or(true);

            if needs_update && debounce_ok {
                info!(
                    "Summary: {} repos, {} commits — calling LLM",
                    total_repos, total_commits
                );

                let content = format_commits_for_prompt(&commits, 200);
                match call_llm(&api_key, &endpoint, &model, DAY_PROMPT, &content) {
                    Ok(summary) => {
                        store.save_summary(&day_date, "day", &summary, &current_shas);
                        last_call = Some(Instant::now());
                        info!("Day summary updated");

                        // Also generate weekly summary from all commits this week
                        let week_since_iso = week_s.format("%Y-%m-%dT%H:%M:%S%z").to_string();
                        let week_commits =
                            collect_commits(&config.settings.repos_dir, &week_since_iso);
                        if !week_commits.is_empty() {
                            let week_content = format_commits_for_prompt(&week_commits, 200);
                            match call_llm(&api_key, &endpoint, &model, WEEK_PROMPT, &week_content) {
                                Ok(week_summary) => {
                                    let week_shas = latest_shas(&week_commits);
                                    store.save_summary(
                                        &week_date,
                                        "week",
                                        &week_summary,
                                        &week_shas,
                                    );
                                    info!("Week summary updated");
                                }
                                Err(e) => warn!("Week summary LLM call failed: {}", e),
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Day summary LLM call failed: {}", e);
                    }
                }
            }

            let data = SummaryData {
                day_summary: store.get_summary(&day_date, "day"),
                week_summary: store.get_summary(&week_date, "week"),
                day_repos: total_repos,
                day_commits: total_commits,
                loading: false,
                no_api_key: false,
            };
            let _ = app.emit("summary-update", &data);
        }
    });
}

/// Tauri command to get current summary state on demand.
pub fn get_current_summary(config: &Config) -> SummaryData {
    if !config.settings.summary_enabled {
        return SummaryData::default();
    }

    let has_key = config.settings.llm_api_key.as_ref().map(|k| !k.is_empty()).unwrap_or(false)
        || std::env::var("DEEPSEEK_API_KEY").map(|k| !k.is_empty()).unwrap_or(false);

    if !has_key {
        return SummaryData { no_api_key: true, ..SummaryData::default() };
    }

    let tz: Tz = config
        .settings
        .timezone
        .parse()
        .unwrap_or(chrono_tz::Europe::Berlin);
    let now_local = Utc::now().with_timezone(&tz);
    let day_s = time_utils::day_start(&now_local, config.settings.day_start_hour);
    let week_s = time_utils::week_start(
        &now_local,
        config.settings.day_start_hour,
        config.settings.week_start_day,
    );

    let day_date = day_s.format("%Y-%m-%d").to_string();
    let week_date = week_s.format("%Y-%m-%d").to_string();

    let db_path = config.config_dir.join("summaries.db");
    if !db_path.exists() {
        return SummaryData::default();
    }

    let con = match Connection::open(&db_path) {
        Ok(c) => c,
        Err(_) => return SummaryData::default(),
    };

    let day_summary = con
        .prepare("SELECT summary FROM summaries WHERE date = ? AND scope = ?")
        .ok()
        .and_then(|mut stmt| {
            stmt.query_row(duckdb::params![&day_date, "day"], |row| {
                row.get::<_, String>(0)
            })
            .ok()
        });

    let week_summary = con
        .prepare("SELECT summary FROM summaries WHERE date = ? AND scope = ?")
        .ok()
        .and_then(|mut stmt| {
            stmt.query_row(duckdb::params![&week_date, "week"], |row| {
                row.get::<_, String>(0)
            })
            .ok()
        });

    // Get today's commit count
    let since_iso = day_s.format("%Y-%m-%dT%H:%M:%S%z").to_string();
    let commits = collect_commits(&config.settings.repos_dir, &since_iso);

    SummaryData {
        day_summary,
        week_summary,
        day_repos: commits.len(),
        day_commits: commits.values().map(|v| v.len()).sum(),
        loading: false,
        no_api_key: false,
    }
}
