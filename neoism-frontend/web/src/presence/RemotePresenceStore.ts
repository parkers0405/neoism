// Wave 7F — INBOUND side of the web presence plane.
//
// TypeScript mirror of the shared Rust `RemotePresenceStore`
// (`neoism-frontend/shared/src/editor/crdt/remote_presence.rs`): feed
// it every `CrdtServerMessage` the daemon pushes; it keeps a
// per-buffer map of remote peer cursors/selections that render code
// (markdown DOM overlay) can query cheaply. It
// deliberately stops at queryable state — drawing carets is the
// renderer's job.

import type {
  CrdtPeerPresence,
  CrdtServerMessage,
} from "../workspace/types";

export class RemotePresenceStore {
  private localPeerId: string | null = null;
  private readonly channels = new Map<string, Map<string, CrdtPeerPresence>>();

  /**
   * Defensive self-filter: even though the daemon never echoes a
   * publisher's own presence, the store also drops entries matching
   * the local peer id so a misbehaving relay can't paint a ghost of
   * the local caret.
   */
  setLocalPeerId(peerId: string): void {
    this.localPeerId = peerId;
  }

  /** Remote cursors for one buffer — the renderer's per-frame read.
   * Already excludes the local peer. */
  cursorsFor(bufferId: string): CrdtPeerPresence[] {
    const channel = this.channels.get(bufferId);
    if (!channel) return [];
    return Array.from(channel.values()).filter(
      (presence) => presence.peer_id !== this.localPeerId,
    );
  }

  /** True when `bufferId` has at least one REMOTE cursor. */
  hasRemoteCursors(bufferId: string): boolean {
    return this.cursorsFor(bufferId).length > 0;
  }

  /**
   * Fold one daemon push into the store. Returns `true` when remote
   * presence changed (a redraw of the affected pane is due).
   * Non-presence CRDT traffic returns `false` untouched.
   */
  applyServerMessage(message: CrdtServerMessage): boolean {
    if ("Presence" in message) {
      const update = message.Presence.update;
      if ("Upsert" in update) {
        const presence = update.Upsert;
        if (presence.peer_id === this.localPeerId) return false;
        return this.upsert(presence);
      }
      return this.remove(update.Remove.buffer_id, update.Remove.peer_id);
    }
    if ("PresenceSnapshot" in message) {
      const { buffer_id, peers } = message.PresenceSnapshot;
      return this.replaceBuffer(
        buffer_id,
        peers.filter((presence) => presence.peer_id !== this.localPeerId),
      );
    }
    return false;
  }

  /**
   * Client-side staleness backstop mirroring the daemon TTL: drop
   * entries that stopped refreshing (e.g. the daemon's Remove got lost
   * in a lagged broadcast). Returns `true` when anything fell out.
   */
  pruneStale(nowMs: number, ttlMs: number): boolean {
    let changed = false;
    for (const [bufferId, channel] of this.channels) {
      for (const [peerId, presence] of channel) {
        if (nowMs - presence.updated_at_ms > ttlMs) {
          channel.delete(peerId);
          changed = true;
        }
      }
      if (channel.size === 0) this.channels.delete(bufferId);
    }
    return changed;
  }

  /** Drop every remote cursor (e.g. on daemon reconnect, before the
   * fresh `RequestPresenceSnapshot` answers). */
  clear(): boolean {
    const hadPeers = this.channels.size > 0;
    this.channels.clear();
    return hadPeers;
  }

  private upsert(presence: CrdtPeerPresence): boolean {
    let channel = this.channels.get(presence.buffer_id);
    if (!channel) {
      channel = new Map();
      this.channels.set(presence.buffer_id, channel);
    }
    const existing = channel.get(presence.peer_id);
    if (existing && samePresence(existing, presence)) return false;
    channel.set(presence.peer_id, presence);
    return true;
  }

  private remove(bufferId: string, peerId: string): boolean {
    const channel = this.channels.get(bufferId);
    if (!channel) return false;
    const removed = channel.delete(peerId);
    if (channel.size === 0) this.channels.delete(bufferId);
    return removed;
  }

  private replaceBuffer(
    bufferId: string,
    peers: CrdtPeerPresence[],
  ): boolean {
    const next = new Map<string, CrdtPeerPresence>();
    for (const presence of peers) {
      next.set(presence.peer_id, presence);
    }
    const current = this.channels.get(bufferId);
    const changed = current ? !sameChannel(current, next) : next.size > 0;
    if (next.size === 0) {
      this.channels.delete(bufferId);
    } else {
      this.channels.set(bufferId, next);
    }
    return changed;
  }
}

function sameChannel(
  a: Map<string, CrdtPeerPresence>,
  b: Map<string, CrdtPeerPresence>,
): boolean {
  if (a.size !== b.size) return false;
  for (const [peerId, presence] of a) {
    const other = b.get(peerId);
    if (!other || !samePresence(presence, other)) return false;
  }
  return true;
}

function samePresence(a: CrdtPeerPresence, b: CrdtPeerPresence): boolean {
  return (
    a.buffer_id === b.buffer_id &&
    a.peer_id === b.peer_id &&
    a.display_name === b.display_name &&
    a.updated_at_ms === b.updated_at_ms &&
    a.color.r === b.color.r &&
    a.color.g === b.color.g &&
    a.color.b === b.color.b &&
    a.cursor.line === b.cursor.line &&
    a.cursor.column === b.cursor.column &&
    (a.cursor.offset ?? null) === (b.cursor.offset ?? null) &&
    sameSelection(a, b)
  );
}

function sameSelection(a: CrdtPeerPresence, b: CrdtPeerPresence): boolean {
  const sa = a.selection ?? null;
  const sb = b.selection ?? null;
  if (sa === null || sb === null) return sa === sb;
  return (
    sa.anchor.line === sb.anchor.line &&
    sa.anchor.column === sb.anchor.column &&
    sa.head.line === sb.head.line &&
    sa.head.column === sb.head.column
  );
}
