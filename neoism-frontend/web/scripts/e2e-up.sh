#!/usr/bin/env bash
# Spin up the daemon + vite stack the E2E smoke harness needs.
#
# Usage:
#   scripts/e2e-up.sh            # foreground; ctrl-c to tear down
#   scripts/e2e-up.sh --bg       # background mode; writes PID file
#   scripts/e2e-up.sh --down     # stop the background stack
#   scripts/e2e-up.sh --rebuild  # cargo build daemon before starting
#
# Layout:
#   - daemon listens on $NEOISM_DAEMON_ADDR (default 127.0.0.1:7878)
#   - vite listens on http://127.0.0.1:5173/
#   - PID file: /tmp/neoism-e2e.pids
#   - logs:     /tmp/neoism-e2e-daemon.log, /tmp/neoism-e2e-vite.log
#
# The daemon must already be built (or pass --rebuild). Tests assume
# trust-local mode (no NEOISM_REQUIRE_AUTH) because the harness has no
# pairing token to ship.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WEB_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_ROOT="$(cd "$WEB_DIR/../.." && pwd)"
PID_FILE="${NEOISM_E2E_PID_FILE:-/tmp/neoism-e2e.pids}"
DAEMON_LOG="${NEOISM_E2E_DAEMON_LOG:-/tmp/neoism-e2e-daemon.log}"
VITE_LOG="${NEOISM_E2E_VITE_LOG:-/tmp/neoism-e2e-vite.log}"
DAEMON_ADDR="${NEOISM_DAEMON_ADDR:-127.0.0.1:7878}"
DAEMON_BIN="$REPO_ROOT/target/debug/neoism-workspace-daemon"

mode="fg"
rebuild=0
for arg in "$@"; do
  case "$arg" in
    --bg) mode="bg" ;;
    --fg) mode="fg" ;;
    --down) mode="down" ;;
    --rebuild) rebuild=1 ;;
    -h|--help)
      sed -n '2,18p' "$0"
      exit 0
      ;;
    *)
      echo "unknown arg: $arg" >&2
      exit 2
      ;;
  esac
done

if [[ "$mode" == "down" ]]; then
  if [[ ! -f "$PID_FILE" ]]; then
    echo "no pid file at $PID_FILE — nothing to stop" >&2
    exit 0
  fi
  while read -r pid; do
    [[ -z "$pid" ]] && continue
    if kill -0 "$pid" 2>/dev/null; then
      echo "stopping pid $pid"
      kill "$pid" 2>/dev/null || true
    fi
  done < "$PID_FILE"
  rm -f "$PID_FILE"
  exit 0
fi

if [[ "$rebuild" == "1" ]] || [[ ! -x "$DAEMON_BIN" ]]; then
  echo "building daemon..."
  ( cd "$REPO_ROOT" && cargo build -p neoism-workspace-daemon )
fi

if [[ ! -x "$DAEMON_BIN" ]]; then
  echo "daemon binary missing at $DAEMON_BIN — rerun with --rebuild" >&2
  exit 1
fi

# Use a throwaway HOME to keep the harness from polluting the real
# user's auth + workspace state (~/.config/neoism, ~/.local/share/neoism).
ISOLATED_HOME="${NEOISM_E2E_HOME:-/tmp/neoism-e2e-home}"
mkdir -p "$ISOLATED_HOME"

echo "starting daemon -> $DAEMON_LOG ($DAEMON_ADDR, HOME=$ISOLATED_HOME)"
HOME="$ISOLATED_HOME" \
NEOISM_DAEMON_ADDR="$DAEMON_ADDR" \
RUST_LOG="${RUST_LOG:-info,neoism_workspace_daemon=debug}" \
  "$DAEMON_BIN" >"$DAEMON_LOG" 2>&1 &
daemon_pid=$!

# Wait for daemon to bind the port (max 10s).
for _ in $(seq 1 50); do
  if ss -ltn "src $DAEMON_ADDR" 2>/dev/null | grep -q LISTEN; then
    break
  fi
  if ! kill -0 "$daemon_pid" 2>/dev/null; then
    echo "daemon exited before binding — see $DAEMON_LOG" >&2
    tail -20 "$DAEMON_LOG" >&2 || true
    exit 1
  fi
  sleep 0.2
done

echo "starting vite -> $VITE_LOG"
( cd "$WEB_DIR" && npm run dev ) >"$VITE_LOG" 2>&1 &
vite_pid=$!

# Wait for vite to be reachable (max 30s).
for _ in $(seq 1 150); do
  if curl -sf -o /dev/null "http://localhost:5173/"; then
    break
  fi
  if ! kill -0 "$vite_pid" 2>/dev/null; then
    echo "vite exited before serving — see $VITE_LOG" >&2
    tail -20 "$VITE_LOG" >&2 || true
    kill "$daemon_pid" 2>/dev/null || true
    exit 1
  fi
  sleep 0.2
done

printf "%s\n%s\n" "$daemon_pid" "$vite_pid" > "$PID_FILE"
echo "ready — daemon pid=$daemon_pid vite pid=$vite_pid (pidfile: $PID_FILE)"

if [[ "$mode" == "bg" ]]; then
  exit 0
fi

trap 'echo "tearing down"; kill "$daemon_pid" "$vite_pid" 2>/dev/null || true; rm -f "$PID_FILE"' INT TERM EXIT
wait
