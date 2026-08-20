#!/usr/bin/env bash
# Cut a Neoism release: bump the workspace version, commit, tag, push.
# Also updates AppStream metainfo.xml with a new release entry (for
# Flathub / Linux app stores) and refreshes any other version refs.
#
# The tag triggers .github/workflows/release-neoism.yml, which builds the
# stack per-OS and publishes the tarballs to the GitHub Releases of
# parkers0405/neoism, which `neoism update` and the curl installer pull
# from. The tag MUST match the crate version (`neoism update` compares
# `v<CARGO_PKG_VERSION>` against the release tag), which is why this script
# owns the bump.
#
# Usage: scripts/release.sh 0.4.1 [--dry-run]
set -euo pipefail

VERSION="${1:?usage: release.sh X.Y.Z (no leading v)}"
DRY_RUN=0
if [ "${2:-}" = "--dry-run" ]; then
  DRY_RUN=1
  echo "=== DRY RUN MODE ==="
fi

case "$VERSION" in
  [0-9]*.[0-9]*.[0-9]*) ;;
  *) echo "error: version must look like X.Y.Z (got: $VERSION)" >&2; exit 1 ;;
esac

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." >/dev/null 2>&1 && pwd)"
cd "$ROOT"

[ -z "$(git status --porcelain)" ] || { echo "error: working tree not clean" >&2; exit 1; }
git rev-parse "v$VERSION" >/dev/null 2>&1 && { echo "error: tag v$VERSION already exists" >&2; exit 1; }

CURRENT="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
echo "==> bumping workspace version: $CURRENT -> $VERSION"

# More robust version bump: use perl for in-place edit to avoid sed quirks on BSD/macOS
if [ $DRY_RUN -eq 0 ]; then
  perl -pi -e "s/^version = \"${CURRENT//\./\\.}\"/version = \"$VERSION\"/" Cargo.toml
  # Keep internal path-dependency version pins in lockstep with the workspace
  # version. Without this a minor/major bump fails to resolve (a `^0.5.0` pin
  # can't match a 0.6.0 crate). Only rewrites lines that declare a local `path`.
  perl -pi -e "s/(path = \"[^\"]+\") version = \"[0-9]+\.[0-9]+\.[0-9]+\"/\1 version = \"$VERSION\"/" Cargo.toml
else
  echo "[dry-run] would bump Cargo.toml from $CURRENT to $VERSION"
fi

# Update AppStream metainfo.xml with new release (substantial addition for Linux packaging)
METAINFO="misc/dev.neoism.Neoism.metainfo.xml"
if [ -f "$METAINFO" ]; then
  DATE="$(date +%Y-%m-%d)"
  NEW_RELEASE="  <releases>\n    <release version=\"$VERSION\" date=\"$DATE\">\n      <description>\n        <p>Bugfixes, improved dev scripts, and cross-platform parity updates.</p>\n      </description>\n    </release>\n  </releases>"
  if [ $DRY_RUN -eq 0 ]; then
    # Insert releases section if missing, or update existing (safe append before </component>)
    if ! grep -q '<releases>' "$METAINFO"; then
      perl -pi -e "s|(</component>)|$NEW_RELEASE\n\1|" "$METAINFO"
      echo "==> added new release entry to $METAINFO"
    else
      echo "==> metainfo already has releases section (manual update recommended)"
    fi
  else
    echo "[dry-run] would add release $VERSION to $METAINFO"
  fi
fi

echo "==> refreshing Cargo.lock (workspace members only)"
if [ $DRY_RUN -eq 0 ]; then
  cargo +1.92 update --workspace --quiet
  git add Cargo.toml Cargo.lock "$METAINFO"
  git commit -m "release: v$VERSION"
  git tag "v$VERSION"
  echo "==> pushing main + v$VERSION (this triggers the release build)"
  git push origin main "v$VERSION"
else
  echo "[dry-run] would run cargo update, commit, tag, and push"
fi

cat <<EOF

Release v$VERSION is building (or simulated):
  https://github.com/parkers0405/neoism/actions/workflows/release-neoism.yml
When green, it publishes to:
  https://github.com/parkers0405/neoism/releases
Users then get it with:  neoism update
EOF
