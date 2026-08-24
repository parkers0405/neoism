import type { EventOptions, NeoismTransport, RequestDescriptor } from "./transport.js";
import type {
  ApiMeta,
  AuditEntry,
  AgentInfo,
  CommandInfo,
  ArtifactInfo,
  CapabilityInfo,
  EventEnvelope,
  MessageWithParts,
  Page,
  PluginManifest,
  PromptRequest,
  PermissionRequest,
  QuestionRequest,
  Session,
  ToolInfo,
} from "./types.js";

export interface NeoismClient {
  readonly transport: NeoismTransport;
  readonly meta: { get(): Promise<ApiMeta> };
  readonly audit: { list(limit?: number): Promise<AuditEntry[]> };
  readonly capabilities: {
    list(): Promise<CapabilityInfo[]>;
    has(id: string, minimumVersion?: string): Promise<boolean>;
  };
  readonly plugins: {
    list(): Promise<PluginManifest[]>;
    get(id: string): Promise<PluginManifest>;
    request<T>(id: string, path?: string, options?: Omit<RequestDescriptor, "path">): Promise<T>;
  };
  readonly events: {
    subscribe(options?: EventOptions): AsyncIterable<EventEnvelope>;
  };
  readonly artifacts: {
    list(sessionId?: string): Promise<ArtifactInfo[]>;
    get(id: string): Promise<ArtifactInfo>;
    upload(data: Uint8Array | Blob, options?: {
      filename?: string;
      mediaType?: string;
      sessionId?: string;
    }): Promise<ArtifactInfo>;
    download(id: string, signal?: AbortSignal): Promise<Uint8Array>;
    delete(id: string): Promise<void>;
  };
  readonly interactions: {
    permissions: {
      list(sessionId?: string): Promise<PermissionRequest[]>;
      reply(id: string, reply: "once" | "always" | "reject", message?: string): Promise<boolean>;
    };
    questions: {
      list(sessionId?: string): Promise<QuestionRequest[]>;
      reply(id: string, answers: string[][]): Promise<boolean>;
      reject(id: string): Promise<boolean>;
    };
  };
  readonly catalog: {
    agents: { list(directory?: string): Promise<AgentInfo[]>; get(name: string, directory?: string): Promise<AgentInfo> };
    commands: { list(directory?: string): Promise<CommandInfo[]> };
    providers: {
      list(directory?: string): Promise<Record<string, unknown>>;
      configured(directory?: string): Promise<unknown[]>;
      authMethods(directory?: string): Promise<unknown>;
      auth(providerId: string): Promise<unknown>;
      setAuth(providerId: string, credential: unknown): Promise<unknown>;
      removeAuth(providerId: string): Promise<void>;
      oauthAuthorize(providerId: string, input: unknown): Promise<unknown>;
      oauthCallback(providerId: string, input: unknown): Promise<unknown>;
    };
    skills: { list(directory?: string): Promise<unknown[]> };
    tools: { list(directory?: string): Promise<ToolInfo[]> };
  };
  readonly sessions: {
    list(options?: { directory?: string; roots?: boolean; search?: string; limit?: number; start?: number }): Promise<Page<Session>>;
    create(input?: { directory?: string; title?: string }): Promise<Session>;
    get(id: string): Promise<Session>;
    update(id: string, input: Record<string, unknown>): Promise<Session>;
    delete(id: string): Promise<void>;
    messages(id: string, options?: { order?: "asc" | "desc"; limit?: number; slim?: boolean }): Promise<Page<MessageWithParts>>;
    prompt(id: string, request: PromptRequest): Promise<void>;
    abort(id: string): Promise<void>;
    status(): Promise<Record<string, unknown>>;
    queue(id: string): Promise<unknown[]>;
    clearQueue(id: string): Promise<void>;
    popQueue(id: string): Promise<unknown>;
    command(id: string, command: string): Promise<unknown>;
    undo(id: string): Promise<unknown>;
    redo(id: string): Promise<unknown>;
    summarize(id: string): Promise<unknown>;
    pin(id: string, pinned: boolean): Promise<unknown>;
    cancelJob(id: string, jobId: string): Promise<unknown>;
  };
}

export function createNeoismClient(transport: NeoismTransport): NeoismClient {
  const request = <T>(path: string) => transport.request<T>({ path });
  return {
    transport,
    meta: { get: () => request<ApiMeta>("/v2/meta") },
    audit: { list: (limit) => transport.request<AuditEntry[]>({ path: "/v2/audit", query: { limit } }) },
    capabilities: {
      list: () => request<CapabilityInfo[]>("/v2/capabilities"),
      async has(id, minimumVersion) {
        const capability = (await request<CapabilityInfo[]>("/v2/capabilities"))
          .find((candidate) => candidate.id === id && candidate.enabled);
        return capability !== undefined &&
          (minimumVersion === undefined || compareVersions(capability.version, minimumVersion) >= 0);
      },
    },
    plugins: {
      list: () => request<PluginManifest[]>("/v2/plugins"),
      get: (id) => request<PluginManifest>(`/v2/plugins/${encodeURIComponent(id)}`),
      request: (id, path = "", options = {}) => {
        const suffix = path
          .replace(/^\/+/, "")
          .split("/")
          .filter(Boolean)
          .map(encodeURIComponent)
          .join("/");
        return transport.request({
          ...options,
          path: `/v2/plugins/${encodeURIComponent(id)}${suffix ? `/${suffix}` : ""}`,
        });
      },
    },
    events: { subscribe: (options) => transport.events(options) },
    artifacts: {
      list: (sessionId) => transport.request<ArtifactInfo[]>({
        path: "/v2/artifacts",
        ...(sessionId ? { query: { sessionId } } : {}),
      }),
      get: (id) => request<ArtifactInfo>(`/v2/artifacts/${encodeURIComponent(id)}`),
      upload: (data, options = {}) => transport.request<ArtifactInfo>({
        method: "POST",
        path: "/v2/artifacts",
        headers: {
          "content-type": options.mediaType ?? (data instanceof Blob ? data.type : "application/octet-stream"),
          ...(options.filename ? { "x-neoism-filename": options.filename } : {}),
          ...(options.sessionId ? { "x-neoism-session-id": options.sessionId } : {}),
        },
        body: data,
      }),
      download: (id, signal) => transport.request<Uint8Array>({
        path: `/v2/artifacts/${encodeURIComponent(id)}/content`,
        response: "bytes",
        ...(signal ? { signal } : {}),
      }),
      async delete(id) {
        await transport.request<void>({ method: "DELETE", path: `/v2/artifacts/${encodeURIComponent(id)}` });
      },
    },
    interactions: {
      permissions: {
        list: (sessionId) => transport.request<PermissionRequest[]>({ path: "/v2/interactions/permissions", query: { sessionId } }),
        reply: (id, reply, message) => transport.request<boolean>({
          method: "POST",
          path: `/v2/interactions/permissions/${encodeURIComponent(id)}/reply`,
          body: { reply, ...(message ? { message } : {}) },
        }),
      },
      questions: {
        list: (sessionId) => transport.request<QuestionRequest[]>({ path: "/v2/interactions/questions", query: { sessionId } }),
        reply: (id, answers) => transport.request<boolean>({
          method: "POST",
          path: `/v2/interactions/questions/${encodeURIComponent(id)}/reply`,
          body: { answers },
        }),
        reject: (id) => transport.request<boolean>({
          method: "POST",
          path: `/v2/interactions/questions/${encodeURIComponent(id)}/reject`,
        }),
      },
    },
    catalog: {
      agents: {
        list: (directory) => transport.request<AgentInfo[]>({ path: "/v2/agents", query: { directory } }),
        get: (name, directory) => transport.request<AgentInfo>({ path: `/v2/agents/${encodeURIComponent(name)}`, query: { directory } }),
      },
      commands: { list: (directory) => transport.request<CommandInfo[]>({ path: "/v2/commands", query: { directory } }) },
      providers: {
        list: (directory) => transport.request<Record<string, unknown>>({ path: "/v2/providers", query: { directory } }),
        configured: (directory) => transport.request<unknown[]>({ path: "/v2/providers/configured", query: { directory } }),
        authMethods: (directory) => transport.request<unknown>({ path: "/v2/providers/auth-methods", query: { directory } }),
        auth: (providerId) => request<unknown>(`/v2/providers/${encodeURIComponent(providerId)}/auth`),
        setAuth: (providerId, body) => transport.request<unknown>({ method: "PUT", path: `/v2/providers/${encodeURIComponent(providerId)}/auth`, body }),
        async removeAuth(providerId) {
          await transport.request<void>({ method: "DELETE", path: `/v2/providers/${encodeURIComponent(providerId)}/auth` });
        },
        oauthAuthorize: (providerId, body) => transport.request<unknown>({ method: "POST", path: `/v2/providers/${encodeURIComponent(providerId)}/oauth/authorize`, body }),
        oauthCallback: (providerId, body) => transport.request<unknown>({ method: "POST", path: `/v2/providers/${encodeURIComponent(providerId)}/oauth/callback`, body }),
      },
      skills: { list: (directory) => transport.request<unknown[]>({ path: "/v2/skills", query: { directory } }) },
      tools: { list: (directory) => transport.request<ToolInfo[]>({ path: "/v2/tools", query: { directory } }) },
    },
    sessions: {
      list: (options = {}) => transport.request<Page<Session>>({ path: "/v2/sessions", query: options }),
      create: (input = {}) => transport.request<Session>({
        method: "POST",
        path: "/v2/sessions",
        ...(input.directory ? { query: { directory: input.directory } } : {}),
        body: input.title ? { title: input.title } : {},
      }),
      get: (id) => request<Session>(`/v2/sessions/${encodeURIComponent(id)}`),
      update: (id, body) => transport.request<Session>({
        method: "PATCH",
        path: `/v2/sessions/${encodeURIComponent(id)}`,
        body,
      }),
      async delete(id) {
        await transport.request<void>({
          method: "DELETE",
          path: `/v2/sessions/${encodeURIComponent(id)}`,
        });
      },
      messages: (id, options = {}) => transport.request<Page<MessageWithParts>>({
        path: `/v2/sessions/${encodeURIComponent(id)}/messages`,
        query: options,
      }),
      async prompt(id, body) {
        await transport.request<void>({
          method: "POST",
          path: `/v2/sessions/${encodeURIComponent(id)}/prompt`,
          body,
        });
      },
      async abort(id) {
        await transport.request<void>({
          method: "POST",
          path: `/v2/sessions/${encodeURIComponent(id)}/abort`,
        });
      },
      status: () => request<Record<string, unknown>>("/v2/sessions/status"),
      queue: (id) => request<unknown[]>(`/v2/sessions/${encodeURIComponent(id)}/queue`),
      async clearQueue(id) {
        await transport.request<void>({ method: "DELETE", path: `/v2/sessions/${encodeURIComponent(id)}/queue` });
      },
      popQueue: (id) => transport.request<unknown>({ method: "POST", path: `/v2/sessions/${encodeURIComponent(id)}/queue/pop`, body: {} }),
      command: (id, command) => transport.request<unknown>({ method: "POST", path: `/v2/sessions/${encodeURIComponent(id)}/commands`, body: { command } }),
      undo: (id) => transport.request<unknown>({ method: "POST", path: `/v2/sessions/${encodeURIComponent(id)}/undo`, body: {} }),
      redo: (id) => transport.request<unknown>({ method: "POST", path: `/v2/sessions/${encodeURIComponent(id)}/redo`, body: {} }),
      summarize: (id) => transport.request<unknown>({ method: "POST", path: `/v2/sessions/${encodeURIComponent(id)}/summarize`, body: {} }),
      pin: (id, pinned) => transport.request<unknown>({ method: "POST", path: `/v2/sessions/${encodeURIComponent(id)}/pin`, body: { pinned } }),
      cancelJob: (id, jobId) => transport.request<unknown>({ method: "DELETE", path: `/v2/sessions/${encodeURIComponent(id)}/jobs/${encodeURIComponent(jobId)}` }),
    },
  };
}

function compareVersions(left: string, right: string): number {
  const a = left.split(".").map(Number);
  const b = right.split(".").map(Number);
  for (let index = 0; index < Math.max(a.length, b.length); index += 1) {
    const difference = (a[index] ?? 0) - (b[index] ?? 0);
    if (difference !== 0) return Math.sign(difference);
  }
  return 0;
}