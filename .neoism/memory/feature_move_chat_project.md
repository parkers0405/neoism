---
name: "Model-driven chat-to-project relocation"
description: "Trusted deferred move_chat tool preserves chat chronology and optionally re-roots active desktop/web workspace."
type: "feature"
scope: "project"
origin: "Implemented in workspace"
created: "2026-09-19"
updated: "2026-09-19"
---

Implemented 2026-09-19. New trusted workspace tool `move_chat` in agent-server tool_registry stages relocation during an active model run; tool args: directory, create_directory (default true), switch_workspace (default true). Pending move is stored in AppState and applied by session_run::try_finish_session_run after durable run completion but before coordinator ownership releases, so current turn retains old cwd and next turn gets new context. New session_move.rs preserves SessionInfo time.created/time.updated, session ID/transcript, updates project/path/context epoch, and emits typed `session.moved`. `/cd` directory-only PATCH now preserves timestamps and emits session.moved with switchWorkspace=false. Tool can create only child paths and cannot leave current workspace scope; no permission/confirmation interaction. `session.moved` added to core event ALL/OpenAPI. Desktop shared classifier updates pane directory and optionally emits SwitchWorkspace; active route invokes set_active_workspace_root. Web TerminalPanel requests workspace root only for current agent session; WASM updates pane directory from raw event. Tests cover /cd chronology, deferred create/move chronology, event classifier, OpenAPI exhaustiveness, and tool registration. Checks passed: agent server, desktop, wasm, web tsc; existing unrelated warnings remain.
