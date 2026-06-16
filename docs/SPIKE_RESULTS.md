# Spike Results — V2 Derisking

**Date:** 2026-06-16
**Outcome:** GO. The SQL-first medallion architecture is validated on real data.
**Scope:** Spikes 0–5 + 4b run. Spike 6 (git join) deferred (low risk).
**Reproduce:** throwaway scripts in `spike/` (gitignored): `spike_duckdb.py`,
`spike_caps.rs`, `spike_scale.py`, `spike_memory.py`, `spike_parity.py`. Engine used:
Python duckdb 1.4.3 for data spikes; the bundled Rust crate (DuckDB 1.5.3) for the
capability spike. Real data: ~308 Claude session files (~273 MB) under `~/.claude/projects`,
subagent logs excluded.

---

## Scorecard

| Dimension | Current (Rust) | V2 (DuckDB) | Spike |
|---|---|---|---|
| Claude parse+aggregate (308 files) | 4.26s | 1.25s (3.4x) | 0 |
| Claude parser LOC | ~70 | ~10 SQL | 0 |
| Pi parse parity (synthetic) | — | PASS | 2 |
| Timeline bucketing parity / LOC | ~57 | PASS / ~3 SQL exprs | 3 |
| Time-label LOC | ~96 | deferred (display, low-risk) | 3 |
| silver+gold build (real data) | — | 0.62s | 5 |
| Gold serving-query latency | <50ms (cached) | 6 ms uncached | 5 |
| Cold ingest, full re-read @1x | — | 1.96s / peak 290 MB | 4 |
| Cold ingest, full re-read @3x | — | 4.90s / peak 415 MB | 4 |
| Cold ingest, micro-batched (16/batch) | — | peak ~150 MB | 4b |
| Steady-state ETL memory (registry) | ~50 MB | ~tens MB | 4 |
| Total ETL core LOC | ~2,239 | target 500–700 | rollup |

---

## Per-spike results

### Spike 0 — Claude ingest parity + speed — PASS
Real 308 files, ~39k assistant rows. DuckDB `read_ndjson_objects` + `json_extract_string`
matched a Python reference mimicking `source_adapter.rs` ClaudeParser: **0 mismatches across
26 days × 4 token columns**, and **1.25s vs 4.26s** for the line-by-line parse (3.4x).
- Correction: `read_ndjson_auto` **OOM-crashes** on the heterogeneous logs (schema
  inference) — must use `read_ndjson_objects` (raw JSON column, no inference).
- Correction: DuckDB's `filename` column is `\`-separated on Windows — path filters must be
  separator-agnostic (`ILIKE '%subagents%'`, not `'%/subagents/%'`).

### Spike 1 — Bundled Rust crate capability — PASS (15/15)
Ran against the actual shipped engine, not the Python binding.
- Bundled crate = **DuckDB v1.5.3**; JSON extension statically linked, works **offline**
  (no `INSTALL`/network).
- Present: `read_ndjson_objects`+filename, `json_extract[_string]`, `generate_series`,
  `date_trunc`, `epoch`, `strftime`, `LAG`, `LAG(... IGNORE NULLS)`, `ON CONFLICT`, `MERGE`.
- Correction: **`width_bucket` does not exist** in 1.5.3 → use `time_bucket` or arithmetic
  `floor((epoch(ts)-lo)/binsize)` (both verified).
- Build note: standalone example bins must force-link `rstrtmgr.lib` (the app already does).

### Spike 2 — Pi parser parity (synthetic) — PASS
Synthetic fixture (model_change + assistant/user messages, camelCase usage, nested
`usage.cost.*`, unix-ms timestamps). `model_change` carry-forward via
`LAST_VALUE(modelId IGNORE NULLS) OVER (ORDER BY row_number())` resolved alpha/beta/gamma
exactly; explicit per-message `model` correctly overrides the carried value.
- File-order question **answered**: `row_number() OVER ()` over `read_ndjson_objects`
  preserved file line order, so no Rust-side line index is needed.
- Gap: unverified on REAL Pi data (none on disk) — re-confirm when Pi sessions exist.

### Spike 3 — Timeline bucketing parity — PASS
39,078 real points, 48 buckets. SQL
`LEAST(CAST(floor((epoch(ts)-lo)/((hi-lo)/48.0)) AS INT), 47)` with the `0<=off<total` guard
reproduced `bucket_*` (data.rs:246-306) exactly — both input and output bucket arrays
matched. Time-axis labels deferred: pure display logic, no data-integrity risk.

### Spike 4 — Scale + memory + incremental — PASS (with verdict)
Ingest time is cheap (1.96s @273 MB, 4.90s @820 MB — fine for a 60s cycle). **But a full
re-glob re-read peaks ~290 MB @1x / 415 MB @3x vs current Rust ~50 MB.** Stat-registry
`(path, mtime, size)` semantics verified: re-run = 0 files, append = delta only, rotation
(size shrink) = full re-read of that file.
- Verdict: keep incremental ingestion via a **simple stat-registry** (reprocess only changed
  files, read whole — no byte seek/tail/partial-line buffering). This deletes the complex
  state machine (usage_store.rs:277-434) while keeping steady-state memory low.

### Spike 4b — Memory/performance tradeoff — characterized
| Strategy | mem_limit | peak RSS | result |
|---|---|---|---|
| Full read (in-mem or persistent) | none | 307–351 MB | works |
| Full read | 512 MB | — | OOM |
| Micro-batch 8 files | 256 MB | — | OOM |
| Micro-batch 8 files | 512 MB | 132 MB | works |
| Micro-batch 16 files | 512 MB | 157 MB | works |

- A tight `memory_limit` **OOM-crashes** rather than spilling — the JSON parse path holds
  non-spillable ~32 MB buffers × threads, so the limit is a high guard rail (512 MB), not a
  ceiling. Persistent vs in-memory does not change the peak (it's the read, not the table).
- Verdict: cold rebuild micro-batches (16 files/batch, 512 MB guard) → peak ~150 MB for ~2x
  time. Steady-state never hits this (registry → only changed files).

### Spike 5 — End-to-end gold parity + latency — PASS
silver+gold build 0.62s over real data; tokens, cost (flat-priced in SQL), and sessions all
match the raw reference. **Gold serving-query latency 6 ms uncached** → the RefCell query
cache (~200 LOC) can be deleted.
- Design finding: `COUNT(DISTINCT session_id)` is **not additive** — summing per-day
  `daily_aggregates.session_count` over a week/year over-counts sessions spanning days (a
  latent bug in the current design too). Multi-day `sessions_total` must query `entries`
  directly (6 ms) or keep a separate range-level distinct, never sum daily counts.

### Spike 6 — Git points join into gold — NOT RUN (deferred, low risk)

---

## Corrections folded back into REFACTOR_PLAN_V2.md

1. Bronze uses `read_ndjson_objects` (not `read_ndjson_auto`). [Spike 0]
2. Bucketing uses arithmetic floor / `time_bucket` (not `width_bucket`). [Spike 1]
3. File tracking = simple stat-registry; delete only the byte seek/tail machine; cold
   rebuild micro-batches with a 512 MB guard rail. [Spikes 4, 4b]
4. `sessions_total` over multi-day ranges queries `entries`, not summed daily counts. [Spike 5]

## Open items before / during Phase 1
- Re-confirm Pi parity on REAL Pi session data once any exist on disk.
- Spike 6: confirm git-points → gold `loc_added/deleted` join parity (low risk).
