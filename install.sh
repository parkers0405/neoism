#!/usr/bin/env bash
# Build the branch's GPUIX frontend into a separate preview installation.
set -euo pipefail
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
exec bash "$repo_root/scripts/gpuix.sh" install "$@"
