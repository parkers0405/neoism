#!/usr/bin/env bash
set -euo pipefail
mode="${1:-check}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
spec="$root/neoism-agent/openapi/cloud-host.v1.json"
snapshot="$root/neoism-agent/openapi/cloud-host.v1.sha256"
generated="$root/neoism-agent/sdk/typescript/packages/core/src/generated/host-contract.ts"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
cd "$root"
cargo run --quiet -p neoism-cloud-host --example openapi > "$tmp/cloud-host.v1.json"
node -e 'let b=[]; process.stdin.on("data", c => b.push(c)).on("end", () => { const canonical = JSON.stringify(JSON.parse(Buffer.concat(b))); process.stdout.write(require("crypto").createHash("sha256").update(canonical).digest("hex")); })' < "$tmp/cloud-host.v1.json" > "$tmp/cloud-host.v1.sha256"
node neoism-agent/scripts/generate-contract.mjs "Neoism Cloud Host" "neoism-agent/scripts/cloud-host.sh update" < "$tmp/cloud-host.v1.json" > "$tmp/host-contract.ts"
case "$mode" in
  update)
    mkdir -p "$(dirname "$spec")" "$(dirname "$generated")"
    cp "$tmp/cloud-host.v1.json" "$spec"
    cp "$tmp/cloud-host.v1.sha256" "$snapshot"
    cp "$tmp/host-contract.ts" "$generated"
    ;;
  check)
    cmp "$tmp/cloud-host.v1.sha256" "$snapshot" && cmp "$tmp/host-contract.ts" "$generated" || {
      echo "cloud host contract drifted; run neoism-agent/scripts/cloud-host.sh update" >&2
      exit 1
    }
    node -e 'const fs=require("fs"), assert=require("assert"); const [a,b]=process.argv.slice(1).map(path => JSON.parse(fs.readFileSync(path,"utf8"))); assert.deepStrictEqual(a,b)' "$tmp/cloud-host.v1.json" "$spec"
    ;;
  *) echo "usage: $0 [check|update]" >&2; exit 2 ;;
esac
