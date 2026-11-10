#!/bin/bash
# Render the terminal-ownership, block-action and dashboard scenes to PNGs (headless).
# Usage: rune/scripts/snap-terminal-scenes.sh <binary> <out-dir>
set -e
# A worktree has no built Ghostty artifact of its own: fall back to the main checkout's.
if [ -z "$NEOISM_GHOSTTY_VT" ]; then
  root="$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null)/.."
  [ -f "$root/neoism-frontend/gpuix/artifacts/libghostty-vt.so" ] && export NEOISM_GHOSTTY_VT="$root/neoism-frontend/gpuix/artifacts/libghostty-vt.so"
fi
binary="$1"
out="$2"
mkdir -p "$out"
for scene in terminal-actions terminal-filter terminal-owner conversations-dashboard; do
  NEOISM_RUNE_OFFLINE=1 NEOISM_RUNE_NO_RESTORE=1 "$binary" --snap "$out/$scene.png" --scene "$scene" \
    --size 1252x674 --scale 2 --at 1 >/dev/null 2>&1 || { echo "FAIL $scene"; exit 1; }
  echo "PASS render $scene"
done
