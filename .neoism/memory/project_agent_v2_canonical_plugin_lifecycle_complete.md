---
name: "Agent V2 canonical plugin lifecycle complete"
description: "Single canonical Agent plugin lifecycle shipped in e75eee8b; supersedes incorrect completion claim at 0a954d31a"
type: "project"
scope: "project"
origin: "session completion 2026-08-25"
created: "2026-08-26"
updated: "2026-08-25"
---

Agent V2 plugin lifecycle cutover completed and pushed on `neoism_agent_v2` in commit `e75eee8b8716882ea3bedf26307824e3b0b91377` (`Unify Agent plugin lifecycle`), following intermediate generation-safety commit `0a954d31aa2ab7c50946b07442b8a254122bcfe5`.

The earlier completion claim for `0a954d31a` was incorrect because production still used `AgentPlugin`/`PluginRegistrar` while `PluginFactory`/`PluginInstance` was unused. This commit removes that duplicate production lifecycle.

Final invariants:
- One canonical async `PluginFactory -> PluginInstance` lifecycle; zero production `AgentPlugin`/`PluginRegistrar` paths enforced by architecture guards.
- API v2 major negotiation before lifecycle side effects; explicit trusted/cooperative in-process native factory boundary. Configured third-party behavior remains declarative sandboxed process hooks.
- Every contribution kind preserved: services/sources/tools/hooks/HTTP/WebSocket/events/parts.
- Disabled factories skipped before create and resources structurally inaccessible.
- Capability attenuation, provenance-bound first-party route prefixes, canonical third-party namespaces, descriptor-driven route/session authorization.
- Generation leases own all resource access; refresh is serialized/monotonic and preserves LKG; retired generations admit no new leases.
- Refresh does not pre-drain leased old generations. Terminal teardown pre-drains current and retired transport owners, uses bounded waits, and transfers timed-out cleanup ownership to quarantine/reapers.
- Failed install/retirement cleanup retains ownership for retry; terminal shutdown and registry closure are bounded/terminal.
- Service priority honored; host/testkit share validation.

Final verification:
- Strict `-D warnings` checks passed for core, plugin-api, builtins, server, CLI.
- Plugin API 22 unit tests, 11 architecture guards, 1 native conformance passed.
- Builtins 70 passed.
- Server 432 tests: 427 passed, 5 environment-dependent ignored.
- CLI 38 passed.
- SDK generator and TypeScript typecheck passed.
- OpenAPI contract and `git diff --check` passed.
- Only `Cargo.lock` and `neoism-agent/**` committed; unrelated frontend/Firecrawl work remains unstaged.
