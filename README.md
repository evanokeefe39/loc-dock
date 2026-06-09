# LOC Dock

Floating desktop widget that tracks your daily dev metrics at a glance.

![screenshot](screenshot.png)

![platform](https://img.shields.io/badge/platform-Windows%20%7C%20Linux%20%7C%20macOS-blue)

## What it shows

- **LOC changes** — lines added/deleted across all git repos today
- **Token usage** — Claude Code input, output, cache write, cache read totals
- **Cost estimate** — dollar cost breakdown with hover tooltip
- **Session counts** — active and today's Claude Code sessions

Three chart modes (click to toggle):
- **LOC** — stacked bar chart of additions (green) and deletions (red)
- **COST** — cost over time (purple)
- **TOKENS** — stacked token types: IN (white), OUT (pink), CW (yellow), CR (blue)

## Requirements

- Python 3.10+
- [uv](https://docs.astral.sh/uv/) (handles dependencies automatically)
- tkinter (included with most Python installations)
- Git repos in a single directory

## Usage

```
uv run dock.py
```

Dependencies (`duckdb`, `tzdata`) are installed automatically by `uv`.

## Configuration

Set environment variables or copy `.env.example` to `.env` and edit:

| Variable | Default | Description |
|---|---|---|
| `LOCDOCK_REPOS_DIR` | `~/repos` | Directory containing your git repositories |
| `LOCDOCK_CLAUDE_DIR` | `~/.claude` | Claude Code data directory |
| `LOCDOCK_TIMEZONE` | `Europe/Berlin` | IANA timezone for day boundary |
| `LOCDOCK_DAY_START_HOUR` | `7` | Hour when "today" starts (24h) |

## How it works

- Scans all git repos in `LOCDOCK_REPOS_DIR` for commits since day start
- Reads Claude Code JSONL session files via DuckDB with message-level deduplication
- Data refreshes every 60s in a background thread; UI updates every 30s
- Draggable, always-on-top, positions bottom-right above the taskbar
