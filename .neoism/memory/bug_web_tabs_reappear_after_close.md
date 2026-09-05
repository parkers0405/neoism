---
name: "Web/mobile closed tabs reappearing — fixed"
description: "Web/mobile tab closes now tombstone all daemon authorities and reject stale hydration until ack or explicit reopen"
type: "bug"
scope: "project"
origin: "coding-session"
created: "2026-09-01"
updated: "2026-09-01"
---

Fixed web/mobile tabs that closed locally and then returned from daemon hydration. Root cause: TerminalPanel's shared BufferTabs close drain only closed PTYs; code/Agent editor-surface bindings remained in daemon persistence, Markdown/code/terminal workspace-tab snapshots remained authoritative, terminal logical sessions were not closed, and stale EditorSurfaceList/HostWorkspaceTree replies could rematerialize paths. Positional `${workspace}-web-${index}` IDs also aliased after removal.

Implemented `tabCloseLifecycle.ts`: per-identity, per-authority tombstones (workspace/surface/session/pty), reconnect generation retention, idempotent duplicate close, explicit-open supersession, dirty confirmation and fallback policy tests. TerminalPanel now funnels desktop pointer/mobile simulated close/keyboard close through one lifecycle; removes local UI once; derives and closes owning surface IDs; closes PTY + logical workspace session; replays pending idempotent closes after reconnect; filters stale surface/workspace inventories; acknowledges missing authoritative snapshots; clears late pending Agent creation route; dedupes duplicate close indices; and permits explicit path/Agent reopen. CRDT presence/doc pumps naturally unbind on active fallback; conversations are not deleted.

App publishes stable identity-based workspace tab IDs instead of positional IDs and provides active workspace identity to TerminalPanel. Daemon CloseEditorSurface and CloseSession are idempotent. WorkspaceManager session removal also clears PTY links, workspace-tab persistence, main/active references, associated editor surfaces, and rebuilt pane layouts.

Tests: new ordered lifecycle tests cover stale ListSurfaces, reconnect close, duplicate close, dirty cancel/confirm, explicit reopen, active fallback, and terminal/agent/editor distinctions. Added daemon duplicate surface-close and session registry/tab/surface cleanup assertions. Verified web typecheck + all 169 web tests, 78 shared BufferTabs tests, all 203 daemon lib tests, Rust checks for daemon/UI/WASM, wasm32 web-feature check, and diff check.
