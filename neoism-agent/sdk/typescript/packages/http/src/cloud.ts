import type { Allocation, CloudRuntimeRequest, FailureCode, MachineHandle, MachineStatus, RuntimeCapabilities } from "@neoism/sdk-core";

export interface CloudRuntimeClientOptions {
  /** Generic lifecycle bridge, not an agent API or vendor VM endpoint. */
  endpoint: string;
  provider: string;
  /** Explicit host credential; never read from agent/account/provider configuration. */
  bearer: string;
  capabilities: RuntimeCapabilities;
  timeoutMs?: number;
  /** Development/private bridge opt-in; production defaults to HTTPS only. */
  allowPlaintext?: boolean;
  /** Explicit opt-in to truthful container capabilities; never production parity. */
  developmentContainers?: boolean;
  fetch?: typeof globalThis.fetch;
}

/** Deliberately excludes URLs, credentials, raw causes and response bodies. */
export class CloudRuntimeError extends Error {
  constructor(readonly code: FailureCode, readonly retryable: boolean) {
    super(`Cloud runtime: ${code} (retryable=${retryable})`);
    this.name = "CloudRuntimeError";
  }
}
export function fail(code: FailureCode = "protocol", retryable = false): never { throw new CloudRuntimeError(code, retryable); }
export const id = (v: unknown): v is string => typeof v === "string" && /^[A-Za-z0-9_-]{1,128}$/.test(v);
export const ownerId = (v: unknown): v is string => typeof v === "string" && /^[A-Za-z0-9_.:-]{1,128}$/.test(v);
export const integer = (v: unknown, min: number, max = Number.MAX_SAFE_INTEGER): v is number => typeof v === "number" && Number.isSafeInteger(v) && v >= min && v <= max;
export function record(v: unknown, required: string[], optional: string[] = []): Record<string, unknown> {
  if (!v || typeof v !== "object" || Array.isArray(v)) return fail();
  const r = v as Record<string, unknown>;
  if (required.some(k => !Object.hasOwn(r, k)) || Object.keys(r).some(k => !required.includes(k) && !optional.includes(k))) return fail();
  return r;
}
function owner(v: unknown): void {
  const r = record(v, ["tenant", "workspace"]);
  if (!ownerId(r.tenant) || !ownerId(r.workspace)) fail("identity");
}
export function handle(v: unknown): asserts v is MachineHandle {
  const r = record(v, ["owner", "generation", "provider", "machine_id"]);
  owner(r.owner);
  if (!integer(r.generation, 1) || !id(r.provider) || !id(r.machine_id)) fail("identity");
}
export function same(a: MachineHandle, b: MachineHandle): boolean {
  return a.owner.tenant === b.owner.tenant && a.owner.workspace === b.owner.workspace && a.generation === b.generation && a.provider === b.provider && a.machine_id === b.machine_id;
}
export function endpoint(value: unknown, plaintext: boolean): URL {
  if (typeof value !== "string" || !value || value.length > 2048 || /[\s\p{Cc}\\]/u.test(value) || value.includes("?") || value.includes("#")) return fail();
  let url: URL;
  try { url = new URL(value); } catch { return fail(); }
  if (!url.hostname || url.username || url.password || url.search || url.hash || !(url.protocol === "https:" || plaintext && url.protocol === "http:")) return fail();
  if (!url.pathname.endsWith("/")) url.pathname += "/";
  if (url.href.length > 2048) fail();
  return url;
}
/** Namespace paths belong to the worker VM, never the SDK host filesystem. */
export function vmPath(v: unknown): v is string {
  if (typeof v !== "string" || v.length > 4096 || /[\\\p{Cc}]/u.test(v)) return false;
  const tail = v.startsWith("/") ? v.slice(1) : /^[A-Z]:\//.test(v) ? v.slice(3) : "";
  return !!tail && tail.split("/").every(part => !!part && part !== "." && part !== ".." && !/[:<>|?*]/.test(part) && !/[. ]$/.test(part));
}
export function validateLaunch(value: unknown): void {
  const r = record(value, ["version", "runtime_id", "root", "state_root", "expires_at", "verification_key"]);
  // Canonical unpadded base64url for exactly 32 public-key bytes (last 2 bits zero).
  if (r.version !== 1 || !id(r.runtime_id) || !vmPath(r.root) || !vmPath(r.state_root) || !integer(r.expires_at, 1) || typeof r.verification_key !== "string" || !/^[A-Za-z0-9_-]{42}[AEIMQUYcgkosw048]$/.test(r.verification_key)) fail("identity");
  const windows = !r.root.startsWith("/") || !r.state_root.startsWith("/");
  const root = windows ? r.root.toLowerCase() : r.root;
  const state = windows ? r.state_root.toLowerCase() : r.state_root;
  if (root === state || root.startsWith(`${state}/`) || state.startsWith(`${root}/`)) fail("identity");
}
export function validateStatus(value: unknown, allocation: Allocation, expected?: MachineHandle): MachineStatus {
  const r = record(value, ["handle", "state", "ready"], ["connection", "failure"]);
  handle(r.handle);
  const h = r.handle;
  if (h.owner.tenant !== allocation.owner.tenant || h.owner.workspace !== allocation.owner.workspace || h.generation !== allocation.generation || h.provider !== allocation.provider || expected && !same(h, expected)) fail("identity");
  if (!["provisioning", "starting", "running", "stopping", "stopped", "failed", "destroyed"].includes(r.state as string) || typeof r.ready !== "boolean") fail();
  if (r.ready && (r.state !== "running" || !r.connection) || r.connection != null && r.state !== "running" || (r.state === "failed") !== (r.failure != null)) fail();
  if (r.failure != null) {
    const f = record(r.failure, ["code", "retryable"]);
    if (!["transport", "timeout", "unauthorized", "not_found", "conflict", "rejected", "unavailable", "protocol", "identity"].includes(f.code as string) || typeof f.retryable !== "boolean") fail();
  }
  if (r.connection != null) {
    const c = record(r.connection, ["version", "handle", "agent_api_base_url", "transport"]);
    handle(c.handle);
    if (!same(c.handle, h)) fail("identity");
    if (c.version !== 1 || !["https", "development_loopback_http"].includes(c.transport as string)) fail();
    const url = endpoint(c.agent_api_base_url, c.transport === "development_loopback_http");
    if (c.transport === "development_loopback_http" && !(url.protocol === "http:" && (/^127\./.test(url.hostname) || url.hostname === "[::1]"))) fail();
    if (url.href !== c.agent_api_base_url) fail();
  }
  // Reconstruct a non-secret DTO, never retain arbitrary response fields.
  return r as unknown as MachineStatus;
}

/** Host-only whole-workspace lifecycle. Never retries, follows redirects, or uses agent transport.
 * The upstream adapter must enforce generation fencing and idempotence for every mutation.
 */
export class CloudRuntimeClient {
  #endpoint: URL;
  #provider: string;
  #bearer: string;
  #fetch: typeof globalThis.fetch;
  #timeout: number;
  #capabilities: Readonly<RuntimeCapabilities>;
  constructor(options: CloudRuntimeClientOptions) {
    // Edge hosts may expose WorkerGlobalScope too; it is not a browser-only signal.
    // Callers must keep lifecycle credentials on trusted hosts, never browser workers.
    if (typeof window !== "undefined") fail("rejected");
    this.#endpoint = endpoint(options.endpoint, options.allowPlaintext === true);
    this.#timeout = options.timeoutMs ?? 30_000;
    const caps = record(options.capabilities, ["isolation", "stop_start", "cpu_limit", "memory_limit", "disk_limit", "durable_workspace"]);
    if (!["virtual_machine", "container"].includes(caps.isolation as string) || ["stop_start", "cpu_limit", "memory_limit", "disk_limit", "durable_workspace"].some(k => typeof caps[k] !== "boolean")) fail("rejected");
    if (!caps.cpu_limit || !caps.memory_limit || !caps.durable_workspace) fail("rejected");
    if (caps.isolation === "container" ? options.developmentContainers !== true || caps.disk_limit !== false : !caps.stop_start || !caps.cpu_limit || !caps.memory_limit || !caps.disk_limit || !caps.durable_workspace) fail("rejected");
    if (!id(options.provider) || !integer(this.#timeout, 1, 600_000) || typeof options.bearer !== "string" || !/^[\x21-\x7e]{1,8192}$/.test(options.bearer)) fail("rejected");
    this.#provider = options.provider;
    this.#bearer = options.bearer;
    this.#fetch = options.fetch ?? globalThis.fetch;
    if (!this.#fetch) fail("rejected");
    this.#capabilities = Object.freeze({ ...options.capabilities });
  }
  get capabilities(): Readonly<RuntimeCapabilities> { return this.#capabilities; }
  ensure(allocation: Allocation): Promise<MachineStatus> { return this.#request("ensure", allocation); }
  start(allocation: Allocation, handle: MachineHandle): Promise<MachineStatus> { return this.#request("start", allocation, handle); }
  inspect(allocation: Allocation, handle?: MachineHandle): Promise<MachineStatus> { return this.#request("inspect", allocation, handle); }
  stop(allocation: Allocation, handle: MachineHandle): Promise<MachineStatus> { return this.#request("stop", allocation, handle); }
  destroy(allocation: Allocation, handle: MachineHandle): Promise<MachineStatus> { return this.#request("destroy", allocation, handle); }

  async #request(action: string, allocation: Allocation, expected?: MachineHandle): Promise<MachineStatus> {
    record(allocation, ["owner", "generation", "provider", "spec"], ["launch"]);
    if (allocation.launch != null) {
      validateLaunch(allocation.launch);
      if ((action === "ensure" || action === "start") && allocation.launch.expires_at <= Date.now() / 1000) fail("rejected");
    }
    owner(allocation.owner);
    const s = record(allocation.spec, ["image", "region", "vcpus", "memory_mib", "disk_gib"]);
    if (!integer(allocation.generation, 1) || allocation.provider !== this.#provider || !id(s.image) || !id(s.region) || !integer(s.vcpus, 1, 1024) || !integer(s.memory_mib, 128, 4_194_304) || !integer(s.disk_gib, 1, 65_536)) fail("identity");
    if (expected) {
      handle(expected);
      if (expected.owner.tenant !== allocation.owner.tenant || expected.owner.workspace !== allocation.owner.workspace || expected.generation !== allocation.generation || expected.provider !== allocation.provider) fail("identity");
    } else if (action !== "ensure" && action !== "inspect") fail("identity");
    // Snapshot validated identity before yielding: callers may mutate their input objects.
    allocation = { ...allocation, owner: { ...allocation.owner }, spec: { ...allocation.spec }, ...(allocation.launch ? { launch: Object.freeze({ ...allocation.launch }) } : {}) };
    expected = expected ? { ...expected, owner: { ...expected.owner } } : undefined;
    const request: CloudRuntimeRequest = { version: 2, allocation, handle: expected ?? null };
    const controller = new AbortController();
    let timer!: ReturnType<typeof setTimeout>;
    const deadline = new Promise<never>((_, reject) => {
      timer = setTimeout(() => {
        controller.abort();
        reject(new CloudRuntimeError("timeout", true));
      }, this.#timeout);
    });
    // Enforce the same total deadline even if injected fetch/read implementations
    // ignore AbortSignal. Promise.race also observes late rejections safely.
    const bounded = <T>(pending: Promise<T>): Promise<T> => Promise.race([pending, deadline]);
    try {
      const response = await bounded(this.#fetch(new URL(`v2/runtime/${action}`, this.#endpoint), {
        method: "POST", redirect: "manual", credentials: "omit", signal: controller.signal,
        headers: { authorization: `Bearer ${this.#bearer}`, "content-type": "application/json", accept: "application/json" },
        body: JSON.stringify(request),
      }));
      if (response.redirected || response.url && new URL(response.url).origin !== this.#endpoint.origin) fail("protocol");
      if (response.status !== 200 && response.status !== 202) {
        const n = response.status;
        const code: FailureCode = n === 401 || n === 403 ? "unauthorized" : n === 404 ? "not_found" : n === 409 || n === 412 ? "conflict" : n === 408 || n === 504 ? "timeout" : n === 429 || n >= 500 && n <= 599 ? "unavailable" : "rejected";
        fail(code, code === "timeout" || code === "unavailable");
      }
      if (Number(response.headers.get("content-length")) > 65_536) return fail();
      const body = response.body;
      if (!body) return fail();
      const reader = body.getReader();
      const chunks: Uint8Array[] = [];
      let length = 0;
      try {
        while (true) {
          const { value, done } = await bounded(reader.read());
          if (done) break;
          length += value.length;
          if (length > 65_536) { void reader.cancel().catch(() => {}); fail(); }
          chunks.push(value);
        }
      } finally {
        if (controller.signal.aborted) void reader.cancel().catch(() => {});
        reader.releaseLock();
      }
      const bytes = new Uint8Array(length);
      let offset = 0;
      for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.length; }
      let decoded: unknown;
      try { decoded = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes)); } catch { return fail(); }
      const envelope = record(decoded, ["version", "status"]);
      if (envelope.version !== 2) fail();
      return validateStatus(envelope.status, allocation, expected);
    } catch (error) {
      if (error instanceof CloudRuntimeError) throw error;
      return fail(controller.signal.aborted ? "timeout" : "transport", true);
    } finally { clearTimeout(timer); }
  }
}
