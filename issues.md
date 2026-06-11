# Issues

## Open

### #1 — Code signing not configured
Installers are unsigned. Windows SmartScreen and macOS Gatekeeper warn on first launch. Needs EV certificate (~$300/yr Windows) or Apple Developer account ($99/yr macOS).

### #2 — No auto-updater
`tauri-plugin-updater` not integrated. Users must manually download new versions. Blocked by #1 (signing required).

### #3 — Active session timeout hardcoded
`data.rs` uses 5-minute timeout to determine active sessions. Should be configurable via settings.

## Resolved

### #4 — Settings don't persist after restart, save breaks after autostart toggle
Root cause: `get_settings` reads from immutable `Arc<Config>` loaded once at startup — never reflects saved values. Also `save_settings` couples .env write with autostart registry write in one error path, so autostart failure blocks the entire save. Fixed by making `get_settings` read from disk and making autostart failure non-fatal in save.
