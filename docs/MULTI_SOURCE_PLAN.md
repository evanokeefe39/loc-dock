# Multi-Source Data Adapter Plan

## 1. Landscape Research — Coding Harness Session Logs

### Claude Code
- **Path**: `~/.claude/projects/<encoded-cwd>/<session-id>.jsonl`
- **Format**: JSONL, one object per line
- **Key fields**:
  ```json
  {
    "type": "user|assistant|summary",
    "sessionId": "uuid",
    "uuid": "unique-entry-id",
    "parentUuid": null,
    "timestamp": "ISO-8601",
    "message": {
      "role": "user|assistant",
      "content": "string | [{type,text},...]",
      "model": "claude-opus-4-20250514",
      "usage": {
        "input_tokens", "output_tokens",
        "cache_creation_input_tokens", "cache_read_input_tokens"
      }
    }
  }
  ```
- **Adapter**: extract `assistant` rows with `usage` — already implemented in `sql/claude-silver.sql`
- **Unique key**: `('claude', sessionId, timestamp, file_path)`

### Pi Coding Agent
- **Path**: `~/.pi/agent/sessions/--<encoded-cwd>--/<timestamp>_<uuid>.jsonl`
- **Format**: JSONL, tree-structured via `id`/`parentId`
- **Key fields**:
  ```json
  {"type":"session","version":3,"id":"uuid","cwd":"/path"}
  {"type":"message","id":"hex8","parentId":"hex8","timestamp":"ISO",
    "message":{
      "role":"assistant",
      "content":[...],
      "provider":"anthropic","model":"claude-sonnet-4-5",
      "usage":{"input":N,"output":N,"cacheWrite":N,"cacheRead":N,
               "cost":{"input":N,"output":N,"cacheWrite":N,"cacheRead":N,"total":N}}
    }
  }
  ```
- **Adapter**: extract `message` entries where `message.role = 'assistant'` and `message.usage` exists — already implemented in `sql/pi-silver.sql`
- **Unique key**: `('pi', session_id extracted from filename, ts, file_path)`

### OpenAI Codex CLI
- **Path**: `~/.codex/sessions/<year>/<month>/<day>/rollout-<timestamp>-<uuid>.jsonl`
- **Format**: JSONL, each line has a `type` field and `payload`
- **Key fields** (from `michaelheap.com` analysis):
  ```json
  {"type":"user_message","payload":{"message":"..."}}
  {"type":"assistant_message","payload":{"role":"assistant","content":...,"model":...,"usage":{...}}}
  {"type":"tool_use","payload":{...}}
  ```
- **Usage tracking**: Codex tracks token usage in `assistant_message` payloads under `usage.input_tokens` / `usage.output_tokens`
- **Session ID**: from filename or `sessionId` field
- **Adapter needed**: extract `assistant_message` with usage data
- **Unique key**: `('codex', session_id, ts, file_path)`

### Gemini CLI
- **Path**: `~/.gemini/tmp/<project_hash>/chats/session-<uuid>.json`
- **Format**: Monolithic JSON file (not JSONL). Currently switching to JSONL per issue.
- **Key fields**: session object contains `messages[]` array with `{role,content,model,usage{...}}`
- **Adapter needed**: parse JSON array, extract assistant messages
- **Unique key**: `('gemini', session_id, ts, file_path)`
- **V1**: skip — JSONL-only for now

### Hermes Agent
- **Path**: `~/.hermes/state.db` (SQLite, WAL mode)
- **Format**: SQLite database, not JSONL.
- **Export**: `hermes sessions export <id>` outputs JSONL per session
- **Adapter needed**: query SQLite directly via DuckDB `sqlite` extension, or trigger export
- **V1**: skip — JSONL-only for now

### Aider
- **Path**: `.aider.chat.history.md` (per-project markdown file)
- **Format**: Markdown chat log, no structured token usage per message
- **Adapter**: minimal — Aider doesn't expose token usage. Skip.

### Cursor IDE
- **Path**: `~/Library/Application Support/Cursor/User/globalStorage/state.vscdb` (SQLite)
- **Format**: SQLite with `cursorDiskStorage` table containing JSON blobs
- **Priority**: low — complex extraction, different path

### OpenCode
- **Path**: `~/.local/share/opencode/project/<data>`
- **Format**: Per-project data, export via `opencode export <sessionID>` as JSON
- **Priority**: medium

### Others (lower priority for v1)
- **Copilot CLI**: `~/.chat-cli/*.json`
- **Crush**: `~/.crush/logs/*.jsonl`
- **Goose**: `~/.config/goose/sessions/*.jsonl`
- **SWE-agent**: output trajectories in JSONL
- **Factory (Droid)**: session logs

---

## 2. Existing projects doing this well

| Project | Approach | Notes |
|---------|----------|-------|
| **[cass](https://github.com/Dicklesworthstone/coding_agent_session_search)** | Rust, 11+ providers, SQLite+semantic search | Over-engineered, but excellent reference for adapters |
| **[hstry](https://github.com/byteowlz/hstry)** | Rust+TS adapters, SQLite, resume across agents | Most relevant — pluggable TypeScript adapters |
| **[ccusage](https://github.com/ryoppippi/ccusage)** | Node.js, Claude Code + others, DuckDB analysis | Similar tech stack to ours |
| **[agent-cost-dashboard](https://github.com/mrexodia/agent-cost-dashboard)** | Python, Pi/Claude/Codex/Gemini | Good reference for pricing approach |
| **[pi-cost](https://pi.dev/packages/pi-cost)** | Pi extension, LiteLLM pricing fallback | Tags cost as "actual" or "estimated" |

**Key insight**: All these tools use the same pattern — extract token counts from log JSONL, then multiply by per-model pricing from a lookup table.

---

## 3. Proposed Architecture — Loc-Dock Multi-Source Adapters

### Design Principles (ponytail full)
- **One new table**: `data_sources` — tracks user-configured source directories
- **One new field** on `entries`: `cost_type` — "estimated" (vs future "actual")
- **Config-driven SQL templates** (existing pattern): one `.sql` file per adapter
- **No adapter Rust code per source** — silver extraction SQL + per-model pricing = new source
- **Settings UI**: "+" button in settings → pick adapter from list → enter directory path

### Data Model Addition

```sql
CREATE TABLE IF NOT EXISTS data_sources (
    id           TEXT PRIMARY KEY,            -- "pi-main", "claude-work"
    adapter      TEXT NOT NULL,               -- "pi", "claude", "codex"
    display_name TEXT NOT NULL,               -- user-friendly label
    path         TEXT NOT NULL,               -- directory containing session files
    enabled      BOOLEAN NOT NULL DEFAULT true,
    created_at   TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
```

### Settings Model

```rust
// Replace the fixed claude_dir/pi_dir fields with:
pub struct DataSource {
    pub id: String,
    pub adapter: String,     // "pi" | "claude" | "codex"
    pub display_name: String,
    pub path: PathBuf,
    pub enabled: bool,
}

pub struct Settings {
    pub data_sources: Vec<DataSource>,  // replaces claude_dir, pi_dir
    pub model_pricing_path: Option<PathBuf>,  // user override for LiteLLM JSON
    // ... existing fields except claude_dir, pi_dir
}
```

### Adapter SQL Templates

Each adapter gets a `.sql` file under `sql/<adapter>-silver.sql` with template variables:
- `{PATHS}` — glob paths for `read_ndjson_objects`
- `{INPUT_PRICE}`, `{OUTPUT_PRICE}`, `{CACHE_WRITE_PRICE}`, `{CACHE_READ_PRICE}` — per-model pricing
- `{MAX_OBJECT_SIZE}` — max JSON obj size

### Pricing via LiteLLM

Replace the flat `pricing.yaml` with **LiteLLM's model pricing JSON** (2,784 models, ~1.5 MB / ~200 KB gzipped).

**Why**: The ecosystem standard. Community-maintained, updated when providers change rates. No maintenance burden on us.

**How it's bundled**:
- CI/CD: `package.json` prebuild script downloads from LiteLLM's repo
- Path: `src-tauri/resources/pricing/litellm.json`
- Load chain: `user override path` → `bundled resource` → `hardcoded defaults`
- User override: settings `model_pricing_path` — copy the bundled JSON, edit a few prices, point settings at it

**Rust data model**:
```rust
#[derive(Deserialize)]
pub struct ModelPricing {
    #[serde(default)]
    pub input_cost_per_token: f64,           // LiteLLM uses per-token
    #[serde(default)]
    pub output_cost_per_token: f64,
    #[serde(default)]
    pub cache_read_input_token_cost: f64,
    #[serde(default)]
    pub cache_creation_input_token_cost: f64,
    #[serde(default)]
    pub litellm_provider: Option<String>,
}

pub struct Pricing {
    models: HashMap<String, ModelPricing>,
    default: ModelPricing,  // fallback when model not in map
    cost_type: String,      // "estimated" — can flip to "actual" if a provider starts reporting
}

impl Pricing {
    fn get_per_million(&self, model: &str) -> [f64; 4] {
        let m = self.models.get(model).unwrap_or(&self.default);
        [
            m.input_cost_per_token * 1_000_000.0,
            m.output_cost_per_token * 1_000_000.0,
            m.cache_creation_input_token_cost * 1_000_000.0,
            m.cache_read_input_token_cost * 1_000_000.0,
        ]
    }
}
```

**How per-model pricing works at query time**:
1. The SQL template is parameterized with `{INPUT_PRICE}`, etc.
2. Before executing, Rust resolves the price for each model name found in the batch
3. For the SQL, the price is sent as a template variable — but since a batch may contain multiple models, we actually need per-row pricing
4. **Better approach**: pass the full pricing map as a DuckDB table and JOIN in SQL:

```sql
WITH pricing AS (
    SELECT * FROM (VALUES
        ('claude-sonnet-4-6', 3.00, 15.00, 1.50, 0.30),
        ('deepseek-v4-flash', 0.14, 0.28, 0.007, 0.0028)
    ) AS p(model, input_price, output_price, cache_write_price, cache_read_price)
)
SELECT
    e.*,
    e.input_tokens / 1e6 * COALESCE(p.input_price, 3.00) AS input_cost,
    ...
FROM extracted e
LEFT JOIN pricing p ON e.model = p.model
```

This way a single SQL query handles multiple models in one batch with correct per-row pricing.

### The Runtime Flow

```rust
// In data loop, replaced claude/pi-specific paths with:
let sources = config.settings.data_sources.iter()
    .filter(|s| s.enabled);

for source in sources {
    // 1. Check `ingested_files` for changed/new files in source.path
    // 2. Run `read_ndjson_objects` (or adapter-specific extraction)
    // 3. Execute sql/<adapter>-silver.sql with pricing params
    // 4. INSERT OR IGNORE into entries with source = adapter name
    // 5. Update ingested_files table
}
```

### Adapter Matrix (v1)

| Adapter | Format | SQL Template | Notes |
|---------|--------|-------------|-------|
| `claude` | JSONL | `sql/claude-silver.sql` | ✅ exists, update for per-model pricing |
| `pi` | JSONL | `sql/pi-silver.sql` | ✅ exists, update for per-model pricing |
| `codex` | JSONL | `sql/codex-silver.sql` | new — similar structure, different fields |

---

## 4. Settings UI (Frontend)

### Current Settings
- Claude directory field
- Pi directory field

### New Settings UI
- **"Data Sources" section**
  - Lists all configured sources (id, adapter icon, path, toggle)
  - "+ Add Source" button → opens a dialog:
    1. Select adapter type (dropdown: Pi, Claude Code, Codex CLI)
    2. Enter display name (optional, defaults to adapter + #)
    3. Browse/paste session directory path
    4. "Add" button validates: exists, has matching files, not a duplicate
  - Each source row: edit name, change path, toggle enable, delete
  - Delete removes from settings + cleans up `ingested_files`

- **"Model Pricing" section** (replaces old pricing)
  - Shows bundled LiteLLM JSON path
  - Optional override path field
  - Link to LiteLLM repo to check for updates

### Frontend Impact
- New Tauri commands: `list_sources`, `add_source`, `remove_source`, `toggle_source`
- Backward compat: on first load with old `settings.json`, auto-create DataSource entries from `claude_dir` and `pi_dir`
- Frontend settings panel: replace Pi/Claude dir fields with data sources list + add button

---

## 5. Implementation Plan

### Phase 1: LiteLLM pricing + Data Sources table + Settings (2-3 days)
1. Add `download-litellm-pricing.mjs` script, wire into `package.json` prebuild
2. Ship bundled JSON as Tauri resource
3. Rewrite `Pricing` to load LiteLLM JSON (per-token → per-million conversion)
4. Update SQL templates to use per-model JOIN instead of flat price params
5. Add `cost_type` column to `entries` ("estimated")
6. Add `data_sources` DDL to schema
7. Replace `claude_dir`/`pi_dir` in `Settings` with `Vec<DataSource>`
8. Add migration for existing config (auto-create 2 sources from old fields)
9. Add Tauri commands: `add_source`, `remove_source`, `toggle_source`, `list_sources`
10. Update settings frontend panel

### Phase 2: Adapter system (1-2 days)
1. Refactor data loop to iterate over `data_sources` instead of hardcoded paths
2. Create `sql/codex-silver.sql` adapter
3. Wire adapter SQL templates into `usage_store.rs`
4. Update `ingested_files` to track per-source files

### Phase 3: Frontend UX (1 day)
1. "Data Sources" section in settings with adapter picker
2. Add source dialog (adapter type, display name, directory path)
3. Source row with toggle/path/delete
4. Toast on add/remove/toggle success/failure

### Phase 4: Polish & edge cases (1 day)
1. Re-scan on settings change (no restart needed)
2. Dedup across sources (same session in two dirs)
3. Handle adapter directory deletion (graceful skip)
4. Community adapter docs for `~/.config/loc-dock/adapters/`
