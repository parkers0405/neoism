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
cargo build --manifest-path "$root/neoism-frontend/cef-runtime/Cargo.toml" "$@"
printf '\nLive HTML artifacts use this worker, not Servo. Launch your debug Neoism with:\n'
if [[ "$cef" == "$HOME/.local/share/cef" ]]; then
    printf '%q\n' "$root/target/debug/neoism"
else
    printf 'NEOISM_CEF_DIR=%q %q\n' "$cef" "$root/target/debug/neoism"
fi
printf 'The worker requests Chromium sandboxing. Do not disable it for untrusted HTML.\n'
