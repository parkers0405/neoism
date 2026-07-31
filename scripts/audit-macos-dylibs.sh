#!/usr/bin/env bash
# Reject macOS release binaries that depend on libraries outside macOS itself.
# In particular, this prevents a GitHub runner's Homebrew libraries from being
# baked into a supposedly self-contained prebuilt release.
set -euo pipefail

if [ "$#" -eq 0 ]; then
  echo "usage: $0 BINARY..." >&2
  exit 2
fi

if command -v otool >/dev/null 2>&1; then
  inspect=(otool -L)
elif command -v llvm-objdump >/dev/null 2>&1; then
  # Lets Linux release checks inspect a downloaded Mach-O artifact too.
  inspect=(llvm-objdump --macho --dylibs-used)
else
  echo "error: otool or llvm-objdump is required to audit macOS binaries" >&2
  exit 2
fi

failed=0
for binary in "$@"; do
  if [ ! -f "$binary" ]; then
    echo "error: macOS dylib audit input does not exist: $binary" >&2
    failed=1
    continue
  fi

  while IFS= read -r dependency; do
    case "$dependency" in
      /System/Library/*|/usr/lib/*) ;;
      *)
        echo "error: $binary has a non-system macOS dependency: $dependency" >&2
        failed=1
        ;;
    esac
  done < <("${inspect[@]}" "$binary" | tail -n +2 | awk '{print $1}')
done

exit "$failed"
