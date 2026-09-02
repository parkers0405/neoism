---
name: "Canonical plugin lifecycle and generation retirement"
description: "Trusted-native plugin lifecycle release audit model"
type: "project"
scope: "project"
origin: "final production blockers 2026-08-01"
created: "2026-08-26"
updated: "2026-08-26"
---

Final two production blockers resolved after trusted-native audit.

Terminal retirement:
- `PluginGenerationSlot` retirement entries now retain both the retired generation Arc and JoinHandle.
- Terminal `WorkspaceRuntime::teardown` pre-drains every tracked/weak retired generation before current generation lease waiting and retirement draining. Normal refresh publication still does not pre-drain.
- Retirement draining uses one bounded deadline. Timed-out tasks are not aborted/dropped/detached: generation + live JoinHandle transfer to registry `GenerationQuarantine`.
- Registry quarantine retry bounds waiting on retained retirement tasks, retains still-running tasks, retries generation cleanup after join failures, and reports aggregate errors.
- Regressions prove app teardown releases an old post-refresh PTY-style generation lease, and a task that misses the terminal deadline transfers both task and generation ownership then cleans later.

Scoped plugin route authorization:
- Authentication now resolves descriptor-matched `RouteScope::Session` plugin paths for every authenticated scoped caller, including non-hosted callers, before session authorization.
- Matched sessions require both `allows_session` and `allows_directory`; denied credentials receive 403. Authorized match metadata supplies the exact directory to plugin dispatch.
- Dispatch's existing route-scope/workspace ownership validation remains defense in depth.
- Regressions cover denied and allowed non-hosted scoped credentials against the built-in goals session route.

Verification: strict warning-denied checks; plugin API 22 unit + 11 architecture + 1 native conformance; builtins 70; server 432 total (427 passed, 5 environment-dependent ignored) serialized; CLI 38; SDK generator/typecheck/OpenAPI; git diff --check. No commit/push.
