---
name: "Agent management plane — publication hardening"
description: "Management API and SDK publication hardening completed; credentials tenancy deferred"
type: "project"
scope: "project"
origin: "neoism-agent"
created: "2026-08-31"
updated: "2026-08-31"
---

Final hardening/reconciliation completed 2026-08-31 in shared worktree. Management remains disabled by default (`NEOISM_AGENT_MANAGEMENT_API=1` enables capability/routes); all `/v2/management` reads and writes require local static operator auth, while trusted-loopback runtime, hosted claims, and workspace-scoped daemon credentials do not grant management. Existing provider/MCP runtime and OAuth routes remain unchanged; shared provider/MCP credential tenancy, JWT, and RBAC are deferred. Hardened canonicalized resource roots, caller root authorization, bounded skill bundle hashing, duplicate workspace-root update rejection, and daemon custom workspace IDs/pre-clone collision checks. Added disabled/enabled route/auth tests and management SDK example. Canonical v2.json/hash/TS contract reconciled; OpenAPI router/spec parity and deterministic check pass. `@neoism/sdk` exports management workspaces/repositories/agents/commands/skills plus optional workflow CRUD/run retry through builtins. Full server (497), service API, and daemon lib tests passed; Rust checks, npm typecheck/tests, npm publish dry-run, and clean local tarball install/import passed. Publish dry-run now restores rewritten manifests/lockfile.
