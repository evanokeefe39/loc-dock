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

### Hermes Agent
- **Path**: `~/.hermes/state.db` (SQLite, WAL mode)
- **Format**: SQLite database, not JSONL. Schema:
  - `sessions(id, source, model, started_at, ended_at, message_count, tool_call_count, input_tokens, output_tokens, ...)`
  - `messages(id, session_id, role, content, tool_calls, timestamp, token_count, ...)`
- **Export**: `hermes sessions export <id>` outputs JSONL per session
- **Adapter needed**: query SQLite directly via DuckDB `sqlite` extension, or trigger export
- **Unique key**: `('hermes', session_id, ts, file_path)`

### Aider
- **Path**: `.aider.chat.history.md` (per-project markdown file, not JSONL)
- **Format**: Markdown chat log, no structured token usage per message
- **Adapter**: minimal — Aider doesn't expose token usage in log files. Skip for now.

### Cursor IDE
- **Path**: `~/Library/Application Support/Cursor/User/globalStorage/state.vscdb` (SQLite)
- **Format**: SQLite `state.vscdb` with `cursorDiskStorage` table containing JSON blobs
- **Adapter**: complex SQLite extraction. Cursor's aim >30000 table stores chat history
- **Priority**: low — rich data but very different extraction path

### OpenCode
- **Path**: `~/.local/share/opencode/project/<data>`
- **Format**: Per-project data, export via `opencode export <sessionID>` as JSON
- **Adapter**: trigger CLI export or parse session data directly
- **Priority**: medium

### Cline (VS Code Extension)
- **Path**: SQLite within VS Code extension storage
- **Format**: `state.vscdb` with extension-specific tables
- **Priority**: low — less transparent than CLI agents

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
| **[cass](https://github.com/Dicklesworthstone/coding_agent_session_search)** (Dicklesworthstone) | Rust, 11+ providers, SQLite+semantic search | Over-engineered for our needs, but excellent reference for adapters |
| **[hstry](https://github.com/byteowlz/hstry)** (byteowlz) | Rust+TS adapters, SQLite, resume across agents | Most relevant — pluggable TypeScript adapters, incremental parsing |
| **[ccusage](https://github.com/ryoppippi/ccusage)** | Node.js, Claude Code only, DuckDB analysis | Similar tech stack to ours |

**Key insight**: Both cass and hstry use **pluggable adapters** with a common normalized schema. hstry's TypeScript adapters are the most relevant pattern for us since we can piggyback on their work.

---

## 3. Proposed Architecture — Loc-Dock Multi-Source Adapters

### Design Principles (ponytail full)
- **One new table**: `data_sources` — tracks user-configured source directories
- **One new column** on `entries`: `source` is already there — we just need to parse different formats into it
- **Config-driven SQL templates** (existing pattern): one `.sql` file per adapter
- **No adapter Rust code per source** — silver extraction SQL + pricing.yaml = new source
- **Settings UI**: "+" button in settings → pick adapter from list → enter directory path

### Data Model Addition

```sql
CREATE TABLE IF NOT EXISTS data_sources (
    id           TEXT PRIMARY KEY,            -- "pi-main", "claude-work", "codex-projectx"
    adapter      TEXT NOT NULL,               -- "pi", "claude", "codex", "gemini", "hermes"
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
    pub adapter: String,     // "pi" | "claude" | "codex" | "gemini" | "hermes"
    pub display_name: String,
    pub path: PathBuf,
    pub enabled: bool,
}

pub struct Settings {
    pub data_sources: Vec<DataSource>,  // replaces claude_dir, pi_dir
    // ... existing fields except claude_dir, pi_dir
}
```

### Adapter SQL Templates

Each adapter gets a `.sql` file under `sql/<adapter>-silver.sql` with template variables:
- `{PATHS}` — glob paths for `read_ndjson_objects`
- `{INPUT_PRICE}`, `{OUTPUT_PRICE}`, `{CACHE_WRITE_PRICE}`, `{CACHE_READ_PRICE}` — from pricing.yaml
- `{MAX_OBJECT_SIZE}` — max JSON obj size

For non-JSONL sources (Hermes SQLite, Gemini monolithic JSON), the adapter can:
1. Use DuckDB's `sqlite` extension (`ATTACH` + query) for Hermes
2. Use a small Rust pre-processor that converts to JSONL on-the-fly

### The Runtime Flow

```rust
// In data loop, replaced claude/pi-specific paths with:
let sources = config.settings.data_sources.iter()
    .filter(|s| s.enabled)
    .map(|s| (s.adapter.as_str(), s.path.as_path()));

for (adapter, dir) in sources {
    // 1. Check `ingested_files` for changed/new files
    // 2. Run `read_ndjson_objects` (or adapter-specific extraction)
    // 3. Execute sql/<adapter>-silver.sql with pricing params
    // 4. INSERT OR IGNORE into entries with source = adapter name
    // 5. Update ingested_files table
}
```

### Adapter Matrix (v1)

| Adapter | Format | SQL Template | Notes |
|---------|--------|-------------|-------|
| `claude` | JSONL | `sql/claude-silver.sql` | ✅ exists |
| `pi` | JSONL | `sql/pi-silver.sql` | ✅ exists |
| `codex` | JSONL | `sql/codex-silver.sql` | new — similar structure, different fields |
| `gemini` | JSON | `sql/gemini-silver.sql` | new — needs JSON array parsing |
| `hermes` | SQLite | `sql/hermes-silver.sql` | new — needs `sqlite` DuckDB extension or pre-processor |

### Adapter v2 (future, no code change — just add SQL)

Formats that need more work (SQLite in `state.vscdb`, markdown logs, etc.) can use a small Rust pre-processor registered in the adapter system:

```rust
trait SourceAdapter {
    fn name(&self) -> &str;
    fn scan(&self, dir: &Path) -> Result<Vec<PathBuf>>;         // find session files
    fn extract(&self, paths: &[PathBuf]) -> Result<()>;         // write to a temp JSONL
    fn silver_sql(&self) -> &str;                               // path to .sql template
}
```

But for v1, we only need the SQL-template path and it works for **all JSONL-based sources**.

---

## 4. Settings UI (Frontend)

### Current Settings (Tauri)
- Claude directory field
- Pi directory field

### New Settings UI
- **"Data Sources" section**
  - Lists all configured sources (id, adapter icon, path, toggle)
  - "+ Add Source" button → opens a dialog:
    1. Select adapter type (dropdown: Pi, Claude Code, Codex CLI, Gemini CLI, Hermes)
    2. Enter display name (optional, defaults to adapter type + #)
    3. Browse/paste session directory path
    4. "Add" button validates:
       - Directory exists
       - Contains files matching the adapter's expected pattern
       - Not a duplicate of an existing source path
  - Each source row has: edit name, change path, toggle enable, delete
  - Delete removes from settings + cleans up `ingested_files` registry (future)

### Frontend Impact
- Settings page: new Rust command `list_sources` / `add_source` / `remove_source` / `toggle_source`
- Backward compat: on first load with old `settings.json`, auto-create a `DataSource` entry from `claude_dir` and `pi_dir`

---

## 5. Implementation Plan

### Phase 1: Data Sources table + Settings (1-2 days)
1. Add `data_sources` DDL to schema
2. Replace `claude_dir`/`pi_dir` in `Settings` with `Vec<DataSource>`
3. Add migration for existing config (auto-create 2 sources from old fields)
4. Add Tauri commands: `add_source`, `remove_source`, `toggle_source`, `list_sources`
5. Update settings frontend panel

### Phase 2: Adapter system (1-2 days)
1. Refactor data loop to iterate over `data_sources` instead of hardcoded paths
2. Create `sql/codex-silver.sql` adapter
3. Create `sql/gemini-silver.sql` adapter (parse JSON array → JSONL)
4. Create `sql/hermes-silver.sql` adapter (DuckDB `sqlite` extension)
5. Wire adapter SQL templates into `usage_store.rs`
6. Update `ingested_files` to track per-source files
7. Add `adapter` column to `entries` (or reuse `source`)

### Phase 3: Frontend UX (1 day)
1. "Data Sources" section in settings
2. Add Source dialog with adapter picker
3. Source row with toggle/path/delete
4. Toast on add/remove/toggle success/failure

### Phase 4: Polish & edge cases (1 day)
1. Re-scan on settings change (no restart)
2. Dedup across sources (same session in two dirs)
3. Handle adapter directory deletion (graceful skip)
4. Add adapter docs for community contributors

---

## 6. Open Questions (for the user)

1. **Adapter for non-JSONL sources**: Hermes (SQLite) and Gemini (monolithic JSON) need either DuckDB extension support or a Rust pre-processor. Which approach?
2. **Config migration**: Auto-create data sources from old `claude_dir`/`pi_dir` on first run? Or prompt user?
3. **Adapter discovery**: Ship adapters built-in, or allow community-created adapters from a folder like `~/.config/loc-dock/adapters/`?
4. **Pricing per source**: Currently `pricing.yaml` covers claude + pi. Codex uses OpenAI pricing, Gemini uses Google. Should each source have its own pricing config, or share the same budget?
