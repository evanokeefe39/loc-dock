mod commands;
mod config;
mod data;
mod git;
mod pricing;
mod theme;
mod tray;
mod types;
mod usage_store;

use config::Config;
use data::SharedStats;
use std::sync::{Arc, RwLock};
use tauri::{Manager, WindowEvent};
use tauri_plugin_autostart::MacosLauncher;
use theme::Theme;
use types::AllStats;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    env_logger::init();

    let config = Arc::new(Config::load());
    let theme = Theme::load(&config.theme_path);
    let stats: SharedStats = Arc::new(RwLock::new(AllStats::default()));
    let autostart_enabled = config.autostart;

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(theme)
        .manage(stats.clone())
        .manage(config.clone())
        .invoke_handler(tauri::generate_handler![
            commands::get_theme,
            commands::get_stats,
            commands::get_settings,
            commands::save_settings,
            commands::restart_app,
            commands::snap_to_corner,
        ])
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .setup(move |app| {
            let handle = app.handle().clone();
            if let Err(e) = tray::setup_tray(&handle) {
                log::warn!("Failed to setup tray: {}", e);
            }

            let autostart = app.state::<tauri_plugin_autostart::AutoLaunchManager>();
            if autostart_enabled {
                let _ = autostart.enable();
            } else {
                let _ = autostart.disable();
            }

            data::spawn_data_loop(handle, config.clone(), stats.clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
