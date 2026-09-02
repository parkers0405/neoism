---
name: "Agent management plane phase 1"
description: "Phase 1 Agent management plane implementation and verification status"
type: "feature"
scope: "project"
origin: "session implementation 2026-08-31"
created: "2026-08-31"
updated: "2026-08-31"
---

Phase 1 implemented in neoism-agent-server: disabled-by-default ManagementPolicy controlled by NEOISM_AGENT_MANAGEMENT_API=1 and injectable constructor; explicit management operation classification rejects hosted claims and mutations require CallerClaims. /v2/management agents/commands/skills CRUD uses ConfigSourceService discovery roots, WorkspaceRuntime refresh, slug/path/symlink protections, SHA-256 revisions, expectedRevision/If-Match, deterministic markdown, fsync+rename writes. Skills support bounded inline bundles, expanded legacy-compatible frontmatter, immutable schema migration 4 skill_versions, restore and history surviving deletion. Core TS SDK has client.management agents/commands/skills and recording tests; canonical OpenAPI source and generated TS contract include operation IDs/schemas, while disabled served OpenAPI strips management. Focused Rust/TS tests and cargo check pass. Committed v2.json/hash snapshot check remains drifted because those files already contained unrelated in-progress edits and were preserved; canonical OpenAPI tests pass.
