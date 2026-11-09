#!/bin/bash
# Build with Rune's pinned toolchain even when PATH points at another rustc.
# Usage: ./build.sh [cargo args...]   (defaults to `build --release`)
set -e
TC="$HOME/.rustup/toolchains/1.99.1-x86_64-unknown-linux-gnu/bin"
export PATH="$TC:$PATH" RUSTC="$TC/rustc"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/neoism-rune-target}"
cd "$(dirname "$0")"
# The About card names the commit a build came from.
export GIT_HASH="${GIT_HASH:-$(git rev-parse --short HEAD 2>/dev/null || echo dev)}"
exec "$TC/cargo" "${@:-build}" ${@:+} $( [ $# -eq 0 ] && echo --release )
