// Wave 7F — INBOUND side of the web presence plane.
//
// Thin adapter over the SHARED RUST `RemotePresenceStore`
// (`neoism-frontend/shared/src/editor/crdt/remote_presence.rs`),
// reached through the wasm `PresenceStoreBridge` exported from
// `wasm/src/rendered/input_policy.rs`. The hand-mirrored TypeScript
// store this file used to contain is gone — upserts, removes,
// snapshot replaces, local-peer filtering, change detection and TTL
// pruning all run in the exact Rust the desktop fork runs.
//
// The wasm bundle loads asynchronously after the panel constructs, so
// the adapter buffers inbound presence messages until the bridge
// exists and replays them in arrival order; queries return empty in
// that window (nothing renders before the wasm renderer is up, so
// there is nothing to draw presence onto). A served bundle predating
// the export degrades to an inert store with a one-shot console
// warning.

import type {
  CrdtPeerPresence,
  CrdtServerMessage,
} from "../workspace/types";
import {
  wasmInputPolicy,
  type WasmInputPolicyModule,
  type WasmPresenceStoreInstance,
} from "../terminal/createTerminal";

/** Bound on the pre-wasm replay buffer. Presence traffic is tiny and
 *  self-superseding (newer upserts/snapshots replace older ones), so
 *  dropping the oldest entries under pressure is lossless in practice. */
const MAX_QUEUED_MESSAGES = 256;

/** Per-buffer avatar peer feed — the `set_presence_index` wire shape. */
export interface AvatarPeersByBufferEntry {
  buffer_id: string;
  peers: Array<{
    peer_id: string;
    display_name: string;
    color: [number, number, number];
    rainbow: boolean;
  }>;
}

let warnedMissingExport = false;
function warnMissingExport(): void {
  if (warnedMissingExport) return;
  warnedMissingExport = true;
  if (typeof console !== "undefined") {
    console.warn(
      "[neoism] served wasm bundle predates the shared presence-store " +
        "export; remote cursors are disabled until the bundle is rebuilt " +
        "(npm run build:wasm).",
    );
  }
}

function isPresenceMessage(message: CrdtServerMessage): boolean {
  return (
    typeof message === "object" &&
    message !== null &&
    ("Presence" in message || "PresenceSnapshot" in message)
  );
}

export class RemotePresenceStore {
  private wasm: WasmPresenceStoreInstance | null = null;
  private localPeerId: string | null = null;
  /** Presence messages received before the wasm bridge existed,
   *  replayed in arrival order the moment it does. */
  private queued: CrdtServerMessage[] = [];
  /** True when a queue replay changed store state; folded into the
   *  next boolean-returning call so the host's redraw gating still
   *  fires for peers that arrived during the load window. */
  private replayDirty = false;

  /** Test seam: a fake input-policy module, or a getter for one (so
   *  tests can model the bundle arriving late). Production resolves
   *  the live wasm module lazily on every call. */
  constructor(
    private readonly bindings?:
      | WasmInputPolicyModule
      | (() => WasmInputPolicyModule | null)
      | null,
  ) {}

  private module(): WasmInputPolicyModule | null {
    if (typeof this.bindings === "function") return this.bindings();
    return this.bindings ?? wasmInputPolicy();
  }

  /** The Rust store, created lazily once the wasm bundle is loaded;
   *  creation applies the pending local-peer id and replays the
   *  buffered messages. */
  private store(): WasmPresenceStoreInstance | null {
    if (this.wasm) return this.wasm;
    const mod = this.module();
    if (!mod) return null; // Bundle still loading.
    const Klass = mod.PresenceStoreBridge;
    if (!Klass) {
      warnMissingExport();
      return null;
    }
    const store = new Klass();
    this.wasm = store;
    if (this.localPeerId !== null) {
      store.set_local_peer_id(this.localPeerId);
    }
    const queued = this.queued.splice(0);
    for (const message of queued) {
      if (store.apply_server_message(message)) {
        this.replayDirty = true;
      }
    }
    return store;
  }

  /** Consume the replay-changed flag into a boolean result. */
  private takeReplayDirty(): boolean {
    const dirty = this.replayDirty;
    this.replayDirty = false;
    return dirty;
  }

  /**
   * Defensive self-filter: even though the daemon never echoes a
   * publisher's own presence, the store also drops entries matching
   * the local peer id so a misbehaving relay can't paint a ghost of
   * the local caret.
   */
  setLocalPeerId(peerId: string): void {
    this.localPeerId = peerId;
    this.wasm?.set_local_peer_id(peerId);
  }

  /** Remote cursors for one buffer — the renderer's per-frame read.
   * Already excludes the local peer. */
  cursorsFor(bufferId: string): CrdtPeerPresence[] {
    const cursors = this.store()?.cursors_for(bufferId);
    return Array.isArray(cursors) ? (cursors as CrdtPeerPresence[]) : [];
  }

  /** True when `bufferId` has at least one REMOTE cursor. */
  hasRemoteCursors(bufferId: string): boolean {
    return this.store()?.has_remote_cursors(bufferId) ?? false;
  }

  /**
   * Per-buffer avatar peers — `{buffer_id, peers}` for every buffer
   * holding at least one REMOTE peer, peers sorted by `peer_id` for a
   * stable cluster order. Hosts feed this to the wasm file tree's
   * presence index (`set_presence_index`) once per presence CHANGE —
   * never per frame.
   */
  avatarPeersByBuffer(): AvatarPeersByBufferEntry[] {
    const entries = this.store()?.avatar_peers_by_buffer();
    return Array.isArray(entries) ? (entries as AvatarPeersByBufferEntry[]) : [];
  }

  /**
   * Fold one daemon push into the store. Returns `true` when remote
   * presence changed (a redraw of the affected pane is due).
   * Non-presence CRDT traffic returns `false` untouched.
   */
  applyServerMessage(message: CrdtServerMessage): boolean {
    if (!isPresenceMessage(message)) return false;
    const store = this.store();
    if (!store) {
      // Bundle still loading: buffer for replay (bounded).
      this.queued.push(message);
      if (this.queued.length > MAX_QUEUED_MESSAGES) {
        this.queued.splice(0, this.queued.length - MAX_QUEUED_MESSAGES);
      }
      return false;
    }
    const changed = store.apply_server_message(message);
    return this.takeReplayDirty() || changed;
  }

  /**
   * Client-side staleness backstop mirroring the daemon TTL: drop
   * entries that stopped refreshing (e.g. the daemon's Remove got lost
   * in a lagged broadcast). Returns `true` when anything fell out.
   */
  pruneStale(nowMs: number, ttlMs: number): boolean {
    const store = this.store();
    if (!store) return false;
    const changed = store.prune_stale(nowMs, ttlMs);
    return this.takeReplayDirty() || changed;
  }

  /** Drop every remote cursor (e.g. on daemon reconnect, before the
   * fresh `RequestPresenceSnapshot` answers). */
  clear(): boolean {
    this.queued = [];
    const store = this.store();
    if (!store) {
      this.replayDirty = false;
      return false;
    }
    const changed = store.clear();
    return this.takeReplayDirty() || changed;
  }
}
