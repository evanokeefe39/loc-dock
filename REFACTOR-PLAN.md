# Loc-Dock Refactor Plan

**Date:** 2026-06-15
**Author:** Staff Engineer Review
**Status:** Draft — requires board sign-off before implementation

---

## 1. Diagnostic Summary

### Current Performance (measured)

| Stage | Time | Notes |
|-------|------|-------|
| Git scan (169 repos) | 6–12s | 103 SHA checks, 47 repos scanned |
| ETL (717 files, 208 MB) | ~2s | Re-reads entire file content every cycle |
| DuckDB queries (×6 per range) | <50ms combined | Not the bottleneck |
| **Total cycle** | **7–19s** | Blocks first paint entirely |

### Memory Impact (current)

- **~208 MB** of JSONL files `read_to_string` every cycle
- Parsed into `Vec<NormalizedEntry>` (~7,680 entries → ~5 MB)
- JSON parsing allocates per-line `serde_json::Value` (temporary, then freed)
- Most rows are duplicates and discarded at `INSERT OR IGNORE` time
- **208 MB disk → 30–50 MB heap peak → nearly all wasted**

### Root Causes (scoped to this refactor)

| # | Problem | Why It Matters |
|---|---------|----------------|
| P1 | Git scan blocks first stats emission | User sees spinner for 6–12s on every app launch |
| P2 | ETL re-reads ALL 717 files every cycle | 208 MB of disk I/O + parsing for ~0 new rows |
| P3 | Aggregates recalculated from raw data every cycle | O(n) over full entry set when O(1) incremental works |
| P4 | Two threads run independent git scans | Data loop + summary loop = 12–24s of competing git log |
| P5 | Event-driven frontend couples UI to backend lifecycle | Over-engineering: frontend just needs query results on demand |
| P6 | No retention/pruning | DB grows unbounded |

---

## 2. Proposed Architecture

### Principles

1. **Append-only data → append-only analytics.** Once a day is finalized, its aggregate never changes.
2. **Files are logs.** Don't re-read entire files — use `stat` + mtime to detect changes. Don't re-parse old lines.
3. **Frontend is a View.** It queries. It doesn't get pushed to. MVC: the View polls or calls commands.
4. **Single pipeline, shared state.** One background ETL. One shared Aggregates table. Both data loop and summary loop read from the same pre-computed data.

### Component Diagram

```
┌─────────────────────────────────────────────────┐
│                  App Startup                      │
│   UsageStore::new() → opens DB, returns cached   │
│   aggregates (if any)                            │
└──────────────────┬──────────────────────────────┘
                   │
       ┌───────────┴───────────┐
       │   Frontend (View)      │
       │   ┌─────────────────┐  │
       │   │ useStats()       │  │
       │   │ → get_stats()    │  │ ← Tauri command, returns JSON
       │   │ → poll 10s       │  │ ← lightweight
       │   └─────────────────┘  │
       └───────────┬───────────┘
                   │ get_stats("day")
                   ▼
┌──────────────────────────────────────────────────┐
│              Backend Queries                      │
│   get_stats(range) → reads from DAILY_AGGREGATES  │
│   (pre-computed, O(1) per range)                  │
│   Returns: {tokens, cost, sessions, loc}          │
└──────────────────┬───────────────────────────────┘
                   │
       ┌───────────┴───────────┐
       │ ┌───────────────────┐  │
       │ │ GitScanner         │  │ ← runs ~60s, parallel rayon
       │ │ ✓ SHA check (103)  │  │ ← fast (<2s)
       │ │ ✓ git log changed  │  │ ← only repos with new commits
       │ │ ✓ updates git_cache│  │
       │ └───────────────────┘  │
       │                        │
       │ ┌───────────────────┐  │
       │ │ ETL Pipeline       │  │ ← runs ~60s
       │ │ ✓ stat JSONL files │  │ ← no read, just metadata
       │ │ ✓ only re-read     │  │ ← files with changed mtime
       │ │ ✓ parse new lines  │  │ ← append-only, tail only
       │ │ ✓ update aggregates│  │ ← incremental upsert
       │ │ ✓ prune old data   │  │ ← retention 14d
       │ └───────────────────┘  │
       │                        │
       │ ┌───────────────────┐  │
       │ │ Aggregates Table   │  │ ← materialized by ETL
       │ │ daily: date,source │  │ ← 1 row per source per day
       │ │ tokens, cost, sess │  │
       │ │ loc added/deleted  │  │ ← from git_cache
       │ └───────────────────┘  │
       └───────────────────────┘
```

---

## 3. Key Design Decisions

### 3a. Materialized Aggregates Instead of Live Queries

**Current:** Every `build_all_stats` call does 6 DuckDB queries scanning all rows with `WHERE ts >= ?`.

**Proposed:** Pre-compute and store daily aggregates during ETL:

```sql
CREATE TABLE daily_aggregates (
    date           DATE NOT NULL,
    source         TEXT NOT NULL,
    input_tokens   BIGINT NOT NULL DEFAULT 0,
    output_tokens  BIGINT NOT NULL DEFAULT 0,
    cache_write_tokens BIGINT NOT NULL DEFAULT 0,
    cache_read_tokens  BIGINT NOT NULL DEFAULT 0,
    input_cost     DOUBLE NOT NULL DEFAULT 0.0,
    output_cost    DOUBLE NOT NULL DEFAULT 0.0,
    cache_write_cost DOUBLE NOT NULL DEFAULT 0.0,
    cache_read_cost  DOUBLE NOT NULL DEFAULT 0.0,
    total_cost     DOUBLE NOT NULL DEFAULT 0.0,
    session_count  BIGINT NOT NULL DEFAULT 0,
    loc_added      BIGINT NOT NULL DEFAULT 0,
    loc_deleted    BIGINT NOT NULL DEFAULT 0,
    UNIQUE(date, source)
);
```

**Incremental ETL update:**

```
On new entries for date D, source S:
  INSERT INTO daily_aggregates (date, source, totals...)
  VALUES (D, S, new_totals...)
  ON CONFLICT(date, source) DO UPDATE SET
    input_tokens = daily_aggregates.input_tokens + EXCLUDED.input_tokens,
    ...
```

**Frontend query:**

```
get_stats("day") → SELECT * FROM daily_aggregates WHERE date = today
get_stats("week") → SELECT source, SUM(...) FROM daily_aggregates WHERE date >= week_start
get_stats("year") → SELECT source, SUM(...) FROM daily_aggregates WHERE date >= year_start
```

**Performance:** O(1) lookups vs O(n) full table scans. Returns 1–365 rows max.

### 3b. Append-Only File Tracking (Log Tailing)

**Current:** `read_to_string(path)` reads ENTIRE file, parses ALL lines, INSERT OR IGNORE.

**Proposed:** Track file state across cycles:

```rust
struct FileTracker {
    path: PathBuf,
    mtime: f64,
    size: u64,        // bytes at last scan
    last_entry_ts: Option<DateTime<Utc>>,  // for dedup
}
```

**Algorithm:**
1. Glob JSONL files in retention window (cheap — just metadata)
2. `stat` each file — compare mtime + size against tracker
3. If mtime unchanged → skip entirely
4. If mtime changed → position the read at `size` from tracker (tail only)
5. Read new bytes into a buffer, split lines, parse JSON
6. Update tracker: new size = new total bytes, new mtime
7. Insert parsed entries → update daily_aggregates

For **cold start** (no tracker): read entire file, parse all, insert all.

**Impact:** 
- Most cycles: 0–3 files changed (active sessions being written to)
- 208 MB scan → ~0 KB for unchanged files, ~50–200 KB for active files
- No re-parsing of old data, no duplicate INSERT attempt cost

### 3c. Frontend Polling Instead of Event Push

**Current:**
```ts
useEffect(() => {
    invoke("get_stats").then(setStats);                    // one-shot on mount
    listen<AllStats>("stats-update", (e) => setStats(...)); // event-driven
}, []);
```

**Proposed:**
```ts
useEffect(() => {
    const fetch = () => invoke("get_stats").then(setStats).catch(console.error);
    fetch();
    const id = setInterval(fetch, 10_000);  // poll every 10s
    return () => clearInterval(id);
}, []);
```

**Why:**
- Frontend controls its own refresh cadence
- No coupling to backend lifecycle
- `get_stats` returns pre-computed aggregates (sub-millisecond)
- Works even if backend event loop is busy/gc-stalled
- Still get <1s first paint (mount → invoke → response)

**Trade-off:** ~10ms network overhead every 10s for a query that returns <1KB JSON. Acceptable.

### 3d. Single Git Scanner, Shared Cache

**Current:** Data loop creates `GitCache` inside thread closure. Summary loop creates its own. Two independent scans of the same 169 repos.

**Proposed:** Share a single `Arc<Mutex<GitCache>>` scoped to the app state.

```
App setup:
  let git_cache = Arc::new(Mutex::new(GitCache::new(&config.settings.git_cache_dir)));
  app.manage(git_cache.clone());

Data loop:
  git_cache.lock().unwrap().latest_ts("repo-a")  // read

  git_handle = thread::spawn(move || {
      git::get_git_loc_timeline(..., &*git_cache.lock().unwrap());
  });

Summary loop:
  git_cache.lock().unwrap().query_since(&since_iso)  // read-only
```

### 3e. Retention & Pruning

**Current:** No DEletes. DB grows forever.

**Proposed:**

At the end of each ETL cycle:
```sql
DELETE FROM entries WHERE ts < date_trunc('day', NOW() - INTERVAL '14 days');
DELETE FROM daily_aggregates WHERE date < date_trunc('day', NOW() - INTERVAL '14 days');
-- For git_cache
DELETE FROM git_points WHERE ts < date_trunc('day', NOW() - INTERVAL '14 days');
```

Optional: `VACUUM` once per week to reclaim space (DuckDB doesn't automatically compact).

---

## 4. Migration Path

### Phase 1: Quick Wins (Day 1 — <1s first paint)

1. **Pre-fill `SharedStats` from aggregates before entering the data loop**
   - Add `get_aggregates(range)` query against `daily_aggregates` (or compute from entries if table doesn't exist yet)
   - First emit on app start now shows real data in <50ms

2. **Add `daily_aggregates` table + incremental update during ETL**
   - Schema migration: create table in `UsageStore::new()` (existing version 4 → 5)
   - After `appender_insert`, upsert into `daily_aggregates`
   - `get_stats` command reads from aggregates instead of raw entries

3. **Swap frontend to polling**
   - Replace `listen("stats-update")` with `setInterval(fetch, 10_000)`
   - Keep `useStats` hook API unchanged (internal refactor)

**Expected result:** First paint <50ms. Cycle no longer blocks UI.

### Phase 2: Efficiency (Day 2–3)

4. **Append-only file tracking**
   - Add `file_tracker` table to usage cache DB: `(file_path TEXT, mtime DOUBLE, size BIGINT, PRIMARY KEY(file_path))`
   - Modify `process_source` to stat and compare before reading
   - Only read new bytes for changed files

5. **Unify git cache access**
   - Convert `spawn_data_loop` to use shared `Arc<Mutex<GitCache>>`
   - Convert summary loop to read from same cache
   - Remove duplicate git scan from summary loop

6. **Retention pruning**
   - Add `DELETE FROM entries` + `DELETE FROM daily_aggregates` + `DELETE FROM git_points` to ETL

**Expected result:** Cycle goes from 7–19s to ~2s (only scan changed repos + active files).

### Phase 3: Cleanup

7. **Remove dead code**
   - `SourceManager::input_price` etc. fields
   - `UsageStore::is_initialized()` method (unused)
   - `TaskQueue::rename()` method (unused)

8. **Time ranges as enum**
   - Replace `&[&str]` with `&[TimeRange]`
   - Replaces 4 match chains with data-driven dispatch

9. **Fix BUG-002: cost_total from DB**
   - `cost_total = cost_breakdown.input + cost_breakdown.output + cache_write + cache_read`
   - Remove `pricing::estimate_cost` (or keep for fallback)

---

## 5. File-by-File Changes

| File | Phase | Changes |
|------|-------|---------|
| `src/usage_store.rs` | 1, 2, 3 | Add `daily_aggregates`, append-only tracker, retention DELETE, remove `is_initialized` |
| `src/data.rs` | 1, 2 | Pre-fill `SharedStats`, unify git cache, remove two-emit pattern |
| `src/commands.rs` | 1 | `get_stats` reads from aggregates |
| `src/git_cache.rs` | 2 | Expose `Arc<Mutex<>>` compatible API, add `query_count` for observability |
| `src/summary.rs` | 2 | Read from shared git cache, not own scan |
| `src/lib.rs` | 2 | Create and manage `SharedGitCache` |
| `src/source_adapter.rs` | 3 | Remove dead pricing fields from `SourceManager` |
| `src/pricing.rs` | 3 | Deprecate `estimate_cost` |
| `src/types.rs` | 3 | Add `TimeRange` enum |
| `src/hooks/useStats.ts` | 1 | Replace listen with polling |
| `src/App.tsx` | 1 | Remove listen import if unused |

---

## 6. Performance Projections

| Metric | Current | Phase 1 | Phase 2 | Phase 3 |
|--------|---------|---------|---------|---------|
| First paint | 8–14s | **<50ms** | <50ms | <50ms |
| Cycle time | 7–19s | 7–19s | **~2s** | ~2s |
| Disk I/O per cycle | 208 MB | 208 MB | **<1 MB** | <1 MB |
| Peak heap per cycle | ~50 MB | ~50 MB | **<2 MB** | <2 MB |
| Git scans per cycle | 2 (data + summary) | 2 | **1** | 1 |
| Frontend refresh latency | Event-driven | 10s poll | 10s poll | 10s poll |
| DB size growth | Infinite | **Capped 14d** | Capped 14d | Capped 14d |

---

## 7. Anti-patterns Fixed

| Anti-pattern | Current | Fixed |
|-------------|---------|-------|
| **God function** | `spawn_data_loop` does git, ETL, stats, emit, logging | Split into `GitScanner`, `EtlPipeline`, `UsageAnalytics` |
| **Premature event push** | Backend pushes stats to frontend on every cycle | Frontend polls; decoupled |
| **Full file re-read** | `read_to_string` every JSONL every cycle | `stat` + increment tail for append-only logs |
| **Duplicate computation** | Data loop + summary loop both scan git | Single shared `GitCache` |
| **No data retention** | DB grows forever | DELETE at 14d retention boundary |
| **Unbounded memory** | 208 MB of JSONL loaded per cycle | ~50 KB per cycle (active files only) |
| **O(n) queries for aggregates** | `SUM(input_tokens) FROM entries WHERE ts >= ?` | `SELECT input_tokens FROM daily_aggregates WHERE date = today` — O(1) |
| **Stringly-typed ranges** | `&[&str]` with 4 match arms | `TimeRange` enum with data-driven dispatch |

---

## 8. Edge Cases & Risk Mitigation

### Edge Cases

| Edge Case | Risk | Mitigation |
|-----------|------|------------|
| **Cold start (empty DB)** | No aggregates to pre-fill → first paint still blank | Fallback to current behavior (scan + emit). Acceptable — DB will have data after first ETL cycle. |
| **File truncated mid-line** | Log file might end mid-JSON line | Buffer partial line, prepend next read. Position tracking must be byte-accurate. |
| **File renamed/moved between cycles** | Tracker references old path → file re-read | Key tracker on `inode` (Unix) or `file_id` (Windows) as well as path. On miss, re-read full file. |
| **Clock skew (file mtime > now)** | File appears "future" → wrong cutoff | Clamp mtime to `min(mtime, now)`. Still discovered within window. |
| **Rolling file rotation** | Old file deleted, new file with same name created | `size < last_scanned_size` → file was rotated → re-read from beginning. |
| **Phase 1 without Phase 2** | Still reading 208 MB but now also updating aggregates | Acceptable — aggregates give query speedup, file tracking gives I/O speedup. Independent benefits. |

### Rollback Plan

Each phase is independently revertible:
- **Phase 1:** Remove aggregate table, restore live queries. Remove poll, add listen back.
- **Phase 2:** Restore full file reads. Use separate `GitCache` per thread.
- **Phase 3:** Restore old type definitions.

### Verification

After each phase:
1. Run `cargo test` — all existing tests must pass
2. `npm run tauri dev` — visual inspection: first paint shows data
3. Check `perf.log` — cycle times decrease as expected
4. Check `usage_cache.db` — row count capped at ~14 days of entries

---

## 9. Open Questions for Sign-Off

1. **Poll interval: 5s, 10s, or 30s?** Polling is cheap (<1ms query, <1KB response). 10s is a reasonable default.
2. **Retention window: 14 days or 30 days?** 14d matches DuckDB storage cost of ~3 MB/wk. 30d adds ~6-8 MB.
3. **Do we keep `pricing::estimate_cost` as a fallback?** Some entries (from parsers that don't provide cost) still need flat-rate estimation. Yes, keep for `fill_costs` but remove from `build_all_stats`.
4. **Summary loop: should it also poll aggregates instead of running its own cycle?** Yes — summary loop can read from the same `daily_aggregates` table after Phase 2.
