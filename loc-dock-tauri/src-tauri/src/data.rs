use crate::time_utils;
use crate::config::Config;
use crate::git::{self, GitPoint};
use crate::git_cache;
use crate::job_log;
use crate::pricing;
use crate::task_queue::TaskQueue;
use crate::types::*;
use crate::usage_store::UsageStore;
use chrono::{DateTime, Duration, Timelike, Utc};
use chrono_tz::Tz;
use log::info;
use std::sync::{Arc, RwLock};
use tauri::{AppHandle, Emitter, Manager};

const N_BUCKETS: usize = 48;

pub type SharedStats = Arc<RwLock<AllStats>>;

pub fn spawn_data_loop(app: AppHandle, config: Arc<Config>, stats: SharedStats) {
    std::thread::spawn(move || {
        let mut store = UsageStore::new(&config.projects_dir, &config.settings.usage_cache_dir);
        let tz: Tz = config.settings.timezone.parse().unwrap_or(chrono_tz::UTC);
        let queue = app.state::<TaskQueue>();

        // Emit cached data immediately so the UI renders in <1s
        {
            let now_local = Utc::now().with_timezone(&tz);
            let week_s = time_utils::week_start(&now_local, config.settings.day_start_hour, config.settings.week_start_day);
            let day_s = time_utils::day_start(&now_local, config.settings.day_start_hour);
            let since_iso = week_s.format("%Y-%m-%dT%H:%M:%S%z").to_string();

            let cache = git_cache::GitCache::new(&config.settings.git_cache_dir);
            let git_points = cache.query_since(&since_iso);

            if !git_points.is_empty() || store.is_initialized() {
                let week_utc_str = week_s.with_timezone(&Utc).format("%Y-%m-%d %H:%M:%S").to_string();
                let day_utc_str = day_s.with_timezone(&Utc).format("%Y-%m-%d %H:%M:%S").to_string();
                let active_str = (Utc::now() - Duration::seconds(config.settings.session_idle_timeout as i64))
                    .format("%Y-%m-%d %H:%M:%S").to_string();

                let all = build_all_stats(
                    &git_points, &store, &week_s, &day_s, &now_local,
                    &week_utc_str, &day_utc_str, &active_str,
                    config.settings.day_start_hour,
                );
                if let Ok(mut s) = stats.write() {
                    *s = all.clone();
                }
                let _ = app.emit("stats-update", &all);
                let msg = format!("Instant emit from cache ({} git points, usage_init={})", git_points.len(), store.is_initialized());
                info!("{}", msg);
                job_log::log_ok("data", &msg);
            } else {
                info!("No cached data available, waiting for first refresh");
                job_log::log_ok("data", "No cached data, waiting for first refresh");
            }
        }

        loop {
            let cycle_start = std::time::Instant::now();
            info!("Data refresh starting");

            let refresh_id = queue.start("Refreshing data");
            let _ = app.emit("tasks-changed", ());

            let now_utc = Utc::now();
            let now_local = now_utc.with_timezone(&tz);

            let week_s = time_utils::week_start(&now_local, config.settings.day_start_hour, config.settings.week_start_day);
            let day_s = time_utils::day_start(&now_local, config.settings.day_start_hour);

            let since_iso = week_s.format("%Y-%m-%dT%H:%M:%S%z").to_string();

            let repos_dir = config.settings.repos_dir.clone();
            let since_clone = since_iso.clone();
            let git_cache_dir = config.settings.git_cache_dir.clone();
            let git_handle = std::thread::spawn(move || {
                let t = std::time::Instant::now();
                let cache = git_cache::GitCache::new(&git_cache_dir);
                let result = git::get_git_loc_timeline(&repos_dir, &since_clone, &cache);
                (result, t.elapsed())
            });

            let jsonl_start = std::time::Instant::now();
            let jsonl_rebuilt = store.load();
            let jsonl_ms = jsonl_start.elapsed().as_millis();

            let (git_points, git_elapsed) = git_handle.join().unwrap_or_else(|_| (Vec::new(), std::time::Duration::ZERO));
            let git_ms = git_elapsed.as_millis();

            let stats_start = std::time::Instant::now();
            let week_utc_str = week_s
                .with_timezone(&Utc)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string();
            let day_utc_str = day_s
                .with_timezone(&Utc)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string();
            let active_str = (Utc::now() - Duration::seconds(config.settings.session_idle_timeout as i64))
                .format("%Y-%m-%d %H:%M:%S")
                .to_string();

            let all = build_all_stats(
                &git_points,
                &store,
                &week_s,
                &day_s,
                &now_local,
                &week_utc_str,
                &day_utc_str,
                &active_str,
                config.settings.day_start_hour,
            );
            let stats_ms = stats_start.elapsed().as_millis();

            if let Ok(mut s) = stats.write() {
                *s = all.clone();
            }
            let _ = app.emit("stats-update", &all);

            queue.complete(refresh_id);
            let _ = app.emit("tasks-changed", ());

            let total_ms = cycle_start.elapsed().as_millis();
            let timing = format!(
                "Refreshed in {}ms (git:{}ms jsonl:{}ms{} stats:{}ms)",
                total_ms, git_ms, jsonl_ms,
                if jsonl_rebuilt { " [rebuilt]" } else { " [cached]" },
                stats_ms
            );
            info!("{}", timing);
            job_log::log_ok("data", &timing);
            crate::summary::perf_log_from(&config.config_dir, &timing);

            std::thread::sleep(std::time::Duration::from_secs(config.settings.refresh_interval.max(10)));
        }
    });
}

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
    let git_buckets_week = bucket_git(git_points, week_s, now);
    let git_buckets_day = bucket_git(git_points, day_s, now);

    let week_loc = sum_loc(&git_buckets_week);
    let day_loc = sum_loc(&git_buckets_day);

    let week_tokens = store.query_since(week_utc_str);
    let day_tokens = store.query_since(day_utc_str);

    let week_cost_breakdown = store.query_cost_breakdown(week_utc_str);
    let day_cost_breakdown = store.query_cost_breakdown(day_utc_str);

    let (week_sess_total, week_sess_active) = store.count_sessions(week_utc_str, active_str);
    let (day_sess_total, day_sess_active) = store.count_sessions(day_utc_str, active_str);

    let cost_timeline = store.query_cost_timeline(week_utc_str);
    let token_timeline = store.query_token_timeline(week_utc_str);

    let cost_buckets_week = bucket_cost(&cost_timeline, week_s, now);
    let cost_buckets_day = bucket_cost(&cost_timeline, day_s, now);

    let token_buckets_week = bucket_tokens(&token_timeline, week_s, now);
    let token_buckets_day = bucket_tokens(&token_timeline, day_s, now);

    let time_labels_week = compute_time_labels(week_s, now, true, day_start_hour);
    let time_labels_day = compute_time_labels(day_s, now, false, day_start_hour);

    AllStats {
        ready: true,
        day: RangeStats {
            loc_added: day_loc.0,
            loc_deleted: day_loc.1,
            cost_total: pricing::estimate_cost(&day_tokens),
            cost_breakdown: day_cost_breakdown,
            tokens: day_tokens,
            sessions_total: day_sess_total,
            sessions_active: day_sess_active,
        },
        week: RangeStats {
            loc_added: week_loc.0,
            loc_deleted: week_loc.1,
            cost_total: pricing::estimate_cost(&week_tokens),
            cost_breakdown: week_cost_breakdown,
            tokens: week_tokens,
            sessions_total: week_sess_total,
            sessions_active: week_sess_active,
        },
        git_buckets_day,
        git_buckets_week,
        cost_buckets_day,
        cost_buckets_week,
        token_buckets_day,
        token_buckets_week,
        time_labels_day,
        time_labels_week,
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
    is_week: bool,
    day_start_hour: u32,
) -> TimeLabels {
    let span = (*now - *since).num_seconds() as f64;
    if span <= 0.0 {
        return TimeLabels::default();
    }

    let start = if is_week {
        since.format("%a %d").to_string()
    } else {
        since.format("%H:%M").to_string()
    };
    let end = if is_week {
        now.format("%a %d").to_string()
    } else {
        now.format("%H:%M").to_string()
    };

    let mut ticks = Vec::new();

    if is_week {
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
    } else {
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

    TimeLabels { start, end, ticks }
}

