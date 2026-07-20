// Wave 7F — OUTBOUND side of the web presence plane.
//
// TypeScript mirror of the shared Rust `PresencePublisher`
// (`neoism-frontend/shared/src/editor/crdt/remote_presence.rs`): a
// pure coalescing state machine. Feed it the local cursor every frame
// (or on a coarse timer) and it emits `CrdtClientMessage`s only when
// something changed (rate-limited to ~13Hz) or when a keep-alive
// heartbeat is due (the daemon expires silent peers after a ~10s TTL).
// Switching buffers — or losing the buffer entirely — emits a
// `ClearPresence` for the buffer being left.
//
// Cursor coordinates are zero-based `(line, column)` with column in
// UTF-16 code units, matching the CRDT text offset policy used by the
// daemon's authoritative replicas.

import type {
  CrdtClientMessage,
  CrdtCursorPosition,
  CrdtPresenceColor,
  CrdtSelectionRange,
} from "../workspace/types";
import { stablePresenceColor } from "./presenceColor";

/** Minimum interval between publishes while the cursor IS moving:
 * 75ms ≈ 13Hz, matching the shared Rust constant. */
export const PRESENCE_PUBLISH_MIN_INTERVAL_MS = 75;

/** Re-publish an UNCHANGED cursor this often so the daemon's ~10s TTL
 * never expires a live-but-idle peer. */
export const PRESENCE_HEARTBEAT_INTERVAL_MS = 4_000;

export interface ActivePresenceTarget {
  bufferId: string;
  cursor: CrdtCursorPosition;
  selection?: CrdtSelectionRange | null;
  /** Local editor is in insert/replace mode. */
  insert?: boolean;
}

interface PublishedState {
  bufferId: string;
  cursor: CrdtCursorPosition;
  selection: CrdtSelectionRange | null;
  insert: boolean;
}

export class PresencePublisher {
  private color: CrdtPresenceColor;
  private rainbow = false;
  private lastPublished: PublishedState | null = null;
  private lastPublishedAtMs = 0;

  constructor(
    private readonly peerId: string,
    private readonly displayName: string,
    private readonly minIntervalMs = PRESENCE_PUBLISH_MIN_INTERVAL_MS,
    private readonly heartbeatIntervalMs = PRESENCE_HEARTBEAT_INTERVAL_MS,
  ) {
    this.color = stablePresenceColor(peerId);
  }

  getPeerId(): string {
    return this.peerId;
  }

  /** Publish under the LOCAL THEME'S cursor color — peers render this
   *  user's caret in the color their cursor actually wears. */
  setColor(color: CrdtPresenceColor): void {
    this.color = color;
  }

  /** Publish the rainbow-preset flag — peers animate the rainbow
   *  locally instead of using `color` (heartbeats are far too slow to
   *  stream an animation). */
  setRainbow(rainbow: boolean): void {
    this.rainbow = rainbow;
  }

  /**
   * Coalesce the local cursor into at most a couple of wire messages.
   * `active` is `null` when no daemon-backed buffer is focused (emits
   * a `ClearPresence` for the buffer being left, once).
   */
  tick(
    active: ActivePresenceTarget | null,
    nowMs: number,
  ): CrdtClientMessage[] {
    const out: CrdtClientMessage[] = [];
    if (active === null) {
      if (this.lastPublished) {
        out.push({
          ClearPresence: {
            buffer_id: this.lastPublished.bufferId,
            peer_id: this.peerId,
          },
        });
        this.lastPublished = null;
      }
      return out;
    }

    if (this.lastPublished && this.lastPublished.bufferId !== active.bufferId) {
      out.push({
        ClearPresence: {
          buffer_id: this.lastPublished.bufferId,
          peer_id: this.peerId,
        },
      });
      this.lastPublished = null;
    }

    const next: PublishedState = {
      bufferId: active.bufferId,
      cursor: {
        line: active.cursor.line,
        column: active.cursor.column,
        offset: active.cursor.offset ?? null,
      },
      selection: active.selection ?? null,
      insert: active.insert ?? false,
    };
    const changed =
      this.lastPublished === null || !sameState(this.lastPublished, next);
    const elapsed = Math.max(0, nowMs - this.lastPublishedAtMs);
    const due =
      this.lastPublished === null
        ? // First sight of this buffer: publish immediately.
          true
        : changed
          ? elapsed >= this.minIntervalMs
          : elapsed >= this.heartbeatIntervalMs;
    if (due) {
      out.push({
        PublishPresence: {
          presence: {
            buffer_id: next.bufferId,
            peer_id: this.peerId,
            display_name: this.displayName,
            color: this.color,
            cursor: { ...next.cursor },
            selection: next.selection,
            insert: next.insert,
            rainbow: this.rainbow,
            updated_at_ms: nowMs,
          },
        },
      });
      this.lastPublished = next;
      this.lastPublishedAtMs = nowMs;
    }
    return out;
  }
}

function sameState(a: PublishedState, b: PublishedState): boolean {
  if ((a.insert ?? false) !== (b.insert ?? false)) return false;
  return (
    a.bufferId === b.bufferId &&
    sameCursor(a.cursor, b.cursor) &&
    sameSelection(a.selection, b.selection)
  );
}

function sameCursor(a: CrdtCursorPosition, b: CrdtCursorPosition): boolean {
  return (
    a.line === b.line &&
    a.column === b.column &&
    (a.offset ?? null) === (b.offset ?? null)
  );
}

function sameSelection(
  a: CrdtSelectionRange | null,
  b: CrdtSelectionRange | null,
): boolean {
  if (a === null || b === null) return a === b;
  return sameCursor(a.anchor, b.anchor) && sameCursor(a.head, b.head);
}
