-- Claude silver extraction.
-- Template variables substituted at runtime:
--   {PATHS}           — comma-separated quoted file paths
--   {INPUT_PRICE}     — input cost per million tokens
--   {OUTPUT_PRICE}    — output cost per million tokens
--   {CACHE_WRITE_PRICE} — cache write cost per million tokens
--   {CACHE_READ_PRICE}  — cache read cost per million tokens
--   {MAX_OBJECT_SIZE} — maximum JSON object size in bytes
--
-- Edit this file when Claude's JSONL schema changes. Place your custom
-- version in ~/.config/loc-dock/sql/claude-silver.sql to override.

INSERT OR IGNORE INTO entries
  (source, session_id, ts, model, provider, role,
   input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens,
   input_cost, output_cost, cache_write_cost, cache_read_cost, total_cost, file_path)
WITH bronze AS (
  SELECT json AS j, replace(filename, '\', '/') AS file_path
  FROM read_ndjson_objects([{PATHS}],
         filename = true, ignore_errors = true, maximum_object_size = {MAX_OBJECT_SIZE})
),
ex AS (
  SELECT
    COALESCE(json_extract_string(j, '$.sessionId'),
             regexp_extract(file_path, '([^/]+)\.jsonl$', 1)) AS session_id,
    TRY_CAST(json_extract_string(j, '$.timestamp') AS TIMESTAMP) AS ts,
    json_extract_string(j, '$.message.model') AS model,
    COALESCE(TRY_CAST(json_extract_string(j, '$.message.usage.input_tokens')  AS BIGINT), 0) AS input_tokens,
    COALESCE(TRY_CAST(json_extract_string(j, '$.message.usage.output_tokens') AS BIGINT), 0) AS output_tokens,
    COALESCE(TRY_CAST(json_extract_string(j, '$.message.usage.cache_creation_input_tokens') AS BIGINT), 0) AS cache_creation_input_tokens,
    COALESCE(TRY_CAST(json_extract_string(j, '$.message.usage.cache_read_input_tokens')     AS BIGINT), 0) AS cache_read_input_tokens,
    file_path
  FROM bronze
  WHERE json_extract_string(j, '$.type') = 'assistant'
    AND json_extract(j, '$.message.usage') IS NOT NULL
)
SELECT
  'claude', session_id, ts, model, 'anthropic', 'assistant',
  input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens,
  CASE WHEN input_tokens > 0 OR output_tokens > 0 THEN input_tokens / 1e6 * {INPUT_PRICE} ELSE 0 END,
  CASE WHEN input_tokens > 0 OR output_tokens > 0 THEN output_tokens / 1e6 * {OUTPUT_PRICE} ELSE 0 END,
  CASE WHEN input_tokens > 0 OR output_tokens > 0 THEN cache_creation_input_tokens / 1e6 * {CACHE_WRITE_PRICE} ELSE 0 END,
  CASE WHEN input_tokens > 0 OR output_tokens > 0 THEN cache_read_input_tokens / 1e6 * {CACHE_READ_PRICE} ELSE 0 END,
  CASE WHEN input_tokens > 0 OR output_tokens > 0
       THEN input_tokens / 1e6 * {INPUT_PRICE} + output_tokens / 1e6 * {OUTPUT_PRICE}
          + cache_creation_input_tokens / 1e6 * {CACHE_WRITE_PRICE} + cache_read_input_tokens / 1e6 * {CACHE_READ_PRICE}
       ELSE 0 END,
  file_path
FROM ex
WHERE ts IS NOT NULL
