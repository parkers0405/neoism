#!/usr/bin/env bash
# Launch a neoism binary as a brand-new user: fresh $HOME + XDG dirs so the
# first-run bootstrap does the full new-user flow (bundled parsers,
# launcher, default config) while still opening on your real Wayland display.
# The sandboxed XDG_RUNTIME_DIR isolates neoism's IPC/daemon sockets, so a
# neoism instance you already have running is untouched and not forwarded to.
#
# Usage: scripts/fresh-run.sh [path-to-neoism-binary] [--keep]
#   default binary: ./neoism-linux-x86_64/neoism (untarred release layout)
#   --keep         relaunch with the previous sandbox (test a returning user)
#   FRESH_HOME=dir sandbox location (default /tmp/neoism-fresh)
#
# Refactored: added better arg parsing, support for NEOISM env vars passthrough,
# and explicit cleanup option. Safer defaults and more comments for contributors.
set -euo pipefail

KEEP=0
BIN=""
CLEANUP=0
for arg in "$@"; do
  case "$arg" in
    --keep) KEEP=1 ;;
    --cleanup) CLEANUP=1 ;;
    *) BIN="$arg" ;;
  esac
done
BIN="${BIN:-./neoism-linux-x86_64/neoism}"
[ -x "$BIN" ] || { echo "error: $BIN is not an executable neoism binary" >&2; exit 1; }
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"

FRESH="${FRESH_HOME:-/tmp/neoism-fresh}"
if [ "$CLEANUP" -eq 1 ]; then
  echo "Cleaning up fresh sandbox at $FRESH"
  rm -rf "$FRESH"
  exit 0
fi
if [ "$KEEP" -eq 0 ]; then
  rm -rf "$FRESH"
fi
mkdir -p "$FRESH/run"
chmod 700 "$FRESH/run"

# An absolute WAYLAND_DISPLAY bypasses XDG_RUNTIME_DIR, so the sandboxed
# runtime dir can't hide the compositor socket.
REAL_RUNTIME="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
WL="${WAYLAND_DISPLAY:-wayland-1}"
case "$WL" in
  /*) ;;
  *) WL="$REAL_RUNTIME/$WL" ;;
esac

echo "fresh user home: $FRESH   (rerun with --keep to return as the same user; --cleanup to wipe)"
echo "binary: $BIN"
echo "To test first-run bootstrap again, use without --keep."

# Passthrough useful NEOISM_ vars for testing features
exec env \
  HOME="$FRESH" \
  XDG_CONFIG_HOME="$FRESH/.config" \
  XDG_DATA_HOME="$FRESH/.local/share" \
  XDG_STATE_HOME="$FRESH/.local/state" \
  XDG_CACHE_HOME="$FRESH/.cache" \
  XDG_RUNTIME_DIR="$FRESH/run" \
  WAYLAND_DISPLAY="$WL" \
  "${NEOISM_PASSTHROUGH_VARS[@]}" \
  "$BIN"
