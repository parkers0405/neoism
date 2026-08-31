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
import {
  CapabilityUnavailableError,
  type PluginSdk,
  type PluginUseOptions,
} from "./extensions.js";

export interface NeoismClient {
  readonly transport: NeoismTransport;
  /** Complete generated V2 surface, keyed by canonical operation ID. */
  readonly operations: ContractClient;
  readonly health: { get(): Promise<OperationResponse<"v2.health">> };
  readonly meta: { get(): Promise<ApiMeta> };
  readonly config: {
    defaults(directory?: string): Promise<OperationResponse<"v2.config.defaults">>;
    get(directory?: string): Promise<OperationResponse<"v2.config.get">>;
    update(input: OperationInput<"v2.config.update">["body"], directory?: string): Promise<OperationResponse<"v2.config.update">>;
    validate(directory?: string): Promise<OperationResponse<"v2.config.validate">>;
  };
  readonly audit: { list(limit?: number): Promise<AuditEntry[]> };
  readonly capabilities: {
    list(directory?: string): Promise<CapabilityInfo[]>;
    has(id: string, minimumVersion?: string, directory?: string): Promise<boolean>;
  };
  readonly plugins: {
    list(directory?: string): Promise<PluginManifest[]>;
    get(id: string, directory?: string): Promise<PluginManifest>;
    use<TClient>(plugin: PluginSdk<TClient>, options?: PluginUseOptions): Promise<TClient>;
    tryUse<TClient>(plugin: PluginSdk<TClient>, options?: PluginUseOptions): Promise<TClient | undefined>;
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
      auth(providerId: string, workspaceId?: string): Promise<OperationResponse<"v2.providers.auth.get">>;
      setAuth(providerId: string, credential: OperationInput<"v2.providers.auth.set">["body"], workspaceId?: string): Promise<boolean>;
      removeAuth(providerId: string, workspaceId?: string): Promise<boolean>;
      oauthAuthorize(providerId: string, input: OperationInput<"v2.providers.oauth.authorize">["body"], workspaceId?: string): Promise<OperationResponse<"v2.providers.oauth.authorize">>;
      oauthCallback(providerId: string, input: OperationInput<"v2.providers.oauth.callback">["body"], workspaceId?: string): Promise<boolean>;
      connections(providerId: string, workspaceId?: string): Promise<OperationResponse<"v2.providers.connections.list">>;
      createConnection(providerId: string, input: OperationInput<"v2.providers.connections.create">["body"], workspaceId?: string): Promise<OperationResponse<"v2.providers.connections.create">>;
      renameConnection(providerId: string, connectionId: string, label: string, workspaceId?: string): Promise<OperationResponse<"v2.providers.connections.rename">>;
      deleteConnection(providerId: string, connectionId: string, workspaceId?: string): Promise<boolean>;
      setDefaultConnection(providerId: string, connectionId: string, workspaceId?: string): Promise<OperationResponse<"v2.providers.connections.setDefault">>;
    };
    skills: { list(directory?: string): Promise<OperationResponse<"v2.skills.list">> };
    tools: { list(directory?: string): Promise<ToolInfo[]> };
  };
  readonly management: {
    workspaces: WorkspaceManagementCollection;
    repositories: RepositoryManagementCollection;
    agents: ManagementCollection<"v2.management.agents">;
    commands: ManagementCollection<"v2.management.commands">;
    skills: ManagementCollection<"v2.management.skills"> & {
      install(input: OperationInput<"v2.management.skills.install">["body"], directory?: string): Promise<OperationResponse<"v2.management.skills.install">>;
      versions(id: string, directory?: string): Promise<OperationResponse<"v2.management.skills.versions.list">>;
      version(id: string, version: string): Promise<OperationResponse<"v2.management.skills.versions.get">>;
      restore(id: string, version: string, options?: { directory?: string; expectedRevision?: string }): Promise<OperationResponse<"v2.management.skills.versions.restore">>;
    };
  };
  readonly sessions: {
    list(options?: { directory?: string; path?: string; roots?: boolean; search?: string; limit?: number; start?: number; cursor?: string }): Promise<Page<Session>>;
    create(input?: NonNullable<OperationInput<"v2.sessions.create">["body"]> & { directory?: string }): Promise<Session>;
    get(id: string): Promise<Session>;
    update(id: string, input: OperationInput<"v2.sessions.update">["body"]): Promise<Session>;
    delete(id: string): Promise<boolean>;
    messages(id: string, options?: { order?: "asc" | "desc"; limit?: number; slim?: boolean }): Promise<Page<MessageWithParts>>;
    prompt(id: string, request: PromptRequest): Promise<void>;
    abort(id: string): Promise<boolean>;
    status(): Promise<OperationResponse<"v2.sessions.status">>;
    queue(id: string): Promise<OperationResponse<"v2.sessions.queue.list">>;
    clearQueue(id: string): Promise<OperationResponse<"v2.sessions.queue.clear">>;
    popQueue(id: string): Promise<OperationResponse<"v2.sessions.queue.pop">>;
    command(id: string, command: string): Promise<OperationResponse<"v2.sessions.commands.execute">>;
    undo(id: string): Promise<OperationResponse<"v2.sessions.undo">>;
    redo(id: string): Promise<OperationResponse<"v2.sessions.redo">>;
    summarize(id: string): Promise<OperationResponse<"v2.sessions.summarize">>;
    pin(id: string, pinned: boolean): Promise<OperationResponse<"v2.sessions.pin">>;
    cancelJob(id: string, jobId: string): Promise<OperationResponse<"v2.sessions.jobs.cancel">>;
  };
}

type ManagementPrefix = "v2.management.agents" | "v2.management.commands" | "v2.management.skills";
type ManagementOperation<P extends ManagementPrefix, S extends "list" | "get" | "create" | "update" | "delete"> = `${P}.${S}` & keyof import("./generated/contract.js").ApiOperations;
export interface ManagementOptions { directory?: string; scope?: "installation" | "workspace"; expectedRevision?: string }
export interface ManagementCollection<P extends ManagementPrefix> {
  list(options?: Omit<ManagementOptions, "expectedRevision">): Promise<OperationResponse<ManagementOperation<P, "list">>>;
  get(id: string, options?: Omit<ManagementOptions, "expectedRevision">): Promise<OperationResponse<ManagementOperation<P, "get">>>;
  create(id: string, input: OperationInput<ManagementOperation<P, "create">>["body"], options?: Pick<ManagementOptions, "directory" | "expectedRevision">): Promise<OperationResponse<ManagementOperation<P, "create">>>;
  update(id: string, input: OperationInput<ManagementOperation<P, "update">>["body"], options?: Pick<ManagementOptions, "directory" | "expectedRevision">): Promise<OperationResponse<ManagementOperation<P, "update">>>;
  delete(id: string, options?: ManagementOptions): Promise<void>;
}

export interface WorkspaceManagementCollection {
  list(): Promise<OperationResponse<"v2.management.workspaces.list">>;
  get(id: string): Promise<OperationResponse<"v2.management.workspaces.get">>;
  create(input: OperationInput<"v2.management.workspaces.create">["body"]): Promise<OperationResponse<"v2.management.workspaces.create">>;
  update(id: string, input: OperationInput<"v2.management.workspaces.update">["body"], expectedRevision?: string): Promise<OperationResponse<"v2.management.workspaces.update">>;
  delete(id: string, expectedRevision?: string): Promise<void>;
}

export interface RepositoryManagementCollection {
  list(): Promise<OperationResponse<"v2.management.repositories.list">>;
  get(id: string): Promise<OperationResponse<"v2.management.repositories.get">>;
  create(input: OperationInput<"v2.management.repositories.create">["body"]): Promise<OperationResponse<"v2.management.repositories.create">>;
  update(id: string, input: OperationInput<"v2.management.repositories.update">["body"], expectedRevision?: string): Promise<OperationResponse<"v2.management.repositories.update">>;
  delete(id: string, expectedRevision?: string): Promise<void>;
}

export function createNeoismClient(transport: NeoismTransport): NeoismClient {
  const operations = createContractClient(transport);
  const clean = <T extends object>(value: T) => Object.fromEntries(
    Object.entries(value).filter(([, item]) => item !== undefined),
  ) as { [Key in keyof T]?: Exclude<T[Key], undefined> };
  const managedCollection = <P extends ManagementPrefix>(prefix: P): ManagementCollection<P> => ({
    list: (options = {}) => operations.request(`${prefix}.list` as ManagementOperation<P, "list">, { query: clean(options) } as never),
    get: (id, options = {}) => operations.request(`${prefix}.get` as ManagementOperation<P, "get">, { path: { id }, query: clean(options) } as never),
    create: (id, body, options = {}) => operations.request(`${prefix}.create` as ManagementOperation<P, "create">, {
      path: { id }, query: clean({ directory: options.directory }), headers: clean({ "If-Match": options.expectedRevision }), body,
    } as never),
    update: (id, body, options = {}) => operations.request(`${prefix}.update` as ManagementOperation<P, "update">, {
      path: { id }, query: clean({ directory: options.directory }), headers: clean({ "If-Match": options.expectedRevision }), body,
    } as never),
    async delete(id, options = {}) {
      await operations.request(`${prefix}.delete` as ManagementOperation<P, "delete">, {
        path: { id }, query: clean({ directory: options.directory, scope: options.scope }), headers: clean({ "If-Match": options.expectedRevision }),
      } as never);
    },
  });
  const client: NeoismClient = {
    transport,
    operations,
    health: { get: () => operations.request("v2.health", {}) },
    meta: { get: () => operations.request("v2.meta.get", {}) },
    config: {
      defaults: (directory) => operations.request("v2.config.defaults", { query: clean({ directory }) }),
      get: (directory) => operations.request("v2.config.get", { query: clean({ directory }) }),
      update: (body, directory) => operations.request("v2.config.update", { query: clean({ directory }), body }),
      validate: (directory) => operations.request("v2.config.validate", { query: clean({ directory }) }),
    },
    audit: { list: (limit) => operations.request("v2.audit.list", { query: clean({ limit }) }) },
    capabilities: {
      list: (directory) => operations.request("v2.capabilities.list", { query: clean({ directory }) }),
      async has(id, minimumVersion, directory) {
        const capability = (await operations.request("v2.capabilities.list", { query: clean({ directory }) }))
          .find((candidate) => candidate.id === id && candidate.enabled);
        return capability !== undefined &&
          (minimumVersion === undefined || compareVersions(capability.version, minimumVersion) >= 0);
      },
    },
    plugins: {
      list: (directory) => operations.request("v2.plugins.list", { query: clean({ directory }) }),
      get: (id, directory) => operations.request("v2.plugins.get", { path: { plugin_id: id }, query: clean({ directory }) }),
      async use<TClient>(plugin: PluginSdk<TClient>, options: PluginUseOptions = {}) {
        const capabilities = await operations.request("v2.capabilities.list", {
          query: clean({ directory: options.directory }),
        });
        const capability = capabilities.find((candidate) =>
          candidate.id === plugin.capability && candidate.enabled
        );
        if (
          !capability ||
          !plugin.supported(capabilities) ||
          (options.minimumVersion !== undefined &&
            compareVersions(capability.version, options.minimumVersion) < 0)
        ) {
          throw new CapabilityUnavailableError(
            plugin.capability,
            plugin.id,
            options.minimumVersion,
          );
        }
        return plugin.client(client);
      },
      async tryUse<TClient>(plugin: PluginSdk<TClient>, options: PluginUseOptions = {}) {
        try {
          return await client.plugins.use(plugin, options);
        } catch (error) {
          if (error instanceof CapabilityUnavailableError) return undefined;
          throw error;
        }
      },
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
        auth: (providerId, workspaceId) => operations.request("v2.providers.auth.get", { path: { provider_id: providerId }, query: clean({ workspaceId }) }),
        setAuth: (providerId, body, workspaceId) => operations.request("v2.providers.auth.set", { path: { provider_id: providerId }, query: clean({ workspaceId }), body }),
        removeAuth: (providerId, workspaceId) => operations.request("v2.providers.auth.delete", { path: { provider_id: providerId }, query: clean({ workspaceId }) }),
        oauthAuthorize: (providerId, body, workspaceId) => operations.request("v2.providers.oauth.authorize", { path: { provider_id: providerId }, query: clean({ workspaceId }), body }),
        oauthCallback: (providerId, body, workspaceId) => operations.request("v2.providers.oauth.callback", { path: { provider_id: providerId }, query: clean({ workspaceId }), body }),
        connections: (providerId, workspaceId) => operations.request("v2.providers.connections.list", { path: { provider_id: providerId }, query: clean({ workspaceId }) }),
        createConnection: (providerId, body, workspaceId) => operations.request("v2.providers.connections.create", { path: { provider_id: providerId }, query: clean({ workspaceId }), body }),
        renameConnection: (providerId, connectionId, label, workspaceId) => operations.request("v2.providers.connections.rename", { path: { provider_id: providerId, connection_id: connectionId }, query: clean({ workspaceId }), body: { label } }),
        deleteConnection: (providerId, connectionId, workspaceId) => operations.request("v2.providers.connections.delete", { path: { provider_id: providerId, connection_id: connectionId }, query: clean({ workspaceId }) }),
        setDefaultConnection: (providerId, connectionId, workspaceId) => operations.request("v2.providers.connections.setDefault", { path: { provider_id: providerId, connection_id: connectionId }, query: clean({ workspaceId }) }),
      },
      skills: { list: (directory) => operations.request("v2.skills.list", { query: clean({ directory }) }) },
      tools: { list: (directory) => operations.request("v2.tools.list", { query: clean({ directory }) }) },
    },
    management: {
      workspaces: {
        list: () => operations.request("v2.management.workspaces.list", {}),
        get: (id) => operations.request("v2.management.workspaces.get", { path: { id } }),
        create: (body) => operations.request("v2.management.workspaces.create", { body }),
        update: (id, body, expectedRevision) => operations.request("v2.management.workspaces.update", {
          path: { id }, headers: clean({ "If-Match": expectedRevision }), body,
        }),
        async delete(id, expectedRevision) {
          await operations.request("v2.management.workspaces.delete", {
            path: { id }, headers: clean({ "If-Match": expectedRevision }), query: {},
          });
        },
      },
      repositories: {
        list: () => operations.request("v2.management.repositories.list", {}),
        get: (id) => operations.request("v2.management.repositories.get", { path: { id } }),
        create: (body) => operations.request("v2.management.repositories.create", { body }),
        update: (id, body, expectedRevision) => operations.request("v2.management.repositories.update", {
          path: { id }, headers: clean({ "If-Match": expectedRevision }), body,
        }),
        async delete(id, expectedRevision) {
          await operations.request("v2.management.repositories.delete", {
            path: { id }, headers: clean({ "If-Match": expectedRevision }), query: {},
          });
        },
      },
      agents: managedCollection("v2.management.agents"),
      commands: managedCollection("v2.management.commands"),
      skills: Object.assign(managedCollection("v2.management.skills"), {
        install: (body: OperationInput<"v2.management.skills.install">["body"], directory?: string) => operations.request("v2.management.skills.install", { query: clean({ directory }), body }),
        versions: (id: string, directory?: string) => operations.request("v2.management.skills.versions.list", { path: { id }, query: clean({ directory }) }),
        version: (id: string, version: string) => operations.request("v2.management.skills.versions.get", { path: { id, version } }),
        restore: (id: string, version: string, options: { directory?: string; expectedRevision?: string } = {}) => operations.request("v2.management.skills.versions.restore", {
          path: { id, version }, query: clean({ directory: options.directory, expectedRevision: options.expectedRevision }), headers: clean({ "If-Match": options.expectedRevision }),
        }),
      }),
    },
    sessions: {
      list: (options = {}) => operations.request("v2.sessions.list", { query: clean({ ...options, roots: options.roots === undefined ? undefined : String(options.roots) }) }),
      create: (input = {}) => {
        const { directory, ...body } = input;
        return operations.request("v2.sessions.create", { query: clean({ directory }), body });
      },
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
  return client;
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