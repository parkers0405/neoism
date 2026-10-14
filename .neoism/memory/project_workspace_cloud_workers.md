---
name: "Whole-runtime cloud workspace workers"
description: "Implemented one VM/workspace foundation: legacy remote tool path removed; cloud lifecycle, verifier-only worker auth, typed SDK/token refresh, packaging and tests."
type: "project"
scope: "project"
origin: "User-approved cloud architecture and implementation"
created: "2026-10-09"
updated: "2026-10-09"
---

# Whole-runtime cloud workspace workers

## Architecture

User chose ONE VM PER WORKSPACE. The whole Neoism Agent server lives in that VM; multiple agents/sessions share its computer. No per-tool cloud execution. Applications own membership, infrastructure choice, VM billing, gateways and routing; model-provider billing remains separate. Clients do not expose local files by default. Desktop is separate; optional full workspace-daemon worker remains a later profile.

## Clean cut

Deleted sandbox_exec, ExecutionProvider/remote lease/materialization/process contracts, ExecutionPolicy::Sandboxed, plugin Sandboxed invocation mode, remote revision CAS/table creation and unused HostedAttestation signing listener. No aliases/fallback/legacy adapter. Local bash has a server-private local_process runner with bounded IO and timeout/cancel/drop process-tree cleanup. Retained unrelated shipped data migrations and plugin bubblewrap isolation.

## Cloud lifecycle

New neoism-cloud-runtime crate: RuntimeProvider whole-machine lifecycle, durable tenant+workspace allocations, generations, pending-intent recovery/reconciliation, atomic persistence/file locks, tombstones, readiness and nonsecret WorkerConnection metadata. Real versioned HttpProvider bridge; no native Boat/Docker adapters yet. Infrastructure bridge must enforce durable idempotency, generation fencing, actual isolation/resource budgets. Binding::worker_connection suppresses routing during pending/failure/retirement/unready state. Colon/dot owner IDs support existing workspace:<id> tenants.

## Worker bootstrap/auth

CLI: neoism-agent serve --worker-bootstrap FILE --worker-verification-key-file FILE. Env alternatives NEOISM_AGENT_WORKER_BOOTSTRAP and NEOISM_AGENT_WORKER_VERIFICATION_KEY_FILE. Bootstrap v1 fields tenantId/workspaceId/runtimeId/runtimeGeneration/canonical root/expiresAt. Stock worker installs one immutable workspace registry, bound local provider/MCP stores outside workspace, strict auth and API-only serving; no auto GUI discovery. Expiry and SIGTERM trigger bounded shutdown. Health and authenticated GET /v2/runtime report truthful camelCase deployment/live state; runtime metadata omits filesystem root and keys.

Controller-only WorkspaceWorkerSigningKey uses exact32-byte Ed25519 seed via ring. VM receives only WorkspaceWorkerVerificationKey public32 bytes. Binding cannot serialize or issue credentials. Signed tokens bind actor, scopes/quotas, directory prefix, tenant/workspace/runtime/generation; max300s, max16KiB. Worker re-verifies independently then demands resolver equality. No local/daemon fallback. Worker scope middleware agent:read/agent:use/workspace:admin; missing/unknown scopes deny. Narrow-prefix directory defaults/search are constrained; narrow event subscriptions require session selection. Worker SSE closes at credential expiry. Private provider/MCP paths reject workspace placement/final symlinks; guards are not same-user shell isolation.

Shared control planes deny native process/plugin/MCP stdio/ACP/task/LSP/PTy/formatter paths before launch. Missing resolver remains deny-all in direct state fixtures rather than panic; listener service validation still rejects missing resolver. Worker plugin/runtime lookups normalize bound tenant+root, preserve session cwd/prefixes, use real registered agent sources. Durable sessions bind logical workspace/tenant/path, not VM generation. Old generation tokens are rejected on replacement; old workers must be externally terminated/fenced, not magically revoked.

## SDK

Separate host-only CloudRuntimeClient uses explicit infrastructure credentials, bounded/redacted HTTP protocol validation, no mutation auto-retries and typed ensure/start/inspect/stop/destroy. inspect allows missing known handle to recover lost provisioning replies. No inherited agent/model credentials. client.runtime.get and plugins.lifecycle are typed. HTTP token accepts string or sync/async callback, resolved per request/SSE reconnect; abort prevents fetch and resolver errors sanitized. Browser WS requires gateway auth; no tokens in URLs. Agent+cloud OpenAPI/sha snapshots and generated TS contracts are produced/checked by scripts/openapi.sh plus cloud-runtime.sh. Native public lifecycle route was missing canonical spec; added typed PackageLifecycleInfo/HostCapability/Diagnostic schemas rather than hiding the route.

## Verification and remaining work

SDK typecheck/full test suite passed (including17 token-refresh scenarios). Service-api38 unit+3 integration+2 compile-fail doctests passed; cloud19 tests passed. HTTP worker sharing/restore/old-token rejection, root plugin lookup, local process6, bash5, caller14, shared native denial6, ACP worker admission, CLI argument and actual child-process worker bootstrap/session/expiry tests passed. OpenAPI full parity/schema suite final rerun was pending memory write; consult session final before assuming this last check complete. cargo checks passed Agent/server tests and workspace-daemon library; daemon whole tests have an unrelated existing todo_items_from_response visibility failure. No release build.

packaging/cloud-worker includes Agent-only prebuilt-artifact Dockerfile, entrypoint, healthcheck, constrained compose, controller provision.rs and docs. Native process tested, NOT Docker image run or Boat/live gateway deployment. Next: real native provider/bridge integration, persistent storage backup/upgrade operations, optional daemon/full-editor and browser/desktop cloud profiles. Account management, billing and application gateway remain host-owned. Existing root Docker image remains daemon-first.
