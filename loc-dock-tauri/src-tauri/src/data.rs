use crate::config::Config;
use crate::git::{self, GitPoint};
use crate::git_cache;
use crate::job_log;
use crate::source_adapter::{GlobFileDiscoverer, SourceKind, SourceManager};
use crate::task_queue::TaskQueue;
use crate::time_utils;
use crate::types::*;
use crate::usage_store::UsageStore;
use chrono::{DateTime, Datelike, Duration, Timelike, Utc};
use chrono_tz::Tz;
use log::{info, warn};
use std::sync::{Arc, RwLock};
use tauri::{AppHandle, Emitter, Manager};

const N_BUCKETS: usize = 48;

pub type SharedStats = Arc<RwLock<AllStats>>;

pub fn spawn_data_loop(app: AppHandle, config: Arc<Config>, stats: SharedStats) {
    std::thread::spawn(move || {
        let claude_discoverer = GlobFileDiscoverer::new(
            config.projects_dir.clone(),
            vec!["subagents".to_string()],
        );
        let pi_discoverer = GlobFileDiscoverer::new(
            config.pi_sessions_dir.clone(),
            vec![],
        );
        let source_manager = SourceManager::with_discoverers(vec![
            (Box::new(claude_discoverer), SourceKind::Claude),
            (Box::new(pi_discoverer), SourceKind::Pi),
        ]);
        let mut store = UsageStore::new(source_manager, &config.settings.usage_cache_dir);
        let tz: Tz = config.settings.timezone.parse().unwrap_or(chrono_tz::UTC);
        let queue = app.state::<TaskQueue>();

        // ── Pre-fill SharedStats from daily_aggregates (<50ms first paint) ──
        // Read pre-computed day+week aggregates so the user sees real data immediately.
        // Month/year are kept as Default (frontend still renders but shows 0s).
        {
            let now_local = Utc::now().with_timezone(&tz);
            let day_s = time_utils::day_start(&now_local, config.settings.day_start_hour);
            let week_s = time_utils::week_start(&now_local, config.settings.day_start_hour, config.settings.week_start_day);

            let day_date = day_s.format("%Y-%m-%d").to_string();
            let week_date = week_s.format("%Y-%m-%d").to_string();

            let build_range = |date: &str| -> RangeStats {
                let (cost_total, cost_breakdown, tokens, sessions) = store.query_aggregates(date);
                let source_breakdown = store.query_aggregate_source_breakdown(date);
                RangeStats {
                    cost_total, cost_breakdown, tokens,
                    sessions_total: sessions,
                    sessions_active: sessions,  // approximate — active count updated on first full cycle
                    source_breakdown,
                    ..Default::default()
                }
            };

            let prefilled = AllStats {
                ready: true,
                day: build_range(&day_date),
                week: build_range(&week_date),
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
            if let Ok(cached) = stats.read() {
                let _ = app.emit("stats-update", &*cached);
            }

            let now_local = Utc::now().with_timezone(&tz);

            let week_s = time_utils::week_start(&now_local, config.settings.day_start_hour, config.settings.week_start_day);
            let day_s = time_utils::day_start(&now_local, config.settings.day_start_hour);

            // Git scan from week start (ponytail: year range cut — re-add as optional manual trigger)
            let since_iso = week_s.format("%Y-%m-%dT%H:%M:%S%z").to_string();

            // Run git scan and ETL in parallel — but with a timeout so a stuck git
            // process doesn't block the cycle forever.
            let repos_dir = config.settings.repos_dir.clone();
            let git_cache_dir = config.settings.git_cache_dir.clone();
            let since_clone = since_iso.clone();
            let git_handle = std::thread::spawn(move || {
                let t = std::time::Instant::now();
                let cache = git_cache::GitCache::new(&git_cache_dir);
                let result = git::get_git_loc_timeline(&repos_dir, &since_clone, &cache);
                (result, t.elapsed())
            });

            // Join git thread with 60s timeout — run_etl hasn't started yet.
            // No parallel benefit here since we need git_points for the first emit,
            // but the timeout prevents hanging forever.
            const GIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
            let git_start_wait = std::time::Instant::now();
            let (git_points, git_ms) = loop {
                if git_handle.is_finished() {
                    let (pts, dur) = git_handle.join().unwrap_or_else(|_| (Vec::new(), std::time::Duration::ZERO));
                    break (pts, dur.as_millis());
                }
                if git_start_wait.elapsed() >= GIT_TIMEOUT {
                    warn!("Git scan timed out after {}s — proceeding without LOC data", GIT_TIMEOUT.as_secs());
                    break (Vec::new(), git_start_wait.elapsed().as_millis());
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            };

            // Time window strings for day+week
            let week_utc_str = week_s.with_timezone(&Utc).format("%Y-%m-%d %H:%M:%S").to_string();
            let day_utc_str = day_s.with_timezone(&Utc).format("%Y-%m-%d %H:%M:%S").to_string();
            let active_str = (Utc::now() - Duration::seconds(config.settings.session_idle_timeout as i64))
                .format("%Y-%m-%d %H:%M:%S")
                .to_string();

            // ── Emit immediately with git data + aggregates (before slow ETL) ──
            // User sees LOC + cost/sessions in ~12s, not minutes.
            macro_rules! emit {
                () => {
                    let s = build_all_stats(
                        &git_points, &store,
                        &week_s, &day_s, &now_local,
                        &week_utc_str, &day_utc_str, &active_str,
                        config.settings.day_start_hour,
                    );
                    if let Ok(mut locked) = stats.write() {
                        *locked = s.clone();
                    }
                    let _ = app.emit("stats-update", &s);
                };
            }
            emit!();

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
            info!("Refreshed in {}ms (git:{}ms etl:{} entries)", total_ms, git_ms, total_new);
            job_log::log_ok("data", &format!("{}ms git:{}ms", total_ms, git_ms));
            crate::summary::perf_log_from(&config.config_dir, &format!("{}ms cycle", total_ms));

            std::thread::sleep(std::time::Duration::from_secs(config.settings.refresh_interval.max(10)));
        }
    });
}

// ponytail: build_all_stats simplified to day+week only.
// Month/year ranges removed from hot path — kept as Default in AllStats type
// for frontend compat. Re-add as optional background/manual trigger if needed.
fn build_all_stats(
    git_points: &[GitPoint],
    store: &UsageStore,
    week_s: &DateTime<Tz>,
    day_s: &DateTime<Tz>,
    now: &DateTime<Tz>,
    week_utc_str: &str,
    day_utc_str: &str,
    active_str: &str,
    day_start_hour: u32,
) -> AllStats {
    // Git buckets — compute day+week only
    let git_buckets_week = bucket_git(git_points, week_s, now);
    let git_buckets_day = bucket_git(git_points, day_s, now);
    let week_loc = sum_loc(&git_buckets_week);
    let day_loc = sum_loc(&git_buckets_day);

    // Time labels — day+week only
    let time_labels_week = compute_time_labels(week_s, now, "week", day_start_hour);
    let time_labels_day = compute_time_labels(day_s, now, "day", day_start_hour);

    // Day stats (use daily_aggregates for fast totals, entries for timeline)
    let (day_cost_total, day_cost_breakdown, day_tokens, _day_sessions) = store.query_aggregates(day_utc_str);
    let day_source_breakdown = store.query_aggregate_source_breakdown(day_utc_str);
    let (day_sess_total, day_sess_active) = store.count_sessions(day_utc_str, active_str);
    let day_cost_timeline = store.query_cost_timeline(day_utc_str);
    let day_token_timeline = store.query_token_timeline(day_utc_str);

    // Week stats
    let (week_cost_total, week_cost_breakdown, week_tokens, _week_sessions) = store.query_aggregates(week_utc_str);
    let week_source_breakdown = store.query_aggregate_source_breakdown(week_utc_str);
    let (week_sess_total, week_sess_active) = store.count_sessions(week_utc_str, active_str);
    let week_cost_timeline = store.query_cost_timeline(week_utc_str);
    let week_token_timeline = store.query_token_timeline(week_utc_str);

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
        cost_buckets_day: bucket_cost(&day_cost_timeline, day_s, now),
        cost_buckets_week: bucket_cost(&week_cost_timeline, week_s, now),
        token_buckets_day: bucket_tokens(&day_token_timeline, day_s, now),
        token_buckets_week: bucket_tokens(&week_token_timeline, week_s, now),
        time_labels_day, time_labels_week,
        ..Default::default()
    }
}

fn bucket_git(points: &[GitPoint], since: &DateTime<Tz>, until: &DateTime<Tz>) -> Vec<(i64, i64)> {
    let total_secs = (*until - *since).num_seconds().max(1) as f64;
    let mut buckets = vec![(0i64, 0i64); N_BUCKETS];
    for p in points {
        let local = p.ts.with_timezone(&since.timezone());
        let offset = (local - *since).num_seconds() as f64;
        if offset < 0.0 || offset >= total_secs {
            continue;
        }
        let idx = ((offset / total_secs) * N_BUCKETS as f64) as usize;
        let idx = idx.min(N_BUCKETS - 1);
        buckets[idx].0 += p.added;
        buckets[idx].1 += p.deleted;
    }
    buckets
}

fn bucket_cost(
    points: &[(f64, f64)],
    since: &DateTime<Tz>,
    until: &DateTime<Tz>,
) -> Vec<f64> {
    let since_epoch = since.timestamp() as f64;
    let until_epoch = until.timestamp() as f64;
    let total_secs = (until_epoch - since_epoch).max(1.0);
    let mut buckets = vec![0.0f64; N_BUCKETS];
    for &(epoch, cost) in points {
        let offset = epoch - since_epoch;
        if offset < 0.0 || offset >= total_secs {
            continue;
        }
        let idx = ((offset / total_secs) * N_BUCKETS as f64) as usize;
        let idx = idx.min(N_BUCKETS - 1);
        buckets[idx] += cost;
    }
    buckets
}

fn bucket_tokens(
    points: &[(f64, i64, i64, i64, i64)],
    since: &DateTime<Tz>,
    until: &DateTime<Tz>,
) -> Vec<(i64, i64, i64, i64)> {
    let since_epoch = since.timestamp() as f64;
    let until_epoch = until.timestamp() as f64;
    let total_secs = (until_epoch - since_epoch).max(1.0);
    let mut buckets = vec![(0i64, 0i64, 0i64, 0i64); N_BUCKETS];
    for &(epoch, inp, out, cw, cr) in points {
        let offset = epoch - since_epoch;
        if offset < 0.0 || offset >= total_secs {
            continue;
        }
        let idx = ((offset / total_secs) * N_BUCKETS as f64) as usize;
        let idx = idx.min(N_BUCKETS - 1);
        buckets[idx].0 += inp;
        buckets[idx].1 += out;
        buckets[idx].2 += cw;
        buckets[idx].3 += cr;
    }
    buckets
}

fn sum_loc(buckets: &[(i64, i64)]) -> (i64, i64) {
    buckets
        .iter()
        .fold((0, 0), |(a, d), (ba, bd)| (a + ba, d + bd))
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

