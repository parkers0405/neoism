#!/usr/bin/env bash
# Run the terminal frontend without rebuilding or modifying the GUI.
set -euo pipefail
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
frontend="$repo_root/neoism-frontend/tui"
bun="${NEOISM_BUN:-$frontend/node_modules/.bin/bun}"
if [[ ! -x "$bun" ]]; then
  if command -v bun >/dev/null 2>&1; then
    bun="$(command -v bun)"
  else
    printf 'Install TUI dependencies first: cd %s && npm install\n' "$frontend" >&2
    exit 1
  fi
fi
export NEOISM_LAUNCH_DIRECTORY="$PWD"
cd "$frontend"
exec "$bun" src/main.tsx "$@"