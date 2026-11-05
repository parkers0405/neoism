import * as Context from "effect/Context";
import * as Data from "effect/Data";
import * as Effect from "effect/Effect";
import * as Layer from "effect/Layer";
import * as Schema from "effect/Schema";
import WebSocket from "ws";
import { batch } from "solid-js";
import type { DatabaseRequest } from "./protocol";
import { parseEnvelope, variant, isRecord, type WireEvent, type ConnectionState, type Family,
  type WorkspaceClientMessage, type FilesClientMessage, type AgentClientMessage, type PtyMessage, type SearchClientMessage, type ConfigClientMessage, type EditorClientMessage, type DiagnosticsClientMessage, type GitClientMessage, type CrdtClientMessage } from "./protocol";

export class DaemonError extends Data.TaggedError("DaemonError")<{ message: string }> {}
type Messages = { Database: DatabaseRequest; Workspace: WorkspaceClientMessage; Files: FilesClientMessage; Agent: AgentClientMessage; Pty: PtyMessage; Search: SearchClientMessage; Config: ConfigClientMessage; Editor: EditorClientMessage; Diagnostics: DiagnosticsClientMessage; Git: GitClientMessage; Crdt:CrdtClientMessage };
interface Pending { family: Family; resume: (result: Effect.Effect<unknown, DaemonError>) => void; timer: ReturnType<typeof setTimeout> }
export type SocketFactory = (endpoint: string, token?: string) => WebSocket;
export interface ConnectionFailure { reason:string; attempt:number; quietMs:number; lastMessage?:{family:Family;kind:string;bytes:number} }
export interface DaemonConnectionConfig {
  endpoint: string;
  authToken?: string;
  pairingToken?: string;
  clientName?: string;
  handshakeTimeoutMs?: number;
}
export interface DaemonConnectionHooks {
  profileStart: () => number;
  profileEnd: (name: string, started: number, bytes: number) => void;
  /** Use the host's Solid instance when the host installs its own renderer runtime. */
  batch?: (run: () => void) => void;
  /** Refresh/start an owned local service before each dial; never replay requests. */
  prepareConnection?: (signal: AbortSignal) => Promise<Partial<DaemonConnectionConfig>>;
}
const noProfiling: DaemonConnectionHooks = { profileStart: () => 0, profileEnd: () => {} };

export class DaemonConnection {
  private socket?: WebSocket;
  private connectedHostId?:string;
  private workspaceLifecycle=false;
  private generation = 0;
  private requestId = 1;
  private desired = false;
  private attempt = 0;
  private reconnectTimer?: ReturnType<typeof setTimeout>;
  private deadline?: ReturnType<typeof setTimeout>;
  private heartbeat?: ReturnType<typeof setInterval>;
  private preparation?: AbortController;
  private lastMessageAt = 0;
  private lastMessage?:ConnectionFailure["lastMessage"];
  private lastFailureLog?:{reason:string;at:number};
  private pending = new Map<number, Pending>();
  private listeners = new Set<(event: WireEvent) => void>();
  private statusListeners = new Set<(status: ConnectionState) => void>();
  state: ConnectionState = { phase: "closed", attempt: 0 };

  constructor(readonly config: DaemonConnectionConfig,
    private readonly createSocket: SocketFactory = (endpoint, token) => {
      // NEOISM_DAEMON_TOKEN is the existing legacy upgrade credential.
      // Authorization: Bearer is a DIFFERENT, paired-device credential.
      const url = new URL(endpoint);
      if (token) url.searchParams.set("token", token);
      return new WebSocket(url.toString(), { maxPayload: 16 * 1024 * 1024 });
    }, private readonly onFailure:(failure:ConnectionFailure)=>void = failure=>console.error(`[${config.clientName ?? "neoism-gpuix"}] Daemon connection lost:`,JSON.stringify(failure)),
    private readonly hooks: DaemonConnectionHooks = noProfiling) {}

  private failureReason(reason:string){
    for(const token of [this.config.authToken,this.config.pairingToken])if(token)reason=reason.replaceAll(token,"[redacted]");
    return reason.replace(/([?&](?:token|access_token)=)[^&\s]+/gi,"$1[redacted]").slice(0,400);
  }

  subscribe = (listener: (event: WireEvent) => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  subscribeStatus = (listener: (state: ConnectionState) => void) => { this.statusListeners.add(listener); listener(this.state); return () => { this.statusListeners.delete(listener); }; };

  private setState(phase: ConnectionState["phase"], reason?: string) {
    this.state = { phase, attempt: this.attempt, reason,...(this.connectedHostId?{connectedHostId:this.connectedHostId}:{}),...(this.workspaceLifecycle?{workspaceLifecycle:true}:{}) };
    for (const listener of this.statusListeners) listener(this.state);
  }
  start() { if (!this.desired) { this.desired = true; this.connect(); } }
  reconnect() { this.stop(); this.attempt = 0; this.start(); }
  stop() {
    this.desired = false; this.generation++;
    this.clearTimers(); this.socket?.terminate(); this.socket = undefined;
    this.rejectPending("Disconnected"); this.setState("closed");
  }
  private clearTimers() {
    this.preparation?.abort(); this.preparation = undefined;
    clearTimeout(this.reconnectTimer); clearTimeout(this.deadline); clearInterval(this.heartbeat);
    this.reconnectTimer = undefined; this.deadline = undefined; this.heartbeat = undefined;
  }
  private rejectPending(message: string) {
    for (const pending of this.pending.values()) { clearTimeout(pending.timer); pending.resume(Effect.fail(new DaemonError({ message }))); }
    this.pending.clear();
  }
  private connect() {
    const generation = ++this.generation;
    this.lastMessageAt=0;this.lastMessage=undefined;
    this.setState("connecting");
    if (this.hooks.prepareConnection) {
      const controller = this.preparation = new AbortController();
      void Promise.resolve().then(() => this.hooks.prepareConnection!(controller.signal)).then(config => {
        if (!this.desired || generation !== this.generation) return;
        this.preparation = undefined;
        Object.assign(this.config, config);
        this.openSocket(generation);
      }).catch(error => this.failed(generation, error instanceof Error ? error.message : "Local daemon recovery failed"));
    } else this.openSocket(generation);
  }
  private openSocket(generation: number) {
    if (!this.desired || generation !== this.generation) return;
    let socket: WebSocket;
    try { socket = this.createSocket(this.config.endpoint, this.config.authToken); }
    catch { this.failed(generation, "Could not open daemon connection"); return; }
    this.socket = socket;
    const current = () => this.desired && generation === this.generation;
    const timeout = this.config.handshakeTimeoutMs ?? (process.platform === "win32" ? 60_000 : 8000);
    this.deadline = setTimeout(() => this.failed(generation, "Daemon WebSocket upgrade timed out"), timeout);
    socket.on("open", () => {
      if (!current()) return;
      clearTimeout(this.deadline);
      this.deadline = setTimeout(() => this.failed(generation, "Daemon handshake timed out"), timeout);
      this.setState("authenticating");
      socket.send(JSON.stringify({ Workspace: { request_id: 0, message: { Hello: {
        token: this.config.pairingToken ?? null, client_name: this.config.clientName ?? "neoism-gpuix",
      } } } }));
    });
    socket.on("message", (data) => {
      if (!current()) return;
      const started = this.hooks.profileStart();
      let family: Family | undefined;
      let bytes = 0;
      try { (this.hooks.batch ?? batch)(() => {
      let event: WireEvent | undefined;
      const raw=data.toString();
      try { event = parseEnvelope(raw); }
      catch { this.failed(generation, "Daemon sent invalid JSON"); return; }
      if (!event) return;
      family = event.family;
      bytes = Buffer.isBuffer(data) ? data.length : Buffer.byteLength(raw);
      this.lastMessageAt = Date.now();
      const message = variant(event.message);
      this.lastMessage={family:event.family,kind:(message?.[0]||"unknown").slice(0,80),bytes};
      if (event.family === "Workspace" && message?.[0] === "HelloAck") {
        let ack: { readonly accepted: boolean; readonly reason?: string | null;readonly connected_host_id?:string|null;readonly workspace_lifecycle?:boolean };
        try { ack = Schema.decodeUnknownSync(Schema.Struct({ accepted: Schema.Boolean, reason: Schema.optionalKey(Schema.NullOr(Schema.String)),connected_host_id:Schema.optionalKey(Schema.NullOr(Schema.String)),workspace_lifecycle:Schema.optionalKey(Schema.Boolean) }))(message[1]); }
        catch { this.failed(generation, "Invalid daemon handshake"); return; }
        if (!ack.accepted) {
          this.desired = false; this.clearTimers(); this.rejectPending("Authentication rejected");
          this.setState("auth-rejected", ack.reason || "Pairing token required"); socket.close(); return;
        }
        clearTimeout(this.deadline); this.attempt = 0;this.connectedHostId=ack.connected_host_id||undefined;this.workspaceLifecycle=ack.workspace_lifecycle===true; this.setState("connected");
        this.heartbeat = setInterval(() => {
          if(!current()||this.state.phase!=="connected")return;
          if (Date.now() - this.lastMessageAt > 45_000) { this.failed(generation, "Daemon stopped responding"); return; }
          try{this.send("Workspace", { Ping: { nonce: crypto.randomUUID() } });}
          catch(error){this.failed(generation,`Daemon heartbeat failed: ${error instanceof Error?error.message:"socket unavailable"}`);}
        }, 15_000);
      } else if (this.state.phase !== "connected") return;
      const pending = this.pending.get(event.requestId);
      if (pending?.family === event.family && message?.[0] !== "SearchProgress") {
        this.pending.delete(event.requestId); clearTimeout(pending.timer);
        if (["Error", "SearchError"].includes(message?.[0] || "") && message && isRecord(message[1]) && typeof message[1].message === "string")
          pending.resume(Effect.fail(new DaemonError({ message: message[1].message })));
        else pending.resume(Effect.succeed(event.message));
      }
      for (const listener of this.listeners) listener(event);
      }); } finally { this.hooks.profileEnd(`daemon.receive.${family ?? "invalid"}`, started, bytes); }
    });
    socket.on("error", (error:Error&{code?:string}) => { if (current()) this.failed(generation, `Daemon connection failed${error.code?` [${error.code}]`:""}: ${error.message}`); });
    socket.on("close", (code?:number) => { if (current()) this.failed(generation, `Daemon disconnected${code?` (WebSocket ${code})`:""}`); });
  }
  private failed(generation: number, reason: string) {
    if (!this.desired || generation !== this.generation) return;
    reason=this.failureReason(reason);
    this.generation++; this.clearTimers(); this.socket?.terminate(); this.socket = undefined;
    this.rejectPending(reason); this.attempt++;
    const now=Date.now();
    if(this.lastFailureLog?.reason!==reason||now-this.lastFailureLog.at>=10_000){
      this.lastFailureLog={reason,at:now};
      this.onFailure({reason,attempt:this.attempt,quietMs:this.lastMessageAt?Math.max(0,now-this.lastMessageAt):0,lastMessage:this.lastMessage});
    }
    this.setState("waiting", reason);
    const delay = Math.min(15_000, 500 * 2 ** Math.min(5, this.attempt - 1));
    this.reconnectTimer = setTimeout(() => { if (this.desired) this.connect(); }, delay);
  }
  send<F extends Family>(family: F, message: Messages[F], workspaceRoot?: string): number {
    if (this.state.phase !== "connected" || this.socket?.readyState !== WebSocket.OPEN) throw new DaemonError({ message: "Daemon is offline" });
    const requestId = this.requestId++;
    const entry = family === "Search" ? variant(message) : undefined;
    const payload = entry && entry[0] !== "CancelSearch" && isRecord(entry[1]) ? { [entry[0]]: { ...entry[1], req_id: requestId } } : message;
    this.socket.send(JSON.stringify({ [family]: { request_id: requestId, message: payload,
      ...(workspaceRoot && ["Files", "Search", "Git", "Editor", "Database"].includes(family) ? { workspace_root: workspaceRoot } : {}) } }));
    return requestId;
  }
  request<F extends Family>(family: F, message: Messages[F], workspaceRoot?: string): Effect.Effect<unknown, DaemonError> {
    return Effect.callback<unknown, DaemonError>((resume) => {
      let requestId: number;
      try { requestId = this.send(family, message, workspaceRoot); }
      catch { resume(Effect.fail(new DaemonError({ message: "Daemon is offline" }))); return; }
      const timer = setTimeout(() => {
        this.pending.delete(requestId);
        resume(Effect.fail(new DaemonError({ message: `${family} request timed out` })));
      }, 15_000);
      this.pending.set(requestId, { family, resume, timer });
      return Effect.sync(() => {
        clearTimeout(timer);
        const unfinished = this.pending.delete(requestId);
        if (unfinished && family === "Search" && this.state.phase === "connected") {
          try { this.send("Search", { CancelSearch: { req_id: requestId } }, workspaceRoot); } catch {}
        }
      });
    });
  }
}

export class Daemon extends Context.Service<Daemon, DaemonConnection>()("neoism/Daemon") {}
export function daemonLayer(connection: DaemonConnection) {
  return Layer.effect(Daemon, Effect.acquireRelease(
    Effect.sync(() => { connection.start(); return connection; }),
    (client) => Effect.sync(() => client.stop()),
  ));
}
