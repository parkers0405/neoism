---
name: "Global agent Skills and Workflows"
description: "Global Skills/Workflows override project default; complete global APIs/UI,563 tests; installed backend still lacks capability"
type: "project"
scope: "project"
origin: "user"
created: "2026-09-10"
updated: "2026-09-10"
---

LATEST USER CORRECTION: Skills/Workflows Library must use GLOBAL agent resources, not current project. This supersedes earlier project-bound default notes. Implemented global config/discovery roots via service-api + native adapter (respect env overrides). Skill operations use scope=installation including list/get/create/update/delete/versions/restore. Workflows add explicit scope=installation and write <configured-global-root>/workflows with matching discovery/watches, edit/activate/history; omitted scope preserves existing workspace API behavior. Execution directory remains distinct from workflow definition storage. GUI no project picker/root-resolution requirement in global Library, no passed directory from App, and Library key excludes selected directory so switching projects does NOT discard global draft. Capability neoism.resources.installation verifies support; no silent project fallback on older backend. Auth/management opt-in/hosted restrictions preserved.

Parent verification: GUI build +563 tests passed, no whitespace errors. Child verified SDK/OpenAPI +19 workflow/20 management/13 native-config tests and cargo check. Read-only current :4096 capability check returned globalResources=false and management=false, so installed backend still requires normal rebuild/restart and authorized management before global writes. No restart or release build performed.

Also latest home footer correction: project selector + hints sit immediately BELOW centered input in normal flow (position:relative;margin10px auto0), not viewport-bottom. Hidden inside chat; chat scanner stays right in model/thinking strip, input lower. App.layout regression covers this. No browser tools used.
