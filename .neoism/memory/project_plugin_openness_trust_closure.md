---
name: "Plugin openness trust/broker closure"
description: "Credential keyring broker, exact extension trust approvals, supervised native/Tree-sitter host, opaque daemon resources, Agent approval routing"
type: "project"
scope: "project"
origin: "current session"
created: "2026-08-06"
updated: "2026-08-06"
---

# Plugin openness trust/broker closure

Implemented in dirty worktree 2026-08-06:
- `neoism-extensions/src/trust.rs`: host-owned `trust-v1.json` outside repositories, exact plugin/revision/package digest/artifact digest/ABI/capability/scope approvals, approved/revoked/failed state, bounded audit, atomic mode-0600 writes.
- Desktop `credential_broker.rs`: OS keyring via keyring crate (macOS Keychain, Windows Credential Manager, Linux Secret Service), metadata-only JSON, aliases, exact PluginOwner grants, user/workspace scopes, host allowlist, revoke/audit; `lua_network` resolves aliases and injects bearer authorization while Lua never receives the secret. `credential:<alias>` capabilities grant/revoke through Extensions UI.
- `native_extension.rs`: native ABI v1 and Tree-sitter modules load only in supervised child Neoism processes, with approval/checksum/platform/symbol/ABI checks, 5s startup timeout, crash containment, async bounded retirement; activation/failure audited. Tree-sitter checks actual runtime ABI and parser acceptance.
- Extension UI lifecycle includes PermissionRequired/Approved/Revoked/Failed; protocol ExtensionSummary includes `trust_lifecycle`; daemon merges approval records into extension inventory.
- `plugin_resource.rs` protocol + daemon broker: socket/workspace/owner/revision/generation-bound opaque handles for files/directories/watch/task/test/real PTY/DAP process resources, bounded I/O, cancel/close/owner-close, old-generation and disconnect cleanup, no host paths/FDs/PIDs returned.
- Agent mutation bridge now routes only matching pending requests to native Agent approval state and never silently approves.
- Parser registration validates immutable checksum, declared/actual ABI 13..15, platform and symbol before activation; existing immutable install publication supplies rollback.

Verification: cargo checks passed for neoism-extensions/workspace-daemon/desktop; focused `approval_is_exact_and_revocable` test passed. No broad suite/release build/format/commit.
