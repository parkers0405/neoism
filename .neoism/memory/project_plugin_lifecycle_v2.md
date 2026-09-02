---
name: "Canonical plugin lifecycle v2 — third audit complete"
description: "Third audit complete: bounded pre-drain breaks WebSocket/PTY lease cycles; teardown/refresh publication is serialized; retirements reap correctly; canonical third-party route namespace, shared validation, preflight, fallible shutdown paths, lifecycle timeouts, unlocked initial build, and descriptor-driven hosted auth are enforced."
type: "project"
scope: "project"
origin: "neoism-agent"
created: "2026-08-26"
updated: "2026-08-25"
---

# Canonical plugin lifecycle v2

Third adversarial audit completed on branch `neoism_agent_v2` (2026-08-25), extending the second-audit lifecycle.

Additional durable invariants:
- Terminal and normal retirement perform a bounded pre-drain before waiting for leases: cancel every generation-owned WebSocket through a watch channel, shut PTY resources, cancel background jobs, and tear down subagent sessions. Canonical plugin shutdown still occurs only after generation leases reach zero.
- Runtime teardown sets `closed` before acquiring the same async `reload` gate used by refresh. Refresh checks closure before and after the gate, and candidate closure-check plus publication happens while serialized. Candidates built before closure are bounded-cleaned and never published after closure.
- Retirement tasks return no generation Arc. Failed generations are moved into an explicit retained-failure queue; completed JoinHandles are synchronously reaped during activity checks, so finished handles do not block idle eviction.
- Third-party routes must use `/v2/plugins/{plugin_id}`. Legacy first-party prefixes require an explicit server-owned `RoutePrefixPolicy` allowlist; plugin `internal` metadata does not bypass policy. Reserved `/v2/tools`, `/v2/sessions`, etc. are unavailable to third parties.
- Host and testkit invoke one shared contribution validator, including per-kind duplicate checks and route metadata ID == descriptor ID.
- RegistrySnapshot exposes a generic priority accessor; system-context, prompt, and provider consumers use descending priority plus stable ID tie-breaking.
- All enabled descriptors are preflighted for API major, runtime scope, grants, and descriptor route policy before any factory create/start.
- Workspace runtime/state/tool lifecycle acquisition is fallible. Shutdown races return inactive/410/closed errors rather than lifecycle expects or panics.
- Server installation and lifecycle shutdown boundaries use Tokio timeouts and catch unwinds. Initial registry construction drops the registry mutex before async host installation, then double-checks insertion under lock and cleans duplicate/closed candidates.
- Hosted plugin session fallback is confirmed against the matched runtime route descriptor and its `:session_id`; middleware skips heuristic plugin-path ownership checks. Workspace resource IDs can no longer be falsely interpreted as session authorization.

Third-audit regression coverage includes terminal PTY/WebSocket lease cycles, refresh/close publication interleaving, retirement reaping plus idle eviction, shutdown panic/timeout containment, descriptor preflight with zero starts, reserved route policy, shared route identity validation, and descriptor-driven hosted authorization.

Verification:
- strict warning-denied core/plugin-api/builtins/server checks passed;
- plugin API: 19 unit, 9 architecture, 1 native conformance passed;
- builtins: 70 passed;
- server: 417 passed, 5 ignored (422 total), serialized;
- CLI: 38 passed;
- SDK generator test, TypeScript typecheck, and OpenAPI contract check passed;
- zero global scope references and `git diff --check` passed.

No commit or push. Unrelated frontend and Firecrawl files were preserved.
