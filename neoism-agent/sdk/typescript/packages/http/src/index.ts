export { CloudRuntimeClient, CloudRuntimeError, type CloudRuntimeClientOptions } from "./cloud.js";
export * from "./host.js";
import {
  createNeoismClient,
  type ApiErrorBody,
  type Event,
  type EventOptions,
  type NeoismTransport,
  type NeoismSocket,
  type RequestDescriptor,
  type SocketDescriptor,
} from "@neoism/sdk-core";

const SEEN_EVENT_WINDOW = 8192;

export interface HttpTransportOptions {
  baseUrl: string;
  /** Resolved per HTTP request and SSE connection, including reconnects.
   * Explicit headers.authorization overrides this token, case-insensitively.
   * Never used for lifecycle bridge requests or WebSocket URLs.
   */
  token?: string | ((signal?: AbortSignal) => string | undefined | Promise<string | undefined>);
  headers?: Record<string, string>;
  fetch?: typeof globalThis.fetch;
  webSocket?: (url: string) => WebSocket;
  reconnect?: { initialDelayMs?: number; maximumDelayMs?: number };
}

export class NeoismApiError extends Error {
  constructor(
    readonly status: number,
    readonly body: ApiErrorBody,
  ) {
    super(body.message);
    this.name = "NeoismApiError";
  }
}

export function createHttpTransport(options: HttpTransportOptions): NeoismTransport {
  const fetchSource = options.fetch ?? globalThis.fetch;
  if (!fetchSource) throw new Error("A fetch implementation is required");
  const fetcher: typeof globalThis.fetch = async (input, init) => {
    try { return await fetchSource(input, init); }
    catch {
      if (init?.signal?.aborted) throw aborted();
      throw new NeoismApiError(503, { code: "transport.request_failed", message: "Transport request failed", retryable: true, details: {} });
    }
  };
  const baseUrl = trustedBaseUrl(options.baseUrl);
  const explicitHeaders = { ...options.headers };
  const tokenSource = options.token;
  const commonHeaders = async (signal?: AbortSignal): Promise<Record<string, string>> => {
    if (signal?.aborted) throw aborted();
    try {
      const token = typeof tokenSource === "function"
        ? await resolveToken(tokenSource, signal)
        : tokenSource;
      if (signal?.aborted) throw aborted();
      // Normalize authorization casing so explicit caller headers always win.
      return mergeHeaders(token ? { authorization: `Bearer ${token}` } : {}, explicitHeaders);
    } catch {
      if (signal?.aborted) throw aborted();
      // Callback errors may contain credentials or broker response bodies. Do not
      // retain the original error as a cause or expose its message/details.
      throw new NeoismApiError(401, {
        code: "auth.token_resolution_failed", message: "Authentication token resolution failed",
        retryable: false, details: {},
      });
    }
  };

  return {
    async request<T>(request: RequestDescriptor): Promise<T> {
      const url = buildUrl(baseUrl, request);
      const headers = await commonHeaders(request.signal);
      if (request.signal?.aborted) throw aborted();
      const binaryBody = isBinaryBody(request.body);
      const init: RequestInit = {
        method: request.method ?? "GET",
        redirect: "manual", credentials: "omit",
        headers: mergeHeaders({
          accept: request.response === "bytes" ? "application/octet-stream" : "application/json",
          ...(request.body === undefined || binaryBody ? {} : { "content-type": "application/json" }),
        }, headers, request.headers ?? {}),
        ...(request.body === undefined
          ? {}
          : { body: binaryBody ? request.body as BodyInit : JSON.stringify(request.body) }),
        ...(request.signal === undefined ? {} : { signal: request.signal }),
      };
      const response = await fetcher(url, init);
      assertResponseOrigin(response, baseUrl);
      if (!response.ok) throw await apiError(response);
      if (response.status === 204) return undefined as T;
      try {
        if (request.response === "bytes") return new Uint8Array(await response.arrayBuffer()) as T;
        if (request.response === "text") return await response.text() as T;
        return await response.json() as T;
      } catch {
        if (request.signal?.aborted) throw aborted();
        throw new NeoismApiError(502, { code: "transport.invalid_response", message: "Invalid transport response", retryable: false, details: {} });
      }
    },
    events(eventOptions: EventOptions = {}) {
      return followEvents(fetcher, baseUrl, commonHeaders, eventOptions, options.reconnect);
    },
    connectSocket(request: SocketDescriptor) {
      const factory = options.webSocket ?? defaultWebSocket;
      return openSocket(factory, buildSocketUrl(baseUrl, request), request.signal);
    },
  };
}

function aborted(): DOMException {
  // An arbitrary AbortSignal.reason may itself contain broker credentials.
  return new DOMException("Request aborted", "AbortError");
}

function mergeHeaders(...sources: Record<string, string>[]): Record<string, string> {
  const headers = new Headers();
  for (const source of sources) {
    for (const [key, value] of Object.entries(source)) headers.set(key, value);
  }
  const merged: Record<string, string> = {};
  headers.forEach((value, key) => { merged[key] = value; });
  return merged;
}

function resolveToken(
  resolver: Exclude<HttpTransportOptions["token"], string | undefined>,
  signal?: AbortSignal,
): Promise<string | undefined> {
  return new Promise((resolve, reject) => {
    const abort = () => { cleanup(); reject(aborted()); };
    const cleanup = () => signal?.removeEventListener("abort", abort);
    if (signal?.aborted) { reject(aborted()); return; }
    signal?.addEventListener("abort", abort, { once: true });
    Promise.resolve().then(() => {
      if (signal?.aborted) throw aborted();
      return resolver(signal);
    }).then(value => {
      cleanup();
      if (signal?.aborted) reject(aborted());
      else resolve(value);
    }, error => { cleanup(); reject(error); });
  });
}

function defaultWebSocket(url: string): WebSocket {
  if (!globalThis.WebSocket) throw new Error("A WebSocket implementation is required");
  return new globalThis.WebSocket(url);
}

function openSocket(
  factory: (url: string) => WebSocket,
  url: string,
  signal?: AbortSignal,
): Promise<NeoismSocket> {
  if (signal?.aborted) return Promise.reject(signal.reason);
  return new Promise((resolve, reject) => {
    const socket = factory(url);
    socket.binaryType = "arraybuffer";
    const queued: Array<string | Uint8Array> = [];
    const waiting: Array<(result: IteratorResult<string | Uint8Array>) => void> = [];
    let ended = false;
    let failure: unknown;
    let opened = false;

    const push = (value: string | Uint8Array) => {
      if (ended) return;
      const waiter = waiting.shift();
      if (waiter) waiter({ value, done: false });
      else queued.push(value);
    };
    const finish = () => {
      ended = true;
      for (const waiter of waiting.splice(0)) waiter({ value: undefined, done: true });
    };
    const abort = () => socket.close(1000, "aborted");
    signal?.addEventListener("abort", abort, { once: true });
    socket.addEventListener("message", (event) => {
      if (typeof event.data === "string") push(event.data);
      else if (event.data instanceof ArrayBuffer) push(new Uint8Array(event.data));
      else if (event.data instanceof Blob) void event.data.arrayBuffer().then((data) => push(new Uint8Array(data)));
    });
    socket.addEventListener("close", () => {
      finish();
      if (!opened) reject(new Error("WebSocket closed before connecting"));
    }, { once: true });
    socket.addEventListener("error", () => {
      failure = new Error("WebSocket connection failed");
      finish();
      reject(failure);
    }, { once: true });
    socket.addEventListener("open", () => {
      opened = true;
      resolve({
        send(data) { socket.send(data); },
        close(code, reason) { socket.close(code, reason); },
        messages() {
          const iterator: AsyncIterableIterator<string | Uint8Array> = {
            [Symbol.asyncIterator]() { return iterator; },
            next(): Promise<IteratorResult<string | Uint8Array>> {
              const value = queued.shift();
              if (value !== undefined) return Promise.resolve({ value, done: false });
              if (failure) return Promise.reject(failure);
              if (ended) return Promise.resolve({ value: undefined, done: true });
              return new Promise((next) => waiting.push(next));
            },
          };
          return iterator;
        },
      });
    }, { once: true });
  });
}

function isBinaryBody(body: unknown): body is Blob | ArrayBuffer | Uint8Array {
  return body instanceof Blob || body instanceof ArrayBuffer || ArrayBuffer.isView(body);
}

export function createHttpClient(options: HttpTransportOptions) {
  return createNeoismClient(createHttpTransport(options));
}

async function* followEvents(
  fetcher: typeof globalThis.fetch,
  baseUrl: string,
  resolveHeaders: (signal?: AbortSignal) => Promise<Record<string, string>>,
  options: EventOptions,
  reconnect: HttpTransportOptions["reconnect"],
): AsyncIterable<Event> {
  let cursor = options.since;
  const seenIds = new Set<string>();
  const seenOrder: string[] = [];
  let delay = reconnect?.initialDelayMs ?? 250;
  const maximumDelay = reconnect?.maximumDelayMs ?? 10_000;
  while (!options.signal?.aborted) {
    try {
      const headers = await resolveHeaders(options.signal);
      if (options.signal?.aborted) return;
      const query = new URLSearchParams();
      if (options.sessionId) query.set("sessionId", options.sessionId);
      if (options.tail && cursor === undefined) query.set("tail", "true");
      if (options.limit !== undefined) query.set("limit", String(options.limit));
      const response = await fetcher(`${baseUrl}/v2/events${query.size ? `?${query}` : ""}`, {
        redirect: "manual", credentials: "omit",
        headers: {
          accept: "text/event-stream",
          ...headers,
          ...(cursor === undefined ? {} : { "last-event-id": String(cursor) }),
        },
        ...(options.signal === undefined ? {} : { signal: options.signal }),
      });
      assertResponseOrigin(response, baseUrl);
      if (!response.ok) throw await apiError(response);
      if (!response.body) throw new Error("Event response has no body");
      delay = reconnect?.initialDelayMs ?? 250;
      for await (const event of parseSse(response.body)) {
        // Dedupe by event id, not sequence order: a transactionally
        // committed event can broadcast slightly after later-stamped
        // events, and a strict sequence gate would silently drop it.
        // The resume cursor still advances to the highest sequence seen.
        if (event.id && seenIds.has(event.id)) continue;
        if (event.id) {
          seenIds.add(event.id);
          seenOrder.push(event.id);
          if (seenOrder.length > SEEN_EVENT_WINDOW) {
            const evicted = seenOrder.shift();
            if (evicted !== undefined) seenIds.delete(evicted);
          }
        }
        if (event.sequence > (cursor ?? 0)) cursor = event.sequence;
        yield event;
      }
    } catch (error) {
      if (options.signal?.aborted) return;
      if (error instanceof NeoismApiError && !error.body.retryable) throw error;
      await sleep(delay, options.signal);
      delay = Math.min(delay * 2, maximumDelay);
    }
  }
}

async function* parseSse(stream: ReadableStream<Uint8Array>): AsyncIterable<Event> {
  const reader = stream.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  try {
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });
      let boundary = buffer.match(/\r?\n\r?\n/);
      while (boundary?.index !== undefined) {
        const frame = buffer.slice(0, boundary.index).replace(/\r/g, "");
        buffer = buffer.slice(boundary.index + boundary[0].length);
        const data = frame
          .split("\n")
          .filter((line) => line.startsWith("data:"))
          .map((line) => line.slice(5).trimStart())
          .join("\n");
        if (data) yield JSON.parse(data) as Event;
        boundary = buffer.match(/\r?\n\r?\n/);
      }
    }
  } finally {
    void reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}

function unsafeTarget(): never {
  throw new NeoismApiError(400, { code: "transport.unsafe_target", message: "Unsafe transport target or redirect", retryable: false, details: {} });
}
function trustedBaseUrl(value: string): string {
  if (/[\\\s\p{Cc}]/u.test(value)) unsafeTarget();
  let url: URL;
  try { url = new URL(value); } catch { return unsafeTarget(); }
  if (!["https:", "http:"].includes(url.protocol) || url.username || url.password || url.search || url.hash) unsafeTarget();
  return url.href.replace(/\/$/, "");
}
function confinedUrl(baseUrl: string, path: string): URL {
  // Descriptor paths are relative API paths, never an alternate bearer destination.
  if (/^[a-z][a-z0-9+.-]*:/i.test(path) || path.startsWith("//") || /[\\\p{Cc}]/u.test(path) || path.includes("?") || path.includes("#")) unsafeTarget();
  const url = new URL(path.replace(/^\/+/, ""), `${baseUrl}/`);
  if (url.origin !== new URL(baseUrl).origin || !url.pathname.startsWith(new URL(`${baseUrl}/`).pathname)) unsafeTarget();
  return url;
}
function assertResponseOrigin(response: Response, baseUrl: string): void {
  if (response.redirected || response.status >= 300 && response.status < 400 || response.type === "opaqueredirect") unsafeTarget();
  if (response.url) {
    let origin: string;
    try { origin = new URL(response.url).origin; } catch { return unsafeTarget(); }
    if (origin !== new URL(baseUrl).origin) unsafeTarget();
  }
}

function buildUrl(baseUrl: string, request: RequestDescriptor): string {
  const url = confinedUrl(baseUrl, request.path);
  for (const [key, value] of Object.entries(request.query ?? {})) {
    if (value !== undefined) url.searchParams.set(key, String(value));
  }
  return url.toString();
}

function buildSocketUrl(baseUrl: string, request: SocketDescriptor): string {
  const url = confinedUrl(baseUrl, request.path);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  for (const [key, value] of Object.entries(request.query ?? {})) {
    if (value !== undefined) url.searchParams.set(key, String(value));
  }
  return url.toString();
}

async function apiError(response: Response): Promise<NeoismApiError> {
  let body: Partial<ApiErrorBody> = {};
  try { body = await response.json() as Partial<ApiErrorBody>; } catch { /* non-JSON proxy error */ }
  return new NeoismApiError(response.status, {
    code: body.code ?? `http.${response.status}`,
    message: body.message ?? (response.statusText || `HTTP ${response.status}`),
    retryable: body.retryable ?? response.status >= 500,
    details: body.details ?? {},
    ...(body.requestId ? { requestId: body.requestId } : {}),
  });
}

function sleep(milliseconds: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(resolve, milliseconds);
    signal?.addEventListener("abort", () => {
      clearTimeout(timer);
      reject(signal.reason);
    }, { once: true });
  });
}