---
name: "Host presence missing on self-hosted peer link"
description: "Root cause and fix for host absent from multiplayer presence in chrome and editors"
type: "bug"
scope: "project"
origin: "Neoism coding session 2026-08-01"
created: "2026-08-01"
updated: "2026-08-01"
---

## Root cause

Commit `53a33b89` gated desktop presence publishing and editor presence application on `ContextManager::current_workspace_is_collaborative()`. That predicate unconditionally returned false whenever `daemon.link_is_peer`, unless `current_workspace_is_remote_joined()` was true. A host that adopts its OWN served workspace over a peer-classified link is not remote-joined, so the host stopped publishing presence even though CRDT document updates continued to work.

Top chrome had a second asymmetry: `RemotePresenceStore` correctly excludes the local peer because the daemon broadcasts presence only to other sockets, and `cell_emit.rs` only inserted the local publisher into the avatar cluster while Agent was open. Thus the host was absent from chrome in Markdown/code views by construction.

## Fix

- `context/manager/daemon_link.rs`: if the current grid has an adopted workspace binding, determine collaboration from that workspace's visibility before applying the generic peer-link/private-grid guard. This restores the real host publisher on self-hosted adopted workspaces without making unrelated local grids collaborative.
- `screen/render/cell_emit.rs`: include the real local publisher identity in top chrome for every collaborative workspace, not only when Agent is visible. Editor cursor overlays remain remote-only to avoid a duplicate local caret.
- Added `self_hosted_adopted_workspace_is_collaborative_on_peer_link` regression test for peer-classified link + locally hosted adopted workspace + Shared visibility.

## Verification

- `cargo check -p neoism --message-format=short` passed.
- Targeted regression test passed: 1 passed.
- `rustfmt --check` on changed files and `git diff --check` passed.
- Existing unrelated warnings remain.
