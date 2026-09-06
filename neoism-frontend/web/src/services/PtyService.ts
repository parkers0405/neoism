// JS-side bridge for `neoism-protocol::pty` — the web frontend's
// shell-spawn surface.
//
// Desktop spawns PTYs in-process via `neoism-terminal-pty`
// (teletypewriter / `pty_process` style fork+exec on a host fd). The
// web frontend has no host process layer, so the daemon owns the real
// PTY and pipes every byte back over the WebSocket. Wire shape
// mirrors `neoism-protocol/src/pty.rs`:
//
//   outbound  ClientMessage::CreatePty / PtyInput / Resize / ClosePty
//   inbound   ServerMessage::PtyCreated { session_id, workspace_root? }
//             ServerMessage::PtyOutput  { session_id, bytes }
//             ServerMessage::PtyClosed  { session_id, exit_code }
//             ServerMessage::Error      { message }
//
// `PtyService` formalises that wire on the JS side so callers don't
// hand-roll `client.createPty(...)` / `client.sendInput(...)` and so
// PTY routing can grow extra concerns (multi-session bookkeeping,
// reconnect, per-session listeners) without leaking into `App.ts`.
//
// Mirrors `SearchService.ts` / `WorkspaceService.ts` in shape: a thin
// fire-and-forget surface plus `ingest*` hooks the `ProtocolClient`
// callbacks delegate to.

import type { ProtocolClient } from "../workspace/ProtocolClient";
import type { CreatePtyArgs } from "../workspace/types";

export type { CreatePtyArgs };

/**
 * Listener for daemon-pushed PTY events. Each variant is a separate
 * callback so consumers (typically `TerminalPanel`) don't have to do
 * variant-dispatch themselves; this matches the existing
 * `ProtocolClientHandlers` shape and keeps the chrome / panel call
 * sites readable.
 */
export interface PtyListener {
  onCreated?(sessionId: string, workspaceRoot: string | null, shell: string | null): void;
  onOutput?(sessionId: string, bytes: Uint8Array): void;
  onClosed?(sessionId: string, exitCode: number | null): void;
  /// Daemon `Error` frames (no session id — these are protocol-level,
  /// e.g. "unknown session", "failed to spawn pty"). Most listeners
  /// only need this for UI surfacing on a failed spawn.
  onError?(message: string): void;
}

/**
 * Owns the PTY-spawn / PTY-input / PTY-output traffic for a
 * `ProtocolClient`. Multiple listeners can subscribe (so future
 * multi-pane web frontends route output to whichever panel owns the
 * `session_id`).
 *
 * The service does no rendering — it just shovels bytes between the
 * WebSocket and registered listeners. Rendering lives in
 * `TerminalPanel` which feeds `bytes` into the wasm `Terminal::feed`.
 */
export class PtyService {
  private readonly listeners = new Set<PtyListener>();

  constructor(private readonly client: ProtocolClient) {}

  /** Subscribe to daemon-pushed PTY events. Returns an unsubscribe fn. */
  subscribe(listener: PtyListener): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  // -- ingest hooks (wired into ProtocolClientHandlers) ----------------

  ingestCreated(sessionId: string, workspaceRoot: string | null = null, shell: string | null = null): void {
    for (const l of this.listeners) {
      try {
        l.onCreated?.(sessionId, workspaceRoot, shell);
      } catch (err) {
        if (typeof console !== "undefined") {
          console.warn("[pty] onCreated listener threw", err);
        }
      }
    }
  }

  ingestOutput(sessionId: string, bytes: Uint8Array): void {
    for (const l of this.listeners) {
      try {
        l.onOutput?.(sessionId, bytes);
      } catch (err) {
        if (typeof console !== "undefined") {
          console.warn("[pty] onOutput listener threw", err);
        }
      }
    }
  }

  ingestClosed(sessionId: string, exitCode: number | null): void {
    for (const l of this.listeners) {
      try {
        l.onClosed?.(sessionId, exitCode);
      } catch (err) {
        if (typeof console !== "undefined") {
          console.warn("[pty] onClosed listener threw", err);
        }
      }
    }
  }

  ingestError(message: string): void {
    for (const l of this.listeners) {
      try {
        l.onError?.(message);
      } catch (err) {
        if (typeof console !== "undefined") {
          console.warn("[pty] onError listener threw", err);
        }
      }
    }
  }

  // -- fire-and-forget senders ----------------------------------------

  /**
   * Ask the daemon to spawn a shell. The reply (`PtyCreated`) lands
   * on `onCreated` listeners with the assigned `session_id`.
   * `cwd === null` uses the daemon's working directory.
   * `shell` is optional; omit to let the daemon pick `$SHELL` / `/bin/sh`.
   */
  spawn(args: CreatePtyArgs): void {
    this.client.createPty(args);
  }

  /** Forward user keystrokes / paste bytes to the shell. */
  sendInput(sessionId: string, bytes: Uint8Array): void {
    this.client.sendInput(sessionId, bytes);
  }

  /** Notify the shell of a new viewport geometry. */
  resize(sessionId: string, cols: number, rows: number): void {
    this.client.resize(sessionId, cols, rows);
  }

  /** Tear the shell down. */
  close(sessionId: string): void {
    this.client.closePty(sessionId);
  }
}
