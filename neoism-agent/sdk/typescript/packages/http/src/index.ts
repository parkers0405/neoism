import {
  createNeoismClient,
  type ApiErrorBody,
  type Event,
  type EventOptions,
  type NeoismTransport,
  type RequestDescriptor,
} from "@neoism/sdk-core";

const SEEN_EVENT_WINDOW = 8192;

export interface HttpTransportOptions {
  baseUrl: string;
  token?: string;
  headers?: Record<string, string>;
  fetch?: typeof globalThis.fetch;
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
  const fetcher = options.fetch ?? globalThis.fetch;
  if (!fetcher) throw new Error("A fetch implementation is required");
  const baseUrl = options.baseUrl.replace(/\/$/, "");
  const commonHeaders = {
    ...(options.token ? { authorization: `Bearer ${options.token}` } : {}),
    ...options.headers,
  };

  return {
    async request<T>(request: RequestDescriptor): Promise<T> {
      const binaryBody = isBinaryBody(request.body);
      const init: RequestInit = {
        method: request.method ?? "GET",
        headers: {
          accept: request.response === "bytes" ? "application/octet-stream" : "application/json",
          ...(request.body === undefined || binaryBody ? {} : { "content-type": "application/json" }),
          ...commonHeaders,
          ...request.headers,
        },
        ...(request.body === undefined
          ? {}
          : { body: binaryBody ? request.body as BodyInit : JSON.stringify(request.body) }),
        ...(request.signal === undefined ? {} : { signal: request.signal }),
      };
      const response = await fetcher(buildUrl(baseUrl, request), init);
      if (!response.ok) throw await apiError(response);
      if (response.status === 204) return undefined as T;
      if (request.response === "bytes") return new Uint8Array(await response.arrayBuffer()) as T;
      if (request.response === "text") return await response.text() as T;
      return await response.json() as T;
    },
    events(eventOptions: EventOptions = {}) {
      return followEvents(fetcher, baseUrl, commonHeaders, eventOptions, options.reconnect);
    },
  };
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
  headers: Record<string, string>,
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
      const query = new URLSearchParams();
      if (options.sessionId) query.set("sessionId", options.sessionId);
      if (options.tail && cursor === undefined) query.set("tail", "true");
      const response = await fetcher(`${baseUrl}/v2/events${query.size ? `?${query}` : ""}`, {
        headers: {
          accept: "text/event-stream",
          ...headers,
          ...(cursor === undefined ? {} : { "last-event-id": String(cursor) }),
        },
        ...(options.signal === undefined ? {} : { signal: options.signal }),
      });
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
      let boundary: number;
      while ((boundary = buffer.indexOf("\n\n")) >= 0) {
        const frame = buffer.slice(0, boundary).replace(/\r/g, "");
        buffer = buffer.slice(boundary + 2);
        const data = frame
          .split("\n")
          .filter((line) => line.startsWith("data:"))
          .map((line) => line.slice(5).trimStart())
          .join("\n");
        if (data) yield JSON.parse(data) as Event;
      }
    }
  } finally {
    reader.releaseLock();
  }
}

function buildUrl(baseUrl: string, request: RequestDescriptor): string {
  const url = new URL(request.path, `${baseUrl}/`);
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