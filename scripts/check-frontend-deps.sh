#!/usr/bin/env bash
# Test the resolved graphs, including transitive dependencies.
set -euo pipefail
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"
check_graph() {
  local graph
  graph="$(bash "$repo_root/scripts/blaze.sh" tree --locked "$@" -e normal --prefix none --format '{p}')"
  if printf '%s\n' "$graph" | rg '^(sugarloaf|mlua|mlua-sys|lua-src|luajit-src|neoism-lua|neoism-terminal-core|neoism-window|neoism-notifier|neoism-proc-macros|rio|crosswords|copa) v'; then
    echo 'Retired frontend/runtime dependency found.' >&2
    return 1
  fi
}
check_graph --manifest-path neoism-frontend/gpuix/native/Cargo.toml
check_graph -p neoism-workspace-daemon
echo 'GPUIX and the daemon have no retired renderer, window host, VT emulator, or Lua runtime dependencies.'
