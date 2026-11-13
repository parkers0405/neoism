import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";
import { createNeoismClient, type ConfigDocument, type NeoismTransport } from "@neoism/sdk-core";
import { createHttpTransport, type HttpTransportOptions } from "@neoism/sdk-http";

const ORIGIN = "http://neoism.embedded.invalid";
const MAX_REQUEST_BYTES = 16 * 1024 * 1024;

export interface EmbeddedOptions {
  database: string;
  /** Immutable host-owned config instead of local config discovery; not a sandbox. */
  config?: ConfigDocument;
  /** Explicit native module path, or the platform artifact packaged with sdk-node. */
  nativeModule?: string;
  shutdownTimeoutMs?: number;
  token?: HttpTransportOptions["token"];
  headers?: Record<string, string>;
}

interface NativeAgent {
  beginRequest(id: number): void;
  cancelRequest(id: number): void;
  request(id: number, method: string, path: string, headers: { name: string; value: string }[], body: Buffer): Promise<{ status: number; headers: { name: string; value: string }[] }>;
  read(id: number): Promise<Buffer | null>;
  close(): Promise<void>;
}

interface NativeModule {
  bindingVersion(): number;
  NativeAgent: { open(options: { database: string; shutdownTimeoutMs?: number; configJson?: string }): Promise<NativeAgent> };
}

function aborted(): DOMException { return new DOMException("Embedded request aborted", "AbortError"); }

async function bodyBytes(request: Request): Promise<Buffer> {
  if (!request.body) return Buffer.alloc(0);
  const reader = request.body.getReader();
  const abort = () => { void reader.cancel().catch(() => {}); };
  request.signal.addEventListener("abort", abort, { once: true });
  const chunks: Uint8Array[] = [];
  let size = 0;
  try {
    for (;;) {
      if (request.signal.aborted) throw aborted();
      const { done, value } = await reader.read();
      if (request.signal.aborted) throw aborted();
      if (done) break;
      size += value.byteLength;
      if (size > MAX_REQUEST_BYTES) throw new RangeError("Embedded request exceeds 16 MiB");
      chunks.push(value);
    }
    return Buffer.concat(chunks, size);
  } catch (error) {
    void reader.cancel().catch(() => {});
    throw error;
  } finally {
    request.signal.removeEventListener("abort", abort);
    reader.releaseLock();
  }
}

/** The existing typed client backed by the existing Rust runtime, not a child process. */
export async function createEmbeddedNeoism(options: EmbeddedOptions) {
  if (typeof AbortSignal.any !== "function" || typeof Symbol.asyncDispose !== "symbol") {
    throw new Error("Native embedding requires AbortSignal.any and Symbol.asyncDispose; use a current Node.js runtime");
  }
  if (!options.database.trim()) throw new TypeError("An explicit database path is required");
  if (options.shutdownTimeoutMs !== undefined && (!Number.isInteger(options.shutdownTimeoutMs) || options.shutdownTimeoutMs <= 0 || options.shutdownTimeoutMs > 0xffffffff)) {
    throw new RangeError("shutdownTimeoutMs must be a positive uint32");
  }
  const path = options.nativeModule ? resolve(options.nativeModule)
    : fileURLToPath(new URL(`../native/${process.platform}-${process.arch}/neoism_agent.node`, import.meta.url));
  let binding: NativeModule;
  try { binding = createRequire(import.meta.url)(path) as NativeModule; }
  catch { throw new Error("Neoism native binding could not be loaded. Build the matching platform artifact with npm run native:build in the SDK workspace, or supply nativeModule."); }
  if (typeof binding.bindingVersion !== "function" || binding.bindingVersion() !== 2 || typeof binding.NativeAgent?.open !== "function") {
    throw new Error("Incompatible Neoism native binding ABI");
  }
  const native = await binding.NativeAgent.open({ database: resolve(options.database),
    ...(options.config === undefined ? {} : { configJson: JSON.stringify(options.config) }),
    ...(options.shutdownTimeoutMs === undefined ? {} : { shutdownTimeoutMs: options.shutdownTimeoutMs }) });
  const shutdown = new AbortController();
  let nextId = 0;
  let closePromise: Promise<void> | undefined;
  const combined = (signal?: AbortSignal | null) => signal ? AbortSignal.any([shutdown.signal, signal]) : shutdown.signal;
  const fetch: typeof globalThis.fetch = async (input, init) => {
    const source = new Request(input, init);
    const signal = combined(source.signal);
    if (signal.aborted) throw aborted();
    const url = new URL(source.url);
    if (url.origin !== ORIGIN || url.username || url.password) throw new TypeError("Embedded transport cannot access another origin");
    const request = new Request(source, { signal });
    const body = await bodyBytes(request);
    if (signal.aborted) throw aborted();
    if (nextId === 0xffffffff) throw new Error("Embedded request ID space exhausted; reopen the host");
    const id = ++nextId;
    native.beginRequest(id);
    let released = false;
    let responseController: ReadableStreamDefaultController<Uint8Array> | undefined;
    const release = () => {
      if (released) return;
      released = true;
      signal.removeEventListener("abort", abort);
      native.cancelRequest(id);
    };
    const abort = () => { release(); responseController?.error(aborted()); };
    signal.addEventListener("abort", abort, { once: true });
    try {
      if (signal.aborted) throw aborted();
      const headers: { name: string; value: string }[] = [];
      request.headers.forEach((value, name) => headers.push({ name, value }));
      const response = await native.request(id, request.method, url.pathname + url.search, headers, body);
      if (signal.aborted) throw aborted();
      const responseHeaders = new Headers();
      for (const { name, value } of response.headers) responseHeaders.append(name, value);
      if (request.method === "HEAD" || [204, 205, 304].includes(response.status)) {
        release();
        return new Response(null, { status: response.status, headers: responseHeaders });
      }
      const stream = new ReadableStream<Uint8Array>({
        start(controller) { responseController = controller; },
        async pull(controller) {
          try {
            if (signal.aborted) throw aborted();
            const chunk = await native.read(id);
            if (signal.aborted) throw aborted();
            if (chunk === null) { release(); controller.close(); }
            else controller.enqueue(chunk);
          } catch (error) {
            release();
            controller.error(signal.aborted ? aborted() : error);
          }
        },
        cancel() { release(); },
      });
      return new Response(stream, { status: response.status, headers: responseHeaders });
    } catch (error) {
      release();
      throw signal.aborted ? aborted() : error;
    }
  };
  const transport = createHttpTransport({ baseUrl: ORIGIN, fetch,
    ...(options.token === undefined ? {} : { token: options.token }),
    ...(options.headers === undefined ? {} : { headers: options.headers }),
    webSocket: () => { throw new Error("Embedded WebSocket upgrades are not supported; use the network transport"); },
  });
  const ownedTransport: NeoismTransport = {
    ...transport,
    request: descriptor => transport.request({ ...descriptor, signal: combined(descriptor.signal) }),
    events: (eventOptions = {}) => transport.events({ ...eventOptions, signal: combined(eventOptions.signal) }),
  };
  const close = () => {
    if (!closePromise) {
      shutdown.abort();
      closePromise = native.close();
    }
    return closePromise;
  };
  return { client: createNeoismClient(ownedTransport), fetch, close, [Symbol.asyncDispose]: close };
}

export type EmbeddedNeoism = Awaited<ReturnType<typeof createEmbeddedNeoism>>;