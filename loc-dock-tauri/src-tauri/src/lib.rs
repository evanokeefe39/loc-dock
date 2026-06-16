mod commands;
mod config;
mod data;
mod git;
mod git_cache;
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
use std::sync::{Arc, RwLock};
use tauri::{Manager, WindowEvent};
use tauri_plugin_autostart::MacosLauncher;
use task_queue::TaskQueue;
use theme::Theme;
use types::AllStats;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let config = Arc::new(Config::load());
    let theme = Theme::load(&config.settings.theme_path);
    let stats: SharedStats = Arc::new(RwLock::new(AllStats::default()));
    let summary_state: summary::SharedSummary = Arc::new(RwLock::new(summary::SummaryData::default()));
    let autostart_enabled = config.settings.autostart;

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
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
            commands::reset_git_cache,
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

            job_log::init(&config.settings.log_dir);
            data::spawn_data_loop(handle.clone(), config.clone(), stats.clone());
            summary::spawn_summary_loop(handle, config.clone(), summary_state.clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
