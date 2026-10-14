#!/usr/bin/env bash
# Real-router source tests only; no desktop executable, agent, or public relay.
set -euo pipefail
cd "$(dirname "$0")/.."
# timeout owns only this new command group and bounds compilation plus tests.
# The integration test separately bounds every handshake/readiness assertion.
# Keep this standalone lockfile's artifacts separate from root-workspace builds.
timeout --kill-after=5s 900s env -u CI cargo test --locked \
  --manifest-path neoism-frontend/desktop/tests/transport_contract/Cargo.toml \
  --target-dir target/server-contracts \
  --test server_transport_contract contract_ -- --test-threads=1 --nocapture
