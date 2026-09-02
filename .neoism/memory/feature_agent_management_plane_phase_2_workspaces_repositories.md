---
name: "Agent management plane phase 2 workspaces/repositories"
description: "Workspace and repository management plane phase 2 implemented; OpenAPI snapshot conflict remains preserved"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-08-31"
updated: "2026-08-31"
---

Phase 2 workspace/repository management implemented in shared dirty worktree. `WorkspaceManagementService` + standalone bounded atomic JSON registry live in neoism-agent-service-api. Agent management routes `/v2/management/workspaces` and `/repositories` are auth+management gated, scoped by caller roots, revision-aware, symlink/traversal protected; repository create discriminates existing/clone and delete only unregisters. Daemon adapter `workspace/management_bridge.rs` injects the existing WorkspaceManager into embedded Agent; startup reordered to pass manager; clone reuses workspace_provision with bounded depth. OpenAPI source and generated TS contract/client/docs/tests updated. Verification: cargo checks pass, service tests 3 pass, daemon provision tests 2 pass, npm typecheck/test and pack dry-run pass, generated contract equals canonical temp, git diff --check pass. Known preserved snapshot conflict: dirty `openapi/v2.json` contains unrelated ConfigDefaults/background-task changes and zero management paths; `v2.sha256` is 3838b... while canonical is d24a43...; openapi.sh check therefore fails at fingerprint. Server lib tests blocked by unrelated dirty `tests_interaction_tools.rs:601` immutable parent borrowed mutably.
