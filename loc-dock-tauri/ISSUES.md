# LOC Dock Tauri — Open Issues

## P0: Token and cost chart Y-axis wrong, most data missing from buckets
- **Symptom:** Cost chart Y-axis shows $1.00 max when total is $1,682. Token chart shows 73K max when total is 130M+.
- **Root cause:** `parse_ts_offset()` in `data.rs:192` parses `"%Y-%m-%d %H:%M:%S"` but DuckDB outputs fractional seconds like `"2026-05-18 07:16:59.651"`. Most timestamps silently fail to parse, dropping data from buckets. Bottom row stats are correct (direct DuckDB aggregation).
- **Fix:** Replace `ts::VARCHAR` with `epoch(ts)` in DuckDB queries. Return `f64` epoch seconds. Delete `parse_ts_offset` entirely.

## P1: Systemic — silent error swallowing throughout data layer
- **Symptom:** Errors return empty defaults with no indication to the user that data is missing.
- **Locations:** `usage_store.rs` all query methods, `git.rs:82` (repo errors), `data.rs:20` (timezone fallback), `usage_store.rs:52` (glob fallback).
- **Fix:** Add structured logging at WARN level for every silent fallback. Consider surfacing a "data quality" indicator in the UI.

## P1: No unit tests in entire Rust backend
- **Symptom:** Bugs like the timestamp parsing issue are only caught visually in production.
- **Functions needing tests:** `parse_ts_offset`, `bucket_git/cost/tokens`, `day_start/week_start`, `compute_time_labels`, git numstat parsing, all DuckDB query methods.
- **Fix:** Add `#[cfg(test)]` modules. Add dev-dependencies for test fixtures.

## P2: Implicit data contracts at every boundary
- **DuckDB -> Rust:** `ts::VARCHAR` format assumed but never validated. JSONL schema (`message.usage.*`) assumed. `ignore_errors=true` silently discards malformed rows.
- **Git -> Rust:** Timestamp parsing tries RFC3339 then manual format, silently falls back. If neither works, `current_time` stays stale — subsequent numstat lines get attributed to the wrong commit.
- **Config -> Runtime:** Invalid timezone silently defaults to Berlin (data.rs:20). No warning logged.
- **Fix:** Add format assertions at boundaries. Log warnings on every fallback.

## P2: Timezone handling landmines
- **`day_start()`/`week_start()`** use `.unwrap_or(*now)` when hour-setting fails — falls back to current time silently.
- **Git timestamps** have their own TZ offsets from `--format=%aI`. Bucketing converts via `with_timezone()` but if configured TZ doesn't match git's TZ, offsets may be wrong.
- **Fix:** Validate timezone at startup (log error if invalid). Add assertions in time boundary functions.

## P2: Transparency makes text see-through
- **Symptom:** Desktop content bleeds through widget text and chart.
- **Root cause:** CSS `opacity` on `.app` makes everything transparent including children.
- **Fix:** Convert hex bg + alpha to `rgba()` background-color instead of opacity.

## P1: Zombie process — dock resists taskkill and can't be closed
- **Symptom:** After killing via taskkill /F, the process persists. Window stays on screen with no way to close it. Only PowerShell Stop-Process works.
- **Root cause:** Likely the background data thread (std::thread::spawn in data.rs) holds the process alive even after the main window is destroyed. The DuckDB connection or git subprocess may be blocking.
- **Fix:** Use a cancellation token / atomic bool checked in the data loop. Ensure clean shutdown on window close. Consider joining the thread on app exit.

## P1: Duplicate windows spawned during dev
- **Symptom:** Two dock windows appear — one large unstyled (tauri.conf size), one small styled (after frontend loads). Sometimes both persist.
- **Root cause:** Tauri file watcher restarts the binary when files change, creating a new window while the old one is still alive. The `visible: false` + frontend `show()` pattern adds complexity.
- **Fix:** Use `--no-watch` in dev. For production, ensure single-instance via Tauri's `single_instance` plugin or mutex.

## P2: Window height from tauri.conf.json not respected
- **Symptom:** Changing height in tauri.conf.json has no visible effect. Window stays same size.
- **Root cause:** DPI scaling may be converting logical pixels differently, or the WebView content constrains the window. The programmatic `setSize(LogicalSize)` call was added but needs verification.
- **Fix:** Debug actual vs expected physical pixel size. May need to account for DPI scale factor.

## P3: Pin dropdown menu clips at window right edge
- **Symptom:** Menu items truncated near right edge.
- **Status:** Partially fixed with `position: fixed; top: 20px; right: 4px`.

## P3: LOC chart shows single bar when commits are clustered
- **Symptom:** Day view shows one tall bar when all commits happen in a short window.
- **Note:** Technically correct — same behavior as tkinter version.
