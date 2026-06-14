use crate::config::{Config, Settings};
use crate::data::SharedStats;
use crate::summary::{self, SummaryData};
use crate::theme::Theme;
use crate::types::AllStats;
use std::sync::Arc;
use tauri::{AppHandle, Manager, Window};

#[tauri::command]
pub fn get_theme(app: AppHandle) -> Theme {
    app.state::<Theme>().inner().clone()
}

#[tauri::command]
pub fn get_stats(app: AppHandle) -> AllStats {
    let stats = app.state::<SharedStats>();
    stats.read().map(|s| s.clone()).unwrap_or_default()
}

#[tauri::command]
pub fn get_summary(app: AppHandle) -> SummaryData {
    let config = app.state::<Arc<Config>>();
    summary::get_current_summary(&config)
}

#[tauri::command]
pub fn get_settings() -> Settings {
    let config = Config::load();
    config.settings
}

#[tauri::command]
pub fn save_settings(app: AppHandle, settings: Settings) -> Result<(), String> {
    let config = app.state::<Arc<Config>>();
    settings.save(&config.config_dir)?;

    let autostart = app.state::<tauri_plugin_autostart::AutoLaunchManager>();
    if settings.autostart {
        if let Err(e) = autostart.enable() {
            log::error!("Failed to enable autostart: {}", e);
        }
    } else {
        if let Err(e) = autostart.disable() {
            log::error!("Failed to disable autostart: {}", e);
        }
    }

    Ok(())
}

#[tauri::command]
pub fn restart_app(app: AppHandle) {
    app.restart();
}

#[tauri::command]
pub async fn snap_to_corner(window: Window, corner: String) -> Result<(), String> {
    if let Some(monitor) = window.current_monitor().map_err(|e| e.to_string())? {
        let monitor_size = monitor.size();
        let monitor_pos = monitor.position();
        let win_size = window.outer_size().map_err(|e| e.to_string())?;

        let x = match corner.as_str() {
            "top-left" | "bottom-left" => monitor_pos.x,
            _ => monitor_pos.x + (monitor_size.width as i32 - win_size.width as i32),
        };
        let y = match corner.as_str() {
            "top-left" | "top-right" => monitor_pos.y,
            _ => monitor_pos.y + (monitor_size.height as i32 - win_size.height as i32),
        };

        window
            .set_position(tauri::Position::Physical(tauri::PhysicalPosition { x, y }))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
