mod commands;
mod config;
mod data;
mod git;
mod job_log;
mod pricing;
mod source_adapter;
mod summary;
mod task_queue;
mod theme;
mod time_utils;
mod tray;
mod types;
mod usage_store;

use config::Config;
use data::SharedStats;
use std::path::Path;
use std::sync::{Arc, RwLock};
use tauri::{Emitter, Listener, Manager, WindowEvent};
use tauri_plugin_autostart::MacosLauncher;
use task_queue::TaskQueue;
use theme::Theme;
use types::AllStats;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let config = Arc::new(RwLock::new(Config::load()));
    {
        let cfg = config.read().unwrap();
        enforce_single_instance(&cfg.settings.usage_cache_dir);
    }
    let theme = {
        let cfg = config.read().unwrap();
        Theme::load(&cfg.settings.theme_path)
    };
    let stats: SharedStats = Arc::new(RwLock::new(AllStats::default()));
    let summary_state: summary::SharedSummary = Arc::new(RwLock::new(summary::SummaryData::default()));
    let autostart_enabled = config.read().unwrap().settings.autostart;

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // A second instance was launched — tell the first to show its window.
            // The plugin kills the duplicate; this callback runs in the first instance.
            let _ = app.emit("show-main-window", ());
        }))
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(theme)
        .manage(stats.clone())
        .manage(summary_state.clone())
        .manage(config.clone())
        .manage(TaskQueue::new())
        .invoke_handler(tauri::generate_handler![
            commands::get_theme,
            commands::get_stats,
            commands::get_summary,
            commands::get_settings,
            commands::save_settings,
            commands::restart_app,
            commands::snap_to_corner,
            commands::get_active_tasks,
            commands::reset_usage_cache,
            commands::reset_summary_cache,
            commands::get_job_logs,
            commands::clear_job_logs,
        ])
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .setup(move |app| {
            let handle = app.handle().clone();

            // Show the dock window immediately, decoupled from the frontend and from
            // any data loading. Position it bottom-right first so it doesn't flash at
            // the default spot. The webview paints an instant boot spinner (index.html)
            // while React + backend data load behind it.
            if let Some(win) = app.get_webview_window("main") {
                if let Ok(Some(monitor)) = win.current_monitor() {
                    let (msize, mpos) = (monitor.size(), monitor.position());
                    if let Ok(wsize) = win.outer_size() {
                        let x = mpos.x + (msize.width as i32 - wsize.width as i32);
                        let y = mpos.y + (msize.height as i32 - wsize.height as i32);
                        let _ = win.set_position(tauri::PhysicalPosition { x, y });
                    }
                }
                let _ = win.show();
            }

            if let Err(e) = tray::setup_tray(&handle) {
                log::warn!("Failed to setup tray: {}", e);
            }

            let autostart = app.state::<tauri_plugin_autostart::AutoLaunchManager>();
            if autostart_enabled {
                if let Err(e) = autostart.enable() {
                    log::error!("Failed to enable autostart: {}", e);
                }
            } else {
                if let Err(e) = autostart.disable() {
                    log::error!("Failed to disable autostart: {}", e);
                }
            }

            let handle2 = handle.clone();
            handle.listen("show-main-window", move |_| {
                if let Some(w) = handle2.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            });

            // Open shared DuckDB database — both loops get a Connection clone
            // so they share the same underlying Database handle.
            let cache_dir;  // keep borrow alive
            {
                let cfg = config.read().unwrap();
                cache_dir = cfg.settings.usage_cache_dir.clone();
                job_log::init(&cfg.settings.log_dir);
            }
            let _ = std::fs::create_dir_all(&cache_dir);
            let marker = cache_dir.join("usage_cache.db.reset");
            let db_path = cache_dir.join("usage_cache.db");
            if marker.exists() {
                let _ = std::fs::remove_file(&db_path);
                let _ = std::fs::remove_file(&marker);
                log::info!("Usage cache reset via marker file");
            }
            let con = usage_store::open_usage_cache(&db_path);

            data::spawn_data_loop(handle.clone(), config.clone(), stats.clone(), summary_state.clone(), con);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Enforce single instance with a PID lock file — works even when the other
/// instance was built before the tauri-plugin-single-instance was added.
fn enforce_single_instance(cache_dir: &Path) {
    use std::io::Write;

    let lock_path = cache_dir.join("instance.lock");
    let _ = std::fs::create_dir_all(cache_dir);

    if let Ok(pid_str) = std::fs::read_to_string(&lock_path) {
        if let Ok(pid) = pid_str.trim().parse::<u32>() {
            if is_pid_alive(pid) {
                eprintln!(
                    "[loc-dock] Another instance is already running (PID {}). Exiting.",
                    pid
                );
                std::process::exit(0);
            }
        }
        // PID file is stale — clean it up
        let _ = std::fs::remove_file(&lock_path);
    }

    // Write our PID to the lock file
    if let Ok(mut f) = std::fs::File::create(&lock_path) {
        let _ = write!(f, "{}", std::process::id());
    }
}

fn is_pid_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        use std::process::Command;
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {}", pid), "/NH"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.contains(&pid.to_string()))
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        // Unix: kill -0 $pid checks if process exists
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .ok()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}
