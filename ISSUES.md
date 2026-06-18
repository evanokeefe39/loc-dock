# ISSUES

Local issue tracker for LOC Dock. No external tracker — this file is the source of truth.

**Conventions:** sequential `#N` ids (never reused). New issues go under **Open** with a
short title, root cause / context, and any blockers. When closed, move the entry to
**Resolved** with a one-line note on the fix (and commit/PR if relevant). Reference issues
in commits as `#N`.

## Open

### #1 — Code signing not configured
Installers are unsigned. Windows SmartScreen and macOS Gatekeeper warn on first launch. Needs EV certificate (~$300/yr Windows) or Apple Developer account ($99/yr macOS).

### #2 — No auto-updater
`tauri-plugin-updater` not integrated. Users must manually download new versions. Blocked by #1 (signing required).

## Resolved

### #8 — Second instance crashes on startup ("/restart kills tauri dev")
Two related bugs. (1) No single-instance guard: launching a second `loc-dock.exe` hit a
DuckDB file lock panic (`expect()` at usage_store.rs:121). (2) `app.restart()` under
`npm run tauri dev` killed the dev watcher, leaving an orphaned process.

Fixes (PR #14):
- **PID lock file** (`instance.lock`) + `tasklist` check — detects old instances even
  when they predate `tauri-plugin-single-instance`
- **Retry loop** — `open_usage_cache()` retries `Connection::open` 5× with backoff +
  stale `.wal`/`.tmp` cleanup
- **Shared DuckDB connection** — both loops get a `try_clone()` of one `Connection`
  opened in `lib.rs`, eliminating file-lock conflicts
- **`restart_app` → no-op** — config changed to `Arc<RwLock<Config>>`; save writes to
  disk + reloads shared state without restarting
- **Graceful `exit()`** — `expect()` replaced with `eprintln!` + `std::process::exit(1)`
  as last-resort safety net

### #6 — Config-driven ETL: externalize pricing + SQL transformations
Pricing constants (input/output/cache per-million-token costs) moved from `pricing.rs` into `pricing.yaml` in the config dir. Silver extraction SQL (claude-silver.sql, pi-silver.sql) moved into user-overridable template files in `~/.config/loc-dock/sql/`. Users can edit these files without recompiling when:
  - Provider pricing changes (edit pricing.yaml)
  - JSONL schema changes for Claude or Pi (edit the .sql files)
  - New models with different pricing (edit pricing.yaml)

Changes:
  - `pricing.rs`: now a `Pricing` struct loaded from pricing.yaml (creates default if absent)
  - `usage_store.rs`: loads SQL templates from bundled include_str! with user override in config dir
  - `config.rs`: holds `pricing` (Pricing) field
  - `sql/claude-silver.sql`, `sql/pi-silver.sql`: bundled template files
  - Dropped unused `stats-update` event emits from data.rs frontend polls get_stats

### #5 — Redundant push/pull data path (hybrid emit + poll)
`stats-update` emits (unused — frontend polls `get_stats`) were dropped. Remaining events (`summary-update`, `tasks-changed`) are each listened to by the frontend with the right pattern:
  - `summary-update`: event-driven with initial fetch (instant UI updates for infrequent data)
  - `tasks-changed`: event notification triggers `get_active_tasks` poll (spinner reactivity)
  - Stats: polling `get_stats` every 10s (right for high-frequency data)

### #3 — Active session timeout hardcoded (already configurable)
`Settings.session_idle_timeout` (default 300s) was already exposed in the Settings struct and the frontend SettingsPanel UI. No code change needed. Stale from earlier version.

### #7 — `tauri dev` fails: `bundled-cmake requires a duckdb-rs checkout`
Two stacked bugs. (1) `features = ["bundled-cmake"]` pairs a crates.io dep with a feature that only works from a duckdb-rs git checkout — the published `libduckdb-sys` ships `duckdb.tar.gz` (amalgamation), not the `duckdb-sources/` CMake tree — so it never built. Fixed by reverting to `features = ["bundled"]` (the cc amalgamation path, crates.io-compatible). (2) That surfaced an MSVC build break: the bundled DuckDB's old `fmt` uses `stdext::checked_array_iterator`, removed in VS 2026's STL (MSVC 14.51.36231), under `#ifdef _SECURE_SCL`. The `CXXFLAGS=-D_ITERATOR_DEBUG_LEVEL=0` "workaround" (commit 2411597) was backwards: explicitly setting `_ITERATOR_DEBUG_LEVEL` makes the STL *define* `_SECURE_SCL`, flipping fmt onto the broken path. Modern MSVC leaves `_SECURE_SCL` undefined by default (safe `#else`), so the fix is to **not** set that flag — removed `src-tauri/.cargo/config.toml`. Also drop dev debug info for `libduckdb-sys` to speed builds. The real cause of CI failing where local succeeded: GitHub's `windows-latest` moved to VS 2026, whose STL removed `stdext::checked_array_iterator`; pinned the Windows runner to `windows-2022` (still ships it) in both workflows. Hardened against future drift by pinning the exact DuckDB version and the Rust toolchain (`rust-toolchain.toml`) — the bundled build compiles DuckDB's C++ from source, so frozen inputs keep it deterministic and cacheable. CI green on Windows/macOS/Linux (PR #12).

### #4 — Settings don't persist after restart, save breaks after autostart toggle
Root cause: `get_settings` reads from immutable `Arc<Config>` loaded once at startup — never reflects saved values. Also `save_settings` couples .env write with autostart registry write in one error path, so autostart failure blocks the entire save. Fixed by making `get_settings` read from disk and making autostart failure non-fatal in save.
