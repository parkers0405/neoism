#!/usr/bin/env bash
# Package only explicitly built artifacts, never an old desktop executable.
set -euo pipefail
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
frontend="$repo_root/neoism-frontend/gpuix"
[[ "$(uname -s)" == Linux && "$(uname -m)" == x86_64 ]] || { echo 'The validated release target is Linux x86_64.' >&2; exit 1; }
out="${NEOISM_RELEASE_DIR:-$repo_root/release}"
mkdir -p "$out"
staging="$(mktemp -d)"
trap 'rm -rf "$staging"' EXIT
bundle="$staging/neoism-linux-x86_64"
mkdir -p "$bundle"
for file in neoism neoism-workspace-daemon neoism-agent-lua-runner libneoism_gpuix_core.so libghostty-vt.so LICENSE.ghostty; do
  [[ -f "$frontend/dist/$file" ]] || { echo "Missing built artifact: $file" >&2; exit 1; }
  cp -p "$frontend/dist/$file" "$bundle/$file"
done
cp -p "$repo_root/target/activation/neoism-agent" "$bundle/neoism-agent"
cp "$repo_root/LICENSE" "$repo_root/NOTICE" "$bundle/"
cp -a "$frontend/dist/licenses" "$bundle/licenses"
cp "$frontend/node_modules/@gpuix/native/LICENSE" "$bundle/licenses/GPUIX-native.txt"
cp "$frontend/node_modules/@gpuix/solid/LICENSE" "$bundle/licenses/GPUIX-solid.txt"
cp "$repo_root/misc/neoism.desktop" "$bundle/neoism.desktop"
cp "$repo_root/neoism-frontend/shared/assets/icons/neoism.png" "$bundle/neoism.png"
# Never copy stale browser distributions from a previous native build.
mkdir -p "$bundle/web"
cp -a "$frontend/dist/web/app" "$frontend/dist/web/native" "$bundle/web/"
printf '%s\n' '<!-- Neoism native runtime resources; no browser UI. -->' > "$bundle/web/index.html"
cp "$repo_root/scripts/install-gpuix-bundle.sh" "$bundle/install-bundle.sh"
tar -C "$staging" -czf "$out/neoism-linux-x86_64.tar.gz.next" neoism-linux-x86_64
mv -f "$out/neoism-linux-x86_64.tar.gz.next" "$out/neoism-linux-x86_64.tar.gz"
(cd "$out" && sha256sum neoism-linux-x86_64.tar.gz > neoism-linux-x86_64.tar.gz.sha256)
echo "$out/neoism-linux-x86_64.tar.gz"
