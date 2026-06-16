# CLAUDE.md

Canonical project and architecture docs live in [AGENTS.md](AGENTS.md), imported below so
Claude Code loads it (Claude Code reads only CLAUDE.md; Pi reads both directly). This file
adds Claude/process specifics on top — keep architecture in AGENTS.md to avoid drift.

@AGENTS.md

## Project

LOC Dock — floating desktop widget tracking daily dev metrics (LOC, tokens, cost, sessions). Tauri v2 + React + Rust + DuckDB.

## Build

```bash
cd loc-dock-tauri
npm install
npm run tauri dev      # dev
npm run tauri build    # release (NSIS installer)
```

First build is slow (DuckDB compiles from source). Subsequent builds are fast.

## Branching

- `master` is protected. No direct commits.
- All work on short-lived branches: `feat/`, `fix/`, `chore/`
- PR into master. Squash-merge or rebase, ff-only.
- Release tags on master: `v0.1.0`, `v0.2.0`, etc.

## Releasing

```bash
./scripts/bump-version.sh 0.2.0
git commit -am "chore: bump version to 0.2.0"
git tag v0.2.0
git push origin master --tags
```

GitHub Actions builds installers for Windows/macOS/Linux and creates a draft release.

## Layout

Architecture and the per-file map are in [AGENTS.md](AGENTS.md). Top-level dirs:
- `loc-dock-tauri/src-tauri/src/` — Rust backend
- `loc-dock-tauri/src/` — React frontend
- `loc-dock-tauri/src-tauri/tauri.conf.json` — Tauri config
- `docs/` — design records (V2 refactor plan, spike results)

## Config

Settings stored in `~/.config/loc-dock/.env` (Windows: `%APPDATA%/loc-dock/.env`). Env vars prefixed `LOCDOCK_`.

## Issue Tracking

Local issues tracked in [ISSUES.md](ISSUES.md) at repo root. No external tracker. Add new
work there under **Open**; reference ids in commits as `#N`.

## Conventions

- Commit messages: conventional commits (`feat:`, `fix:`, `chore:`, `docs:`)
- Version in three places: `tauri.conf.json`, `Cargo.toml`, `package.json` — use bump script
- No code signing yet. Installers trigger SmartScreen/Gatekeeper warnings.
