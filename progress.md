# Progress

## Status
In Progress

## Tasks
### P1d — get_settings reads shared config state (done)
Changed `get_settings()` in `commands.rs` from `Config::load()` (disk I/O on every call) to reading from `Arc<RwLock<Config>>` managed state. Same pattern as other commands.

### P1a — Dead code removal (done by parent)
- Removed `EtlResult` struct
- Removed `N_BUCKETS` constant
- Removed `run_etl()` method

## Files Changed
- `src/commands.rs` — `get_settings` now accepts `AppHandle`, reads from managed state

## Notes
Remaining tasks for parent: P1b (format_sql merge), P1c (perf_log → job_log), P1e (get_head_sha removal), P2a (timestamp helper), P2b (inline FileDiscoverer)
