#!/usr/bin/env bash
# Neoism installer — downloads the prebuilt stack from GitHub Releases.
#
#   curl -fsSL https://raw.githubusercontent.com/parkers0405/neoism/main/scripts/install.sh | bash
#
# This is the public *download* installer: it fetches the latest prebuilt
# release from the main neoism repo's GitHub Releases. The source repo is
# public, so the raw URL above resolves for everyone.
#
# Re-run any time to update to the latest release (idempotent). This is the
# *download* installer; the repo's top-level ./install.sh builds from source.
#
# Env overrides:
#   NEOISM_VERSION   pin a release tag (default: latest)
#   NEOISM_BIN_DIR   install dir (default: ~/.local/bin)
#   NEOISM_REPO      owner/repo (default: parkers0405/neoism)
#   NEOISM_SKIP_CHECKSUM  set to 1 to bypass checksum (for testing/airgap)
set -euo pipefail

REPO="${NEOISM_REPO:-parkers0405/neoism}"  # GitHub repo whose Releases host the prebuilt binaries
BIN_DIR="${NEOISM_BIN_DIR:-${HOME}/.local/bin}"
VERSION="${NEOISM_VERSION:-latest}"
BINARIES=(neoism neoism-workspace-daemon neoism-agent)
SKIP_CHECKSUM="${NEOISM_SKIP_CHECKSUM:-0}"

say()  { printf '\033[1;36m==>\033[0m %s\n' "$*"; }
err()  { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }
warn() { printf '\033[1;33mwarn:\033[0m %s\n' "$*" >&2; }

need() { command -v "$1" >/dev/null 2>&1 || err "missing required tool: $1"; }
need uname; need tar; need install; need mkdir
if command -v curl >/dev/null 2>&1; then DL=(curl -fsSL); DLO=(curl -fsSL -o)
elif command -v wget >/dev/null 2>&1; then DL=(wget -qO-); DLO=(wget -qO)
else err "need curl or wget"; fi

# --- detect platform ------------------------------------------------------
os="$(uname -s)"; arch="$(uname -m)"
case "$os" in
  Linux)  goos=linux ;;
  Darwin) goos=darwin ;;
  *) err "unsupported OS: $os (only Linux + macOS prebuilt; build from source otherwise)";;
esac
case "$arch" in
  x86_64|amd64)  goarch=x86_64 ;;
  aarch64|arm64) goarch=aarch64 ;;
  *) err "unsupported arch: $arch";;
esac
asset="neoism-${goos}-${goarch}.tar.gz"
case "$asset" in
  neoism-linux-x86_64.tar.gz|neoism-darwin-aarch64.tar.gz) ;;
  *)
    err "no prebuilt release asset for ${goos}/${goarch} yet (${asset}). Clone https://github.com/${REPO} and run ./install.sh to build from source."
    ;;
esac

# --- resolve version ------------------------------------------------------
if [ "$VERSION" = "latest" ]; then
  base="https://github.com/${REPO}/releases/latest/download"
else
  base="https://github.com/${REPO}/releases/download/${VERSION}"
fi

say "Installing Neoism (${goos}/${goarch}) from ${REPO} (${VERSION})"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT

say "Downloading ${asset}"
"${DLO[@]}" "$tmp/$asset" "$base/$asset" \
  || err "download failed — is there a release with $asset? (try NEOISM_VERSION=vX.Y.Z)"

# checksum verification — releases ship a per-asset .sha256 file
if [ "$SKIP_CHECKSUM" = "1" ]; then
  say "checksum verification skipped (NEOISM_SKIP_CHECKSUM=1)"
elif command -v sha256sum >/dev/null 2>&1 \
  && "${DL[@]}" "$base/$asset.sha256" >"$tmp/$asset.sha256" 2>/dev/null; then
  ( cd "$tmp" && sha256sum -c "$asset.sha256" >/dev/null 2>&1 ) \
    && say "checksum OK" || err "checksum mismatch for $asset — aborting"
else
  say "checksum not verified (sha256sum or .sha256 asset unavailable)"
fi

say "Extracting to ${BIN_DIR}"
tar -xzf "$tmp/$asset" -C "$tmp"
payload="$tmp/neoism-${goos}-${goarch}"
# Validate the complete payload before touching any installed file. Do not find
# arbitrary binaries inside the .app or mix them with another resource tree.
for b in "${BINARIES[@]}"; do
  [ -f "$payload/$b" ] && [ -x "$payload/$b" ] || err "binary '$b' not found in $asset"
done
[ -f "$payload/web/index.html" ] || err "web/index.html not found in $asset"
[ -f "$payload/web/agent-gui/index.html" ] || err "agent GUI missing from $asset; this release is incomplete"
mkdir -p "$BIN_DIR"
transaction="$(mktemp -d "$BIN_DIR/.neoism-install.XXXXXX")"
mkdir "$transaction/new" "$transaction/old"
components=("${BINARIES[@]}" web)
moved=()
committed=0
cleanup_install() {
  local result=$? i component
  trap - EXIT INT TERM
  if [ "$committed" -eq 0 ]; then
    for ((i=${#moved[@]}-1; i>=0; i--)); do
      component="${moved[$i]}"
      if [ -e "$transaction/old/$component" ] || [ -L "$transaction/old/$component" ]; then
        rm -rf "$BIN_DIR/$component"
        mv "$transaction/old/$component" "$BIN_DIR/$component" || {
          warn "Rollback failed; recovery files remain in $transaction"
          exit 1
        }
      elif [ ! -e "$transaction/new/$component" ]; then
        rm -rf "$BIN_DIR/$component"
      fi
    done
  fi
  rm -rf "$transaction" "$tmp"
  exit "$result"
}
trap cleanup_install EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
for b in "${BINARIES[@]}"; do
  install -m 0755 "$payload/$b" "$transaction/new/$b"
done
# Includes agent-gui and every hashed/static asset; never merge old/new trees.
cp -R "$payload/web" "$transaction/new/web"
for component in "${components[@]}"; do
  moved+=("$component")
  if [ -e "$BIN_DIR/$component" ] || [ -L "$BIN_DIR/$component" ]; then
    mv "$BIN_DIR/$component" "$transaction/old/$component"
  fi
  mv "$transaction/new/$component" "$BIN_DIR/$component"
  printf '   %s\n' "$BIN_DIR/$component"
done
committed=1

say "Done. Neoism ${VERSION} installed."
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *)
    warn "$BIN_DIR is not on PATH, so typing 'neoism' will not work in new shells yet"
    printf '\nRun it now with:\n  %s/neoism\n\nAdd to PATH:\n  export PATH="%s:$PATH"\n' "$BIN_DIR" "$BIN_DIR"
    ;;
esac
if ! "$BIN_DIR/neoism" --version >/dev/null 2>&1; then
  warn "installed binary did not run successfully; if this is NixOS or another non-FHS Linux, build from source with ./install.sh"
fi
printf '\nClose every running Neoism window, then run:  neoism\nUpdate later:  neoism update   (or re-run this installer if an older updater fails)\n'
