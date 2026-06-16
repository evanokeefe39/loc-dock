# Stabilization Plan — LOC Dock ETL & Display

**Goal:** Sub-1 second day view display with correct token/cost data. Reliable ETL for all ranges.

**Date:** 2026-06-15

---

## Phase 0: Bug Fixes (Critical — Do First)

### 0.1 Fix Watermark Poisoning Between ETL Phases

**Problem:** `run_etl_urgent` advances the per-source watermark to the max mtime seen in the last 24h. `run_etl_background` then uses the *same* watermark, causing it to skip files 2–7 days old (their mtimes are lower than the watermark set by Phase 1). These files are **never ingested**, so week/month/year queries return incomplete token/cost/session data forever.

**Fix:** Use a separate watermark key per phase, or — better — make `run_etl_background` ignore the watermark and scan the full window each time with dedup (INSERT OR IGNORE handles this already), or merge into a single-phase ETL that processes the full retention window every cycle and relies on INSERT OR IGNORE for dedup.

### 0.2 `estimate_cost` Double-Calculation Mismatch

**Problem:** `build_all_stats` computes `cost_total` via `pricing::estimate_cost(&day_tokens)` (flat per-token rate) but stores per-entry costs via `fill_costs` in the DB. The `cost_breakdown` reads DB-stored costs. These can diverge if pricing constants change or if `fill_costs` is skipped.

**Fix:** Either read cost_total from the DB breakdown sum, or make both paths use the same function. Prefer: query DB for total cost (SUM(total_cost)) instead of recomputing from tokens.

### 0.3 `estimate_cost` in TopRow Uses Tokens, Not Cost Field

**Check:** The TopRow displays `cost_total` from `RangeStats`, which calls `estimate_cost`. If cost data exists in DB but the query path doesn't reach it, display shows 0. Needs verification that `pricing::estimate_cost` and DB-stored costs agree.

---

## Phase 1: Quick Wins (Sub-1 Day View)

### 1.1 Range-Aware Stats Building

**Problem:** `build_all_stats` computes ALL four ranges (day, week, month, year) every time `emit_stats!()` is called — twice per 60s cycle. Each call makes 16+ DuckDB queries and 4 git bucketing passes.

**Fix:** Make `build_all_stats` accept a parameter for which ranges to compute. For the urgen Phase 1 emit, only compute **day** + **week** (week = 7d, close enough). Compute month/year only in the Phase 2 emit. This cuts ~50% of query work in the critical path.

**Tech Spike:** Measure DuckDB query latency per range. If individual queries are <5ms, this optimization alone won't achieve sub-1s — the bottleneck is elsewhere (git scan).

### 1.2 Git Scan Optimization

**Problem:** `get_git_loc_timeline` runs `git log --since={year_start} --numstat` for every repo that has a SHA change. For repos with large histories, even `--since` filtering can take 200–500ms per repo. This blocks the entire data loop.

**Fix:** Two-pronged:
1. **Incremental git fetch**: Instead of scanning from year_start, scan only since the last cached commit per repo. The cache already tracks `latest_ts`. Use that instead of year_start as the since parameter.
2. **Parallel git per repo**: Use rayon to run git commands per-repo in parallel (they're I/O bound, parallelism helps).

**Target:** <200ms total git scan for 10 repos, regardless of repo age.

### 1.3 Move Git Scan Off Critical Path

**Problem:** Even with optimizations, git scan blocks the first stats emission. The UI shows stale data until git completes.

**Fix:** 
1. Cache git bucket results per-cycle (memoization based on cached data version)
2. On first load, emit stats with cached data immediately, then update asynchronously
3. Only re-scan repos whose HEAD changed

The git cache already stores `head_sha`. If no SHA changed, skip the git log entirely.

### 1.4 Early Emission Without Waiting for ETL

**Problem:** The cycle is: git scan → Phase 1 ETL → emit. The git scan can take 1–3s before any data appears.

**Fix:** Split the cycle:
1. Read cached stats from previous cycle → emit immediately (instant)
2. Git scan (in background thread)
3. Phase 1 ETL (24h, fast)
4. Emit updated stats
5. Phase 2 ETL (background, can take longer)
6. Emit updated stats

This gives the user instant data on app start and incremental updates.

---

## Phase 2: ETL Architecture Redesign

### 2.1 Single-Phase ETL with Proper Dedup

**Problem:** Two-phase ETL with shared watermark is fundamentally broken (0.1). The workaround (ignore watermark in Phase 2) means Phase 2 re-processes all 7-day files every cycle.

**Fix:** Merge into a single ETL phase that:
- Processes files in the 7-day window
- Relies on `INSERT OR IGNORE` (UNIQUE constraint on `(source, session_id, ts, file_path)`) for dedup
- Updates watermark to the max mtime seen
- If watermark hasn't advanced, skip processing entirely (no new files)

This is simpler, correct, and eliminates the Phase 1/Phase 2 confusion.

### 2.2 DuckDB Bulk Insert via Appender

**Problem:** Row-by-row INSERT in a transaction (current `appender_insert`) is slow for large batches. Each INSERT is parsed and planned individually.

**Fix:** Use DuckDB's `Appender` API for columnar bulk inserts. This is the recommended path for high-throughput ingestion and is 5–50x faster.

**Tech Spike:** Benchmark row-by-row INSERT vs Appender with 1000+ entries. If the batch size is typically small (<50 entries per cycle), this won't matter. If large (>500), it will.

### 2.3 Parallel File Parsing with Rayon

**Problem:** `process_source` processes files in chunks of 10, sequentially. Each file parse involves JSON deserialization which is CPU-bound.

**Fix:** Already uses `rayon` in Cargo.toml but it's not used in `process_source`. Use `par_bridge()` or `par_iter()` on the file chunks to parallelize parsing across CPU cores.

### 2.4 Query Caching Layer

**Problem:** Every `emit_stats!()` call hits DuckDB with 4+ queries per range. With 4 ranges × 2 emits = 8+ queries per cycle. Most return the same results if no new data was ingested.

**Fix:** Add a query result cache keyed by (range, query_type, db_version) where db_version increments on each successful ETL insert. If no new entries were added since last emit, return cached results.

**Tech Spike:** Measure whether query latency is significant (<5ms vs >50ms per query). If queries are fast, caching adds unnecessary complexity.

---

## Phase 3: Frontend Rendering Performance

### 3.1 Selective Re-render

**Problem:** The `useStats` hook sets the entire `AllStats` object on every `stats-update` event, causing all components to re-render. The chart component re-draws all buckets even if only the mode changed.

**Fix:** Use React state selectors or `useMemo` to only re-render components whose data actually changed. Split the stats update into targeted events (e.g., `stats-update-day`, `stats-update-chart-mode`) for fine-grained updates.

### 3.2 Chart Virtualization

**Problem:** Canvas-based chart redraws 48 buckets × 4 ranges × 3 modes. For the day view, this is trivial. But the current chart might redraw all 48 buckets every time any data changes.

**Fix:** Memoize bucket SVG elements. Only re-render buckets whose content changed.

---

## Spike Research Items

### Spike 1: DuckDB Query Latency Profiling

Measure per-query latency for each of these queries:
```
query_since          → SELECT SUM(...) FROM entries WHERE ts >= ?
query_cost_breakdown → SELECT SUM(...cost fields...) FROM entries WHERE ts >= ?
query_cost_timeline  → SELECT epoch(ts), total_cost FROM entries WHERE ts >= ? ORDER BY ts
query_token_timeline → SELECT epoch(ts), token fields FROM entries WHERE ts >= ? ORDER BY ts
count_sessions       → SELECT COUNT(DISTINCT session_id) ... FROM entries
query_source_breakdown → SELECT source, ... FROM entries WHERE ts >= ? GROUP BY source
```

Test with 100, 1000, 10000, 50000 entries. Index the `ts` column if not already indexed.

### Spike 2: Git Scan Profiling

Measure per-repo `git log --since` latency for repos with:
- Small history (<100 commits in range)
- Medium history (500-2000 commits in range)
- Large history (5000+ commits in range)

Also test incremental `git log --since={latest_cached_ts}` vs full `--since={year_start}`.

### Spike 3: Alternative to `git log --numstat`

Investigate whether `git diff --numstat` on each commit is faster, or whether `git log --format=... --numstat --since` is the optimal approach. Consider using `git2` (libgit2 bindings) for faster git operations.

---

## Decision: Single-Phase vs Two-Phase ETL

After fixing the watermark bug, evaluate whether two-phase has any benefit:

| Aspect | Single-Phase | Two-Phase |
|--------|------------|-----------|
| Correctness | ✅ Simple, correct | ❌ Watermark bug, complex |
| Day view latency | Same (24h data processed either way) | Same |
| Background load | More work per cycle | Less work per cycle |
| Code complexity | Low | High |

**Recommendation:** Single-phase ETL with INSERT OR IGNORE dedup. Process 7-day window every cycle. If no new files exist (watermark hasn't advanced), skip entirely.

---

## Target Architecture (After All Phases)

```
 ┌─────────────────────────────────────┐
 │  Data Refresh Cycle (60s)            │
 │                                      │
 │  1. Check git HEAD SHAs              │  ~10ms (cached)
 │     → Only scan changed repos        │  ~100ms per changed repo
 │                                      │
 │  2. Single-Phase ETL                 │
 │     → Discover files since watermark │  ~20ms (glob)
 │     → Rayon-parallel parse           │  ~50ms per source
 │     → DuckDB Appender bulk insert    │  ~10ms for 100 entries
 │     → Update watermark               │  ~5ms
 │                                      │
 │  3. Range-aware stats build          │
 │     → Compute DAY only for emit 1    │  ~30ms (3 DB queries)
 │     → Compute WEEK/MONTH/YEAR async  │  ~30ms each
 │                                      │
 │  4. Emit only changed data           │  ~5ms
 │                                      │
 │  Total: < 500ms typical              │
 │  First paint: < 100ms (cached)       │
 └─────────────────────────────────────┘
```

---

## Implementation Order

| Order | Item | Type | Effort | Impact |
|-------|------|------|--------|--------|
| 1 | Fix watermark poisoning (0.1) | Bug fix | 1 day | 🔴 High — data correctness |
| 2 | Early emission from cache (1.4) | Perf | 1 day | 🟢 High — instant first paint |
| 3 | Incremental git scan (1.2) | Perf | 1 day | 🟢 High — <200ms git |
| 4 | Range-aware build (1.1) | Perf | 1 day | 🟡 Medium — <100ms saved |
| 5 | Fix cost double-calculation (0.2) | Bug fix | 0.5 day | 🟡 Medium — data accuracy |
| 6 | Merge to single-phase ETL (2.1) | Refactor | 2 days | 🟡 Medium — simplifies code |
| 7 | Parallel file parsing (2.3) | Perf | 0.5 day | 🟢 High — faster ETL |
| 8 | DuckDB Appender (2.2) | Perf | 1 day | 🟢 High — faster inserts |
| 9 | Query result caching (2.4) | Perf | 1.5 days | 🟡 Medium — small wins |
| 10 | Selective re-render (3.1) | Perf | 1 day | 🟢 High — snappy UI |

---

## Success Criteria

- [ ] Day view renders in <1 second from app launch
- [ ] Day view renders in <100ms on subsequent refreshes
- [ ] Token totals and cost breakdown match between API and display
- [ ] Week/month/year views show complete data (no gaps from skipped files)
- [ ] ETL processes all files within retention window correctly on every run
- [ ] No duplicate entries in DuckDB
- [ ] Git cache correctly tracks SHA changes and only re-scans changed repos
- [ ] App doesn't block UI thread during data processing
