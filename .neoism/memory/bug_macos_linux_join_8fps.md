---
name: "Mac→Linux joined workspace 8 FPS — FIXED"
description: "Linux watcher access-event feedback loop starved macOS joined guest; mutation filter restored"
type: "bug"
scope: "project"
origin: "session"
created: "2026-09-02"
updated: "2026-09-02"
---

# Mac guest joining Linux host runs at ~8 FPS — watcher feedback fixed

Diagnosed 2026-09-02. Linux->Linux joining remained usable and standalone macOS was smooth, but a macOS guest joining a Linux-hosted workspace dropped to ~8 FPS.

Root cause: current main lost the `notify::EventKind` mutation filter previously fixed on non-ancestor commit eab83006e. The demand-driven Linux host watcher forwarded OPEN/READ/ACCESS/CLOSE_NOWRITE events. Guest `FilesReply::Changed` handling re-listed root+expanded dirs and refreshed git; those reads emitted more access events; daemon debounced/broadcast at 300ms, creating a permanent ~3.3Hz feedback loop. macOS amplifies it because daemon queue application and CVDisplayLink redraw share AppKit's main queue. Standalone Mac never enters remote file-tree flow.

Fix restored `is_relevant_watch_event_kind` in `neoism-workspace-daemon/src/fs_watch.rs`: reject non-mutating Access events and access-time-only metadata updates; retain create/remove/rename/data/meaningful metadata, close-after-write, Any/Other. Applied before path enqueue/debounce. Added tests for rejected access events and retained mutations. Also corrected module docs from recursive to demand-driven watcher.

Verification: `cargo test -p neoism-workspace-daemon fs_watch` (5 passed) and `cargo check -p neoism-workspace-daemon` passed.

Hardening opportunities: time-budget daemon UI queue drain + fix wake flag race; conditional/single-flight remote git refresh; transport priority lanes/TCP_NODELAY. These are not required to break the proven loop.
