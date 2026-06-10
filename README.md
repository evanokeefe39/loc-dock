# LOC Dock

Floating desktop widget that tracks your daily dev metrics at a glance.

![screenshot](screenshot.png)

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Tauri](https://img.shields.io/badge/tauri-v2-24C8D8?logo=tauri&logoColor=white)](https://tauri.app)
[![React](https://img.shields.io/badge/react-19-61DAFB?logo=react&logoColor=white)](https://react.dev)
[![Rust](https://img.shields.io/badge/rust-2021-DEA584?logo=rust&logoColor=white)](https://www.rust-lang.org)
[![DuckDB](https://img.shields.io/badge/duckdb-bundled-FFF000?logo=duckdb&logoColor=black)](https://duckdb.org)
![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20Linux%20%7C%20macOS-blue)

## What it shows

- **LOC changes** -- lines added/deleted across all git repos today
- **Token usage** -- Claude Code input, output, cache write, cache read totals
- **Cost estimate** -- dollar cost breakdown with hover tooltip
- **Session counts** -- active and today's Claude Code sessions

Three chart modes (click to toggle):
- **LOC** -- stacked bar chart of additions (green) and deletions (red)
- **COST** -- cost over time (purple)
- **TOKENS** -- stacked token types: IN (white), OUT (pink), CW (yellow), CR (blue)

## Features

- Always-on-top, borderless, semi-transparent
- Draggable with snap-to-corner (pin menu for all 4 corners)
- Resizable -- drag edges/corners to resize, responsive layout collapses at small sizes
- Day/Week toggle for all stats
- System tray with Show/Hide/Quit
- Settings panel (cog icon) for repos dir, claude dir, timezone, day/week start
- Customizable theme via YAML
- Auto-creates default theme on first launch

## Requirements

- [Node.js](https://nodejs.org) 18+
- [Rust](https://rustup.rs) 1.70+
- Git on PATH

## Quick start

```bash
git clone https://github.com/evanokeefe39/loc-dock.git
cd loc-dock/loc-dock-tauri
npm install
npm run tauri dev
```

First build takes 3-5 minutes (DuckDB compiles from source). Subsequent builds are fast.

## Build for production

```bash
npm run tauri build
```

Produces a native installer in `src-tauri/target/release/bundle/`.

## Configuration

Settings are accessible via the cog icon in the widget, or by editing `~/.config/loc-dock/.env`:

| Variable | Default | Description |
|---|---|---|
| `LOCDOCK_REPOS_DIR` | `~/repos` | Directory containing your git repositories |
| `LOCDOCK_CLAUDE_DIR` | `~/.claude` | Claude Code data directory |
| `LOCDOCK_TIMEZONE` | `Europe/Berlin` | IANA timezone for day boundary |
| `LOCDOCK_DAY_START_HOUR` | `7` | Hour when "today" starts (24h) |
| `LOCDOCK_WEEK_START_DAY` | `0` | Day week starts (0=Mon, 6=Sun) |
| `LOCDOCK_THEME_PATH` | `~/.config/loc-dock/theme.yaml` | Path to theme file |

## Theming

Edit `~/.config/loc-dock/theme.yaml` to customize colors and transparency. A default theme is created automatically on first launch. All fields are optional.

```yaml
alpha: 0.92              # window transparency (0.0-1.0)
bg: "#202020"            # main background
chart_bg: "#181818"      # chart background
tooltip_bg: "#2a2a2a"    # cost tooltip background
text: "#e0e0e0"          # primary text
text_dim: "#6b7280"      # secondary text, labels
axis: "#333333"          # chart axis lines
loc_add: "#34d399"       # lines added (green)
loc_del: "#ef4444"       # lines deleted (red)
cost: "#a78bfa"          # cost label and chart (purple)
sessions: "#f97316"      # active sessions (orange)
tok_input: "#e0e0e0"     # input tokens
tok_output: "#f472b6"    # output tokens (pink)
tok_cache_write: "#facc15"  # cache write (yellow)
tok_cache_read: "#38bdf8"   # cache read (blue)
```

You can maintain multiple theme files and switch between them via the settings panel.

## Architecture

```
Rust backend (Tauri)          React frontend
  config.rs                     App.tsx
  theme.rs                      TopRow.tsx
  git.rs ── git log ──>         Chart.tsx (canvas)
  usage_store.rs ── DuckDB ──>  BottomRow.tsx
  data.rs ── 60s loop ──>       CostTooltip.tsx
  commands.rs                   SettingsPanel.tsx
  tray.rs                       ResizeBorders.tsx
```

The Rust backend owns all data: git subprocess scanning, DuckDB queries, stat precomputation. It pushes a single `AllStats` JSON blob to the frontend every 60 seconds via Tauri events. The React frontend is a pure renderer with zero data logic.

## How it works

1. Scans all git repos in `REPOS_DIR` for commits since day/week start
2. Reads Claude Code JSONL session files via DuckDB with message-level deduplication
3. Precomputes all stats (tokens, cost, sessions, chart buckets) for both day and week ranges
4. Pushes updates to the frontend every 60 seconds
5. Frontend picks the active range and renders -- no queries on toggle

## License

[MIT](LICENSE)
