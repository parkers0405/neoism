//! CRDT buffer integration design.
//!
//! K1 decision: use `yrs`, the Rust port of Yjs, for editor buffer CRDTs.
//! The deciding factors are:
//!
//! - `yrs` exposes `Y.Text` as a Rust `TextRef`, which maps directly to
//!   Neoism's plain text nvim buffer surface.
//! - It uses the Yjs binary update model, so the daemon can exchange compact
//!   update bytes over the existing WebSocket protocol and browser clients can
//!   interoperate with standard Yjs tooling.
//! - It builds as Rust for shared/native code and has a WASM-facing sibling
//!   (`ywasm`) if the browser shell ever needs JavaScript-native bindings.
//! - It already has persistent positions (`StickyIndex`) and awareness/presence
//!   concepts, which line up with K3 without putting cursors into document
//!   history.
//!
//! Survey notes:
//!
//! - `automerge` is mature, Rust-backed, sync-friendly, and broadly local-first,
//!   but its value is strongest for structured JSON-like documents. For our
//!   next patch, `Y.Text` plus Yjs protocol compatibility gives lower editor
//!   integration risk than placing a text object inside Automerge.
//! - `diamond-types` is the strongest pure text engine candidate and likely the
//!   fastest for large logs, but its public docs still flag parts of the higher
//!   level API as in flux and it does not give us the browser/editor ecosystem
//!   that Yjs/Yrs does.
//! - `yrs` is the pragmatic pick for a daemon-authoritative model with web
//!   clients: Rust on both daemon/shared code paths, browser compatibility, a
//!   tested update protocol, and text-first primitives.
//!
//! Integration sketch for K2:
//!
//! 1. The daemon owns one authoritative `CrdtTextBuffer` per file buffer and
//!    initializes it from the nvim buffer snapshot.
//! 2. Desktop/web clients keep local replicas with the same root text name.
//!    Keystrokes update the local replica immediately, then send Yrs V1 update
//!    bytes to the daemon.
//! 3. The daemon applies client updates, forwards accepted CRDT updates to every
//!    peer, and applies the converged text back into nvim. For the first K2
//!    bridge, nvim can be reconciled by range patches generated from old/new
//!    snapshots; later patches can carry op deltas.
//! 4. On join/reconnect, peers exchange state vectors and missing update bytes.
//!    After that, incremental updates flow over the existing daemon WebSocket.
//! 5. Presence stays out of this module. Peer cursors/selections should use a
//!    separate ephemeral channel in K3, optionally backed by Yrs awareness.
//!
//! Offset policy:
//!
//! This adapter sets `OffsetKind::Utf16` to match Yjs/browser text positions.
//! Nvim APIs commonly report UTF-8 byte columns, so the nvim bridge must convert
//! between byte columns and CRDT offsets at line boundaries before calling this
//! module.

mod buffer;
mod presence;
mod remote_presence;

pub use buffer::{
    CrdtStickyAnchor, CrdtTextBuffer, CrdtTextBufferError, CrdtTextEdit, CrdtTextOffset,
    CrdtTextUpdate,
};
pub use presence::{
    resolve_presence_display_name, stable_presence_color, PeerCursor, PeerPresence,
    PeerSelection, PresenceBufferId, PresenceChange, PresenceChannel, PresenceColor,
    PresenceGridPoint, PresenceGridSize, PresenceOffset, PresencePeerId,
    PresenceRenderPeer, PresenceRenderSelection, PresenceUpdate,
};
pub use remote_presence::{
    peer_presence_from_wire, peer_presence_to_wire, presence_buffer_id_for_path,
    PresencePublisher, RemotePresenceStore, PRESENCE_HEARTBEAT_INTERVAL_MS,
    PRESENCE_PUBLISH_MIN_INTERVAL_MS,
};
