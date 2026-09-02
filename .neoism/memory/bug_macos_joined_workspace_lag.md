---
name: "macOS ARM joined-workspace UI lag"
description: "Apple Silicon joined lag was a daemon filesystem watcher read/access feedback loop; live probes saw ~3.3 invalidation bursts/sec on unchanged files. Fixed by filtering non-mutating access events."
type: "bug"
scope: "project"
origin: "Deep diagnosis using all three live tailnet systems, passive WebSocket message histograms, server TCP metrics, direct inotify event tracing, and isolated patched-daemon E2E verification."
created: "2026-08-07"
updated: "2026-08-07"
---

## Symptom

On Apple Silicon macOS, joining a Linux-hosted workspace made the whole joined window feel slow, including local-only NEOISM splash hover animation. Switching to a local workspace was immediately smooth. A Linux client joined to the same host remained completely smooth.

## Corrected diagnosis

The original queue-starvation hypothesis was not proven and was removed. The actual live-session evidence showed a daemon filesystem watcher feedback loop:

1. A read-only WebSocket probe to the same hosted workspace received 108 `FilesReply.Changed` pushes in 30 seconds, approximately the daemon debounce ceiling of one batch every 300 ms.
2. Unchanged `docker-compose.yml`, `README.md`, and the workspace root appeared in nearly every batch.
3. A direct Linux kernel inotify trace showed hundreds of `OPEN`, `ACCESS`, and `CLOSE_NOWRITE` events on those files, with no corresponding writes.
4. The daemon's `FsWatchHub` forwarded every `notify::EventKind`, including non-mutating access events.
5. Each guest `Changed` handling refreshes the remote file tree/open directories and reads files; those reads produce more access events on the host, which the daemon rebroadcasts, closing the loop.

Linux-to-Linux being smooth is compatible with this: both guests receive the needless refresh loop, but the Linux desktop absorbs about 3.3 refresh cycles/sec while the Mac client crosses its native UI/render responsiveness threshold. The network itself was healthy while the Mac was awake: direct LAN Tailscale, zero packet loss, normal TCP queues, roughly 4 KB/s daemon traffic. Earlier retransmission queues were measured while the Mac was asleep and were discarded as irrelevant.

## Fix

`neoism-workspace-daemon/src/fs_watch.rs` now filters watcher events before debounce/broadcast:

- Drops read/open/close-read/access and access-time-only metadata events.
- Keeps write-close, create, data writes, meaningful metadata writes, rename, remove, unknown/rescan, and other mutation events.
- Preserves the existing `.git`, `node_modules`, build-output ignore policy.

The speculative desktop inbound queue cap was completely reverted; this fix changes only the proven daemon source.

## Verification

- Live read-only probe before fix: 108 `FilesReply.Changed` in 30 sec; another 10 sec sample produced 33 batches, with unchanged README/docker-compose/root in almost all.
- Direct inotify trace: repeated OPEN/ACCESS/CLOSE_NOWRITE on unchanged files, no writes.
- Focused tests: `cargo test -p neoism-workspace-daemon fs_watch --no-fail-fast` passes all 5 tests, including access-drop/mutation-keep policies.
- `cargo check -p neoism-workspace-daemon` passes.
- `git diff --check` passes.
- Isolated patched-daemon E2E: 40 remote `ReadFile` operations generated zero `Changed` echoes; one deliberate README append generated exactly one `Changed` notification.

The running hosted daemon must be updated/restarted before the live Mac will receive the fix. Do not restart it without user approval because it is an external live process and may affect active workspaces.
