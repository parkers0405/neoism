// JS-side bridge for `neoism-protocol::diagnostics`.
//
// The web frontend has no local language servers, so the daemon
// forwards LSP diagnostics over the WebSocket per-route.
// The chrome subscribes to a route before the daemon starts pushing;
// this keeps per-session bandwidth bounded to whatever the UI is
// actually rendering.
//
// The service has two surfaces:
//
//   1. Pure listener fan-out (`subscribe` / `ingestServerMessage`).
//      Used by App.ts to react to inbound frames before the wasm
//      bridge is online.
//
//   2. Bridge plumbing (`bindBridge`). Once the wasm bridge is ready
//      the host hooks the service to it so each
//      `DiagnosticsServerMessage` variant maps to the matching
//      `set_diagnostics` / `hide_diagnostics` / `set_status_lsp_*`
//      bridge method. Missing methods (older wasm bundles) silently
//      no-op.

import type { ProtocolClient } from "../workspace/ProtocolClient";
import type { TerminalAdapter } from "../terminal/createTerminal";
import type {
  DiagnosticsServerMessage,
  LspDiagnosticItem,
  LspState,
} from "../workspace/types";

export type { DiagnosticsServerMessage, LspDiagnosticItem, LspState };

export type DiagnosticsListener = (msg: DiagnosticsServerMessage) => void;

/** Minimal subset of `TerminalAdapter` this service touches. Exists
 *  so callers can hand the adapter in without taking the whole
 *  `TerminalAdapter` shape. */
export type DiagnosticsBridge = Pick<
  TerminalAdapter,
  | "setDiagnostics"
  | "hideDiagnostics"
  | "setStatusLspActive"
  | "setStatusLspInitializing"
  | "setStatusLspMissing"
  | "setStatusLspOff"
  | "diagnosticsEvent"
>;

export class DiagnosticsService {
  private readonly listeners = new Set<DiagnosticsListener>();
  private readonly subscriptions = new Set<number>();
  private bridge: DiagnosticsBridge | null = null;
  private unbindBridge: (() => void) | null = null;

  constructor(private readonly client: ProtocolClient) {}

  /** Subscribe to diagnostics pushes. */
  subscribe(listener: DiagnosticsListener): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  /**
   * Wire the service to the wasm bridge. Each inbound
   * `DiagnosticsServerMessage` variant is mapped to a specific bridge
   * call:
   *
   *   - `DiagnosticsPush` -> `setDiagnostics(JSON.stringify(items))`
   *   - `DiagnosticsCleared` -> `hideDiagnostics()`
   *   - `LspStatusUpdate { state: "Ready" }` -> `setStatusLspActive(server)`
   *   - `LspStatusUpdate { state: "Starting" | "Indexing" }` ->
   *       `setStatusLspInitializing()`
   *   - `LspStatusUpdate { state: "Stopped" }` -> `setStatusLspOff()`
   *   - `LspStatusUpdate { state: { Failed: ... } }` ->
   *       `setStatusLspMissing()`
   *
   * The service also still hands the raw envelope to the bridge's
   * `diagnosticsEvent` (when present) so any bridge-side bookkeeping
   * the granular methods don't cover still happens.
   *
   * Returns an unbind closure; calling it stops routing variants to
   * the previously-bound bridge.
   */
  bindBridge(bridge: DiagnosticsBridge): () => void {
    this.unbindBridge?.();
    this.bridge = bridge;
    const unsub = this.subscribe((msg) => this.dispatchToBridge(msg));
    this.unbindBridge = () => {
      unsub();
      if (this.bridge === bridge) {
        this.bridge = null;
      }
    };
    return this.unbindBridge;
  }

  /**
   * Hand a daemon-pushed `DiagnosticsServerMessage` to every
   * listener. Wired into `ProtocolClient.onDiagnosticsReply`.
   */
  ingestServerMessage(msg: DiagnosticsServerMessage): void {
    for (const listener of this.listeners) {
      try {
        listener(msg);
      } catch (err) {
        if (typeof console !== "undefined") {
          console.warn("[diagnostics] listener threw", err);
        }
      }
    }
  }

  /**
   * Subscribe the daemon to push diagnostics for `routeId`. Tracks
   * the subscription locally so we can replay it on reconnect.
   */
  watch(routeId: number): void {
    this.subscriptions.add(routeId);
    this.client.subscribeDiagnostics(routeId);
  }

  /** Drop a route subscription. */
  unwatch(routeId: number): void {
    this.subscriptions.delete(routeId);
    this.client.unsubscribeDiagnostics(routeId);
  }

  /** Active route subscriptions (for replay after reconnect). */
  activeRoutes(): number[] {
    return Array.from(this.subscriptions);
  }

  private dispatchToBridge(msg: DiagnosticsServerMessage): void {
    const bridge = this.bridge;
    if (!bridge) return;
    try {
      if ("DiagnosticsPush" in msg) {
        bridge.setDiagnostics?.(JSON.stringify(msg.DiagnosticsPush.items));
      } else if ("DiagnosticsCleared" in msg) {
        bridge.hideDiagnostics?.();
      } else if ("LspStatusUpdate" in msg) {
        const { server, state } = msg.LspStatusUpdate;
        if (state === "Ready") {
          bridge.setStatusLspActive?.(server);
        } else if (state === "Starting" || state === "Indexing") {
          bridge.setStatusLspInitializing?.();
        } else if (state === "Stopped") {
          bridge.setStatusLspOff?.();
        } else if (typeof state === "object" && state && "Failed" in state) {
          bridge.setStatusLspMissing?.();
        }
      }
      // Forward the raw envelope too — any bridge-side bookkeeping
      // the granular methods don't cover still gets to see it.
      bridge.diagnosticsEvent?.(JSON.stringify(msg));
    } catch (err) {
      if (typeof console !== "undefined") {
        console.warn("[diagnostics] bridge dispatch threw", err);
      }
    }
  }
}
