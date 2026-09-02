---
name: "Agent plugin-first API + SDK"
description: "Plugin-first Agent platform now has canonical V2, generated SDK contracts, real skills/commands/websearch runtime plugins, and bounded subprocess hooks; larger plugin migrations and hosted isolation remain."
type: "project"
scope: "project"
origin: "session implementation"
created: "2026-08-24"
updated: "2026-08-24"
---

## Current architecture
Neoism Agent is an OpenCode V2-style platform with a small core and public plugin API. Core owns sessions/messages, runs, persistence, durable events, permissions, secrets, artifacts, and plugin supervision.

## Implemented
- Canonical `/v2` API includes sessions, controls, messages, durable SSE, interactions, artifacts, catalogs, provider auth, commands, plugins, capabilities, and canonical-only OpenAPI.
- TypeScript SDK is transport-neutral and includes HTTP/Node adapters, resumable SSE, binary artifacts, catalogs, interactions, session controls, and optional subagent package.
- Deterministic contract pipeline: offline `neoism-agent openapi`, canonical SHA-256 fingerprint, dependency-free generated `Contract` TypeScript namespace, byte drift script, and CI check.
- First-party workspace daemon uses `/v2` for nearly all core traffic, including race-safe `tail=true` SSE normalized into its existing UI events.
- Durable interactions restore after restart, resolve idempotently, and cancel on session abort/delete. Original run waiters cannot resume after restart.
- Public plugin API now retains typed runtime contributions in immutable snapshots: async skill sources, command sources, and runtime tools with host-enforced permission metadata.
- Skills and commands are real runtime plugins and honestly workspace-disableable across discovery and execution.
- Web search is a real runtime tool plugin; its permission name/target argument is declared as trusted host metadata and enforced before plugin execution.
- Subagents remain honestly disableable and most task behavior is extracted under `plugins/subagents.rs`.
- External declarative plugins can specify a subprocess `command` using versioned `neoism-plugin/1` JSON hooks. Each invocation is process-isolated, timeout-bound, killed on timeout, and response-limited to 4 MiB. This avoids an unstable Rust dylib ABI; stronger OS sandboxing and health/restart status remain.
- Auth: bearer token, secure non-loopback default, CORS enforcement, daemon forwarding, Docker token wiring.

## Remaining
- Migrate larger built-ins through actual runtime contracts: agents, providers, MCP, LSP, VCS, workflows, PTY, goals, notes/workspace tools.
- Add OS-level subprocess sandboxing and persistent plugin health/restart reporting.
- Add tenant ownership/scoped claims, quotas, audit log, and artifact retention/scanning.
- Finish minor legacy/plugin-specific daemon calls and broad conformance tests.

## Verification
Plugin API tests, runtime source/tool disablement tests, subprocess protocol test, interaction persistence tests, OpenAPI router parity, deterministic contract drift, SDK `tsc -b`, and targeted server/daemon checks pass. Repository-wide rustfmt still has broad pre-existing drift in this in-progress branch.
