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
  ConfigClientMessage,
  ConfigServerMessage,
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
  isConfigReply,
} from "./types";

export type ProtocolStatus =
  | "idle"
  | "connecting"
  | "open"
  | "closed"
  | "errored";

export type ConnectionPhase =
  | "idle"
  | "connecting"
  | "authenticating"
  | "hydrating"
  | "connected"
  | "waiting"
  | "offline"
  | "auth-rejected"
  | "host-ended"
  | "closed";

export type DisconnectIntent = "manual" | "switch" | "rehome" | "dispose" | "host-ended";

export interface ConnectionState {
  phase: ConnectionPhase;
  generation: number;
  attempt: number;
  reason: string | null;
  retryAt: number | null;
  retryInMs: number | null;
  gateVisible: boolean;
  intentional: boolean;
}

type TimerHandle = ReturnType<typeof setTimeout>;

export interface ProtocolClientRuntime {
  createSocket(url: string): WebSocket;
  setTimeout(callback: () => void, delayMs: number): TimerHandle;
  clearTimeout(handle: TimerHandle): void;
  now(): number;
  random(): number;
  isOnline(): boolean;
  isVisible(): boolean;
  onOnline(callback: () => void): () => void;
  onVisibility(callback: () => void): () => void;
  onPageShow(callback: (persisted: boolean) => void): () => void;
  onPageHide(callback: () => void): () => void;
  onFreeze(callback: () => void): () => void;
  onResume(callback: () => void): () => void;
}

export interface ReconnectOptions {
  baseDelayMs?: number;
  maxDelayMs?: number;
  handshakeTimeoutMs?: number;
  hydrationTimeoutMs?: number;
  modalGraceMs?: number;
  heartbeatIntervalMs?: number;
  livenessTimeoutMs?: number;
}

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
  /** Test/runtime injection. Production callers should omit this. */
  runtime?: ProtocolClientRuntime;
  reconnect?: ReconnectOptions;
}

export interface ProtocolClientHandlers {
  onStatus?: (status: ProtocolStatus, detail?: string) => void;
  onConnectionState?: (state: ConnectionState) => void;
  onDisconnect?: (error: Error, intentional: boolean) => void;
  /** A previously-connected generation answered its resume liveness probe. */
  onAuthenticatedResume?: (generation: number) => void;
  onPtyCreated?: (sessionId: string, workspaceRoot: string | null, shell: string | null) => void;
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
    payload: FilesServerMessage | GitServerMessage | ConfigServerMessage,
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
   * `connectedHostId` is the accepting daemon's durable machine id.
   */
  onHelloAck?: (
    accepted: boolean,
    reason: string | null,
    peerIdentity: string | null,
    connectedHostId?: string | null,
  ) => void;
}

type ServiceReplyPayload =
  | FilesServerMessage
  | GitServerMessage
  | ConfigServerMessage;

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
  /**
   * Promise-tracked request ids (`requestFiles` / `requestGit` /
   * `requestConfig`) start from a high base so they can NEVER collide
   * with ids the wasm chrome allocates for its own fire-and-forget
   * `sendFiles` / `sendGit` calls (those count up from 1, and the wasm
   * git-panel provider uses 0x5000_0000+). `routeReply` gives the
   * pending-promise table priority — a collision would silently steal
   * a chrome reply (e.g. a file-tree `DirListing`) and stall that
   * panel forever.
   */
  private nextRequestId = 0x4000_0000;
  private readonly pending = new Map<number, PendingRequest>();
  private connectedHostId: string | null = null;
  private readonly runtime: ProtocolClientRuntime;
  private readonly reconnect: Required<ReconnectOptions>;
  private generation = 0;
  private desired = false;
  private authenticated = false;
  private failureHandledGeneration = -1;
  private attempt = 0;
  private outageStartedAt: number | null = null;
  private retryAt: number | null = null;
  private retryTimer: TimerHandle | null = null;
  private progressTimer: TimerHandle | null = null;
  private deadlineTimer: TimerHandle | null = null;
  private heartbeatTimer: TimerHandle | null = null;
  private livenessTimer: TimerHandle | null = null;
  private pendingProbe: { generation: number; nonce: string; resume: boolean } | null = null;
  private nextProbeNonce = 1;
  private suspensionSuspected = false;
  private signalUnsubscribers: Array<() => void> = [];
  private connectionState: ConnectionState = {
    phase: "idle",
    generation: 0,
    attempt: 0,
    reason: null,
    retryAt: null,
    retryInMs: null,
    gateVisible: false,
    intentional: false,
  };

  constructor(
    private readonly options: ProtocolClientOptions,
    private readonly handlers: ProtocolClientHandlers = {},
  ) {
    this.runtime = options.runtime ?? browserRuntime();
    this.reconnect = {
      baseDelayMs: options.reconnect?.baseDelayMs ?? 250,
      maxDelayMs: options.reconnect?.maxDelayMs ?? 15_000,
      handshakeTimeoutMs: options.reconnect?.handshakeTimeoutMs ?? 8_000,
      hydrationTimeoutMs: options.reconnect?.hydrationTimeoutMs ?? 15_000,
      modalGraceMs: options.reconnect?.modalGraceMs ?? 2_500,
      heartbeatIntervalMs: options.reconnect?.heartbeatIntervalMs ?? 15_000,
      livenessTimeoutMs: options.reconnect?.livenessTimeoutMs ?? 3_000,
    };
  }

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

  /** The authoritative endpoint used by this live client. Share links use
   * this instead of guessing the daemon port from desktop defaults. */
  endpointUrl(): string {
    return this.options.url;
  }

  /** Stable identity learned from the accepted HelloAck. Older daemons
   * omit it, in which case callers must use the conservative legacy policy. */
  getConnectedHostId(): string | null {
    return this.connectedHostId;
  }

  getConnectionState(): ConnectionState {
    return { ...this.connectionState };
  }

  getGeneration(): number {
    return this.generation;
  }

  connect(): void {
    this.desired = true;
    this.installSignals();
    if (this.socket || this.retryTimer) return;
    this.startAttempt();
  }

  /** Complete the application hydration barrier for the current socket. */
  markHydrated(generation = this.generation): boolean {
    if (
      generation !== this.generation ||
      !this.socket ||
      !this.authenticated ||
      !this.desired
    ) {
      return false;
    }
    this.clearDeadline();
    this.attempt = 0;
    this.outageStartedAt = null;
    this.retryAt = null;
    this.clearProgressTimer();
    this.emitConnectionState("connected", null, false);
    this.setStatus("open");
    this.armHeartbeat();
    return true;
  }

  /** Bypass the current backoff after an explicit button/network wake. */
  retryNow(): void {
    if (this.connectionState.phase === "auth-rejected") {
      // Authentication failures stop *automatic* retry. A deliberate button
      // press may try again (for example after the token was refreshed by the
      // workplace picker) without replacing this stable facade.
      this.desired = true;
      this.installSignals();
    }
    if (!this.desired) return;
    this.clearRetryTimer();
    this.clearProgressTimer();
    if (this.socket) return;
    this.startAttempt();
  }

  /** Validate/recover the transport after a browser lifecycle suspension.
   * This is public for App-level pageshow integrations and deterministic tests;
   * normal production callers use the installed lifecycle listeners. */
  validateAfterResume(): void {
    if (!this.desired) return;
    if (!this.runtime.isOnline()) {
      this.recycleForResume("Browser is offline", false);
      return;
    }
    if (!this.runtime.isVisible()) {
      return;
    }
    const phase = this.connectionState.phase;
    if (
      phase === "auth-rejected" ||
      phase === "host-ended" ||
      phase === "closed"
    ) {
      return;
    }
    if (
      phase === "connected" &&
      this.authenticated &&
      this.socket?.readyState === 1
    ) {
      this.probeLiveness(true);
      this.suspensionSuspected = false;
      return;
    }
    if (this.socket && !this.suspensionSuspected) {
      // A duplicate visible/pageshow signal during an ordinary in-flight dial
      // must not create a second socket.
      return;
    }
    this.recycleForResume("Resuming after browser suspension", true);
  }

  private startAttempt(): void {
    if (!this.desired || this.socket) return;
    if (!this.runtime.isOnline()) {
      this.emitConnectionState("offline", "Browser is offline", false);
      this.armProgressTimer();
      return;
    }
    if (!this.runtime.isVisible()) {
      this.retryAt = null;
      this.emitConnectionState("waiting", "Page is hidden", false);
      return;
    }
    const generation = ++this.generation;
    this.failureHandledGeneration = -1;
    this.connectedHostId = null;
    this.authenticated = false;
    this.suspensionSuspected = false;
    this.retryAt = null;
    this.emitConnectionState("connecting", null, false);
    this.setStatus("connecting");

    let socket: WebSocket;
    try {
      socket = this.runtime.createSocket(
        websocketUrl(this.options.url, this.options.authToken),
      );
    } catch (err) {
      const detail = err instanceof Error ? err.message : String(err);
      this.failAttempt(generation, detail);
      return;
    }
    socket.binaryType = "arraybuffer";
    this.socket = socket;

    this.armDeadline(generation, this.reconnect.handshakeTimeoutMs, "Handshake timed out");
    socket.addEventListener("open", () => {
      if (!this.isCurrentSocket(socket, generation)) return;
      this.emitConnectionState("authenticating", null, false);
      // Ship the `Hello` envelope as the very first frame. The daemon
      // resolves it through `handshake::evaluate_hello` and replies
      // with `HelloAck { accepted, reason }`. We send `Hello` even on
      // legacy / trust-local daemons (token omitted) so the daemon
      // always sees a labelled client and the audit log carries our
      // `clientName`.
      this.sendHello();
    });
    socket.addEventListener("close", (event) => {
      if (!this.isCurrentSocket(socket, generation)) return;
      this.socket = null;
      const detail = closeDetail(event.code, event.reason);
      this.failAttempt(generation, detail);
    });
    socket.addEventListener("error", () => {
      if (!this.isCurrentSocket(socket, generation)) return;
      // Browsers are allowed to emit `error` without a later `close`.
      // Clear the live reference first so a new generation can dial.
      this.socket = null;
      try { socket.close(); } catch { /* already unusable */ }
      this.failAttempt(generation, "WebSocket transport error");
    });
    socket.addEventListener("message", (event) => {
      if (!this.isCurrentSocket(socket, generation)) return;
      this.handleRawMessage(event.data, generation);
    });
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
    const socket = this.socket;
    if (!socket || socket.readyState !== 1) return;
    socket.send(JSON.stringify(ClientMessage.workspace({ message: { Hello: helloPayload } })));
  }

  disconnect(intent: DisconnectIntent = "manual"): void {
    this.desired = false;
    this.generation += 1;
    this.clearAllTimers();
    this.removeSignals();
    this.connectedHostId = null;
    this.authenticated = false;
    this.rejectPending(new Error(`connection closed (${intent})`), true);
    const socket = this.socket;
    this.socket = null;
    if (!socket) {
      this.emitConnectionState(intent === "host-ended" ? "host-ended" : "closed", intent, true);
      this.setStatus("closed", intent);
      return;
    }
    try {
      socket.close(1000, intent);
    } catch {
      // ignore
    }
    this.emitConnectionState(intent === "host-ended" ? "host-ended" : "closed", intent, true);
    this.setStatus("closed", intent);
  }

  send(message: ClientMessage): boolean {
    if (!this.authenticated || !this.socket || this.socket.readyState !== 1) {
      this.handlers.onProtocolError?.(
        "connection unavailable; message not sent",
        message,
      );
      return false;
    }
    this.socket.send(JSON.stringify(message));
    return true;
  }

  /**
   * Escape hatch for callers that already hold a fully-serialized
   * envelope (typically the agent bridge: the wasm side hands JS the
   * envelope JSON pre-encoded so the bridge can reuse one string
   * across both directions of the wire). Drops with a warning if the
   * socket isn't open — matches the behaviour of `send`.
   */
  sendRaw(payload: string): boolean {
    if (!this.authenticated || !this.socket || this.socket.readyState !== 1) {
      this.handlers.onProtocolError?.(
        "connection unavailable; raw payload not sent",
        payload,
      );
      return false;
    }
    this.socket.send(payload);
    return true;
  }

  // Convenience wrappers --------------------------------------------

  createPty(args: CreatePtyArgs): boolean {
    return this.send(ClientMessage.createPty(args));
  }

  attachPty(sessionId: string): boolean {
    return this.send({ AttachPty: { session_id: sessionId } });
  }

  sendInput(sessionId: string, bytes: Uint8Array): boolean {
    return this.send(
      ClientMessage.ptyInput({
        session_id: sessionId,
        bytes: Array.from(bytes),
      }),
    );
  }

  resize(sessionId: string, cols: number, rows: number): boolean {
    return this.send(
      ClientMessage.resize({ session_id: sessionId, cols, rows }),
    );
  }

  closePty(sessionId: string): boolean {
    return this.send(ClientMessage.closePty({ session_id: sessionId }));
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
      if (!this.authenticated || !this.socket || this.socket.readyState !== 1) {
        reject(new Error("socket not open"));
        return;
      }
      this.pending.set(request_id, {
        resolve: (payload) => resolve(payload as FilesServerMessage),
        reject,
      });
      if (!this.send(ClientMessage.files({ request_id, workspace_root, message }))) {
        this.pending.delete(request_id);
        reject(new Error("connection unavailable"));
      }
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
      if (!this.authenticated || !this.socket || this.socket.readyState !== 1) {
        reject(new Error("socket not open"));
        return;
      }
      this.pending.set(request_id, {
        resolve: (payload) => resolve(payload as GitServerMessage),
        reject,
      });
      if (!this.send(ClientMessage.git({ request_id, message }))) {
        this.pending.delete(request_id);
        reject(new Error("connection unavailable"));
      }
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
   * Send a config-plane request (settings get/set, read-only
   * extensions inventory) and resolve with the matching
   * `ConfigServerMessage`. Backs the web Settings + Extensions pages.
   */
  requestConfig(message: ConfigClientMessage): Promise<ConfigServerMessage> {
    const request_id = this.allocateRequestId();
    return new Promise<ConfigServerMessage>((resolve, reject) => {
      if (!this.authenticated || !this.socket || this.socket.readyState !== 1) {
        reject(new Error("socket not open"));
        return;
      }
      this.pending.set(request_id, {
        resolve: (payload) => resolve(payload as ConfigServerMessage),
        reject,
      });
      if (!this.send(ClientMessage.config({ request_id, message }))) {
        this.pending.delete(request_id);
        reject(new Error("connection unavailable"));
      }
    });
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

  private handleRawMessage(raw: unknown, generation = this.generation): void {
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
    // Nothing except HelloAck is dispatched before authentication. This
    // prevents a compromised/misconfigured endpoint from injecting data
    // while the pairing decision is still pending.
    const helloAck = helloAckFrom(msg);
    if (!this.authenticated && !helloAck) {
      this.handlers.onProtocolError?.("received data before HelloAck", parsed);
      return;
    }
    const pong = pongFrom(msg);
    if (this.authenticated && pong) this.noteLiveness(generation, pong.nonce);
    if (helloAck) {
      if (helloAck.accepted) {
        this.authenticated = true;
        this.clearDeadline();
        this.clearHeartbeat();
        this.emitConnectionState("hydrating", null, false);
        this.armDeadline(generation, this.reconnect.hydrationTimeoutMs, "Hydration timed out");
      } else {
        const reason = sanitizeReason(helloAck.reason ?? "Authentication rejected");
        this.desired = false;
        this.clearAllTimers();
        this.rejectPending(new Error(reason), false);
        const socket = this.socket;
        this.socket = null;
        this.emitConnectionState("auth-rejected", reason, false, true);
        this.setStatus("errored", reason);
        try { socket?.close(1008, "authentication rejected"); } catch { /* ignore */ }
      }
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
      case "ConfigReply":
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
        msg.PtyCreated.shell ?? null,
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
            connected_host_id?: string | null;
          };
        }
      ).HelloAck;
      if (helloAck) {
        this.connectedHostId = helloAck.accepted
          ? (helloAck.connected_host_id ?? null)
          : null;
        this.handlers.onHelloAck?.(
          Boolean(helloAck.accepted),
          helloAck.reason ?? null,
          helloAck.peer_identity ?? null,
          this.connectedHostId,
        );
      }
      if ("HostEnded" in inner) {
        const reason = sanitizeReason(inner.HostEnded.reason || "The host ended the session");
        this.handlers.onWorkspaceReply?.(inner);
        this.disconnect("host-ended");
        this.emitConnectionState("host-ended", reason, true);
        return;
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
    if (isConfigReply(msg)) {
      this.routeReply(msg.ConfigReply.request_id, msg.ConfigReply.message);
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

  private isCurrentSocket(socket: WebSocket, generation: number): boolean {
    return this.socket === socket && this.generation === generation;
  }

  private failAttempt(generation: number, rawReason: string): void {
    if (generation !== this.generation || this.failureHandledGeneration === generation) return;
    this.failureHandledGeneration = generation;
    this.clearDeadline();
    this.socket = null;
    this.authenticated = false;
    this.clearHeartbeat();
    this.clearLivenessTimer();
    this.pendingProbe = null;
    const reason = sanitizeReason(rawReason);
    this.rejectPending(new Error(reason), !this.desired);
    if (!this.desired) return;
    this.attempt += 1;
    this.outageStartedAt ??= this.runtime.now();
    this.setStatus("errored", reason);
    if (!this.runtime.isOnline()) {
      this.retryAt = null;
      this.emitConnectionState("offline", reason, false);
      this.armProgressTimer();
      return;
    }
    if (!this.runtime.isVisible()) {
      this.retryAt = null;
      this.emitConnectionState("waiting", reason, false);
      return;
    }
    const cap = Math.min(
      this.reconnect.maxDelayMs,
      this.reconnect.baseDelayMs * 2 ** Math.max(0, this.attempt - 1),
    );
    const delay = Math.max(0, Math.floor(this.runtime.random() * cap));
    this.retryAt = this.runtime.now() + delay;
    this.emitConnectionState("waiting", reason, false);
    this.clearRetryTimer();
    this.retryTimer = this.runtime.setTimeout(() => {
      this.retryTimer = null;
      this.startAttempt();
    }, delay);
    this.armProgressTimer();
  }

  private rejectPending(error: Error, intentional: boolean): void {
    for (const slot of this.pending.values()) slot.reject(error);
    this.pending.clear();
    this.handlers.onDisconnect?.(error, intentional);
  }

  private armDeadline(generation: number, delayMs: number, reason: string): void {
    this.clearDeadline();
    this.deadlineTimer = this.runtime.setTimeout(() => {
      this.deadlineTimer = null;
      if (generation !== this.generation) return;
      const socket = this.socket;
      this.socket = null;
      try { socket?.close(); } catch { /* ignore */ }
      this.failAttempt(generation, reason);
    }, delayMs);
  }

  private clearDeadline(): void {
    if (this.deadlineTimer !== null) this.runtime.clearTimeout(this.deadlineTimer);
    this.deadlineTimer = null;
  }

  private clearRetryTimer(): void {
    if (this.retryTimer !== null) this.runtime.clearTimeout(this.retryTimer);
    this.retryTimer = null;
  }

  private clearProgressTimer(): void {
    if (this.progressTimer !== null) this.runtime.clearTimeout(this.progressTimer);
    this.progressTimer = null;
  }

  private clearAllTimers(): void {
    this.clearDeadline();
    this.clearRetryTimer();
    this.clearProgressTimer();
    this.clearHeartbeat();
    this.clearLivenessTimer();
  }

  private armProgressTimer(): void {
    this.clearProgressTimer();
    if (!this.desired) return;
    const graceRemaining = this.outageStartedAt === null
      ? 1_000
      : Math.max(1, this.reconnect.modalGraceMs - (this.runtime.now() - this.outageStartedAt));
    const delay = Math.min(1_000, graceRemaining);
    this.progressTimer = this.runtime.setTimeout(() => {
      this.progressTimer = null;
      this.emitConnectionState(this.connectionState.phase, this.connectionState.reason, false);
      if (
        this.connectionState.phase === "waiting" ||
        this.connectionState.phase === "offline" ||
        this.connectionState.phase === "connecting" ||
        this.connectionState.phase === "authenticating" ||
        this.connectionState.phase === "hydrating"
      ) {
        this.armProgressTimer();
      }
    }, delay);
  }

  private emitConnectionState(
    phase: ConnectionPhase,
    reason: string | null,
    intentional: boolean,
    forceGate = false,
  ): void {
    const now = this.runtime.now();
    const gateVisible = forceGate || (
      this.outageStartedAt !== null &&
      now - this.outageStartedAt >= this.reconnect.modalGraceMs &&
      phase !== "connected"
    );
    this.connectionState = {
      phase,
      generation: this.generation,
      attempt: this.attempt,
      reason: reason ? sanitizeReason(reason) : null,
      retryAt: this.retryAt,
      retryInMs: this.retryAt === null ? null : Math.max(0, this.retryAt - now),
      gateVisible,
      intentional,
    };
    this.handlers.onConnectionState?.({ ...this.connectionState });
  }

  private installSignals(): void {
    if (this.signalUnsubscribers.length > 0) return;
    this.signalUnsubscribers = [
      this.runtime.onOnline(() => {
        if (this.desired) this.retryNow();
      }),
      this.runtime.onVisibility(() => {
        if (!this.runtime.isVisible()) {
          this.noteSuspended();
        } else if (this.desired) {
          this.validateAfterResume();
        }
      }),
      this.runtime.onPageShow((persisted) => {
        if (!this.desired) return;
        if (persisted) this.suspensionSuspected = true;
        this.validateAfterResume();
      }),
      this.runtime.onPageHide(() => this.noteSuspended()),
      this.runtime.onFreeze(() => this.noteSuspended()),
      this.runtime.onResume(() => {
        if (this.desired) this.validateAfterResume();
      }),
    ];
  }

  private removeSignals(): void {
    for (const unsubscribe of this.signalUnsubscribers) unsubscribe();
    this.signalUnsubscribers = [];
  }

  private noteSuspended(): void {
    this.suspensionSuspected = true;
    // A backoff scheduled before pagehide may wake hours late with stale
    // assumptions. Keep the healthy socket, but pause retry-only timers until
    // a visible/pageshow signal can perform one immediate validated attempt.
    this.clearRetryTimer();
    this.clearHeartbeat();
    this.clearLivenessTimer();
    this.pendingProbe = null;
  }

  private probeLiveness(resume: boolean): void {
    const socket = this.socket;
    if (!socket || !this.authenticated || socket.readyState !== 1) {
      this.recycleForResume("Connection unavailable after resume", true);
      return;
    }
    if (this.pendingProbe?.generation === this.generation) {
      // Visibility + pageshow + resume commonly arrive as one burst.
      if (resume) this.pendingProbe.resume = true;
      return;
    }
    const generation = this.generation;
    const nonce = `${generation}:${this.nextProbeNonce++}:${Math.trunc(this.runtime.now())}`;
    this.pendingProbe = { generation, nonce, resume };
    if (!this.sendWorkspaceMessage({ Ping: { nonce } })) {
      this.pendingProbe = null;
      this.recycleForResume("Liveness probe could not be sent", true);
      return;
    }
    this.clearLivenessTimer();
    this.livenessTimer = this.runtime.setTimeout(() => {
      this.livenessTimer = null;
      if (
        this.pendingProbe?.generation !== generation ||
        this.pendingProbe.nonce !== nonce
      ) return;
      this.pendingProbe = null;
      this.recycleForResume("Connection did not respond after resume", true);
    }, this.reconnect.livenessTimeoutMs);
  }

  private noteLiveness(generation: number, nonce: string): void {
    if (generation !== this.generation) return;
    const probe = this.pendingProbe;
    if (
      !probe ||
      probe.generation !== generation ||
      probe.nonce !== nonce
    ) return;
    this.pendingProbe = null;
    this.clearLivenessTimer();
    this.armHeartbeat();
    if (probe.resume && this.connectionState.phase === "connected") {
      this.handlers.onAuthenticatedResume?.(generation);
    }
  }

  private armHeartbeat(): void {
    this.clearHeartbeat();
    if (
      !this.desired ||
      !this.runtime.isVisible() ||
      this.connectionState.phase !== "connected"
    ) return;
    this.heartbeatTimer = this.runtime.setTimeout(() => {
      this.heartbeatTimer = null;
      if (this.connectionState.phase === "connected") this.probeLiveness(false);
    }, this.reconnect.heartbeatIntervalMs);
  }

  private clearHeartbeat(): void {
    if (this.heartbeatTimer !== null) this.runtime.clearTimeout(this.heartbeatTimer);
    this.heartbeatTimer = null;
  }

  private clearLivenessTimer(): void {
    if (this.livenessTimer !== null) this.runtime.clearTimeout(this.livenessTimer);
    this.livenessTimer = null;
  }

  private recycleForResume(reason: string, reconnectImmediately: boolean): void {
    if (!this.desired) return;
    const socket = this.socket;
    this.generation += 1; // invalidate every callback/timer from the old socket
    this.socket = null;
    this.authenticated = false;
    this.connectedHostId = null;
    this.failureHandledGeneration = -1;
    this.clearAllTimers();
    this.pendingProbe = null;
    if (reconnectImmediately && !this.connectionState.gateVisible) {
      // Time spent suspended must not make a quick successful return flash the
      // modal. Start the visible grace window when recovery actually begins.
      this.outageStartedAt = this.runtime.now();
    } else {
      this.outageStartedAt ??= this.runtime.now();
    }
    this.retryAt = null;
    this.rejectPending(new Error(reason), false);
    try { socket?.close(); } catch { /* stale Safari socket */ }
    this.suspensionSuspected = false;
    if (!this.runtime.isOnline()) {
      this.emitConnectionState("offline", reason, false);
      this.armProgressTimer();
      return;
    }
    if (reconnectImmediately) {
      this.startAttempt();
    } else {
      this.emitConnectionState("offline", reason, false);
    }
  }

  private sendWorkspaceMessage(message: WorkspaceClientMessage): boolean {
    return this.send(ClientMessage.workspace({ message }));
  }
}

function helloAckFrom(msg: ServerMessage): {
  accepted: boolean;
  reason?: string | null;
} | null {
  if (!isWorkspaceReply(msg)) return null;
  const inner = msg.WorkspaceReply.message as { HelloAck?: { accepted: boolean; reason?: string | null } };
  return inner.HelloAck ?? null;
}

function pongFrom(msg: ServerMessage): { nonce: string } | null {
  if (!isWorkspaceReply(msg)) return null;
  const inner = msg.WorkspaceReply.message as { Pong?: { nonce?: unknown } };
  return typeof inner.Pong?.nonce === "string"
    ? { nonce: inner.Pong.nonce }
    : null;
}

function sanitizeReason(reason: string): string {
  return reason
    .replace(/([?&](?:token|auth|key|secret)=)[^\s&]+/gi, "$1[redacted]")
    .replace(/(\b(?:token|secret|authorization|credential)\b\s*[:=]\s*)\S+/gi, "$1[redacted]")
    .replace(/(wss?:\/\/)[^\s/@]+:[^\s/@]+@/gi, "$1[redacted]@")
    .replace(/\s+/g, " ")
    .trim()
    .slice(0, 240) || "Connection lost";
}

function closeDetail(code: number, reason: string): string {
  const clean = sanitizeReason(reason || "Connection closed");
  return code === 1000 ? clean : `${clean} (code ${code})`;
}

function browserRuntime(): ProtocolClientRuntime {
  return {
    createSocket: (url) => new WebSocket(url),
    setTimeout: (callback, delayMs) => setTimeout(callback, delayMs),
    clearTimeout: (handle) => clearTimeout(handle),
    now: () => Date.now(),
    random: () => Math.random(),
    isOnline: () => typeof navigator === "undefined" || navigator.onLine !== false,
    isVisible: () => typeof document === "undefined" || document.visibilityState === "visible",
    onOnline: (callback) => {
      if (typeof window === "undefined") return () => undefined;
      window.addEventListener("online", callback);
      return () => window.removeEventListener("online", callback);
    },
    onVisibility: (callback) => {
      if (typeof document === "undefined") return () => undefined;
      document.addEventListener("visibilitychange", callback);
      return () => document.removeEventListener("visibilitychange", callback);
    },
    onPageShow: (callback) => {
      if (typeof window === "undefined") return () => undefined;
      const listener = (event: PageTransitionEvent) => callback(event.persisted);
      window.addEventListener("pageshow", listener);
      return () => window.removeEventListener("pageshow", listener);
    },
    onPageHide: (callback) => {
      if (typeof window === "undefined") return () => undefined;
      window.addEventListener("pagehide", callback);
      return () => window.removeEventListener("pagehide", callback);
    },
    onFreeze: (callback) => {
      if (typeof document === "undefined") return () => undefined;
      document.addEventListener("freeze", callback);
      return () => document.removeEventListener("freeze", callback);
    },
    onResume: (callback) => {
      if (typeof document === "undefined") return () => undefined;
      document.addEventListener("resume", callback);
      return () => document.removeEventListener("resume", callback);
    },
  };
}

function websocketUrl(url: string, authToken?: string): string {
  const token = authToken?.trim();
  if (!token) return url;
  try {
    const parsed = new URL(url);
    if (!parsed.searchParams.has("token")) {
      parsed.searchParams.set("token", token);
    }
    return parsed.toString();
  } catch {
    const joiner = url.includes("?") ? "&" : "?";
    return `${url}${joiner}token=${encodeURIComponent(token)}`;
  }
}
