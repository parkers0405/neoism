---
name: "Yarn PnP TypeScript SDK support"
description: "Automatic bounded Yarn PnP detection, cached patched TS SDK selection, degraded fallback, protocol/UI observability, and content identity."
type: "feature"
scope: "project"
origin: "Implementation review revision"
created: "2026-09-24"
updated: "2026-09-24"
---

Revised after independent review. TypeScript SDK discovery now walks from the resolved nested package root up to (and never above) the opened workspace boundary, looking for canonical Yarn `.pnp.cjs` and `.yarn/sdks/typescript/lib`; `.pnp.js` is intentionally not supported because modern Yarn's canonical loader is `.pnp.cjs`. Missing SDK is degraded, not fatal: TLS launches its normal fallback, status remains Available/Connected with actionable warning, and an already-running SDK-backed client is reused if only the automatic SDK disappeared (real endpoint/settings/user-init changes still replace it). SDK probes use a bounded 128-entry, 2-second TTL cache; expensive wrapper reads/package parsing happen only on refresh, so appearance/change is observed within 2s. Identity hashes SDK wrapper/package contents, catching same-size replacements; deletion falls back without needless live-client eviction. Runtime source/path/version is now carried through agent JSON/OpenAPI, public language-server exports, daemon LspSnapshot protocol, desktop/remote/wasm mapping, and the Server Details Runtime row. Tests cover nested monorepo, boundary, missing fallback, option merge/precedence, malformed options, cache behavior, same-size replacement/deletion, status JSON, OpenAPI, protocol roundtrip, daemon projection (source test added but daemon lib-test compilation currently blocked by unrelated sessions.rs errors), and UI runtime rendering.
