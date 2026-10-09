#!/usr/bin/env bash
# Stage only owned binaries/scripts and their runtime libraries; never send the workspace to Docker.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
image="${1:-neoism-agent-worker:development}"
agent="${NEOISM_AGENT_BINARY:-$root/target/debug/neoism-agent}"
lua="${NEOISM_AGENT_LUA_RUNNER_BINARY:-$root/target/debug/neoism-agent-lua-runner}"
[[ "$(uname -s)" == Linux ]] || { echo 'The native development image requires Linux binaries.' >&2; exit 1; }
[[ -x "$agent" && -x "$lua" ]] || { echo 'Supply executable Agent and Lua-runner binaries before building the image.' >&2; exit 1; }
context="$(mktemp -d)"
trap 'rm -rf "$context"' EXIT
mkdir -p "$context/worker-artifacts/native-libs" "$context/packaging/cloud-worker"
cp "$agent" "$context/worker-artifacts/neoism-agent"
cp "$lua" "$context/worker-artifacts/neoism-agent-lua-runner"
for binary in "$agent" "$lua"; do
  while IFS= read -r library; do
    cp -L "$library" "$context/worker-artifacts/native-libs/$(basename "$library")"
  done < <(ldd "$binary" | awk '/=> \// { print $3 } /^[[:space:]]*\// { print $1 }')
done
cp "$root/packaging/cloud-worker/"{Dockerfile.development,entrypoint.sh,healthcheck.sh} "$context/packaging/cloud-worker/"
docker build --file "$context/packaging/cloud-worker/Dockerfile.development" --tag "$image" "$context"
