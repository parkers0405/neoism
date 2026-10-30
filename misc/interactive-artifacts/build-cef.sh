#!/usr/bin/env bash
set -euo pipefail

root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
cef="${NEOISM_CEF_DIR:-${CEF_PATH:-${HOME}/.local/share/cef}}"

if [[ ! -d "$cef" ]]; then
    printf 'CEF binaries not found at %s\n' "$cef" >&2
    printf 'Install CEF matching cef-rs 154.5.0, then set NEOISM_CEF_DIR or CEF_PATH.\n' >&2
    printf 'See neoism-frontend/cef-runtime/README.md for installation instructions.\n' >&2
    exit 1
fi

export CEF_PATH="$cef"
bash "$root/scripts/blaze.sh" build --manifest-path "$root/neoism-frontend/cef-runtime/Cargo.toml" "$@"
printf '\nStandalone experimental CEF worker built. GPUIX does not currently host live HTML artifacts.\n'
