import type { EventEnvelope } from "./types.js";

export interface RequestDescriptor {
  method?: "GET" | "POST" | "PUT" | "PATCH" | "DELETE";
  path: string;
  query?: Record<string, string | number | boolean | undefined>;
  headers?: Record<string, string>;
  body?: unknown;
  response?: "json" | "bytes" | "text";
  signal?: AbortSignal;
}

export interface EventOptions {
  since?: number;
  tail?: boolean;
  sessionId?: string;
  signal?: AbortSignal;
}

export interface NeoismTransport {
  request<T>(request: RequestDescriptor): Promise<T>;
  events(options?: EventOptions): AsyncIterable<EventEnvelope>;
}