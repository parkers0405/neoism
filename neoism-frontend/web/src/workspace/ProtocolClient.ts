import {
  ClientMessage,
  CreatePtyArgs,
  ServerMessage,
  FilesClientMessage,
  FilesServerMessage,
  GitClientMessage,
  GitServerMessage,
  EditorClientMessage,
  AgentClientMessage,
  AgentServerMessage,
  SearchClientMessage,
  SearchServerMessage,
  WorkspaceClientMessage,
  WorkspaceServerMessage,
  DiagnosticsClientMessage,
  DiagnosticsServerMessage,
  CursorOverlayClientMessage,
  CursorOverlayServerMessage,
  CrdtClientMessage,
  CrdtServerMessage,
  isPtyCreated,
  isPtyOutput,
  isPtyClosed,
  isSessionCwd,
  isServerError,
  isFilesReply,
  isGitReply,
  isEditorReply,
  isAgentReply,
  isSearchReply,
  isWorkspaceReply,
  isDiagnosticsReply,
  isCursorOverlayReply,
  isCrdtReply,
} from "./types";

export type ProtocolStatus =
  | "idle"
  | "connecting"
  | "open"
  | "closed"
  | "errored";

export interface ProtocolClientOptions {
  url: string;
  authToken?: string;
  /**
   * Pairing token for the daemon's `Hello` handshake. Distinct from
   * `authToken` (which is appended to the URL as `?token=...` and
   * authenticates the WebSocket upgrade against the legacy
   * `NEOISM_DAEMON_TOKEN` env var). When set, the client sends
   * `WorkspaceClientMessage::Hello { token: pairingToken, ... }` as
   * its first frame after the socket opens; the daemon answers with
   * `HelloAck { accepted, reason }` and closes the socket if
   * `accepted=false`. Leave `undefined` for legacy / trust-local
   * daemons.
   */
  pairingToken?: string;
  /**
   * Human-facing label included in the `Hello` envelope (e.g.
   * `neoism-web`, `iPhone`). Used by the daemon for log / audit
   * lines only; never participates in the auth decision. Defaults
   * to `neoism-web`.
   */
  clientName?: string;
}

export interface ProtocolClientHandlers {
  onStatus?: (status: ProtocolStatus, detail?: string) => void;
  onPtyCreated?: (sessionId: string, workspaceRoot: string | null) => void;
  onPtyOutput?: (sessionId: string, bytes: Uint8Array) => void;
  onPtyClosed?: (sessionId: string, exitCode: number | null) => void;
  /// Daemon-tracked live cwd of a PTY's foreground process. Pushed
  /// whenever the shell `cd`s; clients re-root the tree/LSP off it.
  onSessionCwd?: (sessionId: string, cwd: string) => void;
  onServerError?: (message: string) => void;
  onProtocolError?: (message: string, raw: unknown) => void;
  /**
   * Fired for every files/git reply that has no pending in-flight
   * promise registered for its `request_id`. Chrome panels that
   * surfaced `IoError::Pending(req_id)` to the host route the payload
   * back into the wasm chrome as a `UiEvent::ServiceReply`.
   */
  onServiceReply?: (
    requestId: number,
    payload: FilesServerMessage | GitServerMessage,
  ) => void;
  /**
   * Fired for every `AgentReply` envelope the daemon ships off the
   * Claude API SSE pump. Unlike file/git replies these are not
   * request-response — the daemon emits zero-or-more events per
   * outbound envelope. The handler is the agent service's hook into
   * the wasm bridge's `agent_event(...)` method.
   */
  onAgentReply?: (requestId: number, payload: AgentServerMessage) => void;
  /// Unsolicited editor reply frames. The embedded-nvim grid consumer
  /// is gone; the future native CodePane will consume this channel.
  onEditorReply?: (requestId: number, payload: unknown) => void;
  /**
   * Fired for every daemon-emitted `SearchReply`. The request id
   * arrives from the inner message variant (`req_id`) — the host's
   * `JsSearchService` looks it up against the matching pending slot.
   */
  onSearchReply?: (payload: SearchServerMessage) => void;
  /**
   * Fired for every daemon-emitted `WorkspaceReply`. Workspace
   * messages are push-style (no per-request id) so the handler sees
   * the whole `WorkspaceServerMessage` payload.
   */
  onWorkspaceReply?: (payload: WorkspaceServerMessage) => void;
  /**
   * Fired for every daemon-emitted `DiagnosticsReply`. Diagnostics
   * are routed by `route_id` (carried inside the variant payload),
   * not request id, so the handler sees the whole message.
   */
  onDiagnosticsReply?: (payload: DiagnosticsServerMessage) => void;
  /**
   * Fired for every daemon-emitted `CursorOverlayReply`. The daemon
   * translates editor cursor + yank events into these push-style
   * envelopes; the handler routes them through the chrome bridge's
   * `setTrailCursor` / `setCustomCursor` / `setCursorlineOverlay` /
   * `setYankFlash` setters after a cell→pixel translation in the
   * dispatcher.
   */
  onCursorOverlayReply?: (
    requestId: number,
    payload: CursorOverlayServerMessage,
  ) => void;
  /** Fired for CRDT sync/presence replies and broadcasts. */
  onCrdtReply?: (requestId: number, payload: CrdtServerMessage) => void;
  /**
   * Fired exactly once per connection, after the daemon answers the
   * client's `Hello` envelope. `accepted=false` means the daemon will
   * close the socket shortly — the chrome should surface `reason` (a
   * short human-readable string like `"invalid pairing token"`) and
   * tear down per-connection state. `accepted=true` may include a
   * `peerIdentity` resolved server-side via `tailscale whois`,
   * useful for rendering "connected to laptop-A (you@tailnet)".
   */
  onHelloAck?: (
    accepted: boolean,
    reason: string | null,
    peerIdentity: string | null,
  ) => void;
}

type ServiceReplyPayload = FilesServerMessage | GitServerMessage;

/**
 * Thin WebSocket client that speaks the `neoism-protocol` JSON wire
 * format. All boundary data is `unknown` and parsed/validated before
 * being handed to typed callbacks.
 */
interface PendingRequest {
  resolve: (payload: ServiceReplyPayload) => void;
  reject: (err: Error) => void;
}

export class ProtocolClient {
  private socket: WebSocket | null = null;
  private status: ProtocolStatus = "idle";
  private nextRequestId = 1;
  private readonly pending = new Map<number, PendingRequest>();

  constructor(
    private readonly options: ProtocolClientOptions,
    private readonly handlers: ProtocolClientHandlers = {},
  ) {}

  /**
   * Allocate the next request id. Files and git share a single id
   * space so the chrome can stash a single u64 in `IoError::Pending`
   * without caring which service it came from.
   */
  allocateRequestId(): number {
    const id = this.nextRequestId;
    this.nextRequestId += 1;
    return id;
  }

  getStatus(): ProtocolStatus {
    return this.status;
  }

  connect(): void {
    if (this.socket) {
      return;
    }
    this.setStatus("connecting");

    let socket: WebSocket;
    try {
      socket = new WebSocket(this.options.url);
    } catch (err) {
      const detail = err instanceof Error ? err.message : String(err);
      this.setStatus("errored", detail);
      return;
    }
    socket.binaryType = "arraybuffer";
    this.socket = socket;

    socket.addEventListener("open", () => {
      this.setStatus("open");
      // Ship the `Hello` envelope as the very first frame. The daemon
      // resolves it through `handshake::evaluate_hello` and replies
      // with `HelloAck { accepted, reason }`. We send `Hello` even on
      // legacy / trust-local daemons (token omitted) so the daemon
      // always sees a labelled client and the audit log carries our
      // `clientName`.
      this.sendHello();
    });
    socket.addEventListener("close", (event) =>
      this.setStatus("closed", `code=${event.code} reason=${event.reason}`),
    );
    socket.addEventListener("error", () => this.setStatus("errored"));
    socket.addEventListener("message", (event) =>
      this.handleRawMessage(event.data),
    );
  }

  /**
   * Send the first-frame `Hello` envelope carrying our pairing token
   * (if any) and a human label. Called once from the `open` event;
   * exposed as a method (rather than inlined) so tests can drive it
   * deterministically without races against the WebSocket lifecycle.
   */
  private sendHello(): void {
    const clientName = this.options.clientName ?? "neoism-web";
    // `WorkspaceClientMessage::Hello`'s serde tags both fields with
    // `#[serde(default)]`, so omitting `token` is wire-equivalent to
    // sending `null`. We pick omit-when-absent so newer daemons that
    // tighten the type later don't break the legacy / trust-local
    // path.
    const helloPayload: { token?: string; client_name: string } = {
      client_name: clientName,
    };
    if (this.options.pairingToken && this.options.pairingToken.length > 0) {
      helloPayload.token = this.options.pairingToken;
    }
    this.sendWorkspace({ Hello: helloPayload });
  }

  disconnect(): void {
    if (!this.socket) {
      return;
    }
    try {
      this.socket.close();
    } catch {
      // ignore
    }
    this.socket = null;
  }

  send(message: ClientMessage): void {
    if (!this.socket || this.socket.readyState !== WebSocket.OPEN) {
      this.handlers.onProtocolError?.(
        "socket not open; dropping message",
        message,
      );
      return;
    }
    this.socket.send(JSON.stringify(message));
  }

  /**
   * Escape hatch for callers that already hold a fully-serialized
   * envelope (typically the agent bridge: the wasm side hands JS the
   * envelope JSON pre-encoded so the bridge can reuse one string
   * across both directions of the wire). Drops with a warning if the
   * socket isn't open — matches the behaviour of `send`.
   */
  sendRaw(payload: string): void {
    if (!this.socket || this.socket.readyState !== WebSocket.OPEN) {
      this.handlers.onProtocolError?.(
        "socket not open; dropping raw payload",
        payload,
      );
      return;
    }
    this.socket.send(payload);
  }

  // Convenience wrappers --------------------------------------------

  createPty(args: CreatePtyArgs): void {
    this.send(ClientMessage.createPty(args));
  }

  sendInput(sessionId: string, bytes: Uint8Array): void {
    this.send(
      ClientMessage.ptyInput({
        session_id: sessionId,
        bytes: Array.from(bytes),
      }),
    );
  }

  resize(sessionId: string, cols: number, rows: number): void {
    this.send(
      ClientMessage.resize({ session_id: sessionId, cols, rows }),
    );
  }

  closePty(sessionId: string): void {
    this.send(ClientMessage.closePty({ session_id: sessionId }));
  }

  /**
   * Send a files request and return a promise that resolves with the
   * `FilesServerMessage` payload tagged with the matching request id.
   * If the socket isn't open the promise rejects immediately.
   */
  requestFiles(
    message: FilesClientMessage,
    workspace_root?: string | null,
  ): Promise<FilesServerMessage> {
    const request_id = this.allocateRequestId();
    return new Promise<FilesServerMessage>((resolve, reject) => {
      if (!this.socket || this.socket.readyState !== WebSocket.OPEN) {
        reject(new Error("socket not open"));
        return;
      }
      this.pending.set(request_id, {
        resolve: (payload) => resolve(payload as FilesServerMessage),
        reject,
      });
      this.send(ClientMessage.files({ request_id, workspace_root, message }));
    });
  }

  /**
   * Fire-and-forget files send for callers that already hold a
   * `request_id` (e.g. wasm chrome surfacing `IoError::Pending`).
   * The reply will arrive via `onServiceReply`.
   */
  sendFiles(
    request_id: number,
    message: FilesClientMessage,
    workspace_root?: string | null,
  ): void {
    this.send(ClientMessage.files({ request_id, workspace_root, message }));
  }

  /**
   * Send a git request and return a promise that resolves with the
   * `GitServerMessage` payload tagged with the matching request id.
   */
  requestGit(message: GitClientMessage): Promise<GitServerMessage> {
    const request_id = this.allocateRequestId();
    return new Promise<GitServerMessage>((resolve, reject) => {
      if (!this.socket || this.socket.readyState !== WebSocket.OPEN) {
        reject(new Error("socket not open"));
        return;
      }
      this.pending.set(request_id, {
        resolve: (payload) => resolve(payload as GitServerMessage),
        reject,
      });
      this.send(ClientMessage.git({ request_id, message }));
    });
  }

  /**
   * Fire-and-forget git send for callers that already hold a
   * `request_id`. The reply will arrive via `onServiceReply`.
   */
  sendGit(request_id: number, message: GitClientMessage): void {
    this.send(ClientMessage.git({ request_id, message }));
  }

  /**
   * Ship an editor-service request. Returns the allocated request id
   * so callers that care can correlate daemon errors; replies arrive
   * through `onEditorReply`. The daemon currently answers with an
   * "editor backend unavailable" error — the native CodePane will
   * service this wire.
   */
  sendEditor(
    message: EditorClientMessage,
    workspace_root?: string | null,
  ): number {
    const request_id = this.allocateRequestId();
    this.send(ClientMessage.editor({ request_id, workspace_root, message }));
    return request_id;
  }

  /**
   * Ship a pre-built `AgentClientMessage` envelope. `requestId` is the
   * value the wasm bridge allocated alongside its `agent_send_message`
   * call so streaming replies route through the same correlation
   * slot. Mirrors `agent.ts`'s `sendEnvelope`.
   */
  sendAgent(requestId: number, message: AgentClientMessage): void {
    this.send(ClientMessage.agent({ request_id: requestId, message }));
  }

  /**
   * Ship a `SearchClientMessage` to the daemon. Each variant carries
   * its own `req_id`; the daemon echoes it on every reply
   * (including incremental `SearchProgress` and terminal
   * `SearchError` frames). The host wraps in the `Search` service
   * envelope; the `request_id` field on the envelope is currently
   * unused by the daemon (it routes via the inner `req_id`) but
   * carries the same value for symmetry with the other service
   * envelopes.
   */
  sendSearch(message: SearchClientMessage): void {
    const request_id = this.extractSearchReqId(message);
    this.send(ClientMessage.search({ request_id, message }));
  }

  /** Ship a `WorkspaceClientMessage` to the daemon. */
  sendWorkspace(message: WorkspaceClientMessage): void {
    this.send(ClientMessage.workspace({ message }));
  }

  /**
   * HTTP base URL of the daemon (e.g. `http://127.0.0.1:7878`),
   * derived from the same `ws://` / `wss://` URL the WebSocket was
   * opened against. Used by browser frontends to build `<img src>`
   * URLs that hit the daemon's REST surface (e.g. the
   * `/clipboard-image/<filename>` route that serves materialised
   * paste images). Returns `null` if the URL doesn't look like the
   * websocket endpoint we expect.
   */
  getDaemonHttpBase(): string | null {
    try {
      const url = new URL(this.options.url);
      if (url.protocol === "ws:") {
        url.protocol = "http:";
      } else if (url.protocol === "wss:") {
        url.protocol = "https:";
      } else {
        return null;
      }
      // Drop the `/session` (or whichever ws path) — clipboard images
      // are served from the root, not under the websocket route.
      url.pathname = "/";
      url.search = "";
      url.hash = "";
      // `toString()` always ends with `/` after the pathname rewrite
      // above; strip it so callers can append `/clipboard-image/...`
      // without doubling up.
      return url.toString().replace(/\/$/, "");
    } catch {
      return null;
    }
  }

  bindEditorSurface(
    surfaceId: string,
    sessionId: string,
    path: string | null = null,
  ): void {
    this.sendWorkspace({
      BindEditorSurface: {
        surface_id: surfaceId,
        session_id: sessionId,
        path,
      },
    });
  }

  listEditorSurfaces(): void {
    this.sendWorkspace("ListEditorSurfaces");
  }

  closeEditorSurface(surfaceId: string): void {
    this.sendWorkspace({ CloseEditorSurface: { surface_id: surfaceId } });
  }

  /** Ship a `CursorOverlayClientMessage` to the daemon. */
  sendCursorOverlay(message: CursorOverlayClientMessage): void {
    this.send(
      ClientMessage.cursorOverlay({
        request_id: 0,
        message,
      }),
    );
  }

  /** Ship a `CrdtClientMessage` to the daemon. */
  sendCrdt(message: CrdtClientMessage): void {
    this.send(
      ClientMessage.crdt({
        request_id: this.allocateRequestId(),
        message,
      }),
    );
  }

  /** Ship a `DiagnosticsClientMessage` to the daemon. */
  sendDiagnostics(message: DiagnosticsClientMessage): void {
    this.send(ClientMessage.diagnostics({ message }));
  }

  /**
   * Convenience: subscribe to diagnostics for a route. The daemon
   * keeps the subscription alive until `unsubscribeDiagnostics` (or
   * the WebSocket drops).
   */
  subscribeDiagnostics(routeId: number): void {
    this.sendDiagnostics({ SubscribeDiagnostics: { route_id: routeId } });
  }

  /** Convenience: drop a diagnostics subscription. */
  unsubscribeDiagnostics(routeId: number): void {
    this.sendDiagnostics({ UnsubscribeDiagnostics: { route_id: routeId } });
  }

  // Internals -------------------------------------------------------

  /**
   * Pull the inner `req_id` out of a `SearchClientMessage` so the
   * outer envelope's `request_id` matches it. Falls back to 0 for
   * variants that don't carry one (none today, but safe-by-default).
   */
  private extractSearchReqId(message: SearchClientMessage): number {
    if (typeof message !== "object" || message === null) return 0;
    const obj = message as Record<string, { req_id?: number }>;
    for (const inner of Object.values(obj)) {
      if (inner && typeof inner.req_id === "number") {
        return inner.req_id;
      }
    }
    return 0;
  }

  private setStatus(next: ProtocolStatus, detail?: string): void {
    this.status = next;
    this.handlers.onStatus?.(next, detail);
  }

  private handleRawMessage(raw: unknown): void {
    if (typeof raw !== "string") {
      this.handlers.onProtocolError?.(
        "expected text frame, received non-string",
        raw,
      );
      return;
    }
    let parsed: unknown;
    try {
      parsed = JSON.parse(raw);
    } catch (err) {
      this.handlers.onProtocolError?.(
        err instanceof Error ? err.message : "JSON parse error",
        raw,
      );
      return;
    }
    const msg = this.coerceServerMessage(parsed);
    if (!msg) {
      this.handlers.onProtocolError?.("unrecognised server frame", parsed);
      return;
    }
    this.dispatchServerMessage(msg);
  }

  private coerceServerMessage(value: unknown): ServerMessage | null {
    if (!value || typeof value !== "object") {
      return null;
    }
    const obj = value as Record<string, unknown>;
    const keys = Object.keys(obj);
    if (keys.length !== 1) {
      return null;
    }
    const tag = keys[0];
    const payload = obj[tag];
    if (!payload || typeof payload !== "object") {
      return null;
    }
    switch (tag) {
      case "PtyCreated":
      case "PtyOutput":
      case "PtyClosed":
      case "Error":
      case "FilesReply":
      case "GitReply":
      case "AgentReply":
      case "EditorReply":
      case "SearchReply":
      case "WorkspaceReply":
      case "DiagnosticsReply":
      case "CursorOverlayReply":
      case "CrdtReply":
        return obj as unknown as ServerMessage;
      default:
        return null;
    }
  }

  private dispatchServerMessage(msg: ServerMessage): void {
    if (isPtyCreated(msg)) {
      this.handlers.onPtyCreated?.(
        msg.PtyCreated.session_id,
        msg.PtyCreated.workspace_root ?? null,
      );
      return;
    }
    if (isPtyOutput(msg)) {
      const { session_id, bytes } = msg.PtyOutput;
      this.handlers.onPtyOutput?.(session_id, Uint8Array.from(bytes));
      return;
    }
    if (isPtyClosed(msg)) {
      const { session_id, exit_code } = msg.PtyClosed;
      this.handlers.onPtyClosed?.(session_id, exit_code);
      return;
    }
    if (isSessionCwd(msg)) {
      const { session_id, cwd } = msg.SessionCwd;
      this.handlers.onSessionCwd?.(session_id, cwd);
      return;
    }
    if (isServerError(msg)) {
      this.handlers.onServerError?.(msg.Error.message);
      return;
    }
    if (isFilesReply(msg)) {
      this.routeReply(msg.FilesReply.request_id, msg.FilesReply.message);
      return;
    }
    if (isGitReply(msg)) {
      this.routeReply(msg.GitReply.request_id, msg.GitReply.message);
      return;
    }
    if (isEditorReply(msg)) {
      this.handlers.onEditorReply?.(
        msg.EditorReply.request_id,
        msg.EditorReply.message,
      );
      return;
    }
    if (isAgentReply(msg)) {
      this.handlers.onAgentReply?.(
        msg.AgentReply.request_id,
        msg.AgentReply.message,
      );
      return;
    }
    if (isSearchReply(msg)) {
      this.handlers.onSearchReply?.(msg.SearchReply.message);
      return;
    }
    if (isWorkspaceReply(msg)) {
      const inner = msg.WorkspaceReply.message;
      // Intercept `HelloAck` so the workplace service can surface
      // accept / reject without every consumer having to type-narrow
      // the `WorkspaceServerMessage` union. We still forward the
      // payload to `onWorkspaceReply` so push-style subscribers see
      // a complete event stream.
      const helloAck = (
        inner as {
          HelloAck?: {
            accepted: boolean;
            reason?: string | null;
            peer_identity?: string | null;
          };
        }
      ).HelloAck;
      if (helloAck) {
        this.handlers.onHelloAck?.(
          Boolean(helloAck.accepted),
          helloAck.reason ?? null,
          helloAck.peer_identity ?? null,
        );
      }
      this.handlers.onWorkspaceReply?.(inner);
      return;
    }
    if (isDiagnosticsReply(msg)) {
      this.handlers.onDiagnosticsReply?.(msg.DiagnosticsReply.message);
      return;
    }
    if (isCursorOverlayReply(msg)) {
      this.handlers.onCursorOverlayReply?.(
        msg.CursorOverlayReply.request_id,
        msg.CursorOverlayReply.message,
      );
      return;
    }
    if (isCrdtReply(msg)) {
      this.handlers.onCrdtReply?.(
        msg.CrdtReply.request_id,
        msg.CrdtReply.message,
      );
      return;
    }
  }

  private routeReply(requestId: number, payload: ServiceReplyPayload): void {
    const slot = this.pending.get(requestId);
    if (slot) {
      this.pending.delete(requestId);
      slot.resolve(payload);
      return;
    }
    this.handlers.onServiceReply?.(requestId, payload);
  }
}
