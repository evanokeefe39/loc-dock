use crate::config::Config;
use crate::data::SharedStats;
use crate::theme::Theme;
use crate::types::AllStats;
use serde::{Deserialize, Serialize};
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

#[derive(Serialize, Deserialize, Clone)]
pub struct SettingsData {
    pub repos_dir: String,
    pub claude_dir: String,
    pub timezone: String,
    pub day_start_hour: u32,
    pub week_start_day: u32,
    pub config_dir: String,
    pub theme_path: String,
}

#[tauri::command]
pub fn get_settings(app: AppHandle) -> SettingsData {
    let config = app.state::<Arc<Config>>();
    SettingsData {
        repos_dir: config.repos_dir.to_string_lossy().to_string(),
        claude_dir: config.claude_dir.to_string_lossy().to_string(),
        timezone: config.timezone.clone(),
        day_start_hour: config.day_start_hour,
        week_start_day: config.week_start_day,
        config_dir: config.config_dir.to_string_lossy().to_string(),
        theme_path: config.config_dir.join("theme.yaml").to_string_lossy().to_string(),
    }
}

#[tauri::command]
pub fn save_settings(app: AppHandle, settings: SettingsData) -> Result<(), String> {
    let config = app.state::<Arc<Config>>();
    let env_path = config.config_dir.join(".env");
    std::fs::create_dir_all(&config.config_dir).map_err(|e| e.to_string())?;
    let content = format!(
        "LOCDOCK_REPOS_DIR={}\nLOCDOCK_CLAUDE_DIR={}\nLOCDOCK_TIMEZONE={}\nLOCDOCK_DAY_START_HOUR={}\nLOCDOCK_WEEK_START_DAY={}\n",
        settings.repos_dir, settings.claude_dir, settings.timezone,
        settings.day_start_hour, settings.week_start_day,
    );
    std::fs::write(&env_path, content).map_err(|e| e.to_string())?;
    Ok(())
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
