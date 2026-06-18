-- Codex silver extraction placeholder.
-- Template variables: {PATHS}, {INPUT_PRICE}, {OUTPUT_PRICE},
--   {CACHE_WRITE_PRICE}, {CACHE_READ_PRICE}, {MAX_OBJECT_SIZE}
--
-- Edit this file when Codex's JSONL schema is confirmed. Place your custom
-- version in ~/.config/loc-dock/sql/codex-silver.sql to override.
--
-- TODO: Extract assistant_message entries with usage.tokens and model fields
-- from Codex JSONL files under ~/.codex/sessions/**/*.jsonl
INSERT OR IGNORE INTO entries
  (source, session_id, ts, model, provider, role,
   input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens,
   input_cost, output_cost, cache_write_cost, cache_read_cost, total_cost, cost_type, file_path)
SELECT 'codex', '', NULL, '', '', '',
  0, 0, 0, 0,
  0, 0, 0, 0, 0, 'estimated', ''
WHERE 1=0  -- no-op until real template is written
