---
name: "Alt+P cd workspace-root picker"
description: "Alt+P cd directory picker changes declared workspace root via fff/remote directory search without changing existing terminal cwd."
type: "feature"
scope: "project"
origin: "Implemented in interactive coding session"
created: "2026-08-28"
updated: "2026-08-28"
---

Alt+P command palette supports an application-level `cd` DSL. Exact `cd` or `cd ` prefix switches shared palette rows to host-supplied directories while preserving the raw query; Tab completes as `cd <absolute-path>`. Desktop searches directories with FffWorkspaceSearchService and commits through Screen::set_active_workspace_root(path, true). Web uses SearchDirectories protocol/daemon bounded directory-only search, wasm PaletteIntent::Directory, and WorkspaceService::setWorkspaceRoot. Existing PTYs are deliberately untouched; new terminals inherit the declared root. Root mutation expands ~ and relative paths on the host, strips balanced quotes, requires an existing directory, canonicalizes, and never creates typo paths. Tests: shared command_palette 75 pass; daemon relative/quoted/missing path regression; desktop/daemon/wasm checks and web typecheck pass.
