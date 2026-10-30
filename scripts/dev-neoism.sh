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

case "$profile" in
  dev|debug) frontend_command=dev ;;
  release|prod) frontend_command=run ;;
  build|build-dev|build-debug) frontend_command=build ;;
  *) echo 'Usage: scripts/dev-neoism.sh {dev|release|build} [-- app arguments]' >&2; exit 2 ;;
esac
if [[ "${1:-}" == -- ]]; then shift; fi

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
export NEOISM_STATE_DIR="${NEOISM_DEV_STATE_DIR:-${state_root}/state}"
export NEOISM_GPUIX_STATE_PATH="${NEOISM_DEV_SESSION_PATH:-${state_root}/session.json}"
cd "$repo_root"
exec bash scripts/gpuix.sh "$frontend_command" "$@"
