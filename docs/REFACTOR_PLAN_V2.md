# Loc-Dock Refactor Plan V2 — SQL-First Medallion Architecture

**Date:** 2026-06-16
**Status:** Draft — requires sign-off before implementation
**Supersedes:** `REFACTOR-PLAN.md` (V1) for the data-processing layer. V1's performance
items that remain valid are carried forward in §7.

---

## 1. Intent

LOC Dock tracks daily dev metrics (LOC, tokens, cost, sessions) in a floating desktop
dock. The data layer should be a small, declarative, SQL-first pipeline where **DuckDB is
the data-processing framework** and Rust is thin orchestration. The current backend does
in Rust what DuckDB does natively (JSON parsing, aggregation, bucketing, time-axis
generation), producing ~2,239 LOC of ETL core where ~500 would do.

This is an architecture change, not a language change. We keep Tauri + Rust + React. We
stop treating Rust as the ETL engine and start treating DuckDB as one.

### Why not pivot to Python

Assessed and rejected. Python+cron does not solve the root cause (the pain is
orchestrating data in the host language instead of in SQL — true in Rust or Python),
and it costs us: no maintained cross-compiler (build per-OS, same as now), +50-90 MB
sidecar, antivirus friction on top of unsigned installers, and no universal scheduler
(still branch per-OS). DuckDB-as-processor makes the host-language question moot.

---

## 2. Medallion Architecture (single-machine, batch ELT)

Three logical layers by data quality. Not distributed — this is medallion-lite for a
local DuckDB tool. Lambda/Kappa are overkill (no real-time, no streaming; the 60s refresh
is batch cadence).

```
  Sources                 BRONZE                SILVER                 GOLD
  (immutable logs)        (raw landing)         (cleaned/conformed)    (serving)
  ───────────────         ─────────────         ──────────────────     ───────────
  ~/.claude/**/*.jsonl ─► read_ndjson_auto ──►  normalized entries ──► daily_aggregates
  pi sessions/*.jsonl     glob + filename        deduped, typed,        (date, source,
  git log (subprocess) ─► (Rust → git_points)    cost computed          tokens, cost,
                                                 + git_points joined     sessions, loc)
                                                                     ──► timeline views
                                                                         (width_bucket)
```

### Layer contracts

**Bronze — raw immutable landing.** DuckDB reads source files directly via glob with the
`filename` pseudo-column. No transformation. One code path; replayable.

**Use `read_ndjson_objects`, NOT `read_ndjson_auto`** (spike finding §11): schema
auto-inference over the real heterogeneous Claude logs OOM-crashes. `read_ndjson_objects`
returns one raw `JSON` column per line with zero inference, then silver extracts fields by
path. This is the robust pattern for heterogeneous append-only logs.

```sql
CREATE OR REPLACE TABLE bronze_sessions AS
SELECT json AS j, filename AS source_file
FROM read_ndjson_objects(
       ['~/.claude/projects/**/*.jsonl', '<pi-sessions-glob>'],
       filename = true, ignore_errors = true);
```

**Silver — cleaned, conformed, typed.** One `INSERT ... SELECT` per source schema maps
heterogeneous bronze into the canonical `entries` shape, computes cost as a column,
dedupes by key, and (for Pi) carries `model_change` forward with `LAG()`. Replaces both
hand-rolled Rust parsers and `fill_costs`.

```sql
-- Claude: extract by JSON path from the raw bronze column (spike-verified parity)
INSERT INTO entries
SELECT
  'claude'                                                       AS source,
  json_extract_string(j, '$.sessionId')                         AS session_id,
  CAST(json_extract_string(j, '$.timestamp') AS TIMESTAMP)      AS ts,
  json_extract_string(j, '$.message.model')                     AS model,
  COALESCE(CAST(json_extract_string(j, '$.message.usage.input_tokens')  AS BIGINT), 0) AS input_tokens,
  COALESCE(CAST(json_extract_string(j, '$.message.usage.output_tokens') AS BIGINT), 0) AS output_tokens,
  COALESCE(CAST(json_extract_string(j, '$.message.usage.cache_creation_input_tokens') AS BIGINT), 0),
  COALESCE(CAST(json_extract_string(j, '$.message.usage.cache_read_input_tokens')     AS BIGINT), 0),
  input_tokens  / 1e6 * :input_price  AS input_cost,
  output_tokens / 1e6 * :output_price AS output_cost,
  /* ... */
  source_file
FROM bronze_sessions
WHERE json_extract_string(j, '$.type') = 'assistant'
  AND json_extract(j, '$.message.usage') IS NOT NULL
  AND source_file NOT ILIKE '%subagents%'   -- separator-agnostic (Windows \ paths)
ON CONFLICT (source, session_id, ts) DO NOTHING;
```

**Gold — serving.** `daily_aggregates` (already exists) is the gold aggregate table.
Timelines become views using arithmetic floor-bucketing/`time_bucket` (NOT `width_bucket` —
absent in 1.5.3, Spike 1); axis labels via `generate_series`. Frontend reads gold only.

**Caveat — distinct counts are not additive (Spike 5).** Additive measures (tokens, cost,
loc) can be `SUM`med across daily gold rows for week/year. But `sessions_total =
COUNT(DISTINCT session_id)` CANNOT: summing per-day session counts over-counts sessions
spanning multiple days (a latent bug in the current `daily_aggregates.session_count` too).
Multi-day `sessions_total` must query `entries` directly (measured 6 ms) or maintain a
separate range-level distinct / HLL sketch — never sum daily counts.

```sql
-- NOTE: width_bucket does NOT exist in the bundled DuckDB 1.5.3 (Spike 1).
-- Use arithmetic bucketing (matches the current Rust floor-index convention) or time_bucket.
CREATE OR REPLACE VIEW gold_cost_timeline AS
SELECT date_trunc('day', ts) AS day,
       CAST(floor((epoch(ts) - epoch(day_start)) / ((epoch(day_end) - epoch(day_start)) / 48.0)) AS INT) AS bucket,
       SUM(total_cost) AS cost
FROM entries GROUP BY 1, 2;
```

### Idempotent / incremental ingestion

- `filename` pseudo-column + an `ingested_files` registry → skip already-ingested files.
- Dedup in the SELECT (`GROUP BY` key) before upsert — DuckDB `ON CONFLICT` does **not**
  dedupe within a single statement.
- Gold tables are small; full-refresh per cycle is acceptable and removes a class of
  double-counting bugs. Use `MERGE INTO` (DuckDB ≥1.4) for incremental gold if needed.

---

## 3. Behavioral Contracts

```
GIVEN a directory of session JSONL files
WHEN the ETL cycle runs
THEN bronze reflects every file via read_ndjson_auto (no Rust line parsing)
 AND silver `entries` row count == sum of valid JSON lines, deduped by (source,session_id,ts)
 AND each entry's costs == flat-price formula applied in SQL

GIVEN entries spanning multiple days/sources
WHEN gold is built
THEN daily_aggregates has exactly one row per (date, source)
 AND SUM(gold tokens/cost) == SUM(silver tokens/cost) for the same window

GIVEN a cost/token timeline request for a range
WHEN the timeline view is queried
THEN it returns N_BUCKETS buckets via width_bucket
 AND bucket totals match the pre-refactor Rust bucketing within rounding tolerance

GIVEN the same source files across two consecutive cycles with no new lines
WHEN the second cycle runs
THEN no file is re-ingested (ingested_files registry hit)
 AND gold values are unchanged
```

---

## 4. Edge Case Inventory

- Malformed / partial trailing JSON line → `ignore_errors = true`; row dropped, logged.
- Divergent schemas across files (Claude vs Pi, field renames) → `union_by_name = true`;
  silver SELECT maps per-source with `COALESCE` over alternate field names.
- Pi `model_change` events → `LAG(model) ... IGNORE NULLS` window carry-forward in silver.
- DuckDB JSON type inference sampling miss → set `sample_size = -1` (scan all) on bronze.
- 16 MB `maximum_object_size` default → raise if any session line exceeds it.
- Empty / cold-start DB → bronze/silver/gold rebuild from full glob; correct by construction.
- Clock skew / future mtime → not relevant once we ingest by glob + registry, not mtime tail.
- Retention boundary → `DELETE` from silver+gold beyond window before/after refresh.

---

## 5. Migration Phases

Each phase is independently shippable and revertible. Validate parity against current Rust
output before deleting Rust code (keep both paths behind a flag for one cycle).

**Phase 0 — Smoke test (gate, per global CLAUDE.md External Integration Gate).**
Before writing pipeline code: run `read_ndjson_auto` against 1 real Claude file and 1 real
Pi file in the DuckDB CLI. Inspect inferred schema. Confirm field names/nesting/types match
assumptions. Document the actual schemas. No batch code until this passes.

**Phase 1 — Bronze + Silver ingest (highest LOC win, ~190 LOC out).**
Replace `ClaudeParser`/`PiParser` (`source_adapter.rs:117-309`) and `fill_costs`
(`usage_store.rs:897-912`) with bronze glob read + silver `INSERT...SELECT` + cost columns.
Drop manual multi-row VALUES parameter binding (`usage_store.rs:458-521`).

**Phase 2 — Gold timelines + labels as views (~150 LOC out).**
Replace `bucket_git`/`bucket_cost`/`bucket_tokens` and `compute_time_labels`
(`data.rs:246-410`) with `width_bucket`/`time_bucket` + `generate_series` views.

**Phase 3 — Collapse query layer (~200 LOC out).**
Replace the 7 RefCell-cached query methods (`usage_store.rs:687-848`) with views over gold
and 1-2 CTE queries. Data is small; the in-memory cache is unnecessary.

**Phase 4 — Simplify ingestion control (refined by Spike 4).**
Replace the seek/tail byte-positioning state machine (`usage_store.rs:277-434`) with a
simple `ingested_files(path, mtime, size)` registry: each cycle, stat the glob, and
reprocess only files whose mtime/size changed — reading those files **whole** (no byte
seek, no partial-line buffering); size-shrink ⇒ rotation ⇒ full re-read. Do NOT drop
tracking entirely: Spike 4 showed a full re-glob re-read peaks ~290 MB (vs current ~50 MB),
a memory regression. The registry keeps memory at tens of MB while still removing the
complex state machine. (Supersedes V1 §3b and the earlier V2 premise that full re-read is
free.)

Cold rebuild (first launch / retention reset / empty registry) must read everything once.
Spike 4b showed a single full-read query peaks ~300 MB and that a tight `memory_limit`
OOM-crashes (the JSON parse path doesn't spill). So the cold path **micro-batches**: ingest
~16 files per `INSERT`, with `memory_limit='512MB'` + `preserve_insertion_order=false` as
guard rails, which caps peak at ~150 MB for ~2x the time. Steady-state cycles never hit
this path (registry → only changed files).

**Phase 5 — Cleanup.** Remove dead pricing fields, unify timestamp handling on DuckDB
`TIMESTAMP`, delete the parity flag and old Rust paths.

Target: ETL core ~2,239 → ~500-700 LOC.

---

## 6. What Stays in Rust

- Git subprocess invocation + numstat text parse (`git.rs`, `git_cache.rs`). Git output is
  unstructured text from an external process — parsed in Rust, then handed to DuckDB as rows.
- Commit collection + regex filtering + LLM HTTP summary (`summary.rs`).
- Tauri commands, tray, window/IPC, app bootstrap, thread spawning (`commands.rs`,
  `tray.rs`, `lib.rs`, `task_queue.rs`).
- File discovery may stay in Rust or move to DuckDB globs — decide in Phase 1.

Rust becomes: spawn git, run `.sql` against the DuckDB connection, push gold JSON to React.

---

## 7. Carried Forward From V1 (still valid)

- Materialized `daily_aggregates` as the gold serving table — keep (already shipped, §3a V1).
- Frontend polling instead of event push (V1 §3c) — keep; orthogonal to this refactor.
- Single shared git scanner `Arc<Mutex<GitCache>>` (V1 §3d) — keep.
- Retention pruning at 14d (V1 §3e) — keep, expressed as gold/silver `DELETE`.

Superseded by V2:
- V1 §3b append-only seek/tail file tracking → replaced by glob + registry (Phase 4).
- V1's premise that aggregation/bucketing lives in Rust → moved into SQL.

---

## 8. Definition of Done

- [ ] Phase 0 smoke test documented (real schemas captured)
- [ ] Bronze/silver/gold layers exist as named tables/views
- [ ] No JSON parsed in Rust (`read_ndjson_auto` is the only ingest path)
- [ ] No aggregation/bucketing/label loops in Rust (`data.rs` reduced to orchestration)
- [ ] Parity tests: gold totals == legacy Rust totals on a frozen fixture dataset
- [ ] Idempotency test: re-running a cycle with no new lines changes nothing
- [ ] ETL core LOC reduced to target range
- [ ] `cargo test` green; `npm run tauri dev` first paint shows correct data
- [ ] Old Rust ETL paths + parity flag removed

---

## 9. Negative Space

- Frontend (React) and dock window behavior do not change.
- Tauri/Rust shell stays; no Python, no Electron.
- Git/LLM/IPC Rust code is out of scope except where it hands rows to DuckDB.
- No new runtime dependencies (DuckDB, serde, chrono already present). Polars/DataFusion
  NOT introduced — DuckDB subsumes them here.

## 11. Spike Results (2026-06-16 — Phase 0 executed)

Throwaway spike (`spike/spike_duckdb.py`) ran DuckDB 1.4.3 (same major as the bundled
crate) against the **real** 307 Claude session files (~39k assistant rows), comparing SQL
ingestion to a Python reference that mimics `source_adapter.rs` ClaudeParser line-by-line.

| Claim under test | Result |
|---|---|
| DuckDB can ingest the real heterogeneous logs | **Yes** — via `read_ndjson_objects` |
| SQL aggregation == Rust parser semantics | **PASS** — 39053 rows, 0 mismatches across 26 days × 4 token columns |
| Faster, not slower (the Python-perf worry) | **~3.4x faster** — 1.25s SQL vs 4.26s line-by-line parse |
| Less code | ~10 lines SQL replaces ~70 LOC Rust Claude parser + bucketing |

Findings that changed the plan:
1. **`read_ndjson_auto` is unusable here** — OOM/allocation failure from schema inference
   over heterogeneous nested logs (even with `sample_size=-1`, `union_by_name`). Bronze
   must use `read_ndjson_objects` + path extraction. (§2 updated.)
2. **Windows path separator** — DuckDB's `filename` column uses `\`; path filters must be
   separator-agnostic (`ILIKE '%subagents%'`). A `'%/subagents/%'` filter silently let
   subagent files through and doubled the row count until fixed. (§2 silver updated.)
3. **Parity tests are cheap and sensitive** — the Python-reference-vs-SQL diff caught the
   separator bug immediately. Keep this harness as the §8 parity gate.

Verdict: V2 is confirmed worth doing. The thesis holds on real data — DuckDB matches Rust
exactly, runs faster, and collapses the parser to ~10 lines. Proceed to Phase 1.

Still unverified by spike (do before/at Phase 1): Pi parser parity (no Pi files were
present to test the `LAG()` model-change carry-forward), and `width_bucket` timeline parity.

## 10. Open Questions

- (none — Phase 0 smoke test executed; results in §11)
