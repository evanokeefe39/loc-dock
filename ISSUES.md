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

### #3 — Active session timeout hardcoded
`data.rs` uses 5-minute timeout to determine active sessions. Should be configurable via settings.

### #5 — Redundant push/pull data path (hybrid emit + poll)
Backend both emits Tauri events (`stats-update`, `summary-update`, `tasks-changed`) and serves the same data via async commands (`get_stats`, `get_summary`). Frontend hooks only poll the commands (~10s); the event emits are unused by the current UI. Decide during the hardening pass whether to drop the event emits (simpler, one path) or switch the frontend to event-driven (lower latency, no polling). Pick one path. Low risk; cleanup, not correctness.

### #6 — Review and harden current architecture
Post-V2 hardening pass over the shipped medallion ETL + decoupled client/server design (see AGENTS.md). Scope TBD: error-path/edge-case review (cold start, retention boundary, malformed JSONL), the `RwLock` access patterns, ingest registry correctness under rotation, resolving #5, and confirming the gotchas in AGENTS.md still hold. Produce a focused plan before changing code.

## Resolved

### #7 — `tauri dev` fails: `bundled-cmake requires a duckdb-rs checkout`
Two stacked bugs. (1) `features = ["bundled-cmake"]` pairs a crates.io dep with a feature that only works from a duckdb-rs git checkout — the published `libduckdb-sys` ships `duckdb.tar.gz` (amalgamation), not the `duckdb-sources/` CMake tree — so it never built. Fixed by reverting to `features = ["bundled"]` (the cc amalgamation path, crates.io-compatible). (2) That surfaced an MSVC build break: the bundled DuckDB's old `fmt` uses `stdext::checked_array_iterator`, removed in VS 2026's STL (MSVC 14.51.36231), under `#ifdef _SECURE_SCL`. The `CXXFLAGS=-D_ITERATOR_DEBUG_LEVEL=0` "workaround" (commit 2411597) was backwards: explicitly setting `_ITERATOR_DEBUG_LEVEL` makes the STL *define* `_SECURE_SCL`, flipping fmt onto the broken path. Modern MSVC leaves `_SECURE_SCL` undefined by default (safe `#else`), so the fix is to **not** set that flag — removed `src-tauri/.cargo/config.toml`. Also drop dev debug info for `libduckdb-sys` to speed builds. Verified locally (`cargo check` green); awaiting CI confirmation on PR #12 before merge.

### #4 — Settings don't persist after restart, save breaks after autostart toggle
Root cause: `get_settings` reads from immutable `Arc<Config>` loaded once at startup — never reflects saved values. Also `save_settings` couples .env write with autostart registry write in one error path, so autostart failure blocks the entire save. Fixed by making `get_settings` read from disk and making autostart failure non-fatal in save.
