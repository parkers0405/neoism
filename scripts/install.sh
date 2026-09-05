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
mkdir -p "$BIN_DIR"
for b in "${BINARIES[@]}"; do
  src="$(find "$tmp" -type f -name "$b" -perm -u+x | head -1)"
  [ -n "$src" ] || err "binary '$b' not found in $asset"
  # Stage beside the destination, then rename. This never leaves a partially
  # written executable if the download/install is interrupted, and Unix can
  # safely replace the path while an older process still has its image mapped.
  staged="$BIN_DIR/.$b.new"
  install -m 0755 "$src" "$staged"
  mv -f "$staged" "$BIN_DIR/$b"
  printf '   %s\n' "$BIN_DIR/$b"
done

web_index="$(find "$tmp" -type f -path '*/web/index.html' | head -1)"
[ -n "$web_index" ] || err "web/index.html not found in $asset"
web_src="$(dirname "$web_index")"
rm -rf "$BIN_DIR/.web.new"
cp -R "$web_src" "$BIN_DIR/.web.new"
rm -rf "$BIN_DIR/web"
mv "$BIN_DIR/.web.new" "$BIN_DIR/web"
printf '   %s\n' "$BIN_DIR/web"

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
