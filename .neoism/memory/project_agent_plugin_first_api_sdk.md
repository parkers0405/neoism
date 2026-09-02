---
name: "Agent plugin-first API + SDK"
description: "Plugin-first Agent platform completed with canonical API, generic SDK plugin access, hosted security, sandboxed external plugins, and honest optional built-in disablement."
type: "project"
scope: "project"
origin: "session implementation"
created: "2026-08-24"
updated: "2026-08-24"
---

## Current architecture
Neoism Agent is an OpenCode V2-style plugin-first platform. The kernel owns sessions/messages, run execution, persistence, durable events, permissions, secrets, artifacts, provider execution/system prompts, and plugin supervision.

## Implemented
- Canonical `/v2` API with deterministic OpenAPI fingerprinting, generated TypeScript contracts, router parity tests, durable resumable SSE, interactions, artifacts, audit, catalogs, provider auth, and session controls.
- Frontend-neutral TypeScript SDK split into core/HTTP/Node/optional packages. `client.plugins.request<T>()` consumes arbitrary plugin APIs under `/v2/plugins/{id}` without coupling SDK core to plugin payloads.
- Public Rust plugin API with immutable dependency-ordered snapshots and typed agents, skills, commands, runtime tools, host-enforced permission metadata, and versioned subprocess hook DTOs.
- External plugins use process-per-invocation `neoism-plugin/1`, bounded timeout/output, Bubblewrap sandboxing, and recoverable health reporting.
- Hosted mode has token claims, tenant/directory ownership, rate/in-flight/session/artifact quotas, audit records, artifact retention/scanning, safe credential restrictions, and secure non-loopback defaults.
- Optional built-ins are honestly workspace-disableable across discovery and execution: agents, skills, commands, websearch, subagents, MCP, LSP, VCS, workflows, PTY, semantic search, goals, notes tools, and workspace filesystem/search/background tools.
- MCP/LSP/VCS/workflow/PTY/semantic/goals expose canonical plugin routes. Legacy routes remain for compatibility. Providers and system prompts intentionally remain non-disableable kernel run services.
- First-party workspace daemon uses canonical `/v2` calls, including resumable `tail=true` events, configured providers, and background-job cancellation.

## Verification
- OpenAPI byte/fingerprint drift, router parity, SDK `tsc -b`, plugin API/core tests, CLI/server/daemon checks, disablement tests, sandbox tests, and targeted goals/MCP/LSP/workflow route tests pass.
- Full serial server suite: 474 passed, 5 ignored. Two unrelated pre-existing platform-path tests fail on Linux: `safe_filesystem_tools_execute_inside_project` resolves a Windows-style `src\\lib.rs`, and `windows_process::detects_drive_and_verbatim_roots` expects `C:\\` to be a filesystem root. A parallel LSP crash/restart test is timing-sensitive but passes targeted and serial runs.

## Known limitation
Persisted interaction requests survive restart, but original in-flight run waiters cannot be reconstructed. True continuation requires durable run requeue semantics.
