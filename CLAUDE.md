# CLAUDE.md

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

## Architecture

Rust backend owns all data. React frontend is a pure renderer. Backend pushes `AllStats` JSON every 60s. Frontend has zero data logic.

Key dirs:
- `loc-dock-tauri/src-tauri/src/` — Rust backend
- `loc-dock-tauri/src/` — React frontend
- `loc-dock-tauri/src-tauri/tauri.conf.json` — Tauri config

## Config

Settings stored in `~/.config/loc-dock/.env` (Windows: `%APPDATA%/loc-dock/.env`). Env vars prefixed `LOCDOCK_`.

## Issue Tracking

Local issues tracked in `issues.md` at repo root. No external tracker.

## Conventions

- Commit messages: conventional commits (`feat:`, `fix:`, `chore:`, `docs:`)
- Version in three places: `tauri.conf.json`, `Cargo.toml`, `package.json` — use bump script
- No code signing yet. Installers trigger SmartScreen/Gatekeeper warnings.
