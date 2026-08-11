# AGENTS.md — Critical Architecture Knowledge

## Current Architecture (V2 — SQL-first medallion)

The data layer was rewritten (commits `55a12df`..`4c234e8`, schema v6). DuckDB is the
ETL engine; Rust is thin orchestration. See [`docs/DATA_FLOWS.md`](docs/DATA_FLOWS.md)
for detailed end-to-end data flow diagrams.

### Single data loop (no independent summary loop)

- `data.rs::spawn_data_loop` — builds a `SourceManager` from config's `data_sources` list,
  ingests changed JSONL files per source through bronze→silver→gold, scans git incrementally,
  runs AI summaries on changed repos. Builds `AllStats` (day/week/month/year) into
  `SharedStats` (`Arc<RwLock<AllStats>>`). Prefills all four ranges from `daily_aggregates`
  for <50ms first paint. Sleeps `refresh_interval.max(10)`s.
- **Incremental git scan** — `commit_stats` table (`repo, sha, ts, msg, added, deleted`)
  stores per-commit data. Each cycle queries `MAX(ts)` from the table and runs
  `git log --after={ts}` — typically 0–5 new commits → <100ms vs 7.6s re-scanning
  the full week window. Past commits are never re-scanned.
- **Summaries on change** — after git insert, checks each repo's `head_sha` against
  the `repo_summaries` table. If SHA changed, calls LLM per-repo and caches highlights.
  `SummaryData` is rebuilt from cached highlights + `count_repo_commits_since()` queries.
- **No independent summary loop** — `summary.rs` only contains types (`RepoSummary`,
  `SummaryData`), LLM helper functions, and `reset_summaries`. The old
  `spawn_summary_loop` (with its own git scan, debounce, and SHA tracking) is removed.
- **Frontend**: TanStack Query polls `get_stats` / `get_summary` every ~10s.
  Built-in `isLoading` / `isFetching` states replace the hand-rolled hooks and the
  backend `ready` flag for UI state. Zustand holds UI-pure state (range, mode, panel
  visibility) — no prop drilling. Commands are `async` and only read shared state —
  they never block the UI.

### Medallion ETL (`usage_store.rs`):

- **Bronze** — ephemeral `read_ndjson_objects` CTE per batch. Raw JSONL via glob, zero
  schema inference (`read_ndjson_auto` OOM-crashes on heterogeneous logs).
- **Silver** — `entries` table. One `INSERT...SELECT` per source loads SQL from
  `.sql` template files (`sql/claude-silver.sql`, `sql/pi-silver.sql`). Extracts
  fields by JSON path, flat-prices cost per model via LiteLLM JSON, dedups by
  `(source, session_id, ts)`.
- **Gold** — `daily_aggregates` materialized per-date/per-source rollup. Serving queries
  read gold; multi-day `sessions_total` queries `entries` directly (distinct counts are
  not additive across days).
- **Config-driven** — silver extraction SQL is externalized to user-editable `.sql` files
  (`sql/claude-silver.sql`, `sql/pi-silver.sql`, `sql/codex-silver.sql`). Pricing comes
  from the community-maintained LiteLLM JSON (2,800+ models). No recompile needed when
  provider pricing or JSONL schemas change.
- **Multi-source adapters** — `source_adapter.rs` defines `SourceKind` (Claude, Pi,
  Codex) and `GlobFileDiscoverer` which walks each configured `data_sources` directory
  with adapter-specific skip rules (e.g. skips `subagents/` for Claude). The data loop
  builds a `SourceManager` from the config's `data_sources` list and processes every
  source on each cycle.
- **Incremental ingest** — `ingested_files (path, mtime, size)` registry; each cycle
  re-reads only changed files. Cold start micro-batches the full glob to cap JSON-parse
  memory.

### Startup & single-instance:

- `lib.rs::enforce_single_instance` — writes a PID lock file (`instance.lock`). On subsequent
  launches, reads the file, checks if the PID is alive via `tasklist` (Windows) / `kill -0`
  (Unix), and exits cleanly if another instance is running. Works even for old builds that
  predate `tauri-plugin-single-instance`.
- `tauri-plugin-single-instance` — additional guard for builds that have the plugin. The
  callback fires on the **first** instance when a duplicate connects; the plugin kills the
  second instance automatically.
- **DuckDB open with retry** — `open_usage_cache()` retries `Connection::open` 5× with
  linear backoff (200-1000ms). On retry, removes stale `.wal`/`.tmp` files from crashed
  processes. Handles hot-reload races where `tauri dev` kills + restarts the app.
- **Deferred window show** — on startup the window is positioned and then hidden. The
  frontend emits a `frontend-ready` event after React mounts; only then does Rust show
  the window. This avoids a WebView2 navigation race that would display "can't reach this
  page" during the flash. The `show-main-window` event (from `tauri-plugin-single-instance`
  callback) also shows the window, reusing the same handler.
- **Close to tray** — `WindowEvent::CloseRequested` is intercepted: `api.prevent_close()`
  + `win.hide()` keeps the app running in the system tray.

### Config hot-reload (no restart needed):

- Config is stored as `Arc<RwLock<Config>>` in Tauri managed state.
- `save_settings` writes to disk + reloads the shared `RwLock` from a fresh `Config::load()`.
- `restart_app` is now a **no-op** — settings take effect immediately. Under `tauri dev`,
  the old `app.restart()` killed the dev watcher, leaving an orphaned app.
- The data loop re-reads config each cycle via `config.read().unwrap()` on the shared
  `Arc<RwLock<Config>>` — API key, model, endpoint, and data_sources changes are picked
  up live without restart.

## Lessons Learned

1. **Establish data architecture early.** DuckDB + medallion (bronze/silver/gold)
   handles ETL, JSON parsing, aggregation, and bucketing natively. The first version
   did all of this in Rust (~2,239 LOC of ETL core where ~500 would do). DuckDB is the
   ETL engine; Rust is thin orchestration around it.

2. **Use industry-standard dashboard stack.** TanStack Query handles polling,
   loading/error states, stale-while-revalidate, and cache dedup out of the box.
   Zustand eliminates prop drilling. The project previously hand-rolled all of this
   in custom React hooks + useState, reinventing wheels that come free with the
   standard stack.

## Gotchas & Hard Constraints

Findings that still bind current code.

### `Connection` is `Send` but not `Clone`

DuckDB's `Connection` implements `Send` but not `Clone`. The connection is opened once
in `lib.rs` and passed into `spawn_data_loop` — only one thread uses it. If you ever need
to share across threads, use `Connection::try_clone()`. Never call `Connection::open()`
twice on the same database.

### Stale WAL files block DuckDB open

When the previous process crashes or is killed, DuckDB's `.wal` file persists and claims
the database is still open. The retry loop in `open_usage_cache()` removes `.wal` and
`.tmp` files on each retry attempt. On Windows, `std::fs::remove_file` fails silently if
the file is actively locked by a live process, so this is safe.

### DuckDB Appender does NOT support `INSERT OR IGNORE`

Silver ingest uses a prepared `INSERT OR IGNORE` statement inside a transaction — **not**
the columnar `Appender` API. The Appender is 40–326x faster in microbenchmarks, but on a
UNIQUE-constraint violation during `flush()` the **entire batch fails**. Since dedup-on-reingest
is core to the pipeline, correctness wins.

### Distinct counts are not additive across days

`sessions_total` = `COUNT(DISTINCT session_id)` cannot be summed over daily gold rows
(a session spanning days is counted once per day). Multi-day session counts query the
`entries` silver table directly (~6ms). Never sum `daily_aggregates` session counts.

### `read_ndjson_objects`, never `read_ndjson_auto`

Schema auto-inference over the real heterogeneous Claude/Pi logs OOM-crashes (even with
`sample_size=-1`). Bronze must read raw `JSON` per line and extract by path in silver.

### Windows path separators in DuckDB `filename`

DuckDB's `filename` pseudo-column uses `\` on Windows. Path filters must be
separator-agnostic (`ILIKE '%subagents%'`, not `'%/subagents/%'`) or subagent logs leak in
and double row counts.

### Aggregate refresh `last_row_count` ordering

`last_row_count` must be updated **after** a successful `refresh_aggregates()` call. If
updated before, a transient DuckDB error permanently poisons the gate — aggregates never
refresh again because `current_count == last_row_count` on subsequent cycles. Fixed in
schema v8.

## File Map

| File | Purpose | Notes |
|------|---------|----------|
| `src/lib.rs` | App bootstrap: manage shared state, register commands, spawn loops | single-instance guard; shared DuckDB connection opened here; config as `Arc<RwLock<>>` |
| `src/data.rs` | Data loop: git scan + ETL + summaries → `SharedStats` + `SharedSummary` | incremental git via `commit_stats` table; on-change LLM per-repo; prefill from gold |
| `src/git.rs` | Git log scanning + `GitCommit` struct + `collect_new_commits()` | returns `RepoCommits` per repo with SHA, messages, LOC; caller stores in DuckDB |
| `src/usage_store.rs` | DuckDB medallion ETL + `commit_stats` + `repo_summaries` + serving queries | schema v8; `INSERT OR IGNORE` not Appender; SQL templates for silver extraction |
| `src/summary.rs` | Types (`RepoSummary`, `SummaryData`), LLM helpers, `summarize_one_repo()` | No more `spawn_summary_loop` — summaries are triggered by the data loop |
| `loc-dock-tauri/src-tauri/sql/claude-silver.sql` | Claude silver extraction SQL template | user override: `~/.config/loc-dock/sql/claude-silver.sql` |
| `loc-dock-tauri/src-tauri/sql/pi-silver.sql` | Pi silver extraction SQL template | user override: `~/.config/loc-dock/sql/pi-silver.sql` |
| `loc-dock-tauri/src-tauri/sql/codex-silver.sql` | Codex CLI silver extraction SQL template | same override pattern |
| `loc-dock-tauri/src-tauri/sql/omp-silver.sql` | Oh My Pi silver extraction SQL template | same override pattern |
| `src/source_adapter.rs` | Source kinds, data source config, glob discoverer | `SourceKind::Claude\|Pi\|Codex\|Omp`; `GlobFileDiscoverer` with skip-subdir support |
| `src/pricing.rs` | `Pricing` struct loaded from LiteLLM JSON | per-model pricing from community-maintained JSON; user override via `model_pricing_path` |
| `src/task_queue.rs` | Active-task tracking for the UI | — |
| `src/job_log.rs` | Job-run logging | — |
| `src/time_utils.rs` | Day/week boundary math | — |
| `src/types.rs` | `AllStats`, `RangeStats` structs | — |
| `src/commands.rs` | Async Tauri command handlers | non-blocking; `restart_app` is a no-op |
| `src/config.rs` | Settings: `settings.json` read/write + `.env` migration | loads LiteLLM pricing; `LOCDOCK_*` env vars as migration fallback |
| `src/theme.rs` | YAML theme load | multi-document support |
| `src/tray.rs` | System tray | — |
| `loc-dock-tauri/src/hooks/queries.ts` | TanStack Query hooks: `useStatsQuery`, `useSummaryQuery`, `useThemeQuery` | polling interval 10s; event-driven summary cache updates |
| `loc-dock-tauri/src/lib/store.ts` | Zustand store: range, mode, tooltip/settings/summary visibility | no prop drilling; components import `useUIStore` directly |
| `loc-dock-tauri/src/components/*.tsx` | React components: TopRow, Chart, BottomRow, SummaryPanel, SettingsPanel, CostTooltip | — |

## Quick Reference

### Build commands
```bash
cd loc-dock-tauri
npm install
npm run tauri dev       # development (Vite dev server + debug Rust)
npm run tauri build     # release — runs beforeBuildCommand (npm run build),
                        # embeds frontend dist, compiles Rust, bundles NSIS/MSI
```

**Only `npm run tauri build` produces a working release binary.**
`cargo build --release` from src-tauri/ bypasses `beforeBuildCommand`, so the
embedded frontend dist may be stale or missing. The webview navigates to
`tauri://localhost/` and gets nothing → "can't reach this page."

For fast iteration (after the first `npm run tauri build` has cached DuckDB):
```bash
npm run build              # rebuild frontend dist fresh
cd src-tauri && cargo build --release   # quick Rust-only recompile
```
This skips the NSIS bundling. The binary at `target/release/loc-dock.exe` is the
same binary `npm run tauri build` produces — just without the installer bundle.

### Tests
```bash
cd loc-dock-tauri/src-tauri
cargo test    # incl. silver parity + idempotency tests in usage_store.rs + git commit parser tests
```

### Version bump locations
- `loc-dock-tauri/tauri.conf.json`
- `loc-dock-tauri/src-tauri/Cargo.toml`
- `loc-dock-tauri/package.json`

### Releases
CI/CD (`.github/workflows/release.yml`) builds all platforms automatically on `v*` tag push.
No manual upload needed — just bump versions, tag, push.

**Release naming convention** — titles are always `LOC Dock vX.Y.Z`.
The `generateReleaseNotes: true` flag in the workflow auto-generates the changelog body
from merged PRs and commits. Never hand-edit the release title — the workflow owns it
consistently. Drafts are published manually after CI completes (see `releaseDraft: true`).

### Version bump locations
## Key Paths

| Path | Platform | Purpose |
|------|----------|----------|
| `%APPDATA%/loc-dock/` | Windows | Config directory (settings, theme, overrides) |
| `~/.config/loc-dock/` | Linux | Config directory |
| `~/Library/Application Support/loc-dock/` | macOS | Config directory |
| `%APPDATA%/loc-dock/settings.json` | Windows | All user settings (API key, model, autostart, data sources, …) |
| `%APPDATA%/loc-dock/theme.yaml` | Windows | Visual theme override |
| `%APPDATA%/loc-dock/sql/` | Windows | SQL template overrides (`claude-silver.sql`, `pi-silver.sql`) |
| `%APPDATA%/loc-dock/litellm.json` | Windows | Pricing data override |
| `target/release/loc-dock.exe` | — | Release binary (after `npm run tauri build`) |
| `HKCU\…\Run` → `LOC Dock` | Windows | Autostart registry key (set via `autostart: true`) |

All config paths use `dirs::config_dir()` (`config.rs:70`). On cold start without settings.json, defaults come from `LOCDOCK_*` env vars or hardcoded fallbacks in `Settings::default()`.
