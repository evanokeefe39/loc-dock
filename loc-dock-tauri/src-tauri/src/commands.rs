use crate::data::SharedStats;
use crate::theme::Theme;
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
pub async fn snap_to_corner(window: Window) -> Result<(), String> {
    if let Some(monitor) = window.current_monitor().map_err(|e| e.to_string())? {
        let monitor_size = monitor.size();
        let monitor_pos = monitor.position();

        let win_size = window.outer_size().map_err(|e| e.to_string())?;

        let x = monitor_pos.x + (monitor_size.width as i32 - win_size.width as i32);
        let y = monitor_pos.y + (monitor_size.height as i32 - win_size.height as i32);

        window
            .set_position(tauri::Position::Physical(tauri::PhysicalPosition { x, y }))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
