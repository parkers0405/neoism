---
name: "Hosted provider workspace credential error — FIXED"
description: "v0.7.76 hosted credential guard rejected all workspace daemon prompts; fixed generation scope fallback for exact workspace delegation."
type: "bug"
scope: "project"
origin: "session"
created: "2026-09-02"
updated: "2026-09-02"
---

# Hosted provider credential guard broke workspace Agent prompts — FIXED

## Symptom
Joined/daemon-scoped Agent prompts displayed:
`Neoism: hosted provider credentials require an injected tenant-isolated store`

## Regression
Introduced by v0.7.76 (`6b30dc6c2e84a7105b57038e5bff7b23c7ea4d0c`). Workspace daemon identities intentionally use `tenant_id=workspace:<workspace_id>` and `hosted=true`, while Neoism injects `LocalProviderCredentialStore` (`supports_hosted_scopes=false`). `ProviderRegistry::stream` rejected the request before provider selection.

The route layer already maps authenticated matching workspace read requests to host-local scope in `neoism-agent-builtins/src/plugin/providers/mod.rs`, but generation bypasses that mapper.

## Fix
In `neoism-agent-builtins/src/provider.rs`, generation credential scope now:
- uses local credentials only when `tenant_id` exactly equals `workspace:<workspace_id>` and the store is local-only;
- keeps arbitrary hosted/mismatched scopes fail-closed;
- preserves the original tenant/workspace scope when a real hosted-capable store is injected.

Provider request metadata remains workspace-scoped; only credential resolution uses local scope. Guest never receives serialized credentials. Tests in `provider_tests.rs` cover matching delegation, arbitrary/mismatched rejection, and hosted-store scope preservation.

## Verification
- `cargo test -p neoism-agent-builtins`: 80 passed
- `cargo check -p neoism-workspace-daemon`: passed
- `git diff --check`: passed
- Workspace-wide `cargo fmt --check` remains pre-existing red across many unrelated files; touched patch whitespace is clean.
