---
name: "Web connection supervisor"
description: "Stable web ProtocolClient reconnect state machine, hydration barrier, preserved-canvas Sugarloaf gate, safe PTY/CRDT/service recovery"
type: "feature"
scope: "project"
origin: "implementation session"
created: "2026-08-01"
updated: "2026-08-01"
---

# Web connection supervisor — implemented

Implemented a reconnecting, generation-guarded stable `ProtocolClient` facade and App hydration barrier.

Key behavior:
- `ProtocolClient` owns one active socket generation, clears dead references on close/error, ignores stale callbacks, and single-flights retry signals.
- State phases: idle, connecting, authenticating, hydrating, connected, waiting, offline, auth-rejected, host-ended, closed.
- Connected is emitted only after accepted HelloAck plus `App`'s minimum workspace/tree + session/surface hydration barrier.
- Transient drops use injectable deterministic exponential full-jitter, online/visible wake, handshake/hydration deadlines, grace-delayed gate, and sanitized reasons. Auth rejection stops automatic retries but manual Retry remains possible.
- No pre-auth dispatch. No application Ping/Pong added because browser transport events plus bounded handshake/hydration provide observable liveness.
- Pending ProtocolClient Files/Git/Config requests reject on disconnect; SearchService now settles JS and wasm bridge slots. Sends return false and emit protocol errors when unavailable instead of silently dropping.
- TerminalPanel freezes PTY/CRDT pumps while disconnected, preserves local canvas, explicitly attaches/resizes PTYs, safely requests CRDT snapshots before releasing preserved outboxes, resumes agent, diagnostics, file tree, Notes, and Git reads. It never queues/replays PTY input or mutations.
- Shared Sugarloaf modal hosts a non-dismissible Connection lost gate, keeps Retry/Switch actions open, and repaints notifications above it.
- Manual workplace switch and resolved rehome preserve the old canvas until target HelloAck, then commit visual teardown and hydrate the destination. Disconnect intents distinguish switch/rehome/manual/host-ended.
- HostEnded is intentional and never auto-retries.

Primary files: web workspace/ProtocolClient.ts + tests/types; app/App.ts; services/WorkplaceService.ts, SearchService.ts + tests; terminal/TerminalPanel.ts/createTerminal.ts; shared chrome/pages/draw/config/chrome + widgets/modal; wasm rendered/overlays.rs.

Verification: web typecheck; all 151 web tests; 2309 neoism-ui lib tests; focused shared connection gate test; cargo check neoism-ui + neoism-terminal-wasm; git diff --check.
