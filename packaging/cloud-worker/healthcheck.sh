#!/bin/sh
set -eu
# Server health is expiry-aware; also check the public bootstrap as defense in depth.
# Health is unauthenticated. HTTP success alone is not worker readiness.
: "${NEOISM_AGENT_WORKER_BOOTSTRAP:?missing worker bootstrap}"
jq -e --argjson now "$(date +%s)" '.version == 1 and .expiresAt > $now' \
    "$NEOISM_AGENT_WORKER_BOOTSTRAP" >/dev/null
curl --fail --silent --show-error --max-time 3 http://127.0.0.1:4096/v2/health \
    | jq -e '.healthy == true and .deployment == "workspace-worker" and .executionAvailable == true' >/dev/null
