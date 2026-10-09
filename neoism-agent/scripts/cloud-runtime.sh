#!/usr/bin/env bash
set -euo pipefail

mode="${1:-check}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
spec="$root/neoism-agent/openapi/cloud-runtime.v2.json"
snapshot="$root/neoism-agent/openapi/cloud-runtime.v2.sha256"
generated="$root/neoism-agent/sdk/typescript/packages/core/src/generated/cloud-contract.ts"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

cd "$root"
cargo run --quiet -p neoism-cloud-runtime --example openapi > "$tmp/cloud-runtime.v2.json"
node -e 'let b=[]; process.stdin.on("data", c => b.push(c)).on("end", () => { const canonical = JSON.stringify(JSON.parse(Buffer.concat(b))); process.stdout.write(require("crypto").createHash("sha256").update(canonical).digest("hex")); })' < "$tmp/cloud-runtime.v2.json" > "$tmp/cloud-runtime.v2.sha256"
node neoism-agent/scripts/generate-contract.mjs "Neoism Cloud Runtime" "neoism-agent/scripts/cloud-runtime.sh update" < "$tmp/cloud-runtime.v2.json" > "$tmp/cloud-contract.ts"

case "$mode" in
  update)
    mkdir -p "$(dirname "$spec")" "$(dirname "$generated")"
    cp "$tmp/cloud-runtime.v2.json" "$spec"
    cp "$tmp/cloud-runtime.v2.sha256" "$snapshot"
    cp "$tmp/cloud-contract.ts" "$generated"
    ;;
  check)
    cmp "$tmp/cloud-runtime.v2.sha256" "$snapshot" && cmp "$tmp/cloud-contract.ts" "$generated" || {
      echo "cloud runtime contract drifted; run neoism-agent/scripts/cloud-runtime.sh update" >&2
      exit 1
    }
    node -e 'const fs=require("fs"), assert=require("assert"); const [a,b]=process.argv.slice(1).map(path => JSON.parse(fs.readFileSync(path,"utf8"))); assert.deepStrictEqual(a,b)' "$tmp/cloud-runtime.v2.json" "$spec"
    ;;
  *)
    echo "usage: $0 [check|update]" >&2
    exit 2
    ;;
esac
