// Wave 7F — OUTBOUND side of the web presence plane.
//
// Thin adapter over the SHARED RUST `PresencePublisher`
// (`neoism-frontend/shared/src/editor/crdt/remote_presence.rs`),
// reached through the wasm `PresencePublisherBridge` exported from
// `wasm/src/rendered/input_policy.rs`. The coalescing state machine —
// rate-limited publishes (~13Hz), TTL keep-alive heartbeats,
// `ClearPresence` on buffer switch/close — runs in the exact Rust the
// desktop fork runs; the hand-mirrored TS copy is gone.
//
// Before the wasm bundle loads, `tick` returns no messages (there is
// no rendered surface whose cursor could be published) and
// `setColor`/`setRainbow` are buffered so they apply the moment the
// bridge exists; the first post-load tick then publishes immediately
// (fresh state machine ⇒ "first sight of this buffer" fast path).
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
import {
  wasmInputPolicy,
  type WasmInputPolicyModule,
  type WasmPresencePublisherInstance,
} from "../terminal/createTerminal";

/** Minimum interval between publishes while the cursor IS moving:
 * 75ms ≈ 13Hz, matching the shared Rust constant. */
export const PRESENCE_PUBLISH_MIN_INTERVAL_MS = 75;

/** Re-publish an UNCHANGED cursor this often so the daemon's ~10s TTL
 * never expires a live-but-idle peer. */
export const PRESENCE_HEARTBEAT_INTERVAL_MS = 4_000;

/** Presence-only target for non-editor tabs inside the current workspace. */
export const WORKSPACE_PRESENCE_BUFFER_ID = "workspace://presence";

export interface ActivePresenceTarget {
  bufferId: string;
  cursor: CrdtCursorPosition;
  selection?: CrdtSelectionRange | null;
  /** Local editor is in insert/replace mode. */
  insert?: boolean;
}

let warnedMissingExport = false;
function warnMissingExport(): void {
  if (warnedMissingExport) return;
  warnedMissingExport = true;
  if (typeof console !== "undefined") {
    console.warn(
      "[neoism] served wasm bundle predates the shared presence-publisher " +
        "export; local-cursor presence is not published until the bundle is " +
        "rebuilt (npm run build:wasm).",
    );
  }
}

export class PresencePublisher {
  private wasm: WasmPresencePublisherInstance | null = null;
  /** Buffered until the bridge exists. */
  private pendingColor: CrdtPresenceColor | null = null;
  private pendingRainbow: boolean | null = null;

  /** Test seam: a fake input-policy module, or a getter for one.
   *  Production resolves the live wasm module lazily per call. */
  constructor(
    private readonly peerId: string,
    private readonly displayName: string,
    private readonly minIntervalMs = PRESENCE_PUBLISH_MIN_INTERVAL_MS,
    private readonly heartbeatIntervalMs = PRESENCE_HEARTBEAT_INTERVAL_MS,
    private readonly bindings?:
      | WasmInputPolicyModule
      | (() => WasmInputPolicyModule | null)
      | null,
  ) {}

  private module(): WasmInputPolicyModule | null {
    if (typeof this.bindings === "function") return this.bindings();
    return this.bindings ?? wasmInputPolicy();
  }

  private publisher(): WasmPresencePublisherInstance | null {
    if (this.wasm) return this.wasm;
    const mod = this.module();
    if (!mod) return null; // Bundle still loading.
    const Klass = mod.PresencePublisherBridge;
    if (!Klass) {
      warnMissingExport();
      return null;
    }
    const publisher = new Klass(
      this.peerId,
      this.displayName,
      this.minIntervalMs,
      this.heartbeatIntervalMs,
    );
    this.wasm = publisher;
    if (this.pendingColor) {
      publisher.set_color(
        this.pendingColor.r,
        this.pendingColor.g,
        this.pendingColor.b,
      );
      this.pendingColor = null;
    }
    if (this.pendingRainbow !== null) {
      publisher.set_rainbow(this.pendingRainbow);
      this.pendingRainbow = null;
    }
    return publisher;
  }

  getPeerId(): string {
    return this.peerId;
  }

  /** Publish under the LOCAL THEME'S cursor color — peers render this
   *  user's caret in the color their cursor actually wears. */
  setColor(color: CrdtPresenceColor): void {
    const publisher = this.publisher();
    if (publisher) {
      publisher.set_color(color.r, color.g, color.b);
    } else {
      this.pendingColor = { ...color };
    }
  }

  /** Publish the rainbow-preset flag — peers animate the rainbow
   *  locally instead of using `color` (heartbeats are far too slow to
   *  stream an animation). */
  setRainbow(rainbow: boolean): void {
    const publisher = this.publisher();
    if (publisher) {
      publisher.set_rainbow(rainbow);
    } else {
      this.pendingRainbow = rainbow;
    }
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
    const publisher = this.publisher();
    if (!publisher) return [];
    const messages = publisher.tick(active, nowMs);
    return Array.isArray(messages) ? (messages as CrdtClientMessage[]) : [];
  }
}
