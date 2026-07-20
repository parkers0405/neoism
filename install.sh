#!/usr/bin/env bash
# Neoism source installer — builds the stack and installs the binaries.
#
#   ./install.sh                # release build into ~/.local/bin
#
# This script only builds and places files:
#   - neoism, neoism-workspace-daemon, neoism-agent  -> BIN_DIR
#   - wasm bundle + Vite web build (optional)        -> neoism-frontend/web/dist
#
# Everything user-facing (terminfo entry, desktop launcher + icons, default
# config) is handled by the
# app's first-run bootstrap on launch — see
# neoism-frontend/desktop/src/bootstrap.rs. Prebuilt installs use
# scripts/install.sh (download) or `neoism update` instead.
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
PREFIX="${PREFIX:-$HOME/.local}"
BIN_DIR="${BIN_DIR:-$PREFIX/bin}"
PROFILE="release"
INSTALL_SYSTEM_DEPS=1
BUILD_WEB=1

usage() {
  cat <<'USAGE'
Usage: ./install.sh [options]

Builds and installs the Neoism stack from source:
  - neoism desktop, neoism-workspace-daemon, neoism-agent -> BIN_DIR
  - web wasm bundle + Vite web build (optional)

Options:
  --prefix DIR          Install prefix (default: ~/.local)
  --bin-dir DIR         Binary install dir (default: PREFIX/bin)
  --debug               Build debug binaries instead of release
  --skip-system-deps    Do not install/check OS packages
  --skip-web            Do not build wasm/web assets
  -h, --help            Show this help
USAGE
}

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mwarn:\033[0m %s\n' "$*" >&2; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

run() {
  printf '+ '
  printf '%q ' "$@"
  printf '\n'
  "$@"
}

sudo_run() {
  if [ "$(id -u)" -eq 0 ]; then
    run "$@"
  elif have sudo; then
    run sudo "$@"
  else
    die "sudo is required for system deps; rerun with --skip-system-deps if deps are already installed"
  fi
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --prefix)
      PREFIX="${2:-}"
      [ -n "$PREFIX" ] || die "--prefix requires a directory"
      BIN_DIR="$PREFIX/bin"
      shift 2
      ;;
    --bin-dir)
      BIN_DIR="${2:-}"
      [ -n "$BIN_DIR" ] || die "--bin-dir requires a directory"
      shift 2
      ;;
    --debug)
      PROFILE="debug"
      shift
      ;;
    --skip-system-deps)
      INSTALL_SYSTEM_DEPS=0
      shift
      ;;
    --skip-web)
      BUILD_WEB=0
      shift
      ;;
    --skip-terminfo|--skip-treesitter|--skip-desktop|--with-tree-sitter-cli|--skip-runtime|--refresh-runtime)
      warn "$1 is obsolete — the app's first-run bootstrap handles this now"
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      die "unknown option: $1"
      ;;
  esac
done

cd "$ROOT_DIR"

# Prefer rustup/cargo-installed tools over distro cargo. The repo pins
# Rust 1.92; use rustup's cargo shim so `cargo +1.92 ...` works.
export PATH="$HOME/.cargo/bin:$PATH"

install_system_deps() {
  [ "$INSTALL_SYSTEM_DEPS" -eq 1 ] || return 0

  log "Installing/checking system dependencies"
  if have apt-get; then
    sudo_run env DEBIAN_FRONTEND=noninteractive apt-get update
    sudo_run env DEBIAN_FRONTEND=noninteractive apt-get install -y \
      build-essential ca-certificates cmake curl git libfontconfig1-dev \
      libfreetype6-dev libxcb-xfixes0-dev libxkbcommon-dev ncurses-bin \
      nodejs npm pkg-config python3 neovim ripgrep
  elif have brew; then
    run brew install \
      cmake fontconfig freetype git neovim node pkg-config ripgrep
  elif have pacman; then
    sudo_run pacman -S --needed --noconfirm \
      base-devel ca-certificates cmake curl fontconfig freetype2 git \
      libxcb libxkbcommon ncurses neovim nodejs npm pkgconf python ripgrep
  elif have dnf; then
    sudo_run dnf install -y \
      ca-certificates cmake curl fontconfig-devel freetype-devel gcc gcc-c++ \
      git libxcb-devel libxkbcommon-devel make ncurses neovim nodejs npm \
      pkgconf-pkg-config python3 ripgrep
  else
    warn "No supported package manager found; assuming deps are installed"
  fi
}

ensure_rust() {
  if ! have rustup; then
    log "Installing rustup"
    have curl || die "curl is required to install rustup"
    run bash -lc 'curl --proto "=https" --tlsv1.2 -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path'
    export PATH="$HOME/.cargo/bin:$PATH"
  fi

  have cargo || die "cargo is missing"
  local toolchain_dir=""
  for candidate in "$HOME"/.rustup/toolchains/1.92-*; do
    if [ -d "$candidate" ]; then
      toolchain_dir="$candidate"
      break
    fi
  done

  if [ -n "$toolchain_dir" ]; then
    log "Found Rust 1.92 toolchain: $toolchain_dir"
  elif have rustup; then
    log "Installing Rust 1.92 toolchain"
    run rustup toolchain install 1.92 --profile minimal --component rustfmt --component clippy
    for candidate in "$HOME"/.rustup/toolchains/1.92-*; do
      if [ -d "$candidate" ]; then
        toolchain_dir="$candidate"
        break
      fi
    done
  else
    die "rustup is missing and Rust 1.92 is not installed"
  fi

  if [ "$BUILD_WEB" -eq 1 ] && [ ! -d "$toolchain_dir/lib/rustlib/wasm32-unknown-unknown" ]; then
    run rustup target add wasm32-unknown-unknown --toolchain 1.92
  fi
}

ensure_web_tools() {
  [ "$BUILD_WEB" -eq 1 ] || return 0
  have npm || die "npm is required for the web build"

  if ! have wasm-pack; then
    log "Installing wasm-pack"
    run cargo +1.92 install wasm-pack --locked
  fi
}

build_binaries() {
  local cargo_args=(+1.92 build -p neoism -p neoism-workspace-daemon -p neoism-agent)
  local target_dir="$ROOT_DIR/target/debug"
  if [ "$PROFILE" = "release" ]; then
    cargo_args+=(--release)
    target_dir="$ROOT_DIR/target/release"
  fi

  log "Building desktop, daemon, and agent (${PROFILE})"
  run cargo "${cargo_args[@]}"

  log "Installing binaries to $BIN_DIR"
  run mkdir -p "$BIN_DIR"
  run install -m 0755 "$target_dir/neoism" "$BIN_DIR/neoism"
  run install -m 0755 "$target_dir/neoism-workspace-daemon" "$BIN_DIR/neoism-workspace-daemon"
  run install -m 0755 "$target_dir/neoism-agent" "$BIN_DIR/neoism-agent"
}

build_web() {
  [ "$BUILD_WEB" -eq 1 ] || return 0

  log "Installing web dependencies"
  run npm --prefix "$ROOT_DIR/neoism-frontend/web" ci

  # `npm run build` regenerates the wasm bundle via scripts/build-wasm.sh
  # before tsc + vite.
  log "Building web app (wasm + vite)"
  run npm --prefix "$ROOT_DIR/neoism-frontend/web" run build
}

install_system_deps
ensure_rust
ensure_web_tools
build_binaries
build_web

cat <<EOF

Installed the Neoism stack.

Binaries:
  $BIN_DIR/neoism
  $BIN_DIR/neoism-workspace-daemon
  $BIN_DIR/neoism-agent
EOF

if [ "$BUILD_WEB" -eq 1 ]; then
  cat <<EOF
Web build:
  $ROOT_DIR/neoism-frontend/web/dist
EOF
fi

cat <<EOF

First launch bootstraps the rest automatically (terminfo, desktop launcher,
default config, parsers). Make sure this is on PATH:
  export PATH="$BIN_DIR:\$PATH"
EOF
