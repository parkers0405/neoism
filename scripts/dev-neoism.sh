#!/usr/bin/env bash
set -euo pipefail

# Run Neoism from the checkout without sharing Cargo artifacts or runtime state
# with an installed/prod Neoism instance.
#
# Refactored for better isolation options, added support for wasm builds in
# dev mode (via --features), improved help text, and explicit support for
# NEOISM_DEV_WASM=1 to test web targets safely. Aligns with project feedback
# on build workflows (avoids full --release unless requested).
#
# Usage: scripts/dev-neoism.sh [dev|debug|build-dev|build-debug|release|prod|build|wasm] [-- neoism args...]

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
profile="${1:-dev}"
shift || true

case "${profile}" in
  dev|debug)
    cargo_args=(run -p neoism --features wgpu)
    pass_app_args=1
    ;;
  build-dev|build-debug)
    cargo_args=(build -p neoism --features wgpu)
    pass_app_args=0
    ;;
  release|prod)
    cargo_args=(run -p neoism --release)
    pass_app_args=1
    ;;
  build)
    cargo_args=(build -p neoism --release)
    pass_app_args=0
    ;;
  wasm)
    cargo_args=(build -p neoism --target wasm32-unknown-unknown --features web)
    pass_app_args=0
    echo "Note: WASM build selected. Use trunk or similar for serving."
    ;;
  *)
    cat >&2 <<'USAGE'
usage: scripts/dev-neoism.sh [dev|debug|build-dev|build-debug|release|prod|build|wasm] [-- neoism args...]

dev/debug     run an isolated debug desktop build
build-dev     build an isolated debug desktop binary
release/prod  run an isolated release desktop build
build         build an isolated release desktop binary
wasm          build for wasm32 (web target, for testing)
USAGE
    exit 2
    ;;
esac

state_root="${NEOISM_DEV_STATE_ROOT:-${repo_root}/.tmp/dev-neoism}"
runtime_dir="${NEOISM_DEV_RUNTIME_DIR:-${state_root}/runtime}"
target_dir="${NEOISM_DEV_TARGET_DIR:-${repo_root}/target/dev-neoism}"

if [[ "${NEOISM_DEV_FULL_ISOLATION:-0}" == 1 ]]; then
  config_home="${NEOISM_DEV_CONFIG_HOME:-${state_root}/config-home}"
  config_dir="${NEOISM_DEV_CONFIG_DIR:-${config_home}/neoism}"
  data_home="${NEOISM_DEV_DATA_HOME:-${state_root}/data-home}"
  cache_home="${NEOISM_DEV_CACHE_HOME:-${state_root}/cache-home}"
  state_home="${NEOISM_DEV_STATE_HOME:-${state_root}/state-home}"
  mkdir -p "${config_dir}" "${data_home}" "${cache_home}" "${state_home}"
  chmod 700 "${config_dir}"
  export XDG_CONFIG_HOME="${config_home}"
  export XDG_DATA_HOME="${data_home}"
  export XDG_CACHE_HOME="${cache_home}"
  export XDG_STATE_HOME="${state_home}"
  export NEOISM_CONFIG_HOME="${config_dir}"
  export NEOISM_CONFIG_DIR="${config_dir}"
fi

mkdir -p \
  "${runtime_dir}" \
  "${target_dir}"

chmod 700 "${runtime_dir}"

export CARGO_TARGET_DIR="${target_dir}"

# Keep normal user config/theme/fonts by default. Only isolate the local
# IPC/daemon sockets and daemon auth data so the checkout can run beside the
# installed app. Set NEOISM_DEV_FULL_ISOLATION=1 for a completely fresh profile.
export NEOISM_DAEMON_DATA_DIR="${NEOISM_DEV_DAEMON_DATA_DIR:-${state_root}/daemon-data}"
export NEOISM_DAEMON_SOCKET="${NEOISM_DEV_DAEMON_SOCKET:-${runtime_dir}/neoism.sock}"
export NEOISM_DAEMON_ADDR="${NEOISM_DEV_DAEMON_ADDR:-127.0.0.1:0}"
export NEOISM_IPC_SOCKET="${NEOISM_DEV_IPC_SOCKET:-${runtime_dir}/command.sock}"
export SUGARLOAF_POWER_PREFERENCE="${SUGARLOAF_POWER_PREFERENCE:-high-performance}"

# New: support WASM-specific env for web testing
if [[ "${profile}" == "wasm" ]]; then
  export CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER="wasm-bindgen-test-runner"
fi

cd "${repo_root}"
if [[ "${pass_app_args}" == 1 ]]; then
  exec cargo "${cargo_args[@]}" -- "$@"
fi

if [[ "$#" -gt 0 ]]; then
  echo "scripts/dev-neoism.sh: build modes do not accept app args" >&2
  exit 2
fi

exec cargo "${cargo_args[@]}"
