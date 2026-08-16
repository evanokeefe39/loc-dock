-- Oh My Pi silver extraction.
-- Template variables substituted at runtime:
--   {PATHS}           — comma-separated quoted file paths
--   {INPUT_PRICE}     — input cost per million tokens
--   {OUTPUT_PRICE}    — output cost per million tokens
--   {CACHE_WRITE_PRICE} — cache write cost per million tokens
--   {CACHE_READ_PRICE}  — cache read cost per million tokens
--   {MAX_OBJECT_SIZE} — maximum JSON object size in bytes
--
-- Edit this file when OMP's JSONL schema changes. Place your custom
-- version in ~/.config/loc-dock/sql/omp-silver.sql to override.
--
-- OMP session files (~/.omp/agent/sessions/<dir>/<ts>_<id>.jsonl) carry
-- model_change entries (model, provider) and message entries (assistant
-- responses).  message.usage is optional — when absent, tokens are
-- estimated from content text length (chars/4) and flat-priced via
-- LiteLLM with cost_type='estimated'.  When usage IS present, real
-- token counts and costs are used directly (cost_type='parsed').
--
-- Key differences from Pi adapter:
--   model_change entry uses $.model (not $.modelId)
--   Fallback token estimation when message.usage is absent
--   Skips __advisor.jsonl files (same rationale as Claude subagents/ skip)

INSERT OR IGNORE INTO entries
  (source, session_id, ts, model, provider, role,
   input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens,
   input_cost, output_cost, cache_write_cost, cache_read_cost, total_cost, cost_type, file_path)
WITH bronze AS (
  SELECT json AS j, replace(filename, '\', '/') AS file_path,
         row_number() OVER () AS rn
  FROM read_ndjson_objects([{PATHS}],
         filename = true, ignore_errors = true, maximum_object_size = {MAX_OBJECT_SIZE})
  -- Skip advisor sessions; their filenames don't carry the session id
  WHERE filename NOT ILIKE '%__advisor%'
),
carried AS (
  SELECT *,
    LAST_VALUE(CASE WHEN json_extract_string(j, '$.type') = 'model_change'
                    THEN json_extract_string(j, '$.model') END IGNORE NULLS)
      OVER (ORDER BY rn ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS carried_model,
    LAST_VALUE(CASE WHEN json_extract_string(j, '$.type') = 'model_change'
                    THEN json_extract_string(j, '$.provider') END IGNORE NULLS)
      OVER (ORDER BY rn ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS carried_provider
  FROM bronze
),
ex AS (
  SELECT
    -- Session ID: OMP filenames are <timestamp>_<sessionId>.jsonl (same as Pi)
    split_part(regexp_extract(file_path, '([^/]+)\.jsonl$', 1), '_', 2) AS session_id,
    COALESCE(
      CASE WHEN TRY_CAST(json_extract_string(j, '$.message.timestamp') AS BIGINT) IS NOT NULL
           THEN to_timestamp(TRY_CAST(json_extract_string(j, '$.message.timestamp') AS BIGINT) / 1000.0) AT TIME ZONE 'UTC'
      END,
      TRY_CAST(json_extract_string(j, '$.timestamp') AS TIMESTAMP)
    ) AS ts,
    COALESCE(json_extract_string(j, '$.message.model'), carried_model) AS model,
    COALESCE(json_extract_string(j, '$.message.provider'), carried_provider) AS provider,
    -- Real token counts (NULL when usage absent)
    COALESCE(TRY_CAST(json_extract_string(j, '$.message.usage.input')      AS BIGINT), 0) AS input_tokens,
    COALESCE(TRY_CAST(json_extract_string(j, '$.message.usage.output')     AS BIGINT), 0) AS output_tokens,
    COALESCE(TRY_CAST(json_extract_string(j, '$.message.usage.cacheWrite') AS BIGINT), 0) AS cache_write,
    COALESCE(TRY_CAST(json_extract_string(j, '$.message.usage.cacheRead')  AS BIGINT), 0) AS cache_read,
    -- Parsed cost (NULL when usage absent)
    TRY_CAST(json_extract_string(j, '$.message.usage.cost.input')      AS DOUBLE) AS p_input_cost,
    TRY_CAST(json_extract_string(j, '$.message.usage.cost.output')     AS DOUBLE) AS p_output_cost,
    TRY_CAST(json_extract_string(j, '$.message.usage.cost.cacheWrite') AS DOUBLE) AS p_cache_write_cost,
    TRY_CAST(json_extract_string(j, '$.message.usage.cost.cacheRead')  AS DOUBLE) AS p_cache_read_cost,
    TRY_CAST(json_extract_string(j, '$.message.usage.cost.total')      AS DOUBLE) AS p_total_cost,
    -- Estimated tokens from content text blocks (chars/4); used as fallback
    -- when real usage is absent.  json_each iterates content array blocks.
    (
      SELECT COALESCE(SUM(length(COALESCE(json_extract_string(value, '$.text'), ''))), 0)
      FROM json_each(json_extract(j, '$.message.content'))
      WHERE json_type(value) = 'OBJECT'
    ) / 4 AS est_tokens,
    -- Flag: was real usage data present?
    json_extract(j, '$.message.usage') IS NULL AS usage_missing,
    file_path
  FROM carried
  WHERE json_extract_string(j, '$.type') = 'message'
    AND json_extract_string(j, '$.message.role') = 'assistant'
)
SELECT
  'omp',
  session_id,
  ts,
  model,
  provider,
  'assistant',
  -- Tokens: prefer real, fall back to estimation
  input_tokens,
  CASE WHEN usage_missing THEN est_tokens ELSE output_tokens END,
  cache_write,
  cache_read,
  -- Cost: use real if available, flat-price estimate otherwise
  CASE WHEN NOT usage_missing AND p_total_cost > 0 THEN COALESCE(p_input_cost, 0)
       WHEN NOT usage_missing AND (input_tokens > 0 OR output_tokens > 0)
       THEN input_tokens / 1e6 * {INPUT_PRICE}
       WHEN usage_missing AND est_tokens > 0
       THEN 0  -- input tokens unknown for estimation; attribute all cost to output
       ELSE 0 END,
  CASE WHEN NOT usage_missing AND p_total_cost > 0 THEN COALESCE(p_output_cost, 0)
       WHEN NOT usage_missing AND (input_tokens > 0 OR output_tokens > 0)
       THEN output_tokens / 1e6 * {OUTPUT_PRICE}
       WHEN usage_missing AND est_tokens > 0
       THEN est_tokens / 1e6 * {OUTPUT_PRICE}
       ELSE 0 END,
  CASE WHEN NOT usage_missing AND p_total_cost > 0 THEN COALESCE(p_cache_write_cost, 0)
       WHEN NOT usage_missing AND cache_write > 0
       THEN cache_write / 1e6 * {CACHE_WRITE_PRICE}
       ELSE 0 END,
  CASE WHEN NOT usage_missing AND p_total_cost > 0 THEN COALESCE(p_cache_read_cost, 0)
       WHEN NOT usage_missing AND cache_read > 0
       THEN cache_read / 1e6 * {CACHE_READ_PRICE}
       ELSE 0 END,
  CASE WHEN NOT usage_missing AND p_total_cost > 0 THEN p_total_cost
       WHEN NOT usage_missing AND (input_tokens > 0 OR output_tokens > 0)
       THEN input_tokens / 1e6 * {INPUT_PRICE} + output_tokens / 1e6 * {OUTPUT_PRICE}
          + cache_write / 1e6 * {CACHE_WRITE_PRICE} + cache_read / 1e6 * {CACHE_READ_PRICE}
       WHEN usage_missing AND est_tokens > 0
       THEN est_tokens / 1e6 * {OUTPUT_PRICE}
       ELSE 0 END,
  CASE WHEN usage_missing THEN 'estimated' ELSE 'parsed' END,
  file_path
FROM ex
WHERE ts IS NOT NULL
