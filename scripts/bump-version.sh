#!/usr/bin/env bash
set -euo pipefail

if [ -z "${1:-}" ]; then
  echo "Usage: ./scripts/bump-version.sh <version>"
  echo "Example: ./scripts/bump-version.sh 0.2.0"
  exit 1
fi

VERSION="$1"
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TAURI_DIR="$REPO_ROOT/loc-dock-tauri"

# package.json
cd "$TAURI_DIR"
npm version "$VERSION" --no-git-tag-version

# Cargo.toml — update only the package version (first occurrence)
sed -i "0,/^version = \".*\"/s//version = \"$VERSION\"/" "$TAURI_DIR/src-tauri/Cargo.toml"

# tauri.conf.json
sed -i "s/\"version\": \".*\"/\"version\": \"$VERSION\"/" "$TAURI_DIR/src-tauri/tauri.conf.json"

echo "Version bumped to $VERSION"
echo ""
echo "Next steps:"
echo "  git add -A && git commit -m 'chore: bump version to $VERSION'"
echo "  git tag v$VERSION"
echo "  git push origin master --tags"
