# Data Flows

How data moves from source files to the dock UI. Five independent pipelines,
all driven by the single data loop in `data.rs::spawn_data_loop`.

**Legend:**
```
SOURCE ──► DuckDB table ──► Rust struct ──► Tauri IPC ──► Frontend
```

---

## 1. Usage Data Flow (JSONL → Cost / Tokens / Sessions)

```
~/.claude/projects/**/*.jsonl   ◄── Claude Code writes session logs here
~/.pi/agent/sessions/*.jsonl    ◄── Pi writes session logs here
        │
        ▼
  SourceManager::with_discoverers
  ├── GlobFileDiscoverer (claude) — globs projects/subagents paths
  └── GlobFileDiscoverer (pi)     — globs sessions directory
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
  │                      │      │    - cost flat-priced per pricing.yaml
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

```
repos_dir/*/.git                  ◄── user's git repositories
        │
        │  incremental scan:
        │  latest_commit_ts() → MAX(ts) FROM commit_stats
        │  if empty → use git_history_days (default 200)
        │
        ▼
  collect_new_commits(repos_dir, since_iso)
  ├── for each repo dir: git log --after={since_iso} --numstat
  │   parses git's tab-separated numstat output
  └── returns Vec<RepoCommits>  (repo, head_sha, Vec<GitCommit>)
        │
        ▼
  insert_commits(repo, commits, head_sha)
  ├── INSERT INTO commit_stats (repo, sha, ts, msg, added, deleted, file_ct)
  │     ON CONFLICT (repo, sha) DO NOTHING
  └── commit_stats stores per-commit rows with LOC counts
        │
        ▼
  Serving queries (usage_store.rs):
  ├── query_commit_buckets(lo, hi, n_buckets)
  │     SELECT SUM(added), SUM(deleted) bucketed by ts
  │     → Vec<(i64,i64)>  per-bucket (added, deleted)
  │
  └── query_commit_totals(since_str)
        SELECT SUM(added), SUM(deleted) FROM commit_stats WHERE ts >= ?
        → (loc_added, loc_deleted)
             │
             ▼
  Included in build_one_range → build_all_stats → SharedStats
  (same path as usage data, merged into AllStats)
```

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
  └── loads pricing.yaml  │
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

```
  ┌──────────────────────────────────────────────────────────────────┐
  │  TanStack Query Client  (src/hooks/queries.ts)                   │
  │                                                                  │
  │  useStatsQuery():                                                │
  │    queryKey: ["stats"]                                           │
  │    queryFn: invoke<AllStats>("get_stats")                        │
  │    refetchInterval: 10_000                                       │
  │    → { data: AllStats | undefined, isLoading, isFetching }       │
  │                                                                  │
  │  useSummaryQuery():                                              │
  │    queryKey: ["summary"]                                         │
  │    queryFn: invoke<SummaryData>("get_summary")                   │
  │    refetchInterval: 10_000                                       │
  │    + event listener "summary-update" → setQueryData              │
  │    initialData: DEFAULT_SUMMARY                                  │
  │    → { data: SummaryData, isLoading, ... }                       │
  │                                                                  │
  │  useThemeQuery():                                                │
  │    queryKey: ["theme"]                                           │
  │    queryFn: invoke<Theme>("get_theme")                           │
  │    staleTime: Infinity  (one-time fetch)                         │
  │    → { data: Theme | undefined, ... }                            │
  └───────────────────┬──────────────────────────────────────────────┘
                      │
                      ▼
  ┌──────────────────────────────────────────────────────────────────┐
  │  Zustand Store  (src/lib/store.ts)                               │
  │                                                                  │
  │  State:         Toggles:                                         │
  │    range        toggleRange()   day↔week↔month↔year              │
  │    mode         toggleMode()    loc↔cost↔tokens                  │
  │    settingsOpen setSettingsOpen(bool)                            │
  │    summaryOpen  setSummaryOpen(bool)                             │
  │    tooltipVisible                                                │
  │    hideNoPrs                                                     │
  │                                                                  │
  │  No provider — any component calls useUIStore() directly         │
  └───────────────────┬──────────────────────────────────────────────┘
                      │
                      ▼
  ┌──────────────────────────────────────────────────────────────────┐
  │  App.tsx                                                         │
  │                                                                  │
  │  const { data: stats, isLoading } = useStatsQuery()              │
  │  const { data: summary } = useSummaryQuery()                     │
  │  const { data: theme } = useThemeQuery()                         │
  │  const { range, mode, ... } = useUIStore()                       │
  │                                                                  │
  │  currentStats = stats?.[range] ?? null                           │
  │  ready = !isLoading                                              │
  └───────┬───────────────┬──────────────┬───────────────────────────┘
          │               │              │
          ▼               ▼              ▼
  TopRow              Chart           BottomRow
  props:              props:          props:
  stats=currentStats  stats=stats?    tokens=currentStats?.tokens
  ready=!isLoading    mode=mode       (null when loading → "--")
  range=range         range=range
  mode=mode           theme=theme??
                      │
                      ▼
              Canvas 2D drawing
              (chart.ts):
              drawLocChart()
              drawCostChart()
              drawTokenChart()
```

**First-paint timing (warm start with cached data):**

```
  0ms    App mounts → TanStack Query fires invoke("get_stats")
  0-5ms  Rust prefill already set SharedStats from daily_aggregates
  5ms    First invoke returns → React renders Chart + TopRow with data
  5-15s  Data loop git scan + ETL + aggregates refresh
  15s+   TanStack Query refetch → new AllStats with latest data
```

**First-paint timing (cold start, no cached data):**

```
  0ms    App mounts → isInitialized() = false → ready = false
  0ms    Chart shows spinner, TopRow shows --
  5-50ms Prefill completes with empty data
  50ms   TanStack Query first invoke returns (ready=false)
         → Chart keeps spinner, TopRow keeps --
  5-15s  First data loop cycle: git scan + ETL + aggregates
  15s+   TanStack Query refetch → AllStats with ready=true
         → Chart draws data, TopRow shows real numbers
```

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
