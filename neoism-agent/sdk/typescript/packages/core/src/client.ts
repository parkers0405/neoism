import type { EventOptions, NeoismTransport, RequestDescriptor } from "./transport.js";
import {
  createContractClient,
  type ContractClient,
  type OperationInput,
  type OperationResponse,
} from "./generated/contract.js";
import type {
  ApiMeta,
  AuditEntry,
  AgentInfo,
  CommandInfo,
  ArtifactInfo,
  CapabilityInfo,
  Event,
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
  /** Complete generated V2 surface, keyed by canonical operation ID. */
  readonly operations: ContractClient;
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
    subscribe(options?: EventOptions): AsyncIterable<Event>;
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
      list(directory?: string): Promise<OperationResponse<"v2.providers.list">>;
      configured(directory?: string): Promise<OperationResponse<"v2.providers.configured">>;
      authMethods(directory?: string): Promise<OperationResponse<"v2.providers.authMethods">>;
      auth(providerId: string): Promise<OperationResponse<"v2.providers.auth.get">>;
      setAuth(providerId: string, credential: OperationInput<"v2.providers.auth.set">["body"]): Promise<boolean>;
      removeAuth(providerId: string): Promise<boolean>;
      oauthAuthorize(providerId: string, input: OperationInput<"v2.providers.oauth.authorize">["body"]): Promise<OperationResponse<"v2.providers.oauth.authorize">>;
      oauthCallback(providerId: string, input: OperationInput<"v2.providers.oauth.callback">["body"]): Promise<boolean>;
    };
    skills: { list(directory?: string): Promise<unknown[]> };
    tools: { list(directory?: string): Promise<ToolInfo[]> };
  };
  readonly sessions: {
    list(options?: { directory?: string; roots?: boolean; search?: string; limit?: number; start?: number }): Promise<Page<Session>>;
    create(input?: { directory?: string; title?: string }): Promise<Session>;
    get(id: string): Promise<Session>;
    update(id: string, input: Record<string, unknown>): Promise<Session>;
    delete(id: string): Promise<boolean>;
    messages(id: string, options?: { order?: "asc" | "desc"; limit?: number; slim?: boolean }): Promise<Page<MessageWithParts>>;
    prompt(id: string, request: PromptRequest): Promise<void>;
    abort(id: string): Promise<boolean>;
    status(): Promise<Record<string, unknown>>;
    queue(id: string): Promise<OperationResponse<"v2.sessions.queue.list">>;
    clearQueue(id: string): Promise<OperationResponse<"v2.sessions.queue.clear">>;
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
  const operations = createContractClient(transport);
  const clean = <T extends object>(value: T) => Object.fromEntries(
    Object.entries(value).filter(([, item]) => item !== undefined),
  ) as { [Key in keyof T]?: Exclude<T[Key], undefined> };
  return {
    transport,
    operations,
    meta: { get: () => operations.request("v2.meta.get", {}) },
    audit: { list: (limit) => operations.request("v2.audit.list", { query: clean({ limit }) }) },
    capabilities: {
      list: () => operations.request("v2.capabilities.list", {}),
      async has(id, minimumVersion) {
        const capability = (await operations.request("v2.capabilities.list", {}))
          .find((candidate) => candidate.id === id && candidate.enabled);
        return capability !== undefined &&
          (minimumVersion === undefined || compareVersions(capability.version, minimumVersion) >= 0);
      },
    },
    plugins: {
      list: () => operations.request("v2.plugins.list", {}),
      get: (id) => operations.request("v2.plugins.get", { path: { plugin_id: id } }),
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
      list: (sessionId) => operations.request("v2.artifacts.list", { query: clean({ sessionId }) }),
      get: (id) => operations.request("v2.artifacts.get", { path: { artifact_id: id } }),
      upload: (data, options = {}) => operations.request("v2.artifacts.create", {
        headers: clean({
          "Content-Type": options.mediaType ?? (data instanceof Blob ? data.type : "application/octet-stream"),
          "X-Neoism-Filename": options.filename,
          "X-Neoism-Session-Id": options.sessionId,
        }), body: data,
      }),
      download: (id, signal) => operations.request("v2.artifacts.content", { path: { artifact_id: id }, ...(signal ? { signal } : {}) }),
      async delete(id) {
        await operations.request("v2.artifacts.delete", { path: { artifact_id: id } });
      },
    },
    interactions: {
      permissions: {
        list: (sessionId) => operations.request("v2.interactions.permissions.list", { query: clean({ sessionId }) }),
        reply: (id, reply, message) => operations.request("v2.interactions.permissions.reply", { path: { request_id: id }, body: clean({ reply, message }) }),
      },
      questions: {
        list: (sessionId) => operations.request("v2.interactions.questions.list", { query: clean({ sessionId }) }),
        reply: (id, answers) => operations.request("v2.interactions.questions.reply", { path: { request_id: id }, body: { answers } }),
        reject: (id) => operations.request("v2.interactions.questions.reject", { path: { request_id: id } }),
      },
    },
    catalog: {
      agents: {
        list: (directory) => operations.request("v2.agents.list", { query: clean({ directory }) }),
        get: (name, directory) => operations.request("v2.agents.get", { path: { name }, query: clean({ directory }) }),
      },
      commands: { list: (directory) => operations.request("v2.commands.list", { query: clean({ directory }) }) },
      providers: {
        list: (directory) => operations.request("v2.providers.list", { query: clean({ directory }) }),
        configured: (directory) => operations.request("v2.providers.configured", { query: clean({ directory }) }),
        authMethods: (directory) => operations.request("v2.providers.authMethods", { query: clean({ directory }) }),
        auth: (providerId) => operations.request("v2.providers.auth.get", { path: { provider_id: providerId } }),
        setAuth: (providerId, body) => operations.request("v2.providers.auth.set", { path: { provider_id: providerId }, body }),
        removeAuth: (providerId) => operations.request("v2.providers.auth.delete", { path: { provider_id: providerId } }),
        oauthAuthorize: (providerId, body) => operations.request("v2.providers.oauth.authorize", { path: { provider_id: providerId }, body }),
        oauthCallback: (providerId, body) => operations.request("v2.providers.oauth.callback", { path: { provider_id: providerId }, body }),
      },
      skills: { list: (directory) => operations.request("v2.skills.list", { query: clean({ directory }) }) },
      tools: { list: (directory) => operations.request("v2.tools.list", { query: clean({ directory }) }) },
    },
    sessions: {
      list: (options = {}) => operations.request("v2.sessions.list", { query: clean({ ...options, roots: options.roots === undefined ? undefined : String(options.roots) }) }),
      create: (input = {}) => operations.request("v2.sessions.create", { query: clean({ directory: input.directory }), body: input.title ? { title: input.title } : {} }),
      get: (id) => operations.request("v2.sessions.get", { path: { session_id: id } }),
      update: (id, body) => operations.request("v2.sessions.update", { path: { session_id: id }, body }),
      delete: (id) => operations.request("v2.sessions.delete", { path: { session_id: id } }),
      messages: (id, options = {}) => operations.request("v2.sessions.messages", { path: { session_id: id }, query: options }),
      async prompt(id, body) {
        await operations.request("v2.sessions.prompt", { path: { session_id: id }, body });
      },
      abort: (id) => operations.request("v2.sessions.abort", { path: { session_id: id } }),
      status: () => operations.request("v2.sessions.status", {}),
      queue: (id) => operations.request("v2.sessions.queue.list", { path: { session_id: id } }),
      clearQueue: (id) => operations.request("v2.sessions.queue.clear", { path: { session_id: id } }),
      popQueue: (id) => operations.request("v2.sessions.queue.pop", { path: { session_id: id } }),
      command: (id, command) => operations.request("v2.sessions.commands.execute", { path: { session_id: id }, body: { command } }),
      undo: (id) => operations.request("v2.sessions.undo", { path: { session_id: id }, body: {} }),
      redo: (id) => operations.request("v2.sessions.redo", { path: { session_id: id }, body: {} }),
      summarize: (id) => operations.request("v2.sessions.summarize", { path: { session_id: id }, body: {} }),
      pin: (id, pinned) => operations.request("v2.sessions.pin", { path: { session_id: id }, body: { pinned } }),
      cancelJob: (id, jobId) => operations.request("v2.sessions.jobs.cancel", { path: { session_id: id, job_id: jobId } }),
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