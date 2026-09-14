---
name: "MCP owner persistence, mutation publication, scope labels"
description: "Bare dedicated MCP map writes + pinned mutation stale catalog fixed; actual owner scope in shared/TS pickers, fixture regressions and caveats"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-09-14"
updated: "2026-09-14"
---

Approved MCP review fixes from ses_f5f54017cffeLhxtWZcEaAjraT implemented on main without commits/releases. Regressions were run before fixes: dedicated global bare mcp map lost beta after alpha toggle (beta Null), routed PATCH enable plus immediate GET both returned enabled:false, shared picker fixture omitted Global/Workspace label.

Fixes: NeoismConfigSourceService translates canonical SetValue mcp paths back to bare dedicated mcp.json representation (global/workspace); wrapped maps stay wrapped. Full map replacement remains safe normalization. Existing JSON writer reserializes and never preserved comments; tests accept JSONC and preserve unrelated values/siblings. Server mcp_owner helper uses exact prior reverse layer lookup; owner policy unchanged. Added explicit AppState::publish_config_mutation which bypasses active request pin and registry refresh throttle, uses existing serialized refresh_plugins publication, propagates publication failures, and returns published lease; normal reads/in-flight pins untouched. No blanket session clear.

ConfigLayer now carries host-supplied ConfigDiscoveryScope, mapped to additive nullable McpCatalogEntry.configScope (global/workspace; None on source read failure). Product/standard installation sources and deployment memory layers are Global; workspace sources Workspace; unpersisted builtin defaults Workspace. Desktop native read-only notes source Global. Both shared native/web MCP picker descriptions and action descriptions use scope; TS McpPicker similarly labels scope and read-only; canonical OpenAPI/fingerprint/TS contract regenerated. Do not infer scope from opaque source IDs.

Regression tests: adapter config::tests::dedicated_mcp_updates_preserve_representation_and_siblings covers 2 servers x global/workspace x bare/wrapped x false/true, JSONC and unrelated fields. server tests_mcp_config.rs routed test covers immediate PATCH→GET, actual global owner persistence, workspace override isolation, unaffected siblings, explicit request pin consistency, other workspace generation stability, read-only rejection. Shared picker tests cover both scopes/read-only and enable/disable. TS McpPicker DOM test + nativeParity tests pass. 26 workspace runtime lifecycle tests pass; native desktop/shared/server/adapter cargo check and wasm host check pass.

Observed caveats outside scope: plugin_adapters::api_error converts rejected read-only updates to PluginRuntimeError, so HTTP is 500 (not 400), with correct read-only rejection and no write. Full service-api test parallel run had 4 server_registry lock-contention failures; serial --test-threads=1 rerun passed 31/31. No live MCP/OAuth manual toggles performed. Routed server regression uses isolated StandardConfigSourceService, product dedicated representation verified in adapter fixtures separately. Global publication explicitly targets mutation workspace, not immediate refresh of every other cached workspace inheriting global config.
