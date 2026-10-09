import { createNeoismClient, type ConnectionGrant, type HostStatus, type MachineHandle, type NeoismClient } from "@neoism/sdk-core";
import { createHttpTransport } from "./index.js";
import { CloudRuntimeError, endpoint, fail, handle, id, integer, ownerId, record, same, validateLaunch, validateStatus } from "./cloud.js";
import { normalizedVmNamespace, sameVmNamespace } from "./namespace.js";

export interface HostWorkspaceClientOptions {
  /** Your application's authorized host/gateway, not the infrastructure bridge. */
  endpoint: string;
  /** External application auth only. No automatic cloud credentials or private keys. */
  token: string | ((signal?: AbortSignal) => string | Promise<string>);
  fetch?: typeof globalThis.fetch;
  timeoutMs?: number;
  allowPlaintext?: boolean;
  /** Explicitly trusted worker origins; defaults to this gateway's origin. */
  trustedWorkerOrigins?: readonly string[];
  developmentContainers?: boolean;
}

function abortError(): DOMException { return new DOMException("Request aborted", "AbortError"); }
function wait<T>(pending: Promise<T>, signal?: AbortSignal): Promise<T> {
  if (!signal) return pending;
  if (signal.aborted) return Promise.reject(abortError());
  return new Promise((resolve, reject) => {
    const abort = () => { cleanup(); reject(abortError()); };
    const cleanup = () => signal.removeEventListener("abort", abort);
    signal.addEventListener("abort", abort, { once: true });
    pending.then(value => { cleanup(); resolve(value); }, error => { cleanup(); reject(error); });
  });
}
function admittedCaps(value: unknown, development: boolean): void {
  const c = record(value, ["isolation", "stop_start", "cpu_limit", "memory_limit", "disk_limit", "durable_workspace"]);
  if (!["virtual_machine", "container"].includes(c.isolation as string) || ["stop_start", "cpu_limit", "memory_limit", "disk_limit", "durable_workspace"].some(k => typeof c[k] !== "boolean")) fail();
  if (!c.cpu_limit || !c.memory_limit || !c.durable_workspace) fail("rejected");
  if (c.isolation === "container" ? !development || c.disk_limit !== false : !c.stop_start || !c.cpu_limit || !c.memory_limit || !c.disk_limit || !c.durable_workspace) fail("rejected");
}
function snapshot(value: unknown, workspace: string, expected?: MachineHandle): HostStatus {
  const s = record(value, ["binding", "verification"]);
  const b = record(s.binding, ["allocation", "retired", "revision"], ["last_error", "pending", "status"]);
  const a = record(b.allocation, ["owner", "generation", "provider", "spec"], ["launch"]);
  const o = record(a.owner, ["tenant", "workspace"]);
  const spec = record(a.spec, ["image", "region", "vcpus", "memory_mib", "disk_gib"]);
  if (!ownerId(o.tenant) || o.workspace !== workspace || !integer(a.generation, 1) || !id(a.provider) || !integer(b.revision, 1) || typeof b.retired !== "boolean" || !id(spec.image) || !id(spec.region) || !integer(spec.vcpus, 1, 1024) || !integer(spec.memory_mib, 128, 4194304) || !integer(spec.disk_gib, 1, 65536)) fail("identity");
  if (a.launch != null) validateLaunch(a.launch);
  const result = s as unknown as HostStatus;
  if (b.status != null) validateStatus(b.status, result.binding.allocation, expected);
  else if (expected) fail("identity");
  if (b.pending != null && !["ensure", "start", "stop", "destroy"].includes(b.pending as string)) fail();
  if (b.last_error != null) {
    const e = record(b.last_error, ["code", "retryable"]);
    if (!["transport", "timeout", "unauthorized", "not_found", "conflict", "rejected", "unavailable", "protocol", "identity"].includes(e.code as string) || typeof e.retryable !== "boolean") fail();
  }
  if (s.verification != null) {
    const v = record(s.verification, ["at", "expires_at", "handle", "descriptor", "revision"]);
    handle(v.handle); validateLaunch(v.descriptor);
    if (b.retired || b.pending != null || !result.binding.status || !same(v.handle, result.binding.status.handle) || !integer(v.at, 1) || !integer(v.expires_at, 1) || v.expires_at <= v.at || v.expires_at <= Date.now() / 1000 || v.revision !== b.revision || ["version", "runtime_id", "root", "state_root", "expires_at", "verification_key"].some(k => (v.descriptor as Record<string, unknown>)[k] !== (a.launch as Record<string, unknown> | undefined)?.[k])) fail("identity");
  }
  return result;
}

/** Optional host lifecycle API. Tenant, resource policy and actor scopes come from
 * injected host authorization, never from user request bodies. No mutation retries.
 */
export class HostWorkspaceClient {
  #endpoint: URL;
  #token: HostWorkspaceClientOptions["token"];
  #fetch: typeof globalThis.fetch;
  #timeout: number;
  #origins: Set<string>;
  #plaintext: boolean;
  #development: boolean;
  constructor(options: HostWorkspaceClientOptions) {
    this.#plaintext = options.allowPlaintext === true;
    this.#endpoint = endpoint(options.endpoint, this.#plaintext);
    this.#token = options.token;
    this.#fetch = options.fetch ?? globalThis.fetch;
    this.#timeout = options.timeoutMs ?? 30_000;
    this.#development = options.developmentContainers === true;
    if (!this.#fetch || !integer(this.#timeout, 1, 600_000) || !(typeof options.token === "function" || typeof options.token === "string" && /^[\x21-\x7e]{1,16384}$/.test(options.token))) fail("rejected");
    this.#origins = new Set((options.trustedWorkerOrigins ?? [this.#endpoint.origin]).map(value => {
      const url = endpoint(value, this.#plaintext);
      if (url.pathname !== "/") fail("rejected");
      return url.origin;
    }));
  }
  status(workspace: string, signal?: AbortSignal): Promise<HostStatus> { return this.#status("status", workspace, undefined, signal); }
  ensure(workspace: string, signal?: AbortSignal): Promise<HostStatus> { return this.#status("ensure", workspace, undefined, signal); }
  start(workspace: string, expected: MachineHandle, signal?: AbortSignal): Promise<HostStatus> { return this.#status("start", workspace, expected, signal); }
  stop(workspace: string, expected: MachineHandle, signal?: AbortSignal): Promise<HostStatus> { return this.#status("stop", workspace, expected, signal); }
  destroy(workspace: string, expected: MachineHandle, signal?: AbortSignal): Promise<HostStatus> { return this.#status("destroy", workspace, expected, signal); }
  async #status(action: string, workspace: string, expected?: MachineHandle, signal?: AbortSignal): Promise<HostStatus> {
    if (expected) { handle(expected); if (expected.owner.workspace !== workspace) fail("identity"); expected = structuredClone(expected); }
    return snapshot(await this.#request(action, workspace, expected ? { expected_handle: expected } : {}, signal), workspace, expected);
  }
  async connection(workspace: string, signal?: AbortSignal): Promise<ConnectionGrant> {
    const value = await this.#request("connection", workspace, {}, signal);
    const g = record(value, ["version", "baseUrl", "handle", "root", "runtimeId", "workerGeneration", "bearer", "expiresAt", "capabilities"]);
    handle(g.handle);
    const url = endpoint(g.baseUrl, this.#plaintext);
    if (g.version !== 1 || g.handle.owner.workspace !== workspace || g.workerGeneration !== g.handle.generation || !id(g.runtimeId) || !normalizedVmNamespace(g.root) || !integer(g.expiresAt, 1) || g.expiresAt <= Date.now() / 1000 || typeof g.bearer !== "string" || !/^[\x21-\x7e]{1,16384}$/.test(g.bearer)) fail("identity");
    if (url.href !== g.baseUrl || !this.#origins.has(url.origin)) fail("identity");
    admittedCaps(g.capabilities, this.#development);
    return value as ConnectionGrant;
  }
  async #request(action: string, workspace: string, body: unknown, signal?: AbortSignal): Promise<unknown> {
    if (!ownerId(workspace)) fail("identity");
    if (signal?.aborted) throw abortError();
    const controller = new AbortController();
    const abort = () => controller.abort();
    signal?.addEventListener("abort", abort, { once: true });
    let timer!: ReturnType<typeof setTimeout>;
    const deadline = new Promise<never>((_, reject) => { timer = setTimeout(() => { controller.abort(); reject(new CloudRuntimeError("timeout", true)); }, this.#timeout); });
    const bounded = <T>(p: Promise<T>): Promise<T> => Promise.race([wait(p, signal), deadline]);
    try {
      const token = await bounded(Promise.resolve().then(() => typeof this.#token === "function" ? this.#token(controller.signal) : this.#token));
      if (typeof token !== "string" || !/^[\x21-\x7e]{1,16384}$/.test(token)) fail("unauthorized");
      const response = await bounded(this.#fetch(new URL(`v1/workspaces/${encodeURIComponent(workspace)}/runtime/${action}`, this.#endpoint), {
        method: action === "status" ? "GET" : "POST", redirect: "manual", credentials: "omit", cache: "no-store", signal: controller.signal,
        headers: { authorization: `Bearer ${token}`, accept: "application/json", ...(action === "status" ? {} : { "content-type": "application/json" }) },
        ...(action === "status" ? {} : { body: JSON.stringify(body) }),
      }));
      if (response.redirected || response.status >= 300 && response.status < 400 || response.url && new URL(response.url).origin !== this.#endpoint.origin) fail();
      if (response.status !== 200) fail(response.status === 401 || response.status === 403 ? "unauthorized" : response.status === 409 ? "conflict" : response.status >= 500 ? "unavailable" : "rejected");
      if (Number(response.headers.get("content-length")) > 65536 || !response.body) fail();
      const reader = response.body.getReader();
      let text = "", length = 0;
      const decoder = new TextDecoder("utf-8", { fatal: true });
      try {
        while (true) {
          const chunk = await bounded(reader.read());
          if (chunk.done) break;
          length += chunk.value.length;
          if (length > 65536) fail();
          text += decoder.decode(chunk.value, { stream: true });
        }
        text += decoder.decode();
      } finally { void reader.cancel().catch(() => {}); reader.releaseLock(); }
      try { return JSON.parse(text); } catch { return fail(); }
    } catch (error) {
      if (signal?.aborted) throw abortError();
      if (error instanceof CloudRuntimeError) throw error;
      return fail("transport", true);
    } finally { clearTimeout(timer); signal?.removeEventListener("abort", abort); }
  }
}

export interface ConnectWorkspaceWorkerOptions {
  host: HostWorkspaceClient;
  workspace: string;
  fetch?: typeof globalThis.fetch;
  signal?: AbortSignal;
  /** 0–30 seconds; short-lived grants use a smaller proportional skew. */
  expirySkewSeconds?: number;
  /** Total initial worker verification deadline, even with injected fetch. */
  connectionTimeoutMs?: number;
}

/** Fixed-identity connection. A replacement handle, runtime, root or endpoint fails
 * closed before sending credentials; call this helper again to explicitly reconnect.
 * One broker refresh per connection, shared by concurrent HTTP/SSE resolutions.
 */
export async function connectWorkspaceWorker(options: ConnectWorkspaceWorkerOptions): Promise<NeoismClient> {
  options = { ...options }; // Pin caller context before the first asynchronous broker read.
  if (!ownerId(options.workspace)) fail("identity");
  const skew = options.expirySkewSeconds ?? 30;
  const timeout = options.connectionTimeoutMs ?? 30_000;
  if (!integer(skew, 0, 30) || !integer(timeout, 1, 600_000)) fail("rejected");
  let cached: ConnectionGrant | undefined;
  let refreshAt = 0;
  let anchor: string | undefined;
  let statusLeaseExpiresAt = 0;
  let changed = false;
  let pending: Promise<ConnectionGrant> | undefined;
  const refresh = async (): Promise<ConnectionGrant> => {
    if (changed) fail("identity");
    const status = await options.host.status(options.workspace);
    const b = status.binding, d = b.allocation.launch, s = b.status;
    if (b.retired || b.pending != null || b.last_error != null || !s || s.state !== "running" || !s.connection || !d || d.expires_at <= Date.now() / 1000) fail("unavailable");
    const grant = await options.host.connection(options.workspace);
    if (!same(grant.handle, s.handle) || grant.runtimeId !== d.runtime_id || !sameVmNamespace(grant.root, d.root) || grant.workerGeneration !== b.allocation.generation || grant.expiresAt > d.expires_at || grant.baseUrl !== s.connection.agent_api_base_url) fail("identity");
    const key = JSON.stringify([grant.handle.owner.tenant, grant.handle.owner.workspace, grant.handle.provider, grant.handle.machine_id, grant.handle.generation, grant.baseUrl, grant.runtimeId, grant.workerGeneration, normalizedVmNamespace(grant.root), normalizedVmNamespace(d.state_root), d.expires_at, d.verification_key]);
    if (anchor !== undefined && anchor !== key) { changed = true; cached = undefined; fail("identity"); }
    anchor = key;
    statusLeaseExpiresAt = d.expires_at;
    cached = grant;
    const now = Date.now() / 1000;
    refreshAt = grant.expiresAt - Math.min(skew, (grant.expiresAt - now) / 10);
    return grant;
  };
  const getGrant = (signal?: AbortSignal): Promise<ConnectionGrant> => {
    if (signal?.aborted) return Promise.reject(abortError());
    if (changed) return Promise.reject(new CloudRuntimeError("identity", false));
    if (cached && Date.now() / 1000 < refreshAt) return wait(Promise.resolve(cached), signal);
    // An individual caller's abort must not cancel other users of the shared refresh.
    pending ??= refresh().finally(() => { pending = undefined; });
    return wait(pending, signal);
  };
  const first = await getGrant(options.signal);
  const transport = createHttpTransport({ baseUrl: first.baseUrl, ...(options.fetch ? { fetch: options.fetch } : {}), token: async signal => (await getGrant(signal)).bearer });
  // Browser WebSocket cannot attach this bearer. Never fall back to ambient cookies.
  transport.connectSocket = async () => { throw new CloudRuntimeError("rejected", false); };
  const client = createNeoismClient(transport);
  const controller = new AbortController();
  const abort = () => controller.abort();
  options.signal?.addEventListener("abort", abort, { once: true });
  let timer!: ReturnType<typeof setTimeout>;
  const deadline = new Promise<never>((_, reject) => { timer = setTimeout(() => { controller.abort(); reject(new CloudRuntimeError("timeout", true)); }, timeout); });
  let runtime;
  try { runtime = await Promise.race([wait(client.runtime.get({ signal: controller.signal }), options.signal), deadline]); }
  finally { clearTimeout(timer); options.signal?.removeEventListener("abort", abort); }
  const info = record(runtime, ["deployment", "executionAvailable"], ["worker"]);
  record(info.worker, ["version", "tenantId", "workspaceId", "runtimeId", "runtimeGeneration", "root", "expiresAt"]);
  if (runtime.deployment !== "workspace-worker" || runtime.executionAvailable !== true || !runtime.worker || runtime.worker.version !== 1 || runtime.worker.tenantId !== first.handle.owner.tenant || runtime.worker.workspaceId !== options.workspace || runtime.worker.runtimeId !== first.runtimeId || runtime.worker.runtimeGeneration !== first.workerGeneration || !sameVmNamespace(runtime.worker.root, first.root) || runtime.worker.expiresAt !== statusLeaseExpiresAt || runtime.worker.expiresAt <= Date.now() / 1000) fail("identity");
  return client;
}
