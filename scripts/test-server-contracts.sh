#!/usr/bin/env bash
# Production daemon routing plus GPUIX host/SSH/collaboration contracts. No windows or public relay.
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"
bash "$repo_root/scripts/blaze.sh" test --locked -p neoism-workspace-daemon server::
cd neoism-frontend/gpuix
node_modules/.bin/bun test tests/server-host.test.ts tests/server-connection.test.ts tests/server-manager.test.ts tests/ssh-transport.test.ts tests/presence-collaboration.test.ts
