#!/bin/bash
# Build with Rune's pinned toolchain even when PATH points at another rustc.
# Usage: ./build.sh [cargo args...]   (defaults to `build --release`)
set -e
export PATH="$HOME/.cargo/bin:$PATH"
if ! command -v rustup >/dev/null && command -v brew >/dev/null; then
    export PATH="$(brew --prefix rustup)/bin:$PATH"
fi
cd "$(dirname "$0")"
TC="$(dirname "$(rustup which rustc)")"
export PATH="$TC:$PATH" RUSTC="$TC/rustc"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/neoism-rune-target}"
# The About card names the commit a build came from.
export GIT_HASH="${GIT_HASH:-$(git rev-parse --short HEAD 2>/dev/null || echo dev)}"
exec "$TC/cargo" "${@:-build}" ${@:+} $( [ $# -eq 0 ] && echo --release )
