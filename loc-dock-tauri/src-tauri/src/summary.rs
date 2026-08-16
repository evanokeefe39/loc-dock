use crate::config::Config;
use crate::job_log;
use duckdb::Connection;
use log::info;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};
use std::time::Instant;

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
    /// True when a cached summary entry exists (even an explicit empty `[]`).
    /// Lets the UI distinguish "summarized, nothing to show" from "still pending".
    pub summarized: bool,
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

// ── Circuit breaker ─────────────────────────────────────────────────

/// Shared circuit breaker for LLM calls. When consecutive failures exceed
/// the threshold, the circuit opens and further calls are skipped for a
/// cooldown period. One half-open probe is allowed after cooldown.
#[derive(Debug)]
pub struct CircuitBreaker {
    consecutive_failures: u32,
    opened_at: Option<Instant>,
}

impl CircuitBreaker {
    pub fn new() -> Self {
        Self { consecutive_failures: 0, opened_at: None }
    }

    /// Returns true if the circuit is open (all calls should be skipped).
    pub fn is_open(&self, cooldown: std::time::Duration) -> bool {
        match self.opened_at {
            Some(opened) => opened.elapsed() < cooldown,
            None => false,
        }
    }

    /// Record a successful LLM call — closes the circuit.
    pub fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.opened_at = None;
    }

    /// Record a failed LLM call. Returns true if the circuit just opened.
    pub fn record_failure(&mut self, threshold: u32) -> bool {
        self.consecutive_failures += 1;
        if self.consecutive_failures >= threshold && self.opened_at.is_none() {
            self.opened_at = Some(Instant::now());
            true
        } else {
            false
        }
    }

    /// After cooldown expires, allow one half-open probe. Returns true
    /// if the circuit was open but cooldown elapsed (proceed with one call).
    pub fn allow_half_open(&mut self, cooldown: std::time::Duration) -> bool {
        match self.opened_at {
            Some(opened) if opened.elapsed() >= cooldown => {
                // Stay "open" until the probe result comes back
                true
            }
            _ => false,
        }
    }

}

const TEST_PROMPT: &str = "Reply with exactly the word 'ok' and nothing else.";

/// Validate LLM connection with a minimal API call. Returns Ok(()) or an
/// error message suitable for display.
pub fn test_connection(api_key: &str, endpoint: &str, model: &str) -> Result<(), String> {
    // ponytail: use a short timeout for the test — if it takes >10s, fail fast
    call_llm_with_timeout(api_key, endpoint, model, TEST_PROMPT, "ping", std::time::Duration::from_secs(10))
        .map(|_| ())
}

/// Token cap for LLM responses. Was 512 — a reasoning model (deepseek-v4-flash)
/// can spend the entire budget on `reasoning_content` and return empty `content`
/// with `finish_reason: "length"`. Raised so a normal reasoning chain + the
/// 1-4 highlight answer fit; input is also truncated (see data.rs) so this is
/// not an open-ended spend.
const MAX_LLM_TOKENS: u32 = 2048;
/// Call an OpenAI-compatible chat completions API with retry + exponential backoff.
pub fn call_llm(api_key: &str, endpoint: &str, model: &str, prompt: &str, content: &str) -> Result<String, String> {
    call_llm_with_timeout(api_key, endpoint, model, prompt, content, std::time::Duration::from_secs(30))
}

fn call_llm_with_timeout(api_key: &str, endpoint: &str, model: &str, prompt: &str, content: &str, timeout: std::time::Duration) -> Result<String, String> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .build()
        .new_agent();
    let url = format!("{}/chat/completions", endpoint.trim_end_matches('/'));
    let body = serde_json::json!({
        "model": model,
        "messages": [
            { "role": "system", "content": prompt },
            { "role": "user", "content": content }
        ],
        "temperature": 0.3,
        "max_tokens": MAX_LLM_TOKENS
    });

    let mut last_err = String::new();
    for attempt in 0..3 {
        if attempt > 0 {
            let base_ms = (1000 * (1 << attempt)) as u64;
            let jitter_ms = (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_millis() as u64) % 1000;
            std::thread::sleep(std::time::Duration::from_millis(base_ms + jitter_ms));
            info!("LLM retry attempt {} after {}ms", attempt + 1, base_ms + jitter_ms);
        }

        let resp = match agent
            .post(&url)
            .header("Authorization", &format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .send_json(&body)
        {
            Ok(r) => r,
            Err(ureq::Error::StatusCode(s)) => {
                last_err = format!("API returned {}", s);
                if s == 429 || s >= 500 {
                    continue;   // retryable
                }
                return Err(format!("API returned {}", s));
            }
            Err(e) => {
                last_err = format!("HTTP request failed: {}", e);
                continue;
            }
        };

        let json: serde_json::Value = match resp.into_body().read_json() {
            Ok(j) => j,
            Err(e) => {
                last_err = format!("JSON parse failed: {}", e);
                continue;
            }
        };

        // Loud failures: log exactly what came back so an empty-content response
        // is diagnosable without endpoint spelunking. deepseek-v4-flash can burn
        // the whole max_tokens budget on reasoning_content and return empty
        // content with finish_reason=length — that is a FAILURE, not success.
        let choice = &json["choices"][0];
        let msg = &choice["message"];
        let finish_reason = choice["finish_reason"].as_str().unwrap_or("unknown");
        let response_text = msg["content"].as_str().unwrap_or_default();
        let reasoning_len = msg["reasoning_content"].as_str().map(|s| s.len()).unwrap_or(0);
        info!(
            "LLM response: content={} chars, reasoning_content={} chars, finish_reason={}, usage={}",
            response_text.len(),
            reasoning_len,
            finish_reason,
            json["usage"]
        );

        if !response_text.is_empty() {
            return Ok(response_text.to_string());
        }
        return Err(format!(
            "LLM returned empty content (finish_reason={}, reasoning_content={} chars, max_tokens={})",
            finish_reason, reasoning_len, MAX_LLM_TOKENS
        ));
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

/// Parse the LLM's raw response into highlight strings. Strict by design:
/// empty or non-JSON output is an error (loud failure), never silently empty
/// highlights — an empty result previously read as "not summarized yet" and
/// wedged the summary panel on "loading…" forever.
fn parse_highlights(text: &str) -> Result<Vec<String>, String> {
    let cleaned = text.trim().trim_start_matches("```json").trim_end_matches("```").trim();
    if cleaned.is_empty() {
        return Err("empty LLM response (no content)".to_string());
    }
    if let Ok(v) = serde_json::from_str::<Vec<String>>(cleaned) {
        return Ok(v);
    }
    // Some models wrap the array in prose — extract the bracketed span.
    if let Some(start) = cleaned.find('[') {
        if let Some(end) = cleaned.rfind(']') {
            if end > start {
                let inner = &cleaned[start..=end];
                if let Ok(v) = serde_json::from_str::<Vec<String>>(inner) {
                    return Ok(v);
                }
            }
        }
    }
    Err("response is not a JSON array of strings".to_string())
}

/// Summarize one repo's commit messages and return highlights.
pub fn summarize_one_repo(api_key: &str, endpoint: &str, model: &str, repo: &str, content: &str) -> Result<Vec<String>, String> {
    match call_llm(api_key, endpoint, model, REPO_PROMPT, content) {
        Ok(text) => match parse_highlights(&text) {
            Ok(highlights) => {
                job_log::log_ok("summary", &format!("LLM response for {}: {} chars -> {} highlights", repo, text.len(), highlights.len()));
                Ok(highlights)
            }
            Err(e) => {
                // Loud: log the raw (truncated) response — the old silent
                // line-split fallback hid this class of failure for weeks.
                let snippet: String = text.chars().take(500).collect();
                job_log::log_err("summary", &format!("{}: unparseable LLM response ({}): {}", repo, e, snippet));
                Err(format!("LLM response unparseable for {}: {}", repo, e))
            }
        },
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

#[cfg(test)]
mod tests {
    use super::parse_highlights;

    #[test]
    fn parse_empty_is_error() {
        assert!(parse_highlights("").is_err());
        assert!(parse_highlights("   ").is_err());
    }

    #[test]
    fn parse_valid_array() {
        let v = parse_highlights("[\"Added auth\",\"Fixed retry\"]").unwrap();
        assert_eq!(v, vec!["Added auth".to_string(), "Fixed retry".to_string()]);
    }

    #[test]
    fn parse_code_fenced_array() {
        let v = parse_highlights("```json\n[\"hi\"]\n```").unwrap();
        assert_eq!(v, vec!["hi".to_string()]);
    }

    #[test]
    fn parse_empty_array_is_ok() {
        // A valid explicit [] is an answer (nothing worth highlighting),
        // distinct from an empty response (failure).
        let v = parse_highlights("[]").unwrap();
        assert!(v.is_empty());
    }

    #[test]
    fn parse_prose_wrapped_array() {
        let v = parse_highlights("Here you go: [\"one\", \"two\"] hope that helps").unwrap();
        assert_eq!(v, vec!["one".to_string(), "two".to_string()]);
    }

    #[test]
    fn parse_garbage_is_error() {
        assert!(parse_highlights("not json at all").is_err());
        assert!(parse_highlights("{'obj': 1}").is_err());
    }
}
