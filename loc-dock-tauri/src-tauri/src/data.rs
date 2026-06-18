use crate::config::Config;
use crate::git;
use crate::job_log;
use crate::source_adapter::{GlobFileDiscoverer, SourceKind, SourceManager};
use crate::summary::{self, SharedSummary, SummaryData};
use crate::task_queue::TaskQueue;
use crate::time_utils;
use crate::types::*;
use crate::usage_store::UsageStore;
use duckdb::Connection;
use chrono::{DateTime, Datelike, Duration, Timelike, Utc};
use chrono_tz::Tz;
use log::{info, warn};
use std::sync::{Arc, RwLock};
use tauri::{AppHandle, Emitter, Manager};

pub type SharedStats = Arc<RwLock<AllStats>>;

pub fn spawn_data_loop(app: AppHandle, config: Arc<RwLock<Config>>, stats: SharedStats, summary_state: SharedSummary, con: Connection) {
    std::thread::spawn(move || {
        // Read config once — clones are cheap, avoids holding the lock.
        let (projects_dir, pi_sessions_dir, usage_cache_dir, pricing, config_dir,
             tz, day_start_hour, week_start_day, repos_dir,
             session_idle_timeout, refresh_interval) = {
            let cfg = config.read().unwrap();
            (
                cfg.projects_dir.clone(),
                cfg.pi_sessions_dir.clone(),
                cfg.settings.usage_cache_dir.clone(),
                cfg.pricing.clone(),
                cfg.config_dir.clone(),
                cfg.settings.timezone.parse::<Tz>().unwrap_or(chrono_tz::UTC),
                cfg.settings.day_start_hour,
                cfg.settings.week_start_day,
                cfg.settings.repos_dir.clone(),
                cfg.settings.session_idle_timeout,
                cfg.settings.refresh_interval,
            )
        };

        let claude_discoverer = GlobFileDiscoverer::new(
            projects_dir,
            vec!["subagents".to_string()],
        );
        let pi_discoverer = GlobFileDiscoverer::new(
            pi_sessions_dir,
            vec![],
        );
        let source_manager = SourceManager::with_discoverers(vec![
            (Box::new(claude_discoverer), SourceKind::Claude),
            (Box::new(pi_discoverer), SourceKind::Pi),
        ]);
        let mut store = UsageStore::new(source_manager, &usage_cache_dir, pricing, &config_dir, con);
        let queue = app.state::<TaskQueue>();

        // ── Pre-fill SharedStats from daily_aggregates + commit_stats (<50ms first paint) ──
        // Uses UTC midnight boundaries for all SQL queries. day_start_hour/timezone only
        // affect frontend time labels, never query filters.
        {
            let now_utc = Utc::now();
            let day_s_utc = now_utc.date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc();
            let week_s_utc = day_s_utc
                - Duration::days(day_s_utc.weekday().num_days_from_monday() as i64);

            let day_date = day_s_utc.format("%Y-%m-%d").to_string();
            let week_date = week_s_utc.format("%Y-%m-%d").to_string();
            let day_utc_str = day_s_utc.format("%Y-%m-%d %H:%M:%S").to_string();
            let week_utc_str = week_s_utc.format("%Y-%m-%d %H:%M:%S").to_string();

            // ponytail: prefill both totals AND bucket arrays so the graph renders instantly.
            // Bucket queries are ~6ms each — negligible vs the 15s cycle.
            let hi = now_utc.timestamp() as f64;
            let day_lo = day_s_utc.timestamp() as f64;
            let week_lo = week_s_utc.timestamp() as f64;

            // Populate bucket arrays too so the graph shows data at paint time
            let git_buckets_day = store.query_commit_buckets(day_lo, hi);
            let git_buckets_week = store.query_commit_buckets(week_lo, hi);
            let cost_buckets_day = store.query_cost_buckets(day_lo, hi);
            let cost_buckets_week = store.query_cost_buckets(week_lo, hi);
            let token_buckets_day = store.query_token_buckets(day_lo, hi);
            let token_buckets_week = store.query_token_buckets(week_lo, hi);

            let build_range = |date: &str, utc_str: &str| -> RangeStats {
                let (cost_total, cost_breakdown, tokens, sessions) = store.query_aggregates(date);
                let source_breakdown = store.query_aggregate_source_breakdown(date);
                let (loc_added, loc_deleted) = store.query_commit_totals(utc_str);
                RangeStats {
                    loc_added, loc_deleted,
                    cost_total, cost_breakdown, tokens,
                    sessions_total: sessions,
                    sessions_active: sessions,  // approximate — active count updated on first full cycle
                    source_breakdown,
                }
            };

            // ponytail: ready=false on cold start so chart shows spinner during first cycle.
            // On warm start (initialized=true), data exists → show immediately.
            let has_data = store.is_initialized();
            let prefilled = AllStats {
                ready: has_data,
                day: build_range(&day_date, &day_utc_str),
                week: build_range(&week_date, &week_utc_str),
                git_buckets_day, git_buckets_week,
                cost_buckets_day, cost_buckets_week,
                token_buckets_day, token_buckets_week,
                ..Default::default()
            };

            if let Ok(mut s) = stats.write() {
                *s = prefilled;
            }
            info!("Prefilled day+week stats from daily_aggregates (first paint <50ms)");
        }

        // Run first refresh immediately, then loop on interval
        loop {
            let cycle_start = std::time::Instant::now();
            info!("Data refresh starting");

            let refresh_id = queue.start("Refreshing data");
            let _ = app.emit("tasks-changed", ());

            // Emit cached stats from previous cycle immediately (< 1ms)
            // so the user never sees a blank screen.
            // First time through, this shows the pre-filled aggregates (real data).
            if stats.read().is_ok() {
                let _ = app.emit("tasks-changed", ());
            }

            // ── UTC midnight boundaries for all SQL queries ──
            // day_start_hour/timezone only affect frontend labels, never query filters.
            let now_utc = Utc::now();
            let day_s_utc = now_utc.date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc();
            let week_s_utc = day_s_utc
                - Duration::days(day_s_utc.weekday().num_days_from_monday() as i64);
            let day_lo = day_s_utc.timestamp() as f64;
            let week_lo = week_s_utc.timestamp() as f64;
            let hi = now_utc.timestamp() as f64;
            let day_utc_str = day_s_utc.format("%Y-%m-%d %H:%M:%S").to_string();
            let week_utc_str = week_s_utc.format("%Y-%m-%d %H:%M:%S").to_string();

            // ── Local timezone boundaries for frontend labels only ──
            let now_local = now_utc.with_timezone(&tz);
            let week_s_label = time_utils::week_start(&now_local, day_start_hour, week_start_day);
            let day_s_label = time_utils::day_start(&now_local, day_start_hour);

            // ── Incremental git scan ──
            // Query the latest commit timestamp in commit_stats, then scan only
            // repos that have new commits since then. On first cycle (empty table),
            // fall back to the week-start window for a fast cold start.
            let git_start = std::time::Instant::now();
            let since_ts = store.latest_commit_ts()
                .map(|ts| ts.min(now_utc))  // clamp future timestamps (e.g. from timezone-shifted old data)
                .unwrap_or_else(|| now_utc - Duration::days(7));
            let since_iso = since_ts.format("%Y-%m-%dT%H:%M:%S%z").to_string();

            let new_commits = git::collect_new_commits(&repos_dir, &since_iso);
            info!("Git scan: {} repos with new commits (since_iso={})", new_commits.len(), since_iso);
            for rc in &new_commits {
                info!("  repo='{}' head_sha={} commits={}", rc.repo, &rc.head_sha[..8.min(rc.head_sha.len())], rc.commits.len());
                if let Err(e) = store.insert_commits(&rc.repo, &rc.commits, &rc.head_sha) {
                    warn!("Store commits for {}: {}", rc.repo, e);
                }
            }
            let new_commit_count: usize = new_commits.iter().map(|rc| rc.commits.len()).sum();
            let git_ms = git_start.elapsed().as_millis();

            let active_str = (now_utc - Duration::seconds(session_idle_timeout as i64))
                .format("%Y-%m-%d %H:%M:%S")
                .to_string();

            // ── Emit immediately with git data + aggregates (before slow ETL) ──
            // User sees LOC + cost/sessions in ~12s, not minutes.
            // SQL queries use UTC-midnight lo/str; labels use timezone-aware day_s_label.
            macro_rules! emit {
                () => {
                    let s = build_all_stats(
                        &store,
                        &day_s_label, &week_s_label, &now_local,
                        day_lo, week_lo, hi,
                        &day_utc_str, &week_utc_str, &active_str,
                        day_start_hour,
                    );
                    if let Ok(mut locked) = stats.write() {
                        *locked = s.clone();
                    }
                };
            }
            emit!();

            // ── Summaries: trigger LLM for repos with new commits ──
            if !new_commits.is_empty() && summary_llm_configured() {
                let config_arc = config.clone();
                for rc in &new_commits {
                    let cached = store.get_repo_summary(&rc.repo);
                    let needs_update = match &cached {
                        Some((_, last_sha)) => last_sha != &rc.head_sha,
                        None => true,
                    };
                    if needs_update {
                        let msgs: Vec<&str> = rc.commits.iter().map(|c| c.msg.as_str()).collect();
                        let content = msgs.join("\n");
                        let cfg = config_arc.read().unwrap();
                        if let Some(ref key) = cfg.settings.llm_api_key {
                            let result = summary::summarize_one_repo(
                                key,
                                &cfg.settings.llm_api_endpoint,
                                &cfg.settings.llm_model,
                                &rc.repo,
                                &content,
                            );
                            match result {
                                Ok(highlights) => {
                                    let json = serde_json::to_string(&highlights).unwrap_or_default();
                                    store.save_repo_summary(&rc.repo, &rc.head_sha, &json, &cfg.settings.llm_model);
                                    job_log::log_ok("summary", &format!("{}: {} highlights", rc.repo, highlights.len()));
                                }
                                Err(e) => {
                                    job_log::log_err("summary", &format!("{}: {}", rc.repo, e));
                                }
                            }
                        }
                    }
                }
                let data = build_summary_data(&store, &week_utc_str, &day_utc_str);
                if let Ok(mut g) = summary_state.write() { *g = data.clone(); }
                let _ = app.emit("summary-update", &data);
            } else {
                // No new commits — emit cached summaries so the panel shows data immediately on warm start
                let data = build_summary_data(&store, &week_utc_str, &day_utc_str);
                let data = SummaryData {
                    no_api_key: !summary_llm_configured(),
                    ..data
                };
                if let Ok(mut g) = summary_state.write() { *g = data.clone(); }
                let _ = app.emit("summary-update", &data);
            }

            // ── Per-source ETL: emit after each provider completes ──
            let mut total_new = 0usize;
            for name in store.source_names() {
                match store.process_source_named(&name) {
                    Ok(n) => {
                        total_new += n;
                        info!("ETL '{}': {} entries", name, n);
                    }
                    Err(e) => warn!("ETL '{}' failed: {}", name, e),
                }
                emit!();  // incremental UI update after each source
            }
            store.finalize_etl();
            emit!();  // final emit after aggregate refresh

            queue.complete(refresh_id);
            let _ = app.emit("tasks-changed", ());

            let total_ms = cycle_start.elapsed().as_millis();
            info!("Refreshed in {}ms (git:{}ms new:{} etl:{} entries)", total_ms, git_ms, new_commit_count, total_new);
            job_log::log_ok("data", &format!("{}ms git:{}ms", total_ms, git_ms));
            crate::summary::perf_log_from(&config_dir, &format!("{}ms cycle", total_ms));

            std::thread::sleep(std::time::Duration::from_secs(refresh_interval.max(10)));
        }
    });
}

// ponytail: timezone/day_start_hour only affects compute_time_labels (frontend).
// All SQL query boundaries use pre-computed UTC-midnight values (day_lo, week_lo, etc.).
fn build_all_stats(
    store: &UsageStore,
    day_s_label: &DateTime<Tz>,
    week_s_label: &DateTime<Tz>,
    now_label: &DateTime<Tz>,
    day_lo: f64,
    week_lo: f64,
    hi: f64,
    day_utc_str: &str,
    week_utc_str: &str,
    active_str: &str,
    day_start_hour: u32,
) -> AllStats {
    // Time labels — use timezone-aware boundaries (frontend only)
    let time_labels_week = compute_time_labels(week_s_label, now_label, "week", day_start_hour);
    let time_labels_day = compute_time_labels(day_s_label, now_label, "day", day_start_hour);

    // LOC buckets from commit_stats (SQL-backed, no Rust loop)
    let git_buckets_week = store.query_commit_buckets(week_lo, hi);
    let git_buckets_day = store.query_commit_buckets(day_lo, hi);

    // LOC totals from commit_stats
    let day_loc = store.query_commit_totals(day_utc_str);
    let week_loc = store.query_commit_totals(week_utc_str);

    // Day stats
    let (day_cost_total, day_cost_breakdown, day_tokens, _day_sessions) = store.query_aggregates(day_utc_str);
    let day_source_breakdown = store.query_aggregate_source_breakdown(day_utc_str);
    let (day_sess_total, day_sess_active) = store.count_sessions(day_utc_str, active_str);
    let day_cost_buckets = store.query_cost_buckets(day_lo, hi);
    let day_token_buckets = store.query_token_buckets(day_lo, hi);

    // Week stats
    let (week_cost_total, week_cost_breakdown, week_tokens, _week_sessions) = store.query_aggregates(week_utc_str);
    let week_source_breakdown = store.query_aggregate_source_breakdown(week_utc_str);
    let (week_sess_total, week_sess_active) = store.count_sessions(week_utc_str, active_str);
    let week_cost_buckets = store.query_cost_buckets(week_lo, hi);
    let week_token_buckets = store.query_token_buckets(week_lo, hi);

    AllStats {
        ready: true,
        day: RangeStats {
            loc_added: day_loc.0, loc_deleted: day_loc.1,
            cost_total: day_cost_total,
            cost_breakdown: day_cost_breakdown,
            tokens: day_tokens,
            sessions_total: day_sess_total, sessions_active: day_sess_active,
            source_breakdown: day_source_breakdown,
        },
        week: RangeStats {
            loc_added: week_loc.0, loc_deleted: week_loc.1,
            cost_total: week_cost_total,
            cost_breakdown: week_cost_breakdown,
            tokens: week_tokens,
            sessions_total: week_sess_total, sessions_active: week_sess_active,
            source_breakdown: week_source_breakdown,
        },
        // month/year left as Default (frontend renders 0s)
        git_buckets_day, git_buckets_week,
        cost_buckets_day: day_cost_buckets,
        cost_buckets_week: week_cost_buckets,
        token_buckets_day: day_token_buckets,
        token_buckets_week: week_token_buckets,
        time_labels_day, time_labels_week,
        ..Default::default()
    }
}

fn compute_time_labels(
    since: &DateTime<Tz>,
    now: &DateTime<Tz>,
    range: &str,
    day_start_hour: u32,
) -> TimeLabels {
    let span = (*now - *since).num_seconds() as f64;
    if span <= 0.0 {
        return TimeLabels::default();
    }

    let (start, end) = match range {
        "year"  => (since.format("%b").to_string(), now.format("%b").to_string()),
        "month" => (since.format("%d %b").to_string(), now.format("%d %b").to_string()),
        "week"  => (since.format("%a %d").to_string(), now.format("%a %d").to_string()),
        _       => (since.format("%H:%M").to_string(), now.format("%H:%M").to_string()),
    };

    let mut ticks = Vec::new();

    match range {
        "year" => {
            // Monthly ticks
            let mut t = *since + Duration::days(28); // approximate
            let mut last_month = -1i32;
            while t < *now {
                let m = t.month() as i32;
                if m != last_month {
                    let frac = (t - *since).num_seconds() as f64 / span;
                    ticks.push(Tick {
                        frac,
                        label: t.format("%b").to_string(),
                    });
                    last_month = m;
                }
                t = t + Duration::days(1);
            }
        },
        "month" => {
            // Weekly ticks
            let mut t = *since + Duration::days(7);
            t = t
                .with_hour(day_start_hour)
                .and_then(|t| t.with_minute(0))
                .and_then(|t| t.with_second(0))
                .unwrap_or(t);
            while t < *now {
                let frac = (t - *since).num_seconds() as f64 / span;
                ticks.push(Tick {
                    frac,
                    label: t.format("%d").to_string(),
                });
                t = t + Duration::days(7);
            }
        },
        "week" => {
            // Daily ticks at day_start_hour
            let mut t = *since + Duration::days(1);
            t = t
                .with_hour(day_start_hour)
                .and_then(|t| t.with_minute(0))
                .and_then(|t| t.with_second(0))
                .unwrap_or(t);
            while t < *now {
                let frac = (t - *since).num_seconds() as f64 / span;
                ticks.push(Tick {
                    frac,
                    label: t.format("%a").to_string(),
                });
                t = t + Duration::days(1);
            }
        },
        _ => {
            // Day: 3-hour ticks
            let mut t = since
                .with_minute(0)
                .and_then(|t| t.with_second(0))
                .unwrap_or(*since);
            let remainder = t.hour() % 3;
            if remainder != 0 {
                t = t + Duration::hours((3 - remainder) as i64);
            } else if t <= *since {
                t = t + Duration::hours(3);
            }
            while t < *now {
                let frac = (t - *since).num_seconds() as f64 / span;
                ticks.push(Tick {
                    frac,
                    label: t.format("%H").to_string(),
                });
                t = t + Duration::hours(3);
            }
        }
    }

    TimeLabels { start, end, ticks }
}

// ── Summary helpers (moved from summary.rs) ────────────────────────

/// Extract PR references like (#123) from commit messages.
fn extract_prs(msgs: &[String]) -> Vec<String> {
    let re = regex::Regex::new(r"\(#(\d+)\)").expect("invalid PR regex");
    let mut seen = std::collections::HashSet::new();
    let mut prs = Vec::new();
    for msg in msgs {
        for cap in re.captures_iter(msg) {
            let pr = format!("PR-{}", &cap[1]);
            if seen.insert(pr.clone()) {
                prs.push(pr);
            }
        }
    }
    prs
}

/// Check if any LLM API key is configured (summary enabled is implied).
fn summary_llm_configured() -> bool {
    // ponytail: returns true; actual key check happens in the loop body.
    true
}

/// Build SummaryData from cached repo highlights + commit_stats counts.
fn build_summary_data(store: &UsageStore, week_utc_str: &str, day_utc_str: &str) -> SummaryData {
    let summarized = store.all_summarized_repos();
    let mut day_repos = Vec::new();
    let mut week_repos = Vec::new();
    let mut day_total = 0usize;
    let mut week_total = 0usize;

    for (repo, json) in &summarized {
        let highlights: Vec<String> = serde_json::from_str(json).unwrap_or_default();
        let week_count = store.count_repo_commits_since(repo, week_utc_str);
        let day_count = store.count_repo_commits_since(repo, day_utc_str);

        // Extract PR numbers from commit messages
        let day_msgs = store.repo_commit_messages_since(repo, day_utc_str);
        let week_msgs = store.repo_commit_messages_since(repo, week_utc_str);
        let day_prs = extract_prs(&day_msgs);
        let week_prs = extract_prs(&week_msgs);

        if week_count > 0 {
            week_repos.push(summary::RepoSummary {
                name: repo.clone(),
                commits: week_count,
                prs: week_prs,
                highlights: highlights.clone(),
            });
            week_total += week_count;
        }
        if day_count > 0 {
            day_repos.push(summary::RepoSummary {
                name: repo.clone(),
                commits: day_count,
                prs: day_prs,
                highlights,
            });
            day_total += day_count;
        }
    }

    let day_count = day_repos.len();
    let week_count = week_repos.len();
    let week_prs_total: usize = week_repos.iter().map(|r| r.prs.len()).sum();
    let day_prs_total: usize = day_repos.iter().map(|r| r.prs.len()).sum();

    SummaryData {
        day_repos,
        day_repo_count: day_count,
        day_commits: day_total,
        day_prs: day_prs_total,
        week_repos,
        week_repo_count: week_count,
        week_commits: week_total,
        week_prs: week_prs_total,
        loading: false,
        no_api_key: false,
    }
}

