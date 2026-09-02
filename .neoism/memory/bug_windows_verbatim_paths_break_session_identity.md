---
name: "Windows verbatim paths break session identity"
description: "Windows verbatim path leakage caused missing agent sessions; normalize public paths with dunce on Windows across desktop, agent, daemon, CRDT, LSP, ACP, and workspace index"
type: "bug"
scope: "project"
origin: "interactive fix after Windows user-root agent sidebar bug"
created: "2026-08-20"
updated: "2026-08-20"
---

## Root cause

On Windows, `std::fs::canonicalize` / `Path::canonicalize` emits verbatim paths such as `\\?\C:\Users\Alice`. The agent server canonicalizes session creation through its dunce-backed helper and stores `C:\Users\Alice`, while the desktop sidebar queried with the verbatim pane directory. `filter_sessions` used strict string equality, so active sessions disappeared specifically when the workspace/agent directory was the Windows user-profile root.

## Invariant

Paths that cross a string, protocol, persistence, URI, process/tool, or identity boundary must never retain Windows verbatim prefixes. Use `dunce::canonicalize` under `cfg(windows)` and preserve existing standard canonicalization under `cfg(not(windows))`. For containment checks, normalize both operands with the same helper. Handle `\\?\UNC\server\share` as ordinary `\\server\share`; never blindly trim only `\\?\`.

## Fix surface

- Desktop workspace root and initial cwd normalization use Windows dunce canonicalization.
- Agent session filtering compares Windows canonical aliases and directory options are prefix-free.
- Agent LSP roots/files, file URIs, shell/background jobs, ACP, and worktrees use the existing `windows_process` helper.
- Workspace daemon has a central path helper for declarations, request roots, editor containment, CRDT reconciliation, project identity, and Windows snapshot migration.
- Desktop Markdown/CRDT, diagnostics keys, and ACP cwd/path containment normalize Windows paths.
- Shared attachment file URLs use `url::Url::from_file_path` after verbatim-prefix cleanup.
- Workspace-index path identity uses a Windows dunce helper.

## Verification caveat

Independent `neoism-workspace-index` and `neoism-ui` Linux checks pass. Full agent/daemon/desktop verification was temporarily blocked by unrelated concurrent WIP declaring missing `neoism-agent-server/src/workflow.rs`.
