# AGENTS.md — Critical Architecture Knowledge

## Current Architecture (V2 — SQL-first medallion)

The data layer was rewritten (commits `55a12df`..`4c234e8`, schema v6). DuckDB is the
ETL engine; Rust is thin orchestration. See [`docs/REFACTOR_PLAN_V2.md`](docs/REFACTOR_PLAN_V2.md)
for the full plan and spike results.

### Single data loop (no independent summary loop)

- `data.rs::spawn_data_loop` — git scan + DuckDB ETL + AI summaries. Builds `AllStats`
  (day/week) into `SharedStats` (`Arc<RwLock<AllStats>>`). Prefills day/week from
  `daily_aggregates` for <50ms first paint. Sleeps `refresh_interval.max(10)`s.
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
- Frontend hooks (`useStats`, `useSummary`) poll `get_stats` / `get_summary` every ~10s.
  Commands are `async` and only read shared state — they never block the UI.

### Medallion ETL (`usage_store.rs`):

- **Bronze** — ephemeral `read_ndjson_objects` CTE per batch. Raw JSONL via glob, zero
  schema inference (`read_ndjson_auto` OOM-crashes on heterogeneous logs — Spike §11).
- **Silver** — `entries` table. One `INSERT...SELECT` per source loads SQL from
  `.sql` template files (`sql/claude-silver.sql`, `sql/pi-silver.sql`). Extracts
  fields by JSON path, flat-prices cost via `pricing.yaml`, dedups by
  `(source, session_id, ts)`.
- **Gold** — `daily_aggregates` materialized per-date/per-source rollup. Serving queries
  read gold; multi-day `sessions_total` queries `entries` directly (distinct counts are
  not additive — Spike 5).
- **Config-driven** — pricing and silver extraction SQL are externalized to
  user-editable files. No recompile needed when provider pricing or JSONL schemas change.
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

### Config hot-reload (no restart needed):

- Config is stored as `Arc<RwLock<Config>>` in Tauri managed state.
- `save_settings` writes to disk + reloads the shared `RwLock` from a fresh `Config::load()`.
- `restart_app` is now a **no-op** — settings take effect immediately. Under `tauri dev`,
  the old `app.restart()` killed the dev watcher, leaving an orphaned app.
- The data loop re-reads config each cycle via a `read_config()` closure — API key,
  model, and endpoint changes are picked up live without restart.

## Gotchas & Hard Constraints

Findings that still bind current code.

### `Connection` is `Send` but not `Clone`

DuckDB's `Connection` implements `Send` but not `Clone`. To share a single `Database`
handle across threads, use `Connection::try_clone()` which returns a `Result<Connection>`.
This is used in `lib.rs` setup to give both the data loop and summary loop a connection
backed by the same in-memory `Database` handle. Do not call `Connection::open()` twice.

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
| `src/pricing.rs` | `Pricing` struct loaded from `pricing.yaml` | externalized; user edits without recompile |
| `src/task_queue.rs` | Active-task tracking for the UI | — |
| `src/job_log.rs` | Job-run logging | — |
| `src/time_utils.rs` | Day/week boundary math | — |
| `src/types.rs` | `AllStats`, `RangeStats` structs | — |
| `src/commands.rs` | Async Tauri command handlers | non-blocking; `restart_app` is a no-op |
| `src/config.rs` | `.env` settings load + settings.json persistence | `LOCDOCK_*` |
| `src/theme.rs` | YAML theme load | multi-document support |
| `src/tray.rs` | System tray | — |

## Quick Reference

### Build commands
```bash
cd loc-dock-tauri
npm install
npm run tauri dev       # development
npm run tauri build     # release (NSIS installer)
```

### Tests
```bash
cd loc-dock-tauri/src-tauri
cargo test    # incl. silver parity + idempotency tests in usage_store.rs + git commit parser tests
```

### Version bump locations
- `loc-dock-tauri/tauri.conf.json`
- `loc-dock-tauri/src-tauri/Cargo.toml`
- `loc-dock-tauri/package.json`
