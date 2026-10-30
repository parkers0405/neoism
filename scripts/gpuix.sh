#!/usr/bin/env bash
# The branch's canonical build/run/install path. Services remain daemon-owned.
set -euo pipefail
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
frontend="$repo_root/neoism-frontend/gpuix"
command_name="${1:-run}"
if (($#)); then shift; fi
export NEOISM_GPUIX_RUST_BUILDER="${NEOISM_GPUIX_RUST_BUILDER:-blaze}"
ensure_js() {
  command -v npm >/dev/null || { echo 'Node.js and npm are required to build Neoism.' >&2; exit 1; }
  if [[ ! -d "$repo_root/neoism-agent/sdk/typescript/node_modules" ]]; then npm --prefix "$repo_root/neoism-agent/sdk/typescript" ci; fi
  if [[ ! -x "$frontend/node_modules/.bin/bun" ]]; then npm --prefix "$frontend" ci; fi
  if ! "$frontend/node_modules/.bin/bun" --version >/dev/null 2>&1; then
    local runtime_tmp
    runtime_tmp="$(mktemp -d "$frontend/node_modules/.bun-setup.XXXXXX")"
    TMPDIR="$runtime_tmp" npm --prefix "$frontend" run setup:runtime
    rm -rf "$runtime_tmp"
  fi
}
build_services() {
  # Service compilation retains a large database. Use a private compiler home
  # and close it before the native frontend loads, sharing the pinned toolchain,
  # library sources, and persistent function cache across both stages.
  local shared_home="${BLAZE_HOME:-$frontend/artifacts/b}" service_home result=0
  mkdir -p -m 0700 "$shared_home" "$repo_root/target"
  shared_home="$(cd "$shared_home" && pwd)"
  mkdir -p "$shared_home/library" "$shared_home/toolchains"
  service_home="$repo_root/target/blaze-services"
  mkdir -p -m 0700 "$service_home"
  ln -sfn "$shared_home/library" "$service_home/library"
  ln -sfn "$shared_home/toolchains" "$service_home/toolchains"
  (cd "$repo_root" && BLAZE_HOME="$service_home" bash "$repo_root/scripts/blaze.sh" build --locked --profile activation -p neoism-workspace-daemon -p neoism-agent) || result=$?
  if ! BLAZE_HOME="$service_home" bash "$repo_root/scripts/blaze.sh" stop; then
    echo 'Unable to release the service compiler worker.' >&2
    if ((result == 0)); then result=1; fi
  fi
  if ((result != 0)); then return "$result"; fi
  export NEOISM_GPUIX_WORKSPACE_DAEMON="$repo_root/target/activation/neoism-workspace-daemon"
}
build_frontend() {
  ensure_js
  for service in neoism-workspace-daemon neoism-agent; do
    if [[ ! -x "$repo_root/target/activation/$service" ]]; then
      echo "Missing built service $service; run scripts/gpuix.sh build first." >&2
      return 1
    fi
  done
  export NEOISM_GPUIX_WORKSPACE_DAEMON="$repo_root/target/activation/neoism-workspace-daemon"
  (cd "$frontend" && npm run build)
  npm --prefix "$repo_root/neoism-agent/sdk/typescript" run gui:build
  node "$repo_root/scripts/package-agent-gui.mjs" "$repo_root/neoism-agent/sdk/typescript/packages/gui/dist" "$frontend/dist/web/agent-gui"
  install -m 0755 "$repo_root/target/activation/neoism-agent" "$frontend/dist/neoism-agent"
  cp "$repo_root/LICENSE" "$repo_root/NOTICE" "$frontend/dist/"
  mkdir -p "$frontend/dist/licenses"
  cp "$repo_root/neoism-paint/src/font/resources/CascadiaCode/LICENSE" "$frontend/dist/licenses/CascadiaCode.txt"
  cp "$repo_root/neoism-paint/src/font/resources/SymbolsNerdFontMono/LICENSE" "$frontend/dist/licenses/SymbolsNerdFontMono.txt"
  cp "$repo_root/neoism-paint/src/font/resources/Geist/OFL.txt" "$frontend/dist/licenses/Geist-OFL.txt"
  cp "$repo_root/neoism-paint/src/font/resources/PressStart2P/OFL.txt" "$frontend/dist/licenses/PressStart2P-OFL.txt"
  cp "$repo_root/vendor/blaze-compat/icu_collator_data-2.4.0/LICENSE" "$frontend/dist/licenses/ICU4X.txt"
  cp "$repo_root/vendor/blaze-compat/licenses/Tantivy.txt" "$frontend/dist/licenses/Tantivy.txt"
  cp "$repo_root/vendor/blaze-compat/licenses/Tantivy-AUTHORS.txt" "$frontend/dist/licenses/Tantivy-AUTHORS.txt"
  cp "$repo_root/vendor/blaze-compat/licenses/Turso.txt" "$frontend/dist/licenses/Turso.txt"
  cp "$frontend/node_modules/@gpuix/native/LICENSE" "$frontend/dist/licenses/GPUIX-native.txt"
  cp "$frontend/node_modules/@gpuix/solid/LICENSE" "$frontend/dist/licenses/GPUIX-solid.txt"
  cp "$repo_root/misc/neoism.desktop" "$frontend/dist/neoism.desktop"
  cp "$repo_root/neoism-frontend/shared/assets/icons/neoism.png" "$frontend/dist/neoism.png"

}
build() {
  ensure_js
  build_services
  build_frontend
}
case "$command_name" in
  build) build ;;
  build-frontend) build_frontend ;;
  run|dev)
    ensure_js
    build_services
    if [[ "$command_name" == run ]]; then export NEOISM_GPUIX_BUILD_PROFILE="${NEOISM_GPUIX_BUILD_PROFILE:-release}"; fi
    (cd "$frontend" && npm run build:core)
    # Preserve the invoking directory for the daemon's initial workspace.
    export NEOISM_LAUNCH_DIRECTORY="$PWD"
    cd "$frontend"
    if [[ "$command_name" == dev ]]; then exec npm run dev -- "$@"; fi
    exec npm start -- "$@"
    ;;
  check)
    ensure_js
    (cd "$frontend" && npm run typecheck && npm run check:design)
    (cd "$repo_root" && bash "$repo_root/scripts/blaze.sh" check --locked --workspace)
    bash "$repo_root/scripts/check-frontend-deps.sh"
    CARGO_TARGET_DIR="${NEOISM_GPUIX_CARGO_TARGET_DIR:-$repo_root/target/gpuix-core-blaze}" bash "$repo_root/scripts/blaze.sh" check --locked --manifest-path "$frontend/native/Cargo.toml" --lib
    ;;
  test)
    ensure_js
    build_services
    (cd "$frontend" && NEOISM_GPUIX_BUILD_PROFILE=release npm run build:core)
    (cd "$repo_root" && node "$repo_root/scripts/test-workspace.mjs" --locked)
    CARGO_TARGET_DIR="${NEOISM_GPUIX_CARGO_TARGET_DIR:-$repo_root/target/gpuix-core-blaze}" bash "$repo_root/scripts/blaze.sh" test --locked --manifest-path "$frontend/native/Cargo.toml" --lib --release
    (cd "$frontend" && npm test)
    ;;
  install)
    # Keep the experimental frontend separate from the daily desktop launcher.
    # Replacing that launcher requires an explicit --prefix or PREFIX override.
    install_prefix="${PREFIX:-$HOME/.local/lib/neoism-gpuix}"
    install_bin="${BIN_DIR:-}"
    while (($#)); do
      case "$1" in
        --prefix) install_prefix="${2:?--prefix needs a directory}"; shift 2 ;;
        --bin-dir) install_bin="${2:?--bin-dir needs a directory}"; shift 2 ;;
        --skip-system-deps|--skip-web) shift ;; # Dependencies are prerequisites; the retired WASM client is not built.
        --help|-h)
          echo 'Usage: ./install.sh [--prefix DIR] [--bin-dir DIR] [--skip-system-deps] [--skip-web]'
          echo 'Default: ~/.local/lib/neoism-gpuix/bin/neoism (separate preview installation).'
          exit 0 ;;
        *) echo "Unknown install option: $1" >&2; exit 2 ;;
      esac
    done
    install_bin="${install_bin:-$install_prefix/bin}"
    build
    bash "$repo_root/scripts/install-gpuix-bundle.sh" "$frontend/dist" "$install_prefix" "$install_bin"
    ;;
  *) echo 'Usage: scripts/gpuix.sh {build|build-frontend|run|dev|check|test|install} [arguments...]' >&2; exit 2 ;;
esac
