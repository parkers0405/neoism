#!/usr/bin/env bash
# Installs an already built bundle. Validation precedes every installed-file write.
set -euo pipefail
payload="${1:?bundle directory required}"
install_prefix="${2:?installation prefix required}"
install_bin="${3:-$install_prefix/bin}"
[[ "$(uname -s)" == Linux ]] || { echo 'This bundle installer currently supports Linux.' >&2; exit 1; }
binaries=(neoism neoism-workspace-daemon neoism-agent)
# Legacy update compatibility companion is present in new nightly bundles.
[[ ! -f "$payload/neoism-agent-lua-runner" ]] || binaries+=(neoism-agent-lua-runner)
resources=(libneoism_gpuix_core.so libghostty-vt.so LICENSE.ghostty LICENSE NOTICE)
for name in "${binaries[@]}"; do [[ -f "$payload/$name" && -x "$payload/$name" ]] || { echo "Missing executable: $name" >&2; exit 1; }; done
for name in "${resources[@]}"; do [[ -f "$payload/$name" ]] || { echo "Missing resource: $name" >&2; exit 1; }; done
[[ -f "$payload/web/agent-gui/index.html" ]] || { echo 'Missing standalone Agent GUI.' >&2; exit 1; }
[[ -d "$payload/licenses" ]] || { echo 'Missing bundled font licenses.' >&2; exit 1; }
mkdir -p "$install_bin"
install_bin="$(cd -- "$install_bin" && pwd -P)"
bundle_in_use() {
  local directory="$1" executable binary
  while IFS= read -r -d '' executable; do
    for binary in "${binaries[@]}"; do
      if [[ "$executable" == "$directory/$binary" ]]; then return 0; fi
    done
  done < <(readlink -z /proc/[0-9]*/exe 2>/dev/null)
  return 1
}
# Completed upgrades may retain a bundle while its daemon/window is running.
# Never prune an interrupted transaction that may still be needed for recovery.
for previous in "$install_bin"/.neoism-install.*; do
  if [[ -f "$previous/committed" ]] && ! bundle_in_use "$previous/old"; then
    rm -rf "$previous"
  fi
done
transaction="$(mktemp -d "$install_bin/.neoism-install.XXXXXX")"
mkdir "$transaction/new" "$transaction/old"
components=("${binaries[@]}" "${resources[@]}" web licenses)
moved=()
committed=0
cleanup() {
  local result=$? index component
  trap - EXIT INT TERM
  if (( ! committed )); then
    for ((index=${#moved[@]}-1; index>=0; index--)); do
      component="${moved[$index]}"
      if [[ -e "$transaction/old/$component" || -L "$transaction/old/$component" ]]; then
        rm -rf "$install_bin/$component"
        mv "$transaction/old/$component" "$install_bin/$component" || { echo "Recovery files remain in $transaction" >&2; exit 1; }
      elif [[ ! -e "$transaction/new/$component" ]]; then
        rm -rf "$install_bin/$component"
      fi
    done
  elif bundle_in_use "$transaction/old"; then
    # current_exe() and executable-relative resources must keep working for
    # already running daemons and windows until those processes exit.
    printf 'committed\n' > "$transaction/committed"
    echo "Previous bundle retained for running processes: $transaction/old"
    exit "$result"
  fi
  rm -rf "$transaction"
  exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
for name in "${binaries[@]}"; do install -m 0755 "$payload/$name" "$transaction/new/$name"; done
for name in "${resources[@]}"; do install -m 0644 "$payload/$name" "$transaction/new/$name"; done
cp -a "$payload/web" "$transaction/new/web"
cp -a "$payload/licenses" "$transaction/new/licenses"
for component in "${components[@]}"; do
  moved+=("$component")
  if [[ -e "$install_bin/$component" || -L "$install_bin/$component" ]]; then mv "$install_bin/$component" "$transaction/old/$component"; fi
  mv "$transaction/new/$component" "$install_bin/$component"
done
committed=1
if [[ -f "$payload/neoism.desktop" && -f "$payload/neoism.png" ]]; then
  install -D -m 0644 "$payload/neoism.desktop" "$install_prefix/share/applications/neoism.desktop"
  install -D -m 0644 "$payload/neoism.png" "$install_prefix/share/icons/hicolor/512x512/apps/neoism.png"
fi
echo "Installed GPUIX Neoism to $install_bin."
