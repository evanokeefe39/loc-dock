# Next Steps — Sub-1s Day View & Correct Data

**Goal:** Sub-1 second day display with correct token/cost data for all ranges.

---

## Step 1: Fix Watermark Poisoning 🚨 (1-2 days)

**Files:** `src-tauri/src/usage_store.rs`, `src-tauri/src/data.rs`

**What:** Replace two-phase ETL (`run_etl_urgent` + `run_etl_background`) with single-phase ETL.

**Changes:**
1. Merge both phases into one `run_etl()` method with a 7-day retention cutoff
2. Remove `run_etl_background()` entirely
3. Update `data.rs` to call `store.run_etl()` once per cycle instead of twice
4. Keep the `emit_stats!()` pattern — call it once after the single ETL pass
5. Rely on `INSERT OR IGNORE` (UNIQUE constraint) for dedup — no watermark filters needed
6. Add logging: `info!("ETL: processed {} entries (claude:{} pi:{})", total, claude, pi)`

**Verification:**
- Run `spike-watermark-bug` to confirm all 9 test files are ingested
- Launch app, verify day/week/month/year all show token/cost data
- Check that cycling through time ranges shows consistent numbers

**Risk:** Low — single-phase is simpler and the fix is verified by spike.

---

## Step 2: Incremental Git Scan 🏎️ (1 day)

**Files:** `src-tauri/src/git.rs`, `src-tauri/src/git_cache.rs`

**What:** Use cached `latest_ts` per-repo as `git log --since` parameter instead of scanning from year start.

**Changes in `git.rs`:**
1. In `get_git_loc_timeline()`, when `sha_matches` is true: **skip the repo entirely** (no git log call at all)
2. When SHA changed: use `cache.latest_ts(repo)` as the since parameter, falling back to `since_iso` if no cached ts exists
3. Add rayon parallelism: wrap the repo iteration with `par_bridge()` so git commands run in parallel

**Verification:**
- Run `spike-git-scan` before and after to measure wall time difference
- First launch should still do a full scan (cold cache)
- Subsequent refreshes should take <500ms for git

**Risk:** Low — git cache already has all the data structures needed.

---

## Step 3: Early Emission from Cache 🖼️ (0.5 days)

**Files:** `src-tauri/src/data.rs`

**What:** On each cycle, emit cached stats immediately before running git/ETL, then update asynchronously.

**Changes:**
1. At the top of the data loop, read `stats` (already in SharedStats), clone and emit via `app.emit("stats-update", &stats)`
2. Then run git scan + ETL as normal
3. After ETL, emit updated stats
4. This means the user never sees a blank screen — they get last cycle's data instantly

**Verification:**
- Launch app, verify stats appear immediately (from previous cycle's cache)
- After ETL completes, stats update to current data
- No blank/flashing UI states

**Risk:** Low — pure addition, no refactoring.

---

## Step 4: DuckDB Appender for Bulk Inserts 📦 (0.5 days)

**Files:** `src-tauri/src/usage_store.rs`

**What:** Replace `appender_insert()` with the DuckDB Appender API.

**Changes:**
1. In `process_source()`, change the chunk insert loop to use `con.appender("entries")` with `append_row(params![...])` + `flush()`
2. Remove the old `appender_insert()` function
3. Keep the `BEGIN TRANSACTION` / `COMMIT` wrapping for atomicity (though Appender is atomic per flush)

**Verification:**
- Run the app, verify data is ingested correctly
- Check that INSERT OR IGNORE still prevents duplicates (it does — UNIQUE constraint is in the schema)

**Risk:** Low — Appender is a well-tested DuckDB API. The spike showed correct behavior.

---

## Step 5: Range-Aware Stats Building 🎯 (1 day)

**Files:** `src-tauri/src/data.rs`

**What:** Only compute the day range on the first stats emit. Compute week/month/year only on the second emit (or on demand).

**Changes:**
1. Change `build_all_stats()` to accept a `ranges: &[&str]` parameter (e.g., `&["day"]` or `&["day", "week", "month", "year"]`)
2. In the data loop, call with `&["day"]` for the urgent emit, `&["day", "week", "month", "year"]` for the full emit
3. Or better: emit day-only first, then compute the other ranges and emit a full update

**Verification:**
- Day view renders in <1s (no longer waiting for week/month/year queries)
- Switching to week/month/year views shows correct data (computed in background)

**Risk:** Medium — requires restructuring the `build_all_stats` function signature and `AllStats` struct. May need a `PartialAllStats` type or optional fields.

---

## Step 6: Query Result Caching 🗃️ (1-2 days)

**Files:** `src-tauri/src/usage_store.rs`

**What:** Cache query results so repeated cycles don't re-query unchanged data.

**Changes:**
1. Track a `generation` counter in `UsageStore` that increments on each successful ETL insert
2. Store cached query results keyed by `(query_type, range)` alongside the `generation` at time of caching
3. In query methods, check if `generation` matches — if so, return cached result; if not, re-query and cache
4. Use a simple `HashMap` or struct with fields for each cached result

**Verification:**
- After first ETL cycle, subsequent cycles should show 0µs query times if no new data
- After new files appear, queries should re-fire and cache updated results

**Risk:** Medium — adds statefulness. Must handle cache invalidation correctly (generation counter is simple but effective).

---

## Optional / Nice-to-Have

### Step 7: Parallel File Parsing with Rayon (0.5 days)
- `process_source` already uses chunks — switch to `par_bridge()` for parallel JSON parsing
- The crate already has `rayon` as a dependency
- Expected: 2-4x faster ETL on multi-core machines

### Step 8: Fix `estimate_cost` Double-Calculation (0.5 days)
- Make `cost_total` come from DB `SUM(total_cost)` instead of `pricing::estimate_cost()`
- Ensures cost_total and cost_breakdown always agree

---

## Implementation Order Summary

| # | Step | Days | Impact | Risk |
|---|---|---|---|---|
| 1 | Fix watermark poisoning | 1-2 | 🔴 Correctness | Low |
| 2 | Incremental git scan | 1 | 🟢 Day view: 47s→0.5s | Low |
| 3 | Early emission from cache | 0.5 | 🟢 First paint: instant | Low |
| 4 | DuckDB Appender | 0.5 | 🟢 ETL: 40-326x faster inserts | Low |
| 5 | Range-aware stats | 1 | 🟡 Day view: 50% less query work | Med |
| 6 | Query result caching | 1-2 | 🟡 Repeat cycles: 0µs queries | Med |

**Steps 1-4** alone should get you to sub-1s day view + correct data. Steps 5-6 are refinements.
