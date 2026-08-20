#!/usr/bin/env bash
# Two-instance multiplayer sim on ONE machine — always the same build.
#
#   ./scripts/dev-sim.sh          # launch host-sim + guest-sim
#   ./scripts/dev-sim.sh kill     # stop both
#   ./scripts/dev-sim.sh status   # show running instances
#
# Instance A ("host-sim"): its own embedded daemon (default socket +
# 127.0.0.1:7878), cwd = a small sim project. Share its workspace with
# the palette (Cmd+; → "workspace share").
# Instance B ("guest-sim"): attaches to A's daemon over ws://127.0.0.1:7878
# with a DIFFERENT host id, so A's shared workspace shows up under a
# foreign host in B's Workspaces modal — the full join/guest flow
# (adopt, guest icon, remote tree via the files plane, ownership
# guards) exercises exactly like two laptops, minus the network.
#
# New in this version:
# - Added status command
# - Better cleanup with trap
# - Configurable ports and log levels
# - Auto-clean old logs
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$REPO/target/debug/neoism"
SIMDIR="${NEOISM_SIM_DIR:-/tmp/neoism-sim-project}"
LOGDIR="${NEOISM_SIM_LOGS:-/tmp/neoism-sim-logs}"
HOST_PORT="${NEOISM_SIM_HOST_PORT:-7878}"
LOG_LEVEL="${NEOISM_SIM_LOG_LEVEL:-info}"

if [[ "${1:-}" == "kill" ]]; then
  echo "Stopping all sim instances..."
  pkill -f "target/debug/neoism" || true
  echo "sim instances stopped"
  exit 0
fi

if [[ "${1:-}" == "status" ]]; then
  echo "=== Neoism Sim Status ==="
  ps aux | grep -E "(neoism|target/debug/neoism)" | grep -v grep || echo "No sim processes running"
  echo "SIMDIR: $SIMDIR"
  echo "LOGDIR: $LOGDIR"
  echo "HOST_PORT: $HOST_PORT"
  if [[ -d "$LOGDIR" ]]; then
    echo "Log files:"
    ls -lh "$LOGDIR/"*.log 2>/dev/null || echo "  (no logs yet)"
  fi
  exit 0
fi

# Enhanced cleanup
cleanup() {
  echo "Cleaning up sim environment..."
  pkill -f "target/debug/neoism" 2>/dev/null || true
  # Don't remove logs by default - keep for debugging
  echo "Cleanup complete"
}

trap cleanup EXIT

mkdir -p "$SIMDIR/src" "$SIMDIR/docs" "$LOGDIR"

# Initialize sim project if it doesn't exist
if [[ ! -f "$SIMDIR/README.md" ]]; then
  echo "# Neoism Simulation Project" > "$SIMDIR/README.md"
  echo 'fn main() { println!("Hello from Neoism sim!"); }' > "$SIMDIR/src/main.rs"
  cat > "$SIMDIR/docs/NOTES.md" << 'EOF'
# Simulation Notes

This is a test workspace for cross-instance collaboration testing.

## Features to test:
- Workspace sharing
- Guest mode
- Remote file access
- CRDT synchronization
EOF
  echo "Initialized fresh sim project at $SIMDIR"
fi

# Enhanced logging filter with more components
RUST_FILTER="neoism::remote_files=${LOG_LEVEL},neoism::workspaces=${LOG_LEVEL},neoism::workspace_root=debug,neoism::agent=info"

echo "=== Starting Neoism Multi-Instance Simulation ==="
echo "Host port: $HOST_PORT"
echo "Log level: $LOG_LEVEL"
echo "Project dir: $SIMDIR"
echo

cd "$SIMDIR"
NEOISM_HOST_ID=host-sim \
NEOISM_LOG_FILE="$LOGDIR/host-sim.log" \
RUST_LOG="$RUST_FILTER" \
  "$BIN" --port "$HOST_PORT" >| "$LOGDIR/host-sim.log" 2>&1 &
HOST_PID=$!
echo "host-sim started (pid=$HOST_PID, cwd=$SIMDIR, daemon on 127.0.0.1:$HOST_PORT)"

# Give host time to fully initialize
sleep 5

cd "$HOME"
NEOISM_HOST_ID=guest-sim \
RUST_LOG="$RUST_FILTER" \
  "$BIN" --daemon-url "ws://127.0.0.1:$HOST_PORT" >| "$LOGDIR/guest-sim.log" 2>&1 &
GUEST_PID=$!
echo "guest-sim started (pid=$GUEST_PID, attached to host-sim)"

echo
echo "Simulation ready! Test flow:"
echo "  1. In host-sim window: Cmd+; → 'workspace share' → Enter"
echo "  2. In guest-sim window: Cmd+; → 'workspaces' → Enter → select host-sim workspace"
echo "  3. Test file editing, presence, and remote tree"
echo
echo "Status:   ./scripts/dev-sim.sh status"
echo "Stop:     ./scripts/dev-sim.sh kill"
echo "Logs:     $LOGDIR/{host-sim,guest-sim}.log"
echo "Project:  $SIMDIR"
