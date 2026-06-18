use crate::config::Config;
use crate::job_log;
use chrono::Utc;
use duckdb::Connection;
use log::info;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, RwLock};

/// Latest computed summary, shared with the `get_summary` command so the UI reads
/// cached state (instant) instead of triggering on-demand git scans on the main
/// thread. Mirrors `data::SharedStats`.
pub type SharedSummary = Arc<RwLock<SummaryData>>;

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct RepoSummary {
    pub name: String,
    pub commits: usize,
    pub prs: Vec<String>,
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

pub fn perf_log_from(config_dir: &Path, msg: &str) {
    let path = config_dir.join("perf.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let ts = Utc::now().format("%Y-%m-%d %H:%M:%S");
        let _ = writeln!(f, "[{}] {}", ts, msg);
    }
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
Strip PR references like (#3) from highlights — they are displayed separately. \
Focus on what changed, not how. No fluff. Example: \
[\"Added user auth with JWT tokens\",\"Fixed payment webhook retry logic\"]. \
Return ONLY the JSON array, no other text.";

/// Summarize one repo's commit messages and return highlights.
pub fn summarize_one_repo(api_key: &str, endpoint: &str, model: &str, repo: &str, content: &str) -> Result<Vec<String>, String> {
    match call_llm(api_key, endpoint, model, REPO_PROMPT, content) {
        Ok(text) => {
            let cleaned = text.trim().trim_start_matches("```json").trim_end_matches("```").trim();
            job_log::log_ok("summary", &format!("LLM response for {}: {} chars", repo, cleaned.len()));
            Ok(serde_json::from_str::<Vec<String>>(cleaned).unwrap_or_else(|_| {
                cleaned.lines().take(4).map(|l| l.trim().trim_matches('"').to_string()).collect()
            }))
        }
        Err(e) => Err(format!("LLM failed for {}: {}", repo, e)),
    }
}

pub fn reset_summaries(config: &Config) -> Result<(), String> {
    let db_path = config.settings.usage_cache_dir.join("usage_cache.db");
    if db_path.exists() {
        let con = Connection::open(&db_path).map_err(|e| e.to_string())?;
        con.execute("DELETE FROM repo_summaries", []).map_err(|e| e.to_string())?;
    }
    job_log::log_ok("summary", "Summary cache reset");
    Ok(())
}
