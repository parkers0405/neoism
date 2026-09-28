---
name: "Pre-chat provider selectors"
description: "T3-style draft provider selectors and disk-cached ACP preflight"
type: "feature"
scope: "project"
origin: "user task and upstream source comparison"
created: "2026-09-25"
updated: "2026-09-25"
---

2026-09-25 follow-up: documented GET/POST `/v2/external/options/preview` in authoritative `openapi.rs`, including conditional selectedOptions, catalogStale, and root creation externalProvider/externalOptions. Also documented six already-undocumented routes (execution activity snapshot/SSE and external catalog/import/session options) to restore router/spec parity. All 11 OpenAPI tests, 7 focused preview tests, `cargo check -p neoism-agent-server -p neoism-ui -p neoism`, and git diff --check pass. Cache review: adapter path/args/env/executable metadata included in key but provider-owned auth/config files are not fingerprinted, so account switches may retain display options for fresh 10m window; first-send revalidates live adapter. No running-GUI verification.
