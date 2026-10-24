---
name: "Windows peer presence stink cloud"
description: "Client OS metadata and cartoon green stink puffs for Windows peers across fast/Tailscale presence; desktop + web/WASM, shared avatar/caret rendering; checks pass"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-17"
updated: "2026-10-17"
---

# Windows presence stink cloud

Client-origin OS is carried as optional `client_os: Option<String>` through `CrdtPeerPresence`, `PeerPresence`, `PresenceAvatarPeer`, and `MarkdownRemoteCursor`. The wire field defaults to None and is omitted when unknown, preserving older clients. Desktop sets `std::env::consts::OS` in its originating publisher; the daemon must preserve this value, never substitute its host OS. Browser detection lives in `web/src/presence/clientOs.ts` (navigator client hints/platform/UA) and buffers via `PresencePublisher.setClientOs()` until WASM is ready. The shared publisher deliberately defaults to unknown rather than using its WASM build target.

`shared/src/editor/markdown/render/draw.rs::draw_presence_avatar_clipped` wraps the existing orb with six staggered olive/green rising puffs only for `client_os == Some("windows")`, reusing `presence_orb_now_seconds()` with no particle traffic or per-peer timers. Shared topbar, file-tree badges, code and markdown remote caret chips use it. Topbar hover explicitly labels Windows. Fast-server and permanent Tailscale joins converge on this same presence path; no transport-specific branching.

Web's WASM `set_presence_index` also deduplicates peers across buffers into the global topbar, including `workspace://presence` for terminal/agent-only participants, even without a file tree. Relayout when peer count changes. Shared Chrome owns presence animation while the topbar/tree or active code/markdown/notebook peers are visible; don't animate parked code tabs. Old relay daemons deserialize away unknown fields, so new clients need an updated daemon for OS metadata to survive.

Verification passed: 5 protocol CRDT tests, 34 shared presence tests, all 5 daemon presence_ws tests (fanout, snapshot, disconnect, TTL), browser typecheck and all 266 browser tests, native cargo check for neoism/protocol/daemon, WASM cargo check with --target wasm32-unknown-unknown --features web. Added coverage includes legacy/null JSON, unknown future OS, OS projections, publisher metadata/heartbeat, Windows relay through a Linux daemon, visible topbar animation teardown, browser detection and WASM buffering. No live GUI/two-machine visual validation performed; no release build run.
