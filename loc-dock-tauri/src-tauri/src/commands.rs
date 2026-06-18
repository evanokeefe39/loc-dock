use crate::config::{Config, Settings};
use crate::data::SharedStats;
use crate::job_log::{self, LogEntry};
use crate::source_adapter::DataSourceConfig;
use crate::summary::{self, SummaryData};
use crate::task_queue::{ActiveTask, TaskQueue};
use crate::theme::Theme;
use crate::types::AllStats;
use crate::usage_store::UsageStore;
use std::sync::{Arc, RwLock};
use tauri::{AppHandle, Manager, Window};

// All commands are `async` so Tauri runs them on the async runtime's thread pool —
// like request handlers in a web server — never on the main/UI thread. This keeps
// the dock responsive no matter how slow a handler is. The frontend already treats
// every command as an async API call (invoke -> Promise) and receives server-push
// updates via emitted events (summary-update, tasks-changed).

#[tauri::command]
pub async fn get_theme(app: AppHandle) -> Theme {
    app.state::<Theme>().inner().clone()
}

#[tauri::command]
pub async fn get_stats(app: AppHandle) -> AllStats {
    let stats = app.state::<SharedStats>();
    let v = stats.read().map(|s| s.clone()).unwrap_or_default();
    v
}

#[tauri::command]
pub async fn get_summary(app: AppHandle) -> SummaryData {
    // Read the latest computed summary from shared state — never re-scans git, so
    // it can't block (the summary loop owns all git/LLM work).
    let v = app.state::<summary::SharedSummary>()
        .read()
        .map(|s| s.clone())
        .unwrap_or_default();
    v
}

#[tauri::command]
pub async fn get_settings(app: AppHandle) -> Settings {
    let config = app.state::<Arc<RwLock<Config>>>().inner().clone();
    let settings = config.read().map(|c| c.settings.clone()).unwrap_or_default();
    settings
}

#[tauri::command]
pub async fn save_settings(app: AppHandle, settings: Settings) -> Result<(), String> {
    // Save to disk, then reload the shared config so background loops pick up changes.
    let config_state = app.state::<Arc<RwLock<Config>>>();
    let config_dir = config_state.read().unwrap().config_dir.clone();
    settings.save(&config_dir)?;
    *config_state.write().unwrap() = Config::load();

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
pub fn restart_app(_app: AppHandle) {
    // ponytail: don't restart under dev mode (kills `tauri dev` watcher).
    // Settings are saved + config reloaded in save_settings. The config is
    // stored in Arc<RwLock<>> so background loops pick up changes on next cycle.
    // If a full restart is truly needed, the user can tray → Exit and re-launch.
    log::info!("Settings saved — no restart needed (config is hot-reloadable)");
}

#[tauri::command]
pub async fn get_active_tasks(app: AppHandle) -> Vec<ActiveTask> {
    app.state::<TaskQueue>().active_tasks()
}

#[tauri::command]
pub async fn reset_usage_cache(app: AppHandle) -> Result<(), String> {
    let dir = app.state::<Arc<RwLock<Config>>>().read().unwrap().settings.usage_cache_dir.clone();
    UsageStore::reset(&dir)?;
    job_log::log_ok("usage_cache", "Usage cache reset");
    Ok(())
}

#[tauri::command]
pub async fn reset_summary_cache(app: AppHandle) -> Result<(), String> {
    let config_arc = app.state::<Arc<RwLock<Config>>>().inner().clone();
    let cfg = config_arc.read().unwrap();
    summary::reset_summaries(&cfg)
}

#[tauri::command]
pub async fn get_job_logs(app: AppHandle) -> Vec<LogEntry> {
    let dir = app.state::<Arc<RwLock<Config>>>().read().unwrap().settings.log_dir.clone();
    job_log::read_logs(&dir, 100)
}

#[tauri::command]
pub async fn clear_job_logs(app: AppHandle) -> Result<(), String> {
    let dir = app.state::<Arc<RwLock<Config>>>().read().unwrap().settings.log_dir.clone();
    job_log::clear_logs(&dir)
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

// ── Data source management ──────────────────────────────────────────────

#[tauri::command]
pub async fn list_sources(app: AppHandle) -> Vec<DataSourceConfig> {
    let state = app.state::<Arc<RwLock<Config>>>();
    let cfg = state.read().unwrap();
    cfg.settings.data_sources.clone()
}

#[tauri::command]
pub async fn add_source(app: AppHandle, source: DataSourceConfig) -> Result<(), String> {
    let state = app.state::<Arc<RwLock<Config>>>();
    let mut cfg = state.write().unwrap();
    if cfg.settings.data_sources.iter().any(|s| s.id == source.id) {
        return Err(format!("Source '{}' already exists", source.id));
    }
    if !source.path.exists() {
        return Err(format!("Path '{}' does not exist", source.path.display()));
    }
    cfg.settings.data_sources.push(source);
    cfg.settings.save(&cfg.config_dir)
}

#[tauri::command]
pub async fn remove_source(app: AppHandle, id: String) -> Result<(), String> {
    let state = app.state::<Arc<RwLock<Config>>>();
    let mut cfg = state.write().unwrap();
    let len_before = cfg.settings.data_sources.len();
    cfg.settings.data_sources.retain(|s| s.id != id);
    if cfg.settings.data_sources.len() == len_before {
        return Err(format!("Source '{}' not found", id));
    }
    cfg.settings.save(&cfg.config_dir)
}

#[tauri::command]
pub async fn toggle_source(app: AppHandle, id: String) -> Result<(), String> {
    let state = app.state::<Arc<RwLock<Config>>>();
    let mut cfg = state.write().unwrap();
    let src = cfg.settings.data_sources.iter_mut().find(|s| s.id == id)
        .ok_or_else(|| format!("Source '{}' not found", id))?;
    src.enabled = !src.enabled;
    cfg.settings.save(&cfg.config_dir)
}

