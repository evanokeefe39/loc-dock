# AGENTS.md — Critical Architecture Knowledge

## Active Bugs

### BUG-001: Watermark Poisoning Between ETL Phases (CRITICAL) — FIXED

**Status:** ✅ Fixed (Step 1 of refactor, schema v4)
**Severity:** Was data loss — week/month/year ranges permanently missing token/cost/session data

**What was fixed:** Merged two-phase ETL into single pass over 7-day window with `INSERT OR IGNORE` dedup. Removed watermark tracking entirely. Schema bumped 3→4.

**Landmine avoided (BUG-004):** DuckDB `Appender` API does NOT support `INSERT OR IGNORE`. Initial implementation used `con.appender()` + `append_row()` per PERF-006 spike results. When a UNIQUE constraint violation occurred during flush, the **entire batch failed**. Switched to prepared `INSERT OR IGNORE` inside a transaction — correct semantics, still fast.

**Spike output:**
```
Single-phase:
  Cycle 1: ingested=9 total=9 ← all files in one pass
  Cycle 2: ingested=1 total=9 ← no new files, no duplicates
```

### BUG-002: `estimate_cost` vs DB-stored cost divergence (MEDIUM)

`build_all_stats` computes `cost_total` via `pricing::estimate_cost(day_tokens)` which recalculates from flat per-token rates, but `cost_breakdown` reads DB-stored costs. These can diverge when:
- Pricing constants change between cycles
- `fill_costs` is skipped for entries that already have non-zero `total_cost` from the parser
- DB-stored costs use per-provider pricing vs flat global rates

**Fix:** Make `cost_total` come from the same source as `cost_breakdown` (DB query `SUM(total_cost)`).

### BUG-004: DuckDB Appender does not support INSERT OR IGNORE (CRITICAL — DEFERRED)

**Status:** 🛑 Deferred. Switched to prepared INSERT OR IGNORE in a transaction.

**Problem:** DuckDB's `Appender` API (`con.appender()` + `append_row()` + `flush()`) does
NOT support `INSERT OR IGNORE` semantics. When a UNIQUE constraint violation occurs
during `flush()`, the **entire batch** fails with:
```
Appender flush: Failed to append: PRIMARY KEY or UNIQUE constraint violation:
duplicate key "claude, ..., ..."
```

This means:
- First ETL cycle: some chunks succeed, some fail → partial data
- Subsequent cycles: nearly every entry is a duplicate → entire flush fails → zero inserted
- Result: **no token/cost/session data in UI** (only LOC from git works)

**Current fix:** Replaced `con.appender()` with a prepared `INSERT OR IGNORE` statement
inside a single transaction. Performance is still good — the transaction provides batch
semantics without the Appender's constraint enforcement issue.

**When to revisit:** If DuckDB adds `INSERT OR IGNORE` to the Appender API, or if
actual performance profiling shows the prepared-statement approach is a bottleneck
(not expected — typical batch size is ~100-200 entries, which takes <100ms).

### BUG-003: Session count SQL uses same param twice (LOW)

In `count_sessions`, the SQL uses `?::TIMESTAMP` for both `since_str` and `active_str`, but `active_str` should use a different parameter. Currently bound as `[since_str, active_str]` which is correct in practice, but the SQL template has two `?` placeholders which are both cast to `TIMESTAMP`. This happens to work because both params are timestamp strings, but it's fragile.

## Performance Issues

### PERF-001: Git scan from year start every cycle

`data.rs` always runs `git log --since={year_start}` per repo. For repos with large histories, this walks thousands of commits even when only day data is needed.

**Spike results** (110 repos on this machine):
- Full scan from Jan 2026: **47s wall time** (serial)
- Incremental scan (since cached HEAD ts): **4.9s wall time** (serial)
- With parallel rayon: **~200ms** (dominated by ~50ms git process startup per repo)
- Busiest repos: paperclip (6.3s), deepagents (5.5s), spectr (5.0s), airbyte-spike (13.3s)
- Most repos have 0 new commits since their cached HEAD (scan returns instantly)

**Fix:** Use the cached `latest_ts` per repo as the since parameter. Only scan commits newer than the last cached commit. Skip repos whose SHA hasn't changed entirely.

### PERF-002: All 4 ranges computed on every emit

`build_all_stats()` queries ALL 4 ranges (day, week, month, year) every time `emit_stats!()` fires — twice per cycle. Each call makes 6+ DuckDB queries × 4 ranges = 24+ queries. Most return the same results.

**Fix:** Range-aware building — only compute the day range on urgent emit. Compute week/month/year asynchronously.

### PERF-003: No DuckDB index on ts column

The `entries` table has no index on `ts`. All queries filter on `WHERE ts >= ?::TIMESTAMP`. Without an index, DuckDB does a full table scan for every query.

**Spike results** (10k rows, 48 buckets):
- SUM tokens (full scan): ~3ms
- SUM tokens (WHERE ts >= 1d ago): ~5ms
- cost breakdown (4 SUMs): ~5ms
- source breakdown (GROUP BY): ~17-22ms (slowest query)
- cost/token timeline (ORDER BY): ~5-16ms
- session count (DISTINCT FILTER): ~1ms (fastest)
- Index does NOT help significantly — DuckDB's zone maps (min/max per row group) are already effective for sequential scans against timestamp columns

**Conclusion:** The queries themselves are not the bottleneck (debug mode ~1-22ms each, release mode ~0.1-2ms). The real bottleneck is doing 32+ queries × 2 emits = 64+ queries per cycle when most return unchanged data.

**Fix:** Query result caching — skip re-querying if no new rows were ingested since last emit.

### PERF-004: No query result caching

Every cycle re-queries the same data. If no new entries were inserted (watermark hasn't advanced), all queries return identical results.

**Fix:** Cache query results keyed by (range, query_type, db_row_count_hash).

### PERF-005: First paint blocked by git scan

The data loop is serial: git scan → Phase 1 ETL → emit. Git scan can take 1–3s on first run. User sees nothing until it completes.

**Fix:** Emit cached stats from previous cycle immediately (< 1ms), then update asynchronously.

### PERF-006: DuckDB row-by-row INSERT instead of Appender

`appender_insert` uses a prepared `INSERT OR IGNORE` statement inside a transaction.
DuckDB's `Appender` API (columnar bulk insert) is vastly faster in benchmarks, but
does NOT support `INSERT OR IGNORE` semantics (see BUG-004).

**Spike results** (16-column table, in-memory, debug mode):
```
    Rows     Raw INSERT (µs)  Prepared (µs)   Appender (µs)  Speedup
      10           281,750        226,067           6,963      40x
      50           881,470        734,862           8,189     107x
     100         7,434,268      7,016,381          22,776     326x
```
Appender is **40-326x faster** than row-by-row INSERT, even for small batches.

**Decision:** Prefer correctness over theoretical speed. The prepared `INSERT OR IGNORE`
in a transaction is still fast enough (actual batches are ~100-200 entries, taking
<100ms). Revisit if DuckDB adds INSERT OR IGNORE support to the Appender API.

## Architecture Decisions

### ADR-001: Single-Phase ETL (replace two-phase)

**Decision:** Merge `run_etl_urgent` and `run_etl_background` into a single ETL method.

**Rationale:**
- Two-phase with shared watermark is fundamentally broken (BUG-001)
- Single-phase with INSERT OR IGNORE dedup is simpler and correct
- Same I/O cost (scan 7d files each cycle) but no redundant processing
- Eliminates mental model complexity

**Migration:**
1. Remove `run_etl_background()`, rename `run_etl_urgent()` to `run_etl()` with 7-day cutoff
2. Keep `build_all_stats` but make range-aware (day only for first emit)
3. Add `CREATE INDEX IF NOT EXISTS` in schema init

### ADR-002: Stats emission should be incremental, not full rebuild

**Decision:** `build_all_stats` should accept a parameter for which ranges to compute.

**Rationale:**
- Day view needs day data only. No point computing year/month/week.
- After Phase 1 (24h), only day + week have new data.
- Full rebuild is wasteful and increases latency.

**Implementation:**
```rust
fn build_stats_for_ranges(
    git_points: &[GitPoint],
    store: &UsageStore,
    ranges: &[TimeRange],  // ["day", "week"] for urgent emit
    ...
) -> Partial<AllStats>
```

### ADR-003: Git scan should be incremental per-repo

**Decision:** Use cached `latest_ts` as `--since` parameter instead of year_start.

**Rationale:**
- Only need to scan commits newer than the cache
- Dramatically reduces git log output for stable repos
- Cache already tracks `head_sha` and `latest_ts`

**Implementation:**
In `get_git_loc_timeline`, when SHA matches (no new commits), skip the repo entirely. When SHA changed, only scan since `latest_ts` (not year_start).

## File Map

| File | Purpose | Concerns |
|------|---------|----------|
| `src/data.rs` | Main data loop, stats building | BUG-002, PERF-002, PERF-005 |
| `src/usage_store.rs` | ETL pipeline, DuckDB queries | BUG-001 (critical), PERF-003, PERF-006 |
| `src/source_adapter.rs` | File discovery, JSON parsing | None critical |
| `src/git.rs` | Git log scanning | PERF-001 |
| `src/git_cache.rs` | Git cache (DuckDB-backed) | ADR-003 |
| `src/pricing.rs` | Per-token cost estimation | BUG-002 related |
| `src/types.rs` | AllStats, RangeStats structs | None |
| `src/commands.rs` | Tauri command handlers | None |
| `src/summary.rs` | AI summary generation | Out of scope |

## Quick Reference

### Build commands
```bash
cd loc-dock-tauri
npm install
npm run tauri dev       # development
npm run tauri build     # release (NSIS installer)
```

### Run spikes
```bash
cd loc-dock-tauri/src-tauri
cargo rustc --bin spike-watermark-bug -- -l rstrtmgr && ./target/debug/spike-watermark-bug.exe
```

### Version bump locations
- `loc-dock-tauri/tauri.conf.json`
- `loc-dock-tauri/src-tauri/Cargo.toml`
- `loc-dock-tauri/package.json`
