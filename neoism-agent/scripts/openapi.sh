#!/usr/bin/env bash
set -euo pipefail

mode="${1:-check}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
snapshot="$root/neoism-agent/openapi/v2.sha256"
spec="$root/neoism-agent/openapi/v2.json"
generated="$root/neoism-agent/sdk/typescript/packages/core/src/generated/contract.ts"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

cd "$root"
cargo run --quiet -p neoism-agent -- openapi > "$tmp/v2.json"
node -e 'let b=[]; process.stdin.on("data", c => b.push(c)).on("end", () => { const canonical = JSON.stringify(JSON.parse(Buffer.concat(b))); process.stdout.write(require("crypto").createHash("sha256").update(canonical).digest("hex")); })' < "$tmp/v2.json" > "$tmp/v2.sha256"
node neoism-agent/scripts/generate-contract.mjs < "$tmp/v2.json" > "$tmp/contract.ts"

case "$mode" in
  update)
    mkdir -p "$(dirname "$snapshot")" "$(dirname "$generated")"
    cp "$tmp/v2.sha256" "$snapshot"
    cp "$tmp/v2.json" "$spec"
    cp "$tmp/contract.ts" "$generated"
    ;;
  check)
    cmp "$tmp/v2.sha256" "$snapshot" || {
      echo "canonical OpenAPI fingerprint drifted; run neoism-agent/scripts/openapi.sh update" >&2
      exit 1
    }
    node -e 'const fs=require("fs"), assert=require("assert"); const [generated,snapshot]=process.argv.slice(1).map(path => JSON.parse(fs.readFileSync(path,"utf8"))); assert.deepStrictEqual(generated,snapshot)' "$tmp/v2.json" "$spec" || {
      echo "committed OpenAPI document drifted; run neoism-agent/scripts/openapi.sh update" >&2
      exit 1
    }
    cmp "$tmp/contract.ts" "$generated" || {
      echo "generated TypeScript contract drifted; run neoism-agent/scripts/openapi.sh update" >&2
      exit 1
    }
    ;;
  *)
    echo "usage: $0 [check|update]" >&2
    exit 2
    ;;
esac