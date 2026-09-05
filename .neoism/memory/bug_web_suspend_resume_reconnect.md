---
name: "Web suspend/resume reconnect — FIXED"
description: "Fix Safari/bfcache OPEN-but-dead sockets and lifecycle resume with authenticated Ping/Pong validation"
type: "bug"
scope: "project"
origin: "user-reported suspend/return disconnect"
created: "2026-08-02"
updated: "2026-08-02"
---

# Web suspend/resume reconnect — fixed

Root cause: the first supervisor subscribed only to online + visibility. Its visible callback called `retryNow()`, which intentionally no-opped whenever `this.socket` was non-null. Safari/bfcache can restore a WebSocket whose `readyState` is still OPEN although the transport is dead, so the facade stayed permanently `connected`; similarly stale connecting/authenticating/hydrating sockets and frozen deadline timers were not recycled on lifecycle return.

Fix:
- Browser runtime now observes visibility hidden/visible, pagehide/pageshow (including persisted bfcache), freeze/resume, and online.
- Hidden/freeze/pagehide never closes a healthy socket. Heartbeat/liveness and pending retry timers are paused and the generation is marked suspect.
- Visible/pageshow/resume validates the stable facade. Waiting/offline/no-socket states wake immediately; suspect connecting/authenticating/hydrating attempts are invalidated and recycled; ordinary duplicate visible signals during a fresh connection stay single-flight.
- Connected OPEN sockets are no longer trusted by readyState alone. Added post-auth Workspace Ping/Pong with generation+nonce, periodic visible heartbeat, and bounded liveness timeout. Missing Pong recycles the dead socket with generation guards. Exact nonce is required; no pre-auth heartbeat data.
- Intentional switch/manual/host-ended and auth-rejected remain stopped; offline remains paused.
- Visible recovery resets modal grace if the gate was not already visible, avoiding a flash after hours suspended.
- A successful resume Pong calls App lightweight authoritative rehydration: workspace snapshot/tree/active tabs, pane inventory, PTY attach/resize, CRDT snapshots/outboxes, diagnostics subscriptions, agent resume, file tree/Notes/Git; it hides any stale gate.

Touched in this increment: web ProtocolClient.ts/test.mts/types.ts and App.ts; neoism-protocol workspace client/server messages + tests; daemon workspace dispatch.

Verification: web typecheck; 157 full web tests; 17 focused ProtocolClient lifecycle tests; cargo check protocol/daemon/shared/wasm; protocol Ping/Pong roundtrip test; git diff --check.
