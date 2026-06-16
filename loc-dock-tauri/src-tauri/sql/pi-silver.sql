-- Pi silver extraction.
-- Template variables substituted at runtime:
--   {PATHS}           — comma-separated quoted file paths
--   {INPUT_PRICE}     — input cost per million tokens
--   {OUTPUT_PRICE}    — output cost per million tokens
--   {CACHE_WRITE_PRICE} — cache write cost per million tokens
--   {CACHE_READ_PRICE}  — cache read cost per million tokens
--   {MAX_OBJECT_SIZE} — maximum JSON object size in bytes
--
-- Edit this file when Pi's JSONL schema changes. Place your custom
-- version in ~/.config/loc-dock/sql/pi-silver.sql to override.
--
-- Pi uses camelCase token fields and carries its own cost nested under
-- usage.cost. model_change events set the active model/provider, carried
-- forward to subsequent assistant rows via a window LAST_VALUE. Flat
-- pricing is applied only when Pi supplied no total cost.

INSERT OR IGNORE INTO entries
  (source, session_id, ts, model, provider, role,
   input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens,
   input_cost, output_cost, cache_write_cost, cache_read_cost, total_cost, file_path)
WITH bronze AS (
  SELECT json AS j, replace(filename, '\', '/') AS file_path,
         row_number() OVER () AS rn
  FROM read_ndjson_objects([{PATHS}],
         filename = true, ignore_errors = true, maximum_object_size = {MAX_OBJECT_SIZE})
),
carried AS (
  SELECT *,
    LAST_VALUE(CASE WHEN json_extract_string(j, '$.type') = 'model_change'
                    THEN json_extract_string(j, '$.modelId') END IGNORE NULLS)
      OVER (ORDER BY rn ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS carried_model,
    LAST_VALUE(CASE WHEN json_extract_string(j, '$.type') = 'model_change'
                    THEN json_extract_string(j, '$.provider') END IGNORE NULLS)
      OVER (ORDER BY rn ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS carried_provider
  FROM bronze
),
ex AS (
  SELECT
    split_part(regexp_extract(file_path, '([^/]+)\.jsonl$', 1), '_', 2) AS session_id,
    COALESCE(
      CASE WHEN TRY_CAST(json_extract_string(j, '$.message.timestamp') AS BIGINT) IS NOT NULL
           THEN to_timestamp(TRY_CAST(json_extract_string(j, '$.message.timestamp') AS BIGINT) / 1000.0) AT TIME ZONE 'UTC'
      END,
      TRY_CAST(json_extract_string(j, '$.timestamp') AS TIMESTAMP)
    ) AS ts,
    COALESCE(json_extract_string(j, '$.message.model'), carried_model) AS model,
    COALESCE(json_extract_string(j, '$.message.provider'), carried_provider) AS provider,
    COALESCE(TRY_CAST(json_extract_string(j, '$.message.usage.input')      AS BIGINT), 0) AS input_tokens,
    COALESCE(TRY_CAST(json_extract_string(j, '$.message.usage.output')     AS BIGINT), 0) AS output_tokens,
    COALESCE(TRY_CAST(json_extract_string(j, '$.message.usage.cacheWrite') AS BIGINT), 0) AS cache_creation_input_tokens,
    COALESCE(TRY_CAST(json_extract_string(j, '$.message.usage.cacheRead')  AS BIGINT), 0) AS cache_read_input_tokens,
    COALESCE(TRY_CAST(json_extract_string(j, '$.message.usage.cost.input')      AS DOUBLE), 0) AS p_input_cost,
    COALESCE(TRY_CAST(json_extract_string(j, '$.message.usage.cost.output')     AS DOUBLE), 0) AS p_output_cost,
    COALESCE(TRY_CAST(json_extract_string(j, '$.message.usage.cost.cacheWrite') AS DOUBLE), 0) AS p_cache_write_cost,
    COALESCE(TRY_CAST(json_extract_string(j, '$.message.usage.cost.cacheRead')  AS DOUBLE), 0) AS p_cache_read_cost,
    COALESCE(TRY_CAST(json_extract_string(j, '$.message.usage.cost.total')      AS DOUBLE), 0) AS p_total_cost,
    file_path
  FROM carried
  WHERE json_extract_string(j, '$.type') = 'message'
    AND json_extract_string(j, '$.message.role') = 'assistant'
    AND json_extract(j, '$.message.usage') IS NOT NULL
)
SELECT
  'pi', session_id, ts, model, provider, 'assistant',
  input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens,
  CASE WHEN p_total_cost = 0 AND (input_tokens > 0 OR output_tokens > 0)
       THEN input_tokens / 1e6 * {INPUT_PRICE} ELSE p_input_cost END,
  CASE WHEN p_total_cost = 0 AND (input_tokens > 0 OR output_tokens > 0)
       THEN output_tokens / 1e6 * {OUTPUT_PRICE} ELSE p_output_cost END,
  CASE WHEN p_total_cost = 0 AND (input_tokens > 0 OR output_tokens > 0)
       THEN cache_creation_input_tokens / 1e6 * {CACHE_WRITE_PRICE} ELSE p_cache_write_cost END,
  CASE WHEN p_total_cost = 0 AND (input_tokens > 0 OR output_tokens > 0)
       THEN cache_read_input_tokens / 1e6 * {CACHE_READ_PRICE} ELSE p_cache_read_cost END,
  CASE WHEN p_total_cost = 0 AND (input_tokens > 0 OR output_tokens > 0)
       THEN input_tokens / 1e6 * {INPUT_PRICE} + output_tokens / 1e6 * {OUTPUT_PRICE}
          + cache_creation_input_tokens / 1e6 * {CACHE_WRITE_PRICE} + cache_read_input_tokens / 1e6 * {CACHE_READ_PRICE}
       ELSE p_total_cost END,
  file_path
FROM ex
WHERE ts IS NOT NULL
