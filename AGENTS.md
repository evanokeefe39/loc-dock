# AGENTS.md — Critical Architecture Knowledge

## Current Architecture (V2 — SQL-first medallion)

The data layer was rewritten (commits `55a12df`..`4c234e8`, schema v6). DuckDB is the
ETL engine; Rust is thin orchestration. See [`docs/REFACTOR_PLAN_V2.md`](docs/REFACTOR_PLAN_V2.md)
for the full plan and spike results.

**Two background loops → shared state → polled by frontend:**
- `data.rs::spawn_data_loop` — git scan + DuckDB ETL, builds `AllStats` (day/week) into
  `SharedStats` (`Arc<RwLock<AllStats>>`). Prefills day/week from `daily_aggregates` for
  <50ms first paint. Sleeps `refresh_interval.max(10)`s between cycles.
- `summary.rs::spawn_summary_loop` — collects recent commits + LLM summary into
  `SharedSummary`. Fully independent of the data loop (owns all its own git/LLM work).
- Frontend hooks (`useStats`, `useSummary`) poll `get_stats` / `get_summary` every ~10s.
  Commands are `async` and only read shared state — they never block the UI. The summary
  loop emits `summary-update` events (frontend listens for instant updates) and both loops
  emit `tasks-changed` events (toast spinner reactivates immediately). Events are used only
  where latency matters — core stats are pure polling (simpler, no serialization on the hot path).

**Medallion ETL (`usage_store.rs`):**
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

**Superseded by V2** (legacy sections below are historical record, not current design):
watermark tracking, two-phase ETL (`run_etl_urgent`/`run_etl_background`), the seek/tail
byte-positioning state machine, the RefCell query cache, and year-start git scanning
(now week-start, incremental per-repo via `git_cache`). The Appender vs `INSERT OR IGNORE`
finding (BUG-004/PERF-006) still holds.

## Gotchas & Hard Constraints

Findings that still bind current code. (The pre-V2 bug/perf/ADR log — watermark
poisoning, two-phase ETL, year-start git scan, missing ts index, query caching, the
`estimate_cost` cost divergence — is resolved or superseded by the V2 architecture above
and is no longer tracked here. Open work lives in [`ISSUES.md`](ISSUES.md).)

### DuckDB Appender does NOT support `INSERT OR IGNORE`

Silver ingest uses a prepared `INSERT OR IGNORE` statement inside a transaction — **not**
the columnar `Appender` API. The Appender is 40–326x faster in microbenchmarks, but on a
UNIQUE-constraint violation during `flush()` the **entire batch fails**:

```
Appender flush: Failed to append: PRIMARY KEY or UNIQUE constraint violation:
duplicate key "claude, ..., ..."
```

Since dedup-on-reingest is core to the pipeline, correctness wins. Batches are ~100–200
rows (<100ms), so the prepared-statement path is fast enough. Only revisit if DuckDB adds
`INSERT OR IGNORE` to the Appender API. Do not "optimize" this back to `con.appender()`.

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

## File Map

| File | Purpose | Notes |
|------|---------|----------|
| `src/lib.rs` | App bootstrap: manage shared state, register commands, spawn both loops | — |
| `src/data.rs` | Data loop: git scan + ETL → builds `AllStats` into `SharedStats` | first-paint prefill from gold |
| `src/usage_store.rs` | DuckDB medallion ETL (bronze/silver/gold) + serving queries + `.sql` template loader | schema v6; `INSERT OR IGNORE` not Appender (BUG-004); SQL from templates with user override in `~/.config/loc-dock/sql/` |
| `loc-dock-tauri/src-tauri/sql/claude-silver.sql` | Claude silver extraction SQL template | user override: `~/.config/loc-dock/sql/claude-silver.sql` |
| `loc-dock-tauri/src-tauri/sql/pi-silver.sql` | Pi silver extraction SQL template | user override: `~/.config/loc-dock/sql/pi-silver.sql` |
| `src/git.rs` | Git log scanning + numstat parse | incremental since cached `latest_ts` |
| `src/git_cache.rs` | Per-repo SHA/ts cache (DuckDB-backed) | skips unchanged repos |
| `src/summary.rs` | Independent loop: commit collection + LLM summary → `SharedSummary` | own git/LLM work; never blocks data loop |
| `src/pricing.rs` | `Pricing` struct loaded from `pricing.yaml` (per-Mtok cost) | externalized; user edits without recompile; creates default if absent |
| `src/task_queue.rs` | Active-task tracking for the UI (`get_active_tasks`) | — |
| `src/job_log.rs` | Job-run logging (`get_job_logs` / `clear_job_logs`) | — |
| `src/time_utils.rs` | Day/week boundary math | — |
| `src/types.rs` | `AllStats`, `RangeStats` structs | — |
| `src/commands.rs` | Async Tauri command handlers (read shared state only) | non-blocking |
| `src/config.rs` | `.env` settings load | `LOCDOCK_*` |
| `src/theme.rs` | YAML theme load | — |
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
cargo test    # incl. silver parity + idempotency tests in usage_store.rs
```

(V2 spike binaries were throwaway and removed in `7935ca2`; spike findings live in
[`docs/REFACTOR_PLAN_V2.md`](docs/REFACTOR_PLAN_V2.md) §11 and [`docs/SPIKE_RESULTS.md`](docs/SPIKE_RESULTS.md).)

### Version bump locations
- `loc-dock-tauri/tauri.conf.json`
- `loc-dock-tauri/src-tauri/Cargo.toml`
- `loc-dock-tauri/package.json`
