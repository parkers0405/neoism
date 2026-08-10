#!/usr/bin/env bash
set -euo pipefail

SCRIPT_PATH="${BASH_SOURCE[0]:-}"
if [ -n "$SCRIPT_PATH" ]; then
  ROOT_DIR="$(cd -- "$(dirname -- "$SCRIPT_PATH")" >/dev/null 2>&1 && pwd)"
else
  ROOT_DIR="$PWD"
fi
PREFIX="${PREFIX:-$HOME/.local}"
BIN_DIR="${BIN_DIR:-$PREFIX/bin}"
NEOISM_REPO="${NEOISM_REPO:-https://github.com/parkers0405/neoism.git}"
NEOISM_GH_REPO="${NEOISM_GH_REPO:-parkers0405/neoism}"
NEOISM_REF="${NEOISM_REF:-main}"
NEOISM_DIR="${NEOISM_DIR:-$HOME/.local/src/neoism}"
PROFILE="release"
FEATURES=""
NO_DEFAULT_FEATURES=0
INSTALL_SYSTEM_DEPS=1
INSTALL_NEOVIM=1
INSTALL_TREESITTER=1
INSTALL_TS_CLI=0
DRY_RUN=0

TREE_SITTER_LANGS_ENV="${TREE_SITTER_LANGS:-}"
TREE_SITTER_LANGS=(rust python javascript typescript tsx go lua json toml yaml nix)

usage() {
  cat <<'USAGE'
Usage: ./install.sh [options]

Build and install this local Neoism checkout without making a release.

Options:
  --prefix DIR              Install prefix (default: ~/.local)
  --bin-dir DIR             Binary install dir (default: PREFIX/bin)
  --debug                   Build target/debug/neoism instead of target/release/neoism
  --features LIST           Cargo features to pass to Neoism
  --no-default-features     Pass --no-default-features to cargo
  --skip-system-deps        Do not use apt/pacman/dnf/etc.
  --skip-neovim             Do not install/check nvim
  --skip-treesitter         Do not install managed Treesitter parsers
  --with-tree-sitter-cli    Install tree-sitter CLI if missing
  --dry-run                 Print commands without running them
  -h, --help                Show this help

Environment:
  PREFIX, BIN_DIR, XDG_DATA_HOME, TREE_SITTER_LANGS
  NEOISM_REPO, NEOISM_GH_REPO, NEOISM_REF, NEOISM_DIR

Examples:
  ./install.sh
  ./install.sh --bin-dir "$HOME/bin"
  ./install.sh --debug
  ./install.sh --no-default-features --features wayland
  TREE_SITTER_LANGS="rust lua toml" ./install.sh
USAGE
}

is_repo_checkout() {
  [ -f "$ROOT_DIR/Cargo.toml" ] && [ -d "$ROOT_DIR/neoism-frontend/desktop" ]
}

bootstrap_checkout_if_needed() {
  if is_repo_checkout; then
    return 0
  fi

  log "No local Neoism checkout detected; bootstrapping into $NEOISM_DIR"
  have git || die "git is required to clone Neoism"

  if [ -d "$NEOISM_DIR/.git" ]; then
    run git -C "$NEOISM_DIR" fetch --all --prune
    run git -C "$NEOISM_DIR" checkout "$NEOISM_REF"
    run git -C "$NEOISM_DIR" pull --ff-only
  else
    run mkdir -p "$(dirname "$NEOISM_DIR")"
    if have gh; then
      run gh repo clone "$NEOISM_GH_REPO" "$NEOISM_DIR" -- --branch "$NEOISM_REF" --depth 1
    else
      run git clone --depth 1 --branch "$NEOISM_REF" "$NEOISM_REPO" "$NEOISM_DIR"
    fi
  fi

  exec bash "$NEOISM_DIR/install.sh" "$@"
}

log() {
  printf '\033[1;34m==>\033[0m %s\n' "$*"
}

warn() {
  printf '\033[1;33mwarn:\033[0m %s\n' "$*" >&2
}

die() {
  printf '\033[1;31merror:\033[0m %s\n' "$*" >&2
  exit 1
}

have() {
  command -v "$1" >/dev/null 2>&1
}

run() {
  if [ "$DRY_RUN" -eq 1 ]; then
    printf '+ '
    printf '%q ' "$@"
    printf '\n'
    return 0
  fi
  "$@"
}

run_shell() {
  if [ "$DRY_RUN" -eq 1 ]; then
    printf '+ %s\n' "$*"
    return 0
  fi
  bash -lc "$*"
}

sudo_run() {
  if [ "$(id -u)" -eq 0 ]; then
    run "$@"
  elif have sudo; then
    run sudo "$@"
  else
    die "sudo is required to install system packages; rerun with --skip-system-deps or install dependencies manually"
  fi
}

detect_package_manager() {
  if have apt-get; then
    printf 'apt'
  elif have pacman; then
    printf 'pacman'
  elif have dnf; then
    printf 'dnf'
  elif have zypper; then
    printf 'zypper'
  elif have xbps-install; then
    printf 'xbps'
  elif have apk; then
    printf 'apk'
  elif have brew; then
    printf 'brew'
  else
    printf 'none'
  fi
}

install_system_deps() {
  [ "$INSTALL_SYSTEM_DEPS" -eq 1 ] || return 0

  local pm
  pm="$(detect_package_manager)"
  log "Installing build/runtime dependencies with ${pm}"

  case "$pm" in
    apt)
      sudo_run env DEBIAN_FRONTEND=noninteractive apt-get update
      sudo_run env DEBIAN_FRONTEND=noninteractive apt-get install -y \
        build-essential ca-certificates cmake curl git libfontconfig1-dev \
        libfreetype6-dev libxcb-xfixes0-dev libxkbcommon-dev ncurses-bin \
        nodejs npm pkg-config python3 neovim
      ;;
    pacman)
      sudo_run pacman -S --needed --noconfirm \
        base-devel ca-certificates cmake curl fontconfig freetype2 git \
        libxcb libxkbcommon ncurses neovim nodejs npm pkgconf python
      ;;
    dnf)
      sudo_run dnf install -y \
        ca-certificates cmake curl fontconfig-devel freetype-devel gcc gcc-c++ \
        git libxcb-devel libxkbcommon-devel make ncurses neovim nodejs npm \
        pkgconf-pkg-config python3
      ;;
    zypper)
      sudo_run zypper --non-interactive install \
        ca-certificates cmake curl fontconfig-devel freetype2-devel gcc gcc-c++ \
        git libxcb-devel libxkbcommon-devel make ncurses neovim nodejs npm \
        pkg-config python3
      ;;
    xbps)
      sudo_run xbps-install -Sy \
        base-devel ca-certificates cmake curl fontconfig-devel freetype-devel \
        git libxcb-devel libxkbcommon-devel ncurses neovim nodejs npm \
        pkg-config python3
      ;;
    apk)
      sudo_run apk add \
        bash build-base ca-certificates cmake curl fontconfig-dev freetype-dev \
        git libxcb-dev libxkbcommon-dev ncurses neovim nodejs npm pkgconf \
        python3
      ;;
    brew)
      run brew install cmake fontconfig freetype git neovim node pkg-config tree-sitter
      ;;
    none)
      warn "No supported package manager found; assuming system dependencies are already installed"
      ;;
    *)
      die "unknown package manager ${pm}"
      ;;
  esac
}

ensure_rust() {
  if ! have cargo; then
    log "Installing Rust with rustup"
    have curl || die "curl is required to install rustup"
    # `--no-modify-path` keeps rustup from writing to user's
    # shell rc files (~/.zshenv, ~/.profile, etc). On macOS
    # those files are sometimes owned by root (e.g. after a
    # prior `sudo` install) and rustup bails with a "could not
    # write rcfile file: ~/.zshenv permission denied" error.
    # We export PATH ourselves on the next line for the rest of
    # the install run; users who want cargo persistently on
    # PATH can add `~/.cargo/bin` to their shell config later.
    run_shell 'curl --proto "=https" --tlsv1.2 -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path'
    export PATH="$HOME/.cargo/bin:$PATH"
  fi

  have cargo || die "cargo is still missing after rustup install"

  if have rustup && [ -f "$ROOT_DIR/rust-toolchain.toml" ]; then
    log "Installing Rust toolchain from rust-toolchain.toml"
    run rustup toolchain install 1.92 --profile minimal --component rustfmt --component clippy
  fi
}

ensure_neovim() {
  [ "$INSTALL_NEOVIM" -eq 1 ] || return 0
  if have nvim; then
    log "Found nvim: $(command -v nvim)"
    return 0
  fi

  if [ "$INSTALL_SYSTEM_DEPS" -eq 1 ]; then
    die "nvim was not found after dependency install; install Neovim manually or rerun with --skip-neovim"
  fi

  die "nvim is required for managed editor panes; install Neovim or rerun with --skip-neovim"
}

ensure_tree_sitter_cli() {
  [ "$INSTALL_TS_CLI" -eq 1 ] || return 0
  if have tree-sitter; then
    log "Found tree-sitter: $(command -v tree-sitter)"
    return 0
  fi

  log "Installing tree-sitter CLI"
  if have cargo; then
    run cargo install tree-sitter-cli --locked
  elif have npm; then
    run npm install -g tree-sitter-cli
  else
    warn "cargo/npm missing; skipping tree-sitter CLI install and using cc fallback"
  fi
}

data_home() {
  if [ -n "${XDG_DATA_HOME:-}" ]; then
    printf '%s' "$XDG_DATA_HOME"
  else
    printf '%s' "$HOME/.local/share"
  fi
}

ts_repo() {
  case "$1" in
    rust) printf '%s' 'https://github.com/tree-sitter/tree-sitter-rust' ;;
    python) printf '%s' 'https://github.com/tree-sitter/tree-sitter-python' ;;
    javascript) printf '%s' 'https://github.com/tree-sitter/tree-sitter-javascript' ;;
    typescript|tsx) printf '%s' 'https://github.com/tree-sitter/tree-sitter-typescript' ;;
    go) printf '%s' 'https://github.com/tree-sitter/tree-sitter-go' ;;
    lua) printf '%s' 'https://github.com/tree-sitter-grammars/tree-sitter-lua' ;;
    json) printf '%s' 'https://github.com/tree-sitter/tree-sitter-json' ;;
    toml) printf '%s' 'https://github.com/tree-sitter-grammars/tree-sitter-toml' ;;
    yaml) printf '%s' 'https://github.com/tree-sitter-grammars/tree-sitter-yaml' ;;
    nix) printf '%s' 'https://github.com/nix-community/tree-sitter-nix' ;;
    *) return 1 ;;
  esac
}

ts_subdir() {
  case "$1" in
    typescript) printf '%s' 'typescript' ;;
    tsx) printf '%s' 'tsx' ;;
    *) printf '%s' '.' ;;
  esac
}

copy_treesitter_queries() {
  local grammar_root="$1"
  local lang="$2"
  local runtime_dir="$3"
  local source_root="${4:-$grammar_root}"
  local source_queries="$grammar_root/queries"
  local dest_queries="$runtime_dir/queries/$lang"
  local highlight_files=()

  # Some multi-grammar repos (notably tree-sitter-typescript) keep parser
  # sources in per-language subdirs but share query files at the repo root.
  if [ ! -d "$source_queries" ] && [ -d "$source_root/queries" ]; then
    source_queries="$source_root/queries"
  fi

  if [ ! -f "$source_queries/highlights.scm" ]; then
    warn "$lang grammar has no queries/highlights.scm; parser installed without queries"
    return 0
  fi

  run mkdir -p "$dest_queries"
  if [ "$DRY_RUN" -eq 1 ]; then
    printf '+ cp %q/*.scm %q/\n' "$source_queries" "$dest_queries"
  else
    cp "$source_queries"/*.scm "$dest_queries"/

    highlight_files=("$source_queries/highlights.scm")
    case "$lang" in
      javascript)
        [ -f "$source_queries/highlights-jsx.scm" ] && highlight_files+=("$source_queries/highlights-jsx.scm")
        [ -f "$source_queries/highlights-params.scm" ] && highlight_files+=("$source_queries/highlights-params.scm")
        ;;
      typescript)
        local js_queries
        js_queries="$(dirname "$source_root")/javascript/queries"
        [ -f "$js_queries/highlights.scm" ] && highlight_files+=("$js_queries/highlights.scm")
        ;;
      tsx)
        local js_queries
        js_queries="$(dirname "$source_root")/javascript/queries"
        [ -f "$js_queries/highlights-jsx.scm" ] && highlight_files+=("$js_queries/highlights-jsx.scm")
        [ -f "$js_queries/highlights.scm" ] && highlight_files+=("$js_queries/highlights.scm")
        ;;
    esac

    : > "$dest_queries/highlights.scm"
    for query in "${highlight_files[@]}"; do
      printf '\n; ---- %s ----\n' "$query" >> "$dest_queries/highlights.scm"
      cat "$query" >> "$dest_queries/highlights.scm"
      printf '\n' >> "$dest_queries/highlights.scm"
    done
  fi
}

compile_treesitter_parser_with_cc() {
  local lang="$1"
  local grammar_root="$2"
  local output="$3"
  local build_root="$4"
  local src_dir="$grammar_root/src"
  local parser_c="$src_dir/parser.c"
  local objects=()
  local needs_cxx=0

  [ -f "$parser_c" ] || die "$grammar_root does not contain src/parser.c"
  have cc || die "cc is required to build Treesitter parser $lang"

  run mkdir -p "$build_root"
  run cc -fPIC -O2 -I "$src_dir" -c "$parser_c" -o "$build_root/parser.o"
  objects+=("$build_root/parser.o")

  if [ -f "$src_dir/scanner.c" ]; then
    run cc -fPIC -O2 -I "$src_dir" -c "$src_dir/scanner.c" -o "$build_root/scanner.o"
    objects+=("$build_root/scanner.o")
  fi

  if [ -f "$src_dir/scanner.cc" ]; then
    have c++ || die "c++ is required to build Treesitter scanner for $lang"
    run c++ -fPIC -O2 -I "$src_dir" -c "$src_dir/scanner.cc" -o "$build_root/scanner_cc.o"
    objects+=("$build_root/scanner_cc.o")
    needs_cxx=1
  fi

  if [ "$needs_cxx" -eq 1 ]; then
    run c++ -shared -o "$output" "${objects[@]}"
  else
    run cc -shared -o "$output" "${objects[@]}"
  fi
}

install_treesitter_parser() {
  local lang="$1"
  local repo subdir data neoism_lsp_root source_root build_root runtime_dir parser_dir grammar_root output

  repo="$(ts_repo "$lang")" || {
    warn "No Treesitter installer mapping for '$lang'; skipping"
    return 0
  }
  subdir="$(ts_subdir "$lang")"
  data="$(data_home)"
  neoism_lsp_root="$data/neoism/lsp"
  source_root="$neoism_lsp_root/treesitter-src/$lang"
  build_root="$neoism_lsp_root/treesitter-build/$lang"
  runtime_dir="$data/neoism/nvim-runtime"
  parser_dir="$runtime_dir/parser"
  output="$parser_dir/$lang.so"

  log "Installing Treesitter parser: $lang"
  have git || die "git is required to fetch Treesitter grammars"
  run mkdir -p "$parser_dir" "$build_root"

  if [ -d "$source_root/.git" ]; then
    run git -C "$source_root" pull --ff-only
  else
    run mkdir -p "$(dirname "$source_root")"
    run git clone --depth 1 "$repo" "$source_root"
  fi

  if [ "$subdir" = "." ]; then
    grammar_root="$source_root"
  else
    grammar_root="$source_root/$subdir"
  fi

  if have tree-sitter; then
    run tree-sitter build --output "$output" "$grammar_root"
  else
    compile_treesitter_parser_with_cc "$lang" "$grammar_root" "$output" "$build_root"
  fi

  copy_treesitter_queries "$grammar_root" "$lang" "$runtime_dir" "$source_root"
}

install_treesitter_parsers() {
  [ "$INSTALL_TREESITTER" -eq 1 ] || return 0

  if [ -n "$TREE_SITTER_LANGS_ENV" ]; then
    # shellcheck disable=SC2206
    TREE_SITTER_LANGS=($TREE_SITTER_LANGS_ENV)
  fi

  ensure_tree_sitter_cli
  for lang in "${TREE_SITTER_LANGS[@]}"; do
    install_treesitter_parser "$lang"
  done
}

build_neoism() {
  local cargo_args=(build -p neoism)
  local output

  if [ "$PROFILE" = "release" ]; then
    cargo_args+=(--release)
    output="$ROOT_DIR/target/release/neoism"
  else
    output="$ROOT_DIR/target/debug/neoism"
  fi

  if [ "$NO_DEFAULT_FEATURES" -eq 1 ]; then
    cargo_args+=(--no-default-features)
  fi

  if [ -n "$FEATURES" ]; then
    cargo_args+=(--features "$FEATURES")
  fi

  log "Building Neoism (${PROFILE})"
  run cargo "${cargo_args[@]}"

  [ "$DRY_RUN" -eq 1 ] || [ -x "$output" ] || die "build finished but $output is missing"

  log "Installing binary to $BIN_DIR/neoism"
  run mkdir -p "$BIN_DIR"
  run install -m 0755 "$output" "$BIN_DIR/neoism"
}

ORIGINAL_ARGS=("$@")

while [ "$#" -gt 0 ]; do
  case "$1" in
    --prefix)
      PREFIX="${2:-}"
      [ -n "$PREFIX" ] || die "--prefix requires a directory"
      BIN_DIR="$PREFIX/bin"
      shift 2
      ;;
    PREFIX=*)
      PREFIX="${1#PREFIX=}"
      [ -n "$PREFIX" ] || die "PREFIX= requires a directory"
      BIN_DIR="$PREFIX/bin"
      shift
      ;;
    --bin-dir)
      BIN_DIR="${2:-}"
      [ -n "$BIN_DIR" ] || die "--bin-dir requires a directory"
      shift 2
      ;;
    BIN_DIR=*)
      BIN_DIR="${1#BIN_DIR=}"
      [ -n "$BIN_DIR" ] || die "BIN_DIR= requires a directory"
      shift
      ;;
    --debug)
      PROFILE="debug"
      shift
      ;;
    --features)
      FEATURES="${2:-}"
      [ -n "$FEATURES" ] || die "--features requires a feature list"
      shift 2
      ;;
    --no-default-features)
      NO_DEFAULT_FEATURES=1
      shift
      ;;
    --skip-system-deps)
      INSTALL_SYSTEM_DEPS=0
      shift
      ;;
    --skip-neovim)
      INSTALL_NEOVIM=0
      shift
      ;;
    --skip-treesitter)
      INSTALL_TREESITTER=0
      shift
      ;;
    --with-tree-sitter-cli)
      INSTALL_TS_CLI=1
      shift
      ;;
    --dry-run)
      DRY_RUN=1
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

# macOS ships bash 3.2, which under `set -u` throws "unbound
# variable" when an empty array is expanded with `"${arr[@]}"`.
# Bash 4.4+ silently expands to nothing. The `${arr[@]+…}`
# parameter-default form here is the canonical bash-3 workaround:
# expand to the array if it has any elements, otherwise expand to
# nothing — avoids the unbound error when `./install.sh` is run
# with zero args.
bootstrap_checkout_if_needed ${ORIGINAL_ARGS[@]+"${ORIGINAL_ARGS[@]}"}

cd "$ROOT_DIR"

log "Neoism source: $ROOT_DIR"
install_system_deps
ensure_rust
ensure_neovim
install_treesitter_parsers
build_neoism

cat <<EOF

Installed local Neoism build.

Binary: $BIN_DIR/neoism
Managed nvim runtime: $(data_home)/neoism/nvim-runtime
Managed LSP/tools root: $(data_home)/neoism/lsp

If $BIN_DIR is not on PATH, add this to your shell config:
  export PATH="$BIN_DIR:\$PATH"

Run:
  $BIN_DIR/neoism
EOF
