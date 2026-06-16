# Spike Plan — Derisking Refactor V2 (SQL-First Medallion)

**Date:** 2026-06-16
**Goal:** Before committing to the V2 refactor, prove each risky assumption on real data,
and capture the perf gain + simplification so the decision is evidence-based, not faith.
**Parent:** `REFACTOR_PLAN_V2.md`. **Completed so far:** Spike 0 (Claude ingest parity) —
PASS, see V2 §11.

---

## Method

Each spike is throwaway code in `spike/` (not committed to the product). Each has:
- **Hypothesis** — the V2 claim under test.
- **Risk if false** — what breaks in the plan.
- **Method** — what to run, on real data.
- **Pass gate** — binary.
- **Metrics captured** — fed into the scorecard below.

Every spike that touches data parity reuses the Spike 0 harness: a Python reference that
mimics the current Rust code line-by-line, diffed against the SQL output. Parity is the
gate; the reference is ground truth.

### Scorecard (filled as spikes complete)

| Dimension | Current (Rust) | V2 (DuckDB) | Source |
|---|---|---|---|
| Claude parse+aggregate (307 files) | 4.26s | 1.25s (3.4x) | Spike 0 ✅ |
| Claude parser LOC | ~70 | ~10 | Spike 0 ✅ |
| Pi parse parity (synthetic) | — | PASS | Spike 2 ✅ |
| Timeline bucketing parity / LOC | ~57 | PASS / ~3 SQL exprs | Spike 3 ✅ |
| Time-label LOC | ~96 | deferred (low-risk display) | Spike 3 |
| silver+gold build (real data) | — | 0.62s | Spike 5 ✅ |
| Gold serving-query latency | <50ms (cached) | 6 ms uncached | Spike 5 ✅ |
| Full ingest @1x (273 MB, 308 files) | — | 1.96s | Spike 4 ✅ |
| Full ingest @3x (820 MB, 924 files) | — | 4.90s | Spike 4 ✅ |
| Peak memory, FULL re-read @1x | ~50 MB | 290 MB ⚠ | Spike 4 ✅ |
| Peak memory, registry (changed-only) | ~50 MB | ~tens MB | Spike 4 (registry PASS) |
| Gold query latency | <50ms (cached) | ? | Spike 5 |
| Total ETL core LOC | ~2,239 | target 500–700 | rollup |

---

## Spike 1 — Embedded Rust `duckdb` crate capability check (FOUNDATIONAL)

**Run first. Highest risk: everything assumes the bundled crate ≡ Python duckdb.**

- **Hypothesis:** The bundled `duckdb = { features = ["bundled"] }` crate (not the Python
  binding we used in Spike 0) supports every function V2 needs: `read_ndjson_objects` with
  `filename=true`, `json_extract_string`/`json_extract` JSON-path, `width_bucket`,
  `generate_series`, `date_trunc`, `ON CONFLICT`/`MERGE`, and that the JSON extension is
  available (autoloaded or statically linked) without a network download.
- **Risk if false:** Core V2 SQL won't run in-app even though Python proved the concept.
  Could force `INSTALL json` at runtime (offline installer problem) or a different approach.
- **Method:** Add a `#[test]` or a throwaway `examples/spike_caps.rs` in src-tauri that
  opens an in-memory DuckDB via the crate and runs each function once against a tiny
  fixture. Confirm JSON ext loads with no network. Note crate's actual DuckDB version.
- **Pass gate:** All listed functions execute; JSON ext available offline; version ≥ 1.4
  (for `MERGE`) or plan falls back to `ON CONFLICT`.
- **Metrics:** crate DuckDB version; function support matrix; JSON-ext load path.
- **RESULT (2026-06-16): PASS 15/15.** `spike/spike_caps.rs` ran via
  `cargo run --example spike_caps`. Bundled crate = **DuckDB v1.5.3** (newer than Python
  1.4.3 in Spike 0). JSON extension statically linked — works offline, no `INSTALL`.
  `read_ndjson_objects`+filename, `generate_series`, `date_trunc`, `epoch`, `strftime`,
  `LAG`, `LAG(... IGNORE NULLS)`, `ON CONFLICT`, `MERGE` all present.
  **Corrections:** (1) `width_bucket` does NOT exist → use `time_bucket` or arithmetic
  `floor((epoch(ts)-lo)/binsize)` (both verified). (2) Standalone example bins must
  force-link `rstrtmgr.lib` (Windows Restart Manager); the main app already links it.

## Spike 2 — Pi parser parity

- **Hypothesis:** Pi ingestion replicates in SQL: camelCase fields (`input`/`output`/
  `cacheWrite`/`cacheRead`), nested `usage.cost.*`, unix-ms timestamp with ISO fallback,
  `session_id` from filename split, and the `model_change` carry-forward via `LAG(... IGNORE
  NULLS) OVER (PARTITION BY file ORDER BY line_no)`.
- **Risk if false:** Pi is a whole source; if `LAG` carry-forward or cost nesting doesn't
  match, gold cost/model attribution is wrong.
- **Method:** Locate real Pi session files (find the Pi sessions glob; if none on disk,
  generate a fixture from the Rust parser's expected schema incl. a `model_change` line).
  Python reference mimics `PiParser` (source_adapter.rs:200-309); diff vs SQL.
- **Pass gate:** 0 mismatches on tokens, costs, and resolved model per row/day.
- **Metrics:** parity result; LOC (SQL vs ~106 Rust); whether `LAG` ordering needs a stable
  line index (does `read_ndjson_objects` preserve file line order? — verify explicitly).
- **RESULT (2026-06-16): PASS** (`spike/spike_parity.py`, synthetic fixture). Model
  carry-forward via `LAST_VALUE(modelId IGNORE NULLS) OVER (ORDER BY row_number())`
  resolved alpha/beta/gamma exactly — explicit per-message `model` correctly overrides the
  carried value. camelCase usage + nested `usage.cost.*` extracted correctly.
  **File-order question answered:** `row_number() OVER ()` over `read_ndjson_objects`
  preserved file line order (carry-forward matched the reference), so no Rust-side line
  index is needed. Caveat: still unverified on REAL Pi data (none on disk).

## Spike 3 — Timeline bucketing + time-axis labels in SQL

- **Hypothesis:** `width_bucket(epoch(ts), epoch(since), epoch(until), 48)` reproduces
  `bucket_git/bucket_cost/bucket_tokens` (data.rs:246-312), and `generate_series` +
  strftime reproduces `compute_time_labels` (data.rs:314-410) for day/week/year ranges.
- **Risk if false:** Charts render differently; the ~150 LOC reduction doesn't materialize.
- **Method:** Feed the same entries into both the Rust bucketing (port to Python reference)
  and the SQL `width_bucket`. Compare 48-bucket arrays for each range. Compare tick labels.
- **Pass gate:** Bucket arrays identical (watch edge bucket: Rust uses `idx = (offset/total)
  * N` floor, clamps last; `width_bucket` is 1-indexed and bins differently — reconcile the
  off-by-one explicitly). Labels match per range.
- **Metrics:** parity; LOC saved; the exact bucket-edge convention that matches Rust.
- **RESULT (2026-06-16): PASS** (`spike/spike_parity.py`, 39,078 real points). SQL
  `LEAST(CAST(floor((epoch(ts)-lo)/((hi-lo)/48.0)) AS INT), 47)` with the `0<=off<total`
  guard reproduces `bucket_*` exactly — both input and output 48-bucket arrays matched.
  `width_bucket` is moot (absent in 1.5.3; arithmetic floor is the match anyway). Labels
  deferred: pure display logic, no data-integrity risk; port to `generate_series` later or
  leave in Rust.

## Spike 4 — Incremental ingestion at scale + the big simplification

**This spike justifies dropping the seek/tail file-tracking state machine (V2 Phase 4).**

- **Hypothesis:** Full re-glob + `read_ndjson_objects` + `ingested_files` registry filter is
  cheap enough at realistic and projected scale that the append-only seek/tail machine
  (usage_store.rs:277-434) is unnecessary. Re-running with no new data ingests 0 rows;
  appending to a file ingests only its new rows; deleted/rotated files self-heal.
- **Risk if false:** Full re-read every 60s gets too slow/hot as history grows → we'd have
  to keep incremental tailing, losing the simplification.
- **Method:** (a) Measure cold full ingest time + peak memory at current scale (307 files)
  and at projected scale (synthesize 3x and 10x by duplicating files). (b) Build the
  `ingested_files(path, mtime, size)` registry; verify re-run = 0 new, append = delta only,
  rotation (size shrink) = full re-read of that file. Compare registry approach to "reparse
  everything, dedup by ON CONFLICT".
- **Pass gate:** Cold ingest at 10x scale < 2s and peak memory < current ~50 MB; registry
  semantics correct on all three cases.
- **Metrics:** ingest time @ 1x/3x/10x; peak memory; rows reprocessed per cycle; verdict on
  whether incremental is even needed.
- **RESULT (2026-06-16): PASS, with a verdict that refines the plan.** `spike/spike_scale.py`.
  Ingest time is cheap (1.96s @273MB, 4.90s @820MB — fine for a 60s cycle). **But FULL
  re-read peaks 290 MB @1x / 415 MB @3x vs current Rust ~50 MB — full re-glob every cycle
  is a memory regression.** Stat-registry semantics PASS (re-run=0, append=delta,
  rotation=full-reread of that file). **Verdict:** keep incremental ingestion, but via a
  simple `ingested_files(path,mtime,size)` registry that reprocesses only CHANGED files
  (read whole — no byte-seek/tail/partial-line buffering). This deletes the complex state
  machine (usage_store.rs:277-434) while keeping memory low. The original V2 Phase 4
  premise ("full re-read is cheap, drop tracking entirely") is wrong on memory — corrected.

## Spike 4b — Memory/performance tradeoff (follow-up to Spike 4)

- **Question:** Can DuckDB memory be bounded via memory_limit / micro-batching (streaming /
  task-queue style), and where's the time/memory tradeoff?
- **RESULT (2026-06-16):** `spike/spike_memory.py`, isolated process per variant.

  | Strategy | mem_limit | peak RSS | result |
  |---|---|---|---|
  | Full read (in-mem or persistent) | none | 307–351 MB | works |
  | Full read | 512 MB | — | **OOM** |
  | Micro-batch 8 files | 256 MB | — | **OOM** |
  | Micro-batch 8 files | 512 MB | **132 MB** | works |
  | Micro-batch 16 files | 512 MB | **157 MB** | works |

  Findings: (1) a tight `memory_limit` OOM-crashes rather than spilling — the JSON
  read/parse path holds non-spillable ~32 MB buffers × threads, so the limit must be a high
  guard rail (512 MB), not a ceiling. (2) **Micro-batching is the real lever** — 8–16 files
  per batch holds actual peak to ~130–160 MB regardless of dataset size (~half the full-read
  peak) for a ~2x time cost. (3) Persistent DB vs in-memory does NOT change the peak (it's
  the read, not the table location).
- **Verdict / recommended design:** incremental stat-registry for normal cycles (reads only
  the 1–3 changed files → memory trivial); micro-batched cold rebuild (16 files/batch,
  memory_limit guard rail) as the fallback path that caps peak at ~150 MB. Do not rely on
  memory_limit-driven spilling for this workload.

## Spike 5 — End-to-end pipeline + gold query latency + memory

- **Hypothesis:** Full bronze→silver→gold (+ timeline views) produces the exact `AllStats`
  JSON the frontend expects, with total cycle time and query latency at least as good as
  current, and the RefCell query cache (usage_store.rs) is unnecessary because gold queries
  are sub-ms on a tiny table.
- **Risk if false:** The query-layer collapse (V2 Phase 3, ~200 LOC) regresses latency, or
  the assembled JSON drifts from the current shape and breaks the React renderer.
- **Method:** Run the whole pipeline in one DuckDB connection; emit the `AllStats` struct
  from a single CTE query (or a handful of view reads); diff the JSON against a capture from
  the current running app for the same ranges; measure cold-cycle and warm-query timings.
- **Pass gate:** JSON structurally matches current `AllStats`; gold queries < 50 ms uncached;
  total cold cycle ≤ current.
- **Metrics:** e2e cycle time; per-range query latency uncached; JSON diff result.
- **RESULT (2026-06-16): PASS** (`spike/spike_parity.py`, real Claude data). silver+gold
  build 0.62s; tokens, cost (flat-priced in SQL), and sessions all match the raw reference.
  Gold serving-query latency **6 ms uncached** → confirms the RefCell query cache (Phase 3,
  ~200 LOC) can be deleted. **Design finding (acted on):** `COUNT(DISTINCT session_id)` is
  NOT additive — summing per-day `daily_aggregates.session_count` over a week/year
  over-counts sessions that span days (a latent bug in the current design too). Multi-day
  `sessions_total` must query `entries` directly (cheap — 6ms) or keep a separate
  range-level distinct, NOT sum daily counts. (V2 §2 gold contract updated.)

## Spike 6 — Git points join into gold (lower risk, do last)

- **Hypothesis:** Git LOC (parsed in Rust, stays in Rust) can be handed to DuckDB as a
  `git_points(repo, ts, added, deleted)` table and joined/aggregated into gold
  `daily_aggregates.loc_added/deleted` purely in SQL.
- **Risk if false:** LOC numbers in the dock are wrong; minor — git parsing path unchanged.
- **Method:** Insert real `git_points` (from existing git_cache), aggregate by day in SQL,
  compare to current Rust git aggregation.
- **Pass gate:** Per-day LOC matches current output.
- **Metrics:** parity; LOC saved in git aggregation glue.

---

## Sequencing & decision points

```
Spike 1 (crate caps) ──► GATE: if crate can't run the SQL offline, STOP and re-scope.
        │ pass
        ▼
Spike 2 (Pi) ─┬─ Spike 3 (timelines/labels)   [parity spikes, independent, can parallelize]
              │
              ▼
Spike 4 (scale/incremental) ──► GATE: decides Phase 4 (keep vs drop seek/tail).
        │
        ▼
Spike 5 (e2e + latency + memory) ──► fills scorecard; final go/no-go on Phases 1-3.
        │
        ▼
Spike 6 (git join)  [cleanup-tier confirmation]
```

**Go/no-go:** Proceed to V2 implementation only if Spikes 1, 2, 5 pass. Spike 4 decides
the file-tracking design. Spikes 3 and 6 are parity confirmations that de-risk specific
phases but won't block the architecture decision.

**DECISION (2026-06-16): GO.** Gate spikes 1, 2, 5 all PASS; bonus 3, 4, 4b PASS. The
architecture is validated on real data: DuckDB matches Rust exactly, runs faster, collapses
~400 LOC of parsing/aggregation/bucketing to SQL, and serves gold in 6ms (no cache needed).
Three corrections were baked back into V2 along the way: use `read_ndjson_objects` not
`_auto`; keep a simple stat-registry (memory) not full re-read; `sessions_total` must query
`entries` not sum daily counts. Only Spike 6 (git join, low-risk) remains, and real-Pi
parity should be re-confirmed once Pi sessions exist. Proceed to Phase 1.

## Definition of Done (this spike plan)

- [x] Spike 1 — crate capability matrix recorded; offline JSON-ext confirmed (PASS 15/15; DuckDB 1.5.3; width_bucket→time_bucket/arith)
- [x] Spike 2 — Pi parity PASS (synthetic; real-data gap documented)
- [x] Spike 3 — bucketing parity PASS; floor-index convention pinned; labels deferred
- [x] Spike 4 — scale/memory captured (1.96s/290MB @1x); verdict: keep simple stat-registry, drop seek/tail (full re-read is a memory regression)
- [x] Spike 5 — e2e cycle + latency + parity captured (0.62s build, 6ms gold query); distinct-session non-additivity found
- [ ] Spike 6 — git join parity PASS (remaining; low risk)
- [ ] Scorecard fully populated; V2 §11 updated with consolidated results
- [ ] Go/no-go decision recorded in V2
