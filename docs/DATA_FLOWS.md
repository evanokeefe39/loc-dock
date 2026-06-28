# Data Flows

How data moves from source files to the dock UI. Five independent pipelines,
all driven by the single data loop in `data.rs::spawn_data_loop`.

**Legend:**
```
SOURCE ──► DuckDB table ──► Rust struct ──► Tauri IPC ──► Frontend
```

---

## Medallion Architecture Overview

```mermaid
flowchart TD
    subgraph Sources["Source Files (immutable logs)"]
        JSONL["Claude / Pi / Codex JSONL\n~/.claude/**/*.jsonl\n~/.pi/agent/sessions/*.jsonl\n~/.codex/sessions/**/*.jsonl"]
        GIT["Git repos\nrepos_dir/*/.git"]
    end

    subgraph Bronze["Bronze — Raw Landing"]
        BRONZE_CTE["read_ndjson_objects CTE\nephemeral per batch\nno schema inference\nraw JSON per line"]
    end

    subgraph Silver["Silver — Cleaned & Typed"]
        ENTRIES["entries table\nUNIQUE(source, session_id, ts)\nfields by JSON path\ncost per model (LiteLLM)\ndeduped ON CONFLICT"]
        COMMIT_STATS["commit_stats table\nUNIQUE(repo, sha)\nincremental git log\nper-commit LOC counts"]
    end

    subgraph Gold["Gold — Serving Aggregates"]
        DAILY_AGG["daily_aggregates table\nper-date, per-source rollup\nSUM tokens, cost, sessions, LOC\nrefreshed when rows grow"]
        SUMMARY_CACHE["repo_summaries table\ncached LLM highlights\nkeyed by repo_path\ncompared to head_sha"]
    end

    subgraph Serving["Rust Serving Queries"]
        QUERIES["usage_store.rs\nquery_aggregates()\nquery_cost_buckets()\nquery_commit_buckets()\ncount_sessions()\nbuild_one_range()"]
        SUMMARY_BUILD["build_summary_data()\nall_repos_with_commits\n+ all_summarized_repos" ]
    end

    subgraph Shared["Shared In-Memory State"]
        ALLSTATS["AllStats\nArc&lt;RwLock&gt;"]
        SUMMDATA["SummaryData\nArc&lt;RwLock&gt;"]
    end

    subgraph Frontend["React Frontend"]
        TQ["TanStack Query\nuseStatsQuery\nuseSummaryQuery"]
        ZS["Zustand store\nrange / mode / panels"]
        UI["TopRow · Chart · BottomRow\nSummaryPanel · SettingsPanel"]
    end

    JSONL -->|ingested_files\nregistry filter| BRONZE_CTE
    BRONZE_CTE -->|INSERT...SELECT\nper-source SQL templates| ENTRIES
    ENTRIES -->|finalize_etl()\nrefresh_aggregates| DAILY_AGG

    GIT -->|git log --after\ncollect_new_commits| COMMIT_STATS
    COMMIT_STATS -->|head_sha check\nchange detection| SUMMARY_CACHE

    DAILY_AGG -->|query_aggregates\nadditive measures| QUERIES
    ENTRIES -->|query_cost_buckets\nnon-additive measures| QUERIES
    COMMIT_STATS -->|query_commit_buckets| QUERIES
    SUMMARY_CACHE ---> SUMMARY_BUILD

    QUERIES -->|build_all_stats ×4\nday/week/month/year| ALLSTATS
    SUMMARY_BUILD -->|Tauri event\n+ shared state| SUMMDATA

    ALLSTATS -->|get_stats command| TQ
    SUMMDATA -->|get_summary command\n+ summary-update event| TQ
    TQ ---> UI
    ZS ---> UI

    style Bronze fill:#CD7F32,color:#000
    style Silver fill:#C0C0C0,color:#000
    style Gold fill:#FFD700,color:#000
    style Sources fill:#333,color:#fff
    style Shared fill:#444,color:#fff
    style Frontend fill:#2d3748,color:#fff
```

### Layer contracts

| Layer | Table/CTE | What it stores | How it's populated | How it's read |
|-------|-----------|----------------|--------------------|---------------|
| **Bronze** | `read_ndjson_objects` (CTE) | Raw JSON per line, no schema | DuckDB glob of JSONL files on each batch | Silver `INSERT...SELECT` extracts by path |
| **Silver** | `entries` | Typed rows: source, session_id, ts, tokens, cost (all fields) | `INSERT...SELECT` per-source SQL templates, deduped | Gold aggregates; serving queries for buckets & sessions |
| **Silver** | `commit_stats` | Per-commit: repo, sha, ts, msg, added, deleted | Rust parses `git log --numstat`, inserts with ON CONFLICT DO NOTHING | Bucket queries for git timeline; latest_commit_ts for incremental scan |
| **Gold** | `daily_aggregates` | Per-date, per-source: SUM(tokens), SUM(cost), SUM(LOC), session_count | `refresh_aggregates()`: INSERT OR REPLACE ... SELECT | `query_aggregates()` for totals; prefill on startup |
| **Gold** | `repo_summaries` | Cached LLM highlights per repo: sha, json, model, timestamp | `save_repo_summary()` after LLM response | `build_summary_data()` for SummaryPanel |

---

## 1. Usage Data Flow (JSONL → Cost / Tokens / Sessions)

```mermaid
flowchart TD
    subgraph Sources["Source Files"]
        CLAUDE_JSONL["Claude Code\n~/.claude/projects/**/*.jsonl"]
        PI_JSONL["Pi\n~/.pi/agent/sessions/*.jsonl"]
        CODEX_JSONL["Codex CLI\n~/.codex/sessions/**/*.jsonl"]
    end

    subgraph Discovery["File Discovery"]
        SM["SourceManager\nGlobFileDiscoverer"]
        REGISTRY["ingested_files\n(mtime, size) check\nskip unchanged"]
    end

    subgraph Bronze_
        BRONZE["read_ndjson_objects\nephemeral CTE\nraw JSON per line\nignore_errors=true"]
    end

    subgraph Silver_
        ENTRIES["INSERT INTO entries\nsource · session_id · ts\nmodel · input_tokens\noutput_tokens\ncache tokens\ninput_cost · output_cost\ncache_write_cost\ncache_read_cost\ntotal_cost · file_path\ncost per model (LiteLLM)\n\nUNIQUE(source, session_id, ts)"]
    end

    subgraph Gold_
        DAILY["INSERT OR REPLACE\nINTO daily_aggregates\ndate · source\nSUM(input_tokens)\nSUM(output_tokens)\nSUM(total_cost)\nCOUNT(DISTINCT session_id)\n\nUNIQUE(date, source)"]
        RETENTION["DELETE entries/daily_agg\nWHERE ts < RETENTION_DAYS"]
    end

    subgraph Serving_["Serving Queries (usage_store.rs)"]
        Q_AGG["query_aggregates()\nSELECT SUM(cost), SUM(tokens)\nFROM daily_aggregates\n→ (cost, breakdown, tokens, sessions)"]
        Q_COST["query_cost_buckets()\nFROM entries\nbucketed timeline\n→ Vec&lt;f64&gt;"]
        Q_TOKENS["query_token_buckets()\nFROM entries\nbucketed timeline\n→ Vec&lt;(i64,i64,i64,i64)&gt;"]
        Q_SESSIONS["count_sessions()\nCOUNT(DISTINCT session_id)\nFILTER(WHERE ts >= active)\n→ (total, active)"]
        Q_SRC["query_aggregate_\nsource_breakdown()\nGROUP BY source\n→ Vec&lt;SourceStats&gt;"]
    end

    subgraph Build_
        ONE_RANGE["build_one_range()\ncalls all 5 queries\npacks RangeResult {stats,\ngit_buckets, cost_buckets,\ntoken_buckets, labels}"]
        ALL_STATS["build_all_stats()\ncalls build_one_range ×4\nday / week / month / year\npacks AllStats"]
    end

    subgraph Shared_
        SS["SharedStats\nArc&lt;RwLock&lt;AllStats&gt;&gt;\nwritten by data loop\nread by get_stats"]
    end

    subgraph Front_
        TQ_["TanStack Query\nuseStatsQuery()\npoll invoke(get_stats)\nevery 10s"]
        COMP["Chart · TopRow · BottomRow\nreads stats[range]\nrenders via Canvas 2D"]
    end

    CLAUDE_JSONL --> SM
    PI_JSONL --> SM
    CODEX_JSONL --> SM
    SM -->|process_source_named| REGISTRY
    REGISTRY -->|only changed files| BRONZE
    BRONZE -->|INSERT...SELECT\nper-source SQL template| ENTRIES
    ENTRIES -->|finalize_etl| DAILY
    DAILY --> RETENTION

    DAILY --> Q_AGG
    DAILY --> Q_SRC
    ENTRIES --> Q_COST
    ENTRIES --> Q_TOKENS
    ENTRIES --> Q_SESSIONS

    Q_AGG --> ONE_RANGE
    Q_COST --> ONE_RANGE
    Q_TOKENS --> ONE_RANGE
    Q_SESSIONS --> ONE_RANGE
    Q_SRC --> ONE_RANGE

    ONE_RANGE -->|×4| ALL_STATS
    ALL_STATS -->|write| SS
    SS -->|get_stats command| TQ_
    TQ_ -->|data + isLoading| COMP

    style Bronze_ fill:#CD7F32,color:#000
    style Silver_ fill:#C0C0C0,color:#000
    style Gold_ fill:#FFD700,color:#000
    style Serving_ fill:#556,color:#fff
    style Build_ fill:#445,color:#fff
    style Shared_ fill:#334,color:#fff
    style Front_ fill:#2d3748,color:#fff
```

**Detailed path:**

```
~/.claude/projects/**/*.jsonl        ◄── Claude Code writes session logs here
~/.pi/agent/sessions/*.jsonl         ◄── Pi writes session logs here
~/.codex/sessions/**/*.jsonl         ◄── Codex CLI writes session logs here
        │
        ▼
  SourceManager::with_discoverers  (built from config data_sources list)
  ├── GlobFileDiscoverer (claude) — globs projects, skips subagents/
  ├── GlobFileDiscoverer (pi)     — globs sessions directory
  └── GlobFileDiscoverer (codex)  — globs sessions/**/*.jsonl
        │
        ▼
  process_source_named(name)  ──┬── filter: only files whose
        │                       │     (mtime, size) changed vs
        │                       │     ingested_files registry
        ▼                       │
  ┌──────────────────────┐      │
  │  BRONZE              │      │    ephemeral CTE per batch:
  │  read_ndjson_objects │      │    no schema inference, raw JSON per line
  └──────────┬───────────┘      │
             │                  │
             ▼                  │
  ┌──────────────────────┐      │
  │  SILVER              │      │    INSERT ... SELECT entries:
  │  entries table        │      │    - fields extracted by JSON path
  │                      │      │    - cost flat-priced per model (LiteLLM JSON)
  │  UNIQUE(source,      │      │    - deduped on conflict (INSERT OR IGNORE)
  │    session_id, ts)   │      │    - per-source SQL templates
  └──────────┬───────────┘      │
             │                  │
             ▼                  │
  ┌──────────────────────┐      │
  │  GOLD                │      │    finalize_etl():
  │  daily_aggregates    │<─────┘    - refresh_aggregates() if rows grew
  │  (date, source,      │            - prune entries older than RETENTION_DAYS
  │   tokens, cost,      │
  │   sessions, loc)     │
  └──────────┬───────────┘
             │
             ▼
  Serving queries (usage_store.rs):
  ├── query_aggregates(since_str)
  │     SELECT SUM(total_cost), SUM(input_tokens), ...
  │     FROM daily_aggregates WHERE date >= ?::DATE
  │     → (cost_total, CostBreakdown, TokenTotals, session_count)
  │
  ├── query_aggregate_source_breakdown(since_str)
  │     SELECT source, SUM(...) GROUP BY source
  │     → Vec<SourceStats>
  │
  ├── query_cost_buckets(lo, hi, n)
  │     SELECT SUM(cost) bucketed by (ts - lo) / span * n
  │     FROM entries  (silver, for precision)
  │     → Vec<f64>  (per-bucket cost, length = n_buckets)
  │
  ├── query_token_buckets(lo, hi, n)
  │     Same bucketing for input/output/cache tokens
  │     → Vec<(i64,i64,i64,i64)>
  │
  └── count_sessions(since_str, active_str)
        COUNT(DISTINCT session_id) FILTER (WHERE ts >= ?)
        → (sessions_total, sessions_active)
             │
             ▼
  build_one_range(store, lo, hi, since_str, ...)
  ├── calls all 5 serving queries
  ├── compute_time_labels() for axis ticks
  └── packs into RangeResult { stats, git_buckets, cost_buckets, token_buckets, labels }
        │
        ▼
  build_all_stats(store, day_lo..year_lo, hi, ...)
  ├── calls build_one_range × 4 (day, week, month, year)
  └── packs into AllStats { ready, day, week, month, year, ...buckets..., ...labels }
        │
        ▼
  write to SharedStats (Arc<RwLock<AllStats>>)
        │
        ▼
  get_stats command reads SharedStats → returns AllStats to frontend
```

**Key properties:**
- JSON is **never parsed in Rust** — DuckDB's `read_ndjson_objects` handles all parsing
- **Incremental ingest** via `ingested_files(mtime, size)` registry — re-reads only changed files
- **Dedup by key** `(source, session_id, ts)` prevents double-counting on re-ingest
- **Distinct counts are not additive** across days — `sessions_total` for week queries `entries` directly, not `daily_aggregates`

---

## 2. Git Data Flow (Repos → LOC / Commits)

```mermaid
flowchart TD
    subgraph Sources_["Git Repos"]
        REPO["repos_dir/*/.git\nuser repos"]
    end

    subgraph Scan["Incremental Scan (data.rs)"]
        LATEST["latest_commit_ts()\nSELECT MAX(ts)\nFROM commit_stats"]
        SCAN["collect_new_commits()\nfor each repo:\ngit log --after={ts} --numstat\nparse tab-separated output"]
        DECIDE{"empty?"}
    end

    subgraph Insert_
        INSERT["insert_commits(repo, commits)\nINSERT INTO commit_stats\n(repo, sha, ts, msg,\n added, deleted, file_ct)\nON CONFLICT DO NOTHING"]
        CS["commit_stats table\nper-commit rows\npersisted across restarts"]
    end

    subgraph Query_["Git Serving Queries"]
        QBUCK["query_commit_buckets()\nSELECT SUM(added), SUM(deleted)\nFLOOR((ts-lo)/span*n) AS bucket\nGROUP BY bucket\nORDER BY bucket\n→ Vec&lt;(i64,i64)&gt;"]
        QTOT["query_commit_totals()\nSELECT SUM(added), SUM(deleted)\nFROM commit_stats\nWHERE ts >= ?::TIMESTAMP\n→ (loc_added, loc_deleted)"]
    end

    subgraph Merge_
        ONER["build_one_range()\nmerges git + usage data\n→ RangeResult"]
    end

    REPO --> SCAN
    CS --> LATEST
    LATEST --> DECIDE
    DECIDE -->|empty →| SCAN
    DECIDE -->|has ts →| SCAN
    SCAN -->|Vec&lt;RepoCommits&gt;| INSERT
    INSERT --> CS
    CS --> QBUCK
    CS --> QTOT
    QBUCK --> ONER
    QTOT --> ONER
    ONER -->|into AllStats| SHARED

    subgraph SHARED["SharedStats (Arc&lt;RwLock&gt;)"]
    end

    style Sources_ fill:#333,color:#fff
    style Scan fill:#445,color:#fff
    style Insert_ fill:#C0C0C0,color:#000
    style Query_ fill:#556,color:#fff
    style Merge_ fill:#445,color:#fff
```

**Detailed path:**

**Key properties:**
- **Incremental** — past commits are never re-scanned, only `git log --after=` the latest known timestamp
- **Typical cycle** — 0–5 new commits, <100ms total
- **Cold start** — first scan goes back `git_history_days` (default 200 days, configurable)
- **No subprocess state** — each cycle starts fresh `git log` invocations, no persistent git daemon

---

## 3. LLM Summary Flow (Commits → Summary Panel)

```
  After git insert completes in data loop:
        │
        ▼
  For each repo with new commits:
  ├── get_repo_summary(repo) → cached (last_summary_sha, highlights_json)
  ├── compare to new head_sha
  └── if changed:
        │
        ▼
  summarize_one_repo(api_key, endpoint, model, repo, commit_msgs)
  ├── calls LLM HTTP API (DeepSeek / OpenAI-compatible)
  ├── parses structured highlights from response
  └── returns Vec<String> of bullet-point highlights
        │
        ▼
  save_repo_summary(repo, head_sha, highlights_json, model)
  ├── INSERT OR REPLACE INTO repo_summaries
  └── (repo_path TEXT PK, last_commit_sha TEXT, highlights TEXT, ...)
        │
        ▼
  build_summary_data(store, week_utc_str, day_utc_str)
  ├── all_repos_with_commits() → repos that appear in commit_stats
  ├── all_summarized_repos() → cached highlights keyed by repo
  ├── count_repo_commits_since(repo, ...) → commits today/this-week per repo
  ├── repo_commit_messages_since(repo, ...) → messages for PR extraction
  ├── extract_prs(msgs) → regex (#123) references
  └── packs into SummaryData {
        week_repos, day_repos,   // Vec<RepoSummary { name, commits, prs, highlights }>
        week_repo_count, day_repo_count,
        week_commits, day_commits,
        week_prs, day_prs,
        loading,                  // true if any repo has commits but no highlights yet
        no_api_key,
      }
        │
        ├──►  write to SharedSummary (Arc<RwLock<SummaryData>>)
        └──►  emit Tauri event "summary-update" with payload
                │
                ▼
  Frontend:
  ├── useSummaryQuery():
  │   - initial fetch: invoke("get_summary")
  │   - on "summary-update" event: queryClient.setQueryData(["summary"], payload)
  │   - fallback polling: refetchInterval 10s
  └── SummaryPanel renders repo cards with highlights + PR badges
```

**Key properties:**
- **On-change only** — LLM is called only when a repo's `head_sha` changes
- **Cached** — highlights persist in `repo_summaries` table across restarts
- **Dual delivery** — summary data is written to both shared state (for polling) and a Tauri event (for instant push)
- **User-configurable** — API key, endpoint, model are in settings
- **No blocking** — LLM calls are made sequentially in the data loop thread, never on the UI thread

---

## 4. Config / Theme Flow (Disk → Runtime)

```
  settings.json          theme.yaml
  ~/.config/loc-dock/    ~/.config/loc-dock/
        │                     │
        ▼                     ▼
  Config::load()          Theme::load()
  ├── reads settings.json │   ├── reads theme.yaml
  │   (or migrates .env)  │   └── fills defaults for missing keys
  └── loads LiteLLM pricing JSON  │
        │                 │
        ▼                 ▼
  Arc<RwLock<Config>>     Tauri managed state<Theme>
  (shared across threads) (Tauri commands)
        │                     │
        ▼                     ▼
  Data loop re-reads      get_theme command
  config each cycle via   reads Theme state
  read_config() closure         │
        │                       ▼
        │                  Frontend:
  Changes picked up        useThemeQuery()
  without restart:         → applyTheme(theme)
  - API key, model         → CSS variables set on :root
  - endpoint, timezone            │
  - repos_dir, refresh_interval   ▼
                            Chart reads theme prop
                            (colors, axis styles, bar colors)
```

**Hot-reload path (save_settings):**
```
  SettingsPanel "Save" button
        │
        ▼
  invoke("save_settings", { ... })
        │
        ▼
  Rust: writes settings.json to disk
        │
        ▼
  Config::load() → fresh Config struct
        │
        ▼
  *shared = Arc::new(RwLock::new(fresh_config))
        │
        ▼
  Data loop reads new config on next cycle
```

---

## 5. Frontend Data Flow (Tauri IPC → Pixels)

```mermaid
flowchart TD
    subgraph Duck["DuckDB (on disk)"]
        DA["daily_aggregates\nSUM(cost, tokens, LOC)\nper date × source"]
        ENT["entries\nraw session rows\nnon-additive queries"]
        CS["commit_stats\ngit LOC per commit"]
        RS["repo_summaries\ncached LLM highlights"]
    end

    subgraph Rust_["Rust Backend"]
        ALLSTATS["SharedStats\nArc&lt;RwLock&lt;AllStats&gt;&gt;"]
        SUMM["SharedSummary\nArc&lt;RwLock&lt;SummaryData&gt;&gt;"]
        THEME["Tauri State\n&lt;Theme&gt;"]
        DL["Data loop\nbuilds AllStats\n+ SummaryData"]
    end

    subgraph Commands_["Tauri Commands (commands.rs)"]
        GET_S["get_stats()\nread SharedStats\n→ AllStats"]
        GET_SUM["get_summary()\nread SharedSummary\n→ SummaryData"]
        GET_T["get_theme()\nread Theme state\n→ Theme"]
        GET_SET["get_settings()\nread Config\n→ Settings"]
    end

    subgraph TQ["TanStack Query Layer (src/hooks/queries.ts)"]
        Q1["useStatsQuery()\nqueryKey: ['stats']\nrefetchInterval: 10s"]
        Q2["useSummaryQuery()\nqueryKey: ['summary']\nrefetchInterval: 10s\n+ summary-update event"]
        Q3["useThemeQuery()\nqueryKey: ['theme']\nstaleTime: Infinity"]
    end

    subgraph ZS["Zustand Store (src/lib/store.ts)"]
        ZS1["range · mode\ntooltipVisible\nsettingsOpen\nsummaryOpen\nhideNoPrs"]
    end

    subgraph App_["App.tsx"]
        A["currentStats = stats?.[range]\nready = !isLoading\ntheme = theme ?? DEFAULT"]
    end

    subgraph Components_["React Components"]
        TR["TopRow\nprops: stats, ready\nshows LOC, cost, sessions"]
        CH["Chart\nprops: stats, mode,\nrange, theme\nCanvas 2D draw*"]
        BR["BottomRow\nprops: tokens\nshows IN/OUT/CW/CR"]
        SP["SummaryPanel\nprops: summary, range\nrepo cards + highlights"]
        CT["CostTooltip\nprops: breakdown\nhover cost detail"]
    end

    DA --> DL
    ENT --> DL
    CS --> DL
    RS --> DL
    DL --> ALLSTATS
    DL --> SUMM

    ALLSTATS ---->|read| GET_S
    SUMM ---->|read| GET_SUM
    THEME ---->|read| GET_T

    GET_S -->|invoke("get_stats")| Q1
    GET_SUM -->|invoke("get_summary")| Q2
    GET_T -->|invoke("get_theme")| Q3

    Q1 -->|data: AllStats, isLoading| A
    Q2 -->|data: SummaryData| A
    Q3 -->|data: Theme| A
    ZS1 -->|range, mode| A

    A -->|stats[range], ready| TR
    A -->|stats, mode, range, theme| CH
    A -->|stats[range].tokens| BR
    A -->|summary| SP
    A -->|stats[range].cost_breakdown| CT

    style Duck fill:#FFF8DC,color:#000
    style Rust_ fill:#444,color:#fff
    style Commands_ fill:#555,color:#fff
    style TQ fill:#FF4154,color:#fff
    style ZS fill:#443E38,color:#fff
    style App_ fill:#2d3748,color:#fff
    style Components_ fill:#2d3748,color:#fff
```

### Polling lifecycle

```mermaid
sequenceDiagram
    participant R as React App
    participant TQ as TanStack Query
    participant IPC as Tauri IPC
    participant CMD as get_stats (Rust)
    participant SS as SharedStats (RwLock)

    Note over R,SS: Startup (warm start)
    R->>TQ: useStatsQuery() mount
    TQ->>IPC: invoke("get_stats")
    IPC->>CMD: deserialize + execute
    CMD->>SS: read()
    SS-->>CMD: AllStats (prefilled from DB)
    CMD-->>IPC: AllStats JSON
    IPC-->>TQ: resolve
    TQ-->>R: { data: AllStats, isLoading: false }
    R->>R: render with data (first paint <50ms)

    Note over R,SS: Every 10s background poll
    TQ->>IPC: invoke("get_stats")
    Note over R: stale data still visible
    IPC->>CMD: execute
    CMD->>SS: read()
    SS-->>CMD: AllStats (updated by data loop)
    CMD-->>IPC: AllStats JSON
    IPC-->>TQ: resolve
    TQ-->>R: { data: AllStats, isFetching: false }
    R->>R: re-render with fresh data

    Note over R,SS: Startup (cold start)
    R->>TQ: useStatsQuery() mount
    TQ-->>R: { data: undefined, isLoading: true }
    R->>R: render spinner + "--"
    TQ->>IPC: invoke("get_stats")
    IPC->>CMD: execute
    CMD->>SS: read()
    SS-->>CMD: AllStats (ready: false, zeros)
    CMD-->>IPC: AllStats JSON
    IPC-->>TQ: resolve
    TQ-->>R: { data: AllStats, isLoading: false }
    R->>R: render spinner (ready=false)

    Note over R,SS: ...data loop completes...
    TQ->>IPC: invoke("get_stats") (10s poll)
    CMD->>SS: read()
    SS-->>CMD: AllStats (ready: true, real data)
    CMD-->>IPC: AllStats JSON
    IPC-->>TQ: resolve
    TQ-->>R: { data: AllStats, isLoading: false }
    R->>R: render real data
```

### Query hooks API

| Hook | Key | Polling | Event-driven | Used by |
|------|-----|---------|-------------|---------|
| `useStatsQuery()` | `['stats']` | 10s | — | TopRow, Chart, BottomRow, CostTooltip |
| `useSummaryQuery()` | `['summary']` | 10s | `summary-update` event updates cache | SummaryPanel |
| `useThemeQuery()` | `['theme']` | never (staleTime: Infinity) | — | Chart (colors), all via CSS variables |

### Key properties

- **Stale-while-revalidate** — during background refetches, the stale data stays visible. No flash.
- **isLoading vs isFetching** — `isLoading` means no data yet (first fetch). `isFetching` means background refresh (stale data still displayed).
- **No backend `ready` flag needed** — TanStack Query's `isLoading` replaces the `AllStats.ready` boolean for UI loading states.
- **Event-driven summary updates** — the `summary-update` Tauri event calls `queryClient.setQueryData()` to push live summary data into the cache without waiting for the next poll.

---

## Data Flow Summary Table

| What | Source | DuckDB Tables | Rust | Frontend Hook | Component |
|------|--------|---------------|------|---------------|-----------|
| LOC added/deleted | `git log --numstat` | `commit_stats` | `query_commit_totals()` | `useStatsQuery` | TopRow, Chart (LOC mode) |
| Git timeline | `git log --numstat` | `commit_stats` | `query_commit_buckets()` | `useStatsQuery` | Chart (LOC mode) |
| Token counts | JSONL files | `entries` → `daily_aggregates` | `query_aggregates()` | `useStatsQuery` | BottomRow |
| Cost estimate | JSONL files | `entries` → `daily_aggregates` | `query_aggregates()` | `useStatsQuery` | TopRow, CostTooltip |
| Cost timeline | JSONL files | `entries` | `query_cost_buckets()` | `useStatsQuery` | Chart (COST mode) |
| Session counts | JSONL files | `entries` | `count_sessions()` | `useStatsQuery` | TopRow |
| AI summaries | commit messages | `repo_summaries` | `build_summary_data()` | `useSummaryQuery` | SummaryPanel |
| Theme colors | `theme.yaml` | — | `Theme::load()` | `useThemeQuery` | Chart, all components |
| Settings | `settings.json` | — | `Config::load()` | `invoke("get_settings")` | SettingsPanel |
