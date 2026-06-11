# Issues

## Open

### #1 — Code signing not configured
Installers are unsigned. Windows SmartScreen and macOS Gatekeeper warn on first launch. Needs EV certificate (~$300/yr Windows) or Apple Developer account ($99/yr macOS).

### #2 — No auto-updater
`tauri-plugin-updater` not integrated. Users must manually download new versions. Blocked by #1 (signing required).

### #3 — Active session timeout hardcoded
`data.rs` uses 5-minute timeout to determine active sessions. Should be configurable via settings.

## Resolved

(none yet)
