/**
 * Author API + runtime host for Neoism serve plugins (`neoism-plugin/2`).
 *
 * A serve plugin is a long-lived process the agent server spawns per
 * workspace. Declare tools, hooks, and event subscriptions with
 * {@link definePlugin}, then hand the definition to {@link runPlugin}:
 *
 * ```ts
 * import { definePlugin, runPlugin } from "@neoism/plugin";
 *
 * await runPlugin(definePlugin({
 *   tools: [{
 *     id: "todo_count",
 *     description: "Count TODO markers in the workspace",
 *     parameters: { type: "object", properties: {} },
 *     async execute(_input, context) {
 *       return { output: `workspace: ${context.directory}` };
 *     },
 *   }],
 *   hooks: {
 *     "chat.options"(_context, value) {
 *       return { ...value, temperature: 0 };
 *     },
 *   },
 *   events: {
 *     namespaces: ["session."],
 *     handler(event) { console.error("saw", event.type); },
 *   },
 * }));
 * ```
 *
 * Configure it in the installation `plugins` map with `serve`, `entry`, or
 * `npm` options; workspace-owned declarations require explicit host trust
 * before the server may spawn them.
 */
import type { Event } from "@neoism/sdk-core";
import { createHttpClient } from "@neoism/sdk-http";

export interface ServiceRequest {
  workspaceId?: string;
  directory?: string;
  options?: Record<string, unknown>;
}

export interface ServiceDeclaration<T> {
  id: string;
  priority?: number;
  handler: T;
}

export interface AgentCatalog {
  agents: unknown[];
  defaultAgent?: string;
}

export interface ConfigDocument {
  values: Record<string, unknown>;
  provenance?: Record<string, string>;
}

export interface SystemContextSection {
  id: string;
  title?: string;
  content: string;
}

export interface PromptRequest {
  promptId: string;
  variables?: Record<string, unknown>;
  service: ServiceRequest;
}

export interface RenderedPrompt {
  content: string;
  system?: boolean;
}

export interface PluginEvent {
  namespace: string;
  name: string;
  payload: unknown;
}

export interface HostRequestOptions {
  signal?: AbortSignal;
  timeoutMs?: number;
}

export type ProcessStreamEnvelope =
  | { kind: "open"; streamId: string }
  | { kind: "item"; streamId: string; value: unknown }
  | { kind: "end"; streamId: string }
  | { kind: "error"; streamId: string; error: string };

export interface ProviderContext extends ServiceHandlerContext {}

export interface PluginProvider {
  descriptor: Record<string, unknown> & { id: string };
  priority?: number;
  stream(request: unknown, context: ProviderContext): AsyncIterable<unknown> | Promise<AsyncIterable<unknown>>;
  metadata?(model: unknown, context: ProviderContext): unknown | Promise<unknown>;
  auth?(providerId: string, context: ProviderContext): unknown | Promise<unknown>;
  route?(request: unknown, context: ProviderContext): unknown | Promise<unknown>;
  media?(request: unknown, context: ProviderContext): unknown | Promise<unknown>;
}

export interface PluginRoute {
  descriptor: Record<string, unknown> & { id: string };
  priority?: number;
  handle(request: unknown, context: ServiceHandlerContext): unknown | Promise<unknown>;
}

export type PluginWebSocketMessage =
  | { kind: "text"; data: string }
  | { kind: "binary" | "ping" | "pong"; data: number[] }
  | { kind: "close" };

export interface PluginWebSocketSession {
  outbound: AsyncIterable<PluginWebSocketMessage>;
  message?(message: PluginWebSocketMessage): void | Promise<void>;
  close?(): void | Promise<void>;
}

export interface PluginWebSocketRoute {
  descriptor: Record<string, unknown> & { id: string };
  priority?: number;
  open(request: unknown, context: ServiceHandlerContext): PluginWebSocketSession | Promise<PluginWebSocketSession>;
}

export interface PluginMessagePart {
  id: string;
  version: number;
  schema: unknown;
  fallbackTextField?: string;
}

export interface PluginMcpDeclaration {
  id: string;
  transport?: string;
  configSchema?: unknown;
  authentication?: string[];
  prompts?: boolean;
  resources?: boolean;
  tools?: boolean;
}

export interface PluginHostServices {
  config: {
    get(key: string, options?: HostRequestOptions): Promise<unknown | null>;
    set(key: string, value: unknown, options?: HostRequestOptions): Promise<void>;
  };
  workspace: {
    read(path: string, options?: HostRequestOptions): Promise<Uint8Array>;
    write(path: string, contents: Uint8Array, options?: HostRequestOptions): Promise<void>;
    list(path: string, options?: HostRequestOptions): Promise<string[]>;
  };
  events: {
    publish(event: PluginEvent, options?: HostRequestOptions): Promise<void>;
  };
  network: HostBroker;
  process: HostCancellableBroker;
  tasks: HostCancellableBroker;
  secrets: { use(operation: string, input?: unknown, options?: HostRequestOptions): Promise<unknown>; read(operation: string, input?: unknown, options?: HostRequestOptions): Promise<unknown> };
  prompts: HostBroker;
  messages: HostBroker;
  responses: HostBroker;
  providers: HostBroker;
  policy: HostBroker;
}

export interface HostBroker {
  call(operation: string, input?: unknown, options?: HostRequestOptions): Promise<unknown>;
}

export interface HostCancellableBroker extends HostBroker {
  cancel(opaqueId: string, options?: HostRequestOptions): Promise<void>;
}

export interface ServiceHandlerContext {
  host: PluginHostServices;
  signal: AbortSignal;
}

export interface PluginServices {
  agents?: ServiceDeclaration<(request: ServiceRequest, context: ServiceHandlerContext) => AgentCatalog | Promise<AgentCatalog>>[];
  commands?: ServiceDeclaration<(request: ServiceRequest, context: ServiceHandlerContext) => unknown[] | Promise<unknown[]>>[];
  skills?: ServiceDeclaration<(request: ServiceRequest, context: ServiceHandlerContext) => unknown[] | Promise<unknown[]>>[];
  systemContext?: ServiceDeclaration<(request: ServiceRequest, context: ServiceHandlerContext) => SystemContextSection[] | Promise<SystemContextSection[]>>[];
  prompts?: ServiceDeclaration<(request: PromptRequest, context: ServiceHandlerContext) => RenderedPrompt | Promise<RenderedPrompt>>[];
  config?: ServiceDeclaration<(request: ServiceRequest, context: ServiceHandlerContext) => ConfigDocument | Promise<ConfigDocument>>[];
  providers?: PluginProvider[];
}

export interface ToolContext {
  directory: string;
  sessionId?: string;
  /** SDK client bound to the local agent server, when the host provided one. */
  client?: ReturnType<typeof createHttpClient>;
  /** Capability-scoped host calls; host paths and credentials are never exposed. */
  host: PluginHostServices;
  signal: AbortSignal;
}

export interface ToolResult {
  output: string | unknown;
  title?: string;
  metadata?: unknown;
}

export interface PluginTool {
  id: string;
  description: string;
  /** JSON Schema for the tool input. */
  parameters: unknown;
  execute(input: unknown, context: ToolContext): ToolResult | Promise<ToolResult>;
}

export type HookHandler = (
  context: unknown,
  value: unknown,
) => unknown | Promise<unknown>;

export interface InitializeContext {
  pluginId: string;
  /** Unique identity for this exact process instance and reload generation. */
  instanceId: string;
  directory: string;
  /** The `config` object from this plugin's workspace configuration entry. */
  config: unknown;
  client?: ReturnType<typeof createHttpClient>;
  host: PluginHostServices;
}

export interface NeoismPlugin {
  name?: string;
  version?: string;
  tools?: PluginTool[];
  /** Keyed by hook name, e.g. "chat.options", "tool.before", "shell.env". */
  hooks?: Record<string, HookHandler>;
  events?: {
    /** Event-type prefixes to receive, e.g. ["session.", "message."]. */
    namespaces?: string[];
    handler?(event: Event): void | Promise<void>;
  };
  services?: PluginServices;
  routes?: PluginRoute[];
  websocketRoutes?: PluginWebSocketRoute[];
  messageParts?: PluginMessagePart[];
  /** Metadata only; executable MCP tools/routes use the ordinary registries. */
  mcp?: PluginMcpDeclaration[];
  initialize?(context: InitializeContext): void | Promise<void>;
}

export function definePlugin(plugin: NeoismPlugin): NeoismPlugin {
  return plugin;
}

interface HostFrame {
  id?: number | null;
  method: string;
  params: Record<string, unknown>;
}

interface ProcessPluginOwner {
  pluginId: string;
  instanceId: string;
  packageRevision?: string;
  registryGeneration?: number;
  scope?: "global" | "user" | "workspace" | "session";
  workspaceId?: string;
  scopeId?: string;
}

interface HostReplyFrame {
  id: number;
  result?: unknown;
  error?: string;
}

/**
 * Serve the plugin over stdio until the host shuts it down. Never resolves in
 * normal operation.
 */
export async function runPlugin(plugin: NeoismPlugin): Promise<void> {
  const { stdin, stdout } = await import("node:process");
  const readline = await import("node:readline");

  const serverUrl = process.env["NEOISM_AGENT_SERVER_URL"];
  const client = serverUrl
    ? createHttpClient({ baseUrl: serverUrl })
    : undefined;
  const tools = new Map((plugin.tools ?? []).map((tool) => [tool.id, tool]));
  let directory = process.env["NEOISM_WORKSPACE_DIR"] ?? ".";
  let owner: ProcessPluginOwner = {
    pluginId: process.env["NEOISM_PLUGIN_ID"] ?? "",
    instanceId: process.env["NEOISM_PLUGIN_INSTANCE_ID"] ?? "",
  };
  let nextHostRequestId = 1;
  const pendingHostRequests = new Map<number, {
    resolve(value: unknown): void;
    reject(error: Error): void;
    cleanup(): void;
  }>();
  const activeCalls = new Map<number, AbortController>();
  const activeStreams = new Map<string, { controller: AbortController; close?: () => void | Promise<void> }>();
  const websocketSessions = new Map<string, PluginWebSocketSession>();
  const MAX_FRAME_BYTES = 1024 * 1024;
  const MAX_PENDING_HOST_REQUESTS = 128;
  const MAX_ACTIVE_CALLS = 128;
  let activeFrames = 0;

  const writeFrame = (frame: unknown) => {
    const encoded = JSON.stringify(frame);
    if (Buffer.byteLength(encoded, "utf8") > MAX_FRAME_BYTES) {
      throw new Error(`neoism-plugin frame exceeds ${MAX_FRAME_BYTES} bytes`);
    }
    stdout.write(`${encoded}\n`);
  };

  const requestHost = (
    method: string,
    params: Record<string, unknown>,
    options: HostRequestOptions = {},
  ): Promise<unknown> => {
    if (pendingHostRequests.size >= MAX_PENDING_HOST_REQUESTS) {
      return Promise.reject(new Error("too many pending host requests"));
    }
    if (!owner.pluginId || !owner.instanceId) {
      return Promise.reject(new Error("plugin has not completed initialization"));
    }
    const id = nextHostRequestId++;
    return new Promise((resolve, reject) => {
      let timer: ReturnType<typeof setTimeout> | undefined;
      const abort = () => {
        pendingHostRequests.delete(id);
        cleanup();
        reject(new Error("host request cancelled"));
      };
      const cleanup = () => {
        if (timer !== undefined) clearTimeout(timer);
        options.signal?.removeEventListener("abort", abort);
      };
      pendingHostRequests.set(id, { resolve, reject, cleanup });
      if (options.signal?.aborted) return abort();
      options.signal?.addEventListener("abort", abort, { once: true });
      if (options.timeoutMs !== undefined) {
        timer = setTimeout(() => {
          pendingHostRequests.delete(id);
          cleanup();
          reject(new Error(`host request timed out after ${options.timeoutMs} ms`));
        }, Math.max(1, options.timeoutMs));
      }
      try {
        writeFrame({ id, method, params, owner });
      } catch (error) {
        pendingHostRequests.delete(id);
        cleanup();
        reject(error instanceof Error ? error : new Error(String(error)));
      }
    });
  };

  const host: PluginHostServices = {
    config: {
      get: (key, options) => requestHost("host.config.get", { key }, options),
      set: async (key, value, options) => { await requestHost("host.config.set", { key, value }, options); },
    },
    workspace: {
      read: async (path, options) => new Uint8Array(await requestHost("host.workspace.read", { path }, options) as number[]),
      write: async (path, contents, options) => { await requestHost("host.workspace.write", { path, contents: Array.from(contents) }, options); },
      list: async (path, options) => await requestHost("host.workspace.list", { path }, options) as string[],
    },
    events: {
      publish: async (event, options) => { await requestHost("host.event.publish", event as unknown as Record<string, unknown>, options); },
    },
    network: { call: async (operation, input, options) => (await requestHost("host.network.request", { operation, input }, options) as { output: unknown }).output },
    process: {
      call: async (operation, input, options) => (await requestHost("host.process.spawn", { operation, input }, options) as { output: unknown }).output,
      cancel: async (opaqueId, options) => { await requestHost("host.process.cancel", { opaqueId }, options); },
    },
    tasks: {
      call: async (operation, input, options) => (await requestHost("host.task.spawn", { operation, input }, options) as { output: unknown }).output,
      cancel: async (opaqueId, options) => { await requestHost("host.task.cancel", { opaqueId }, options); },
    },
    secrets: {
      use: async (operation, input, options) => (await requestHost("host.secret.use", { operation, input }, options) as { output: unknown }).output,
      read: async (operation, input, options) => (await requestHost("host.secret.read", { operation, input }, options) as { output: unknown }).output,
    },
    prompts: { call: async (operation, input, options) => (await requestHost("host.prompt.read", { operation, input }, options) as { output: unknown }).output },
    messages: { call: async (operation, input, options) => (await requestHost("host.message.read", { operation, input }, options) as { output: unknown }).output },
    responses: { call: async (operation, input, options) => (await requestHost("host.response.transform", { operation, input }, options) as { output: unknown }).output },
    providers: { call: async (operation, input, options) => (await requestHost("host.provider.call", { operation, input }, options) as { output: unknown }).output },
    policy: { call: async (operation, input, options) => (await requestHost("host.policy.call", { operation, input }, options) as { output: unknown }).output },
  };

  const reply = (id: number, result: unknown) => {
    writeFrame({ id, result });
  };
  const fail = (id: number, error: unknown) => {
    writeFrame({ id, error: error instanceof Error ? error.message : String(error) });
  };

  const serviceMap = <T>(items: ServiceDeclaration<T>[] | undefined) =>
    new Map((items ?? []).map((item) => [item.id, item]));
  const agentServices = serviceMap(plugin.services?.agents);
  const commandServices = serviceMap(plugin.services?.commands);
  const skillServices = serviceMap(plugin.services?.skills);
  const systemContextServices = serviceMap(plugin.services?.systemContext);
  const promptServices = serviceMap(plugin.services?.prompts);
  const configServices = serviceMap(plugin.services?.config);
  const providers = new Map((plugin.services?.providers ?? []).map((provider) => [provider.descriptor.id, provider]));
  const routes = new Map((plugin.routes ?? []).map((route) => [route.descriptor.id, route]));
  const websocketRoutes = new Map((plugin.websocketRoutes ?? []).map((route) => [route.descriptor.id, route]));
  const declarations = <T>(items: ServiceDeclaration<T>[] | undefined) =>
    (items ?? []).map(({ id, priority }) => ({ id, ...(priority === undefined ? {} : { priority }) }));

  const streamFrame = (requestId: number, stream: ProcessStreamEnvelope) => {
    writeFrame({ owner, requestId, stream });
  };
  const pumpStream = async (
    streamId: string,
    requestId: number,
    values: AsyncIterable<unknown>,
    controller: AbortController,
    close?: () => void | Promise<void>,
  ) => {
    activeStreams.set(streamId, { controller, ...(close ? { close } : {}) });
    const iterator = values[Symbol.asyncIterator]();
    try {
      streamFrame(requestId, { kind: "open", streamId });
      while (!controller.signal.aborted) {
        const next = await new Promise<IteratorResult<unknown> | undefined>((resolveNext, rejectNext) => {
          const abort = () => { cleanup(); resolveNext(undefined); };
          const cleanup = () => controller.signal.removeEventListener("abort", abort);
          controller.signal.addEventListener("abort", abort, { once: true });
          void iterator.next().then((result) => { cleanup(); resolveNext(result); }, (error) => { cleanup(); rejectNext(error); });
        });
        if (!next || next.done) break;
        streamFrame(requestId, { kind: "item", streamId, value: next.value });
      }
      if (!controller.signal.aborted) streamFrame(requestId, { kind: "end", streamId });
    } catch (error) {
      if (!controller.signal.aborted) streamFrame(requestId, { kind: "error", streamId, error: error instanceof Error ? error.message : String(error) });
    } finally {
      activeStreams.delete(streamId);
      await close?.();
      await iterator.return?.();
    }
  };

  const handle = async (frame: HostFrame) => {
    const { id, method, params } = frame;
    try {
      switch (method) {
        case "initialize": {
          directory = String(params["directory"] ?? directory);
           const exactOwner = params["owner"];
           owner = exactOwner && typeof exactOwner === "object"
             ? exactOwner as ProcessPluginOwner
             : { pluginId: String(params["pluginId"] ?? ""), instanceId: String(params["instanceId"] ?? "") };
          await plugin.initialize?.({
            ...owner,
            directory,
            config: params["config"],
            host,
            ...(client ? { client } : {}),
          });
          if (typeof id === "number") {
            reply(id, {
              protocol: "neoism-plugin/2",
              name: plugin.name,
              version: plugin.version,
              tools: (plugin.tools ?? []).map((tool) => ({
                id: tool.id,
                description: tool.description,
                parameters: tool.parameters,
              })),
              hooks: Object.keys(plugin.hooks ?? {}),
              eventNamespaces: plugin.events?.namespaces ?? [],
              services: {
                agents: declarations(plugin.services?.agents),
                commands: declarations(plugin.services?.commands),
                skills: declarations(plugin.services?.skills),
                systemContext: declarations(plugin.services?.systemContext),
                prompts: declarations(plugin.services?.prompts),
                 config: declarations(plugin.services?.config),
                 providers: (plugin.services?.providers ?? []).map((provider) => ({
                   descriptor: provider.descriptor,
                   ...(provider.priority === undefined ? {} : { priority: provider.priority }),
                   media: provider.media !== undefined,
                   administration: provider.route !== undefined,
                 })),
               },
               routes: (plugin.routes ?? []).map(({ descriptor, priority }) => ({ descriptor, ...(priority === undefined ? {} : { priority }) })),
               websocketRoutes: (plugin.websocketRoutes ?? []).map(({ descriptor, priority }) => ({ descriptor, ...(priority === undefined ? {} : { priority }) })),
               messageParts: plugin.messageParts ?? [],
               mcp: plugin.mcp ?? [],
            });
          }
          return;
        }
        case "tool.invoke": {
          const tool = tools.get(String(params["tool"]));
          if (!tool) throw new Error(`unknown tool ${String(params["tool"])}`);
          const controller = new AbortController();
          if (typeof id === "number") activeCalls.set(id, controller);
          const result = await tool.execute(params["input"], {
            directory: String(params["directory"] ?? directory),
            ...(typeof params["sessionId"] === "string"
              ? { sessionId: params["sessionId"] }
              : {}),
            host,
            signal: controller.signal,
            ...(client ? { client } : {}),
          });
          if (typeof id === "number") activeCalls.delete(id);
          if (typeof id === "number") {
            reply(id, {
              output:
                typeof result.output === "string"
                  ? result.output
                  : JSON.stringify(result.output),
              title: result.title,
              metadata: result.metadata,
            });
          }
          return;
        }
        case "hook.invoke": {
          const hook = plugin.hooks?.[String(params["hook"])];
          const value =
            hook === undefined
              ? params["value"]
              : await hook(params["context"], params["value"]);
          if (typeof id === "number") reply(id, value);
          return;
        }
        case "agent.list":
        case "command.list":
        case "skill.list":
        case "systemContext.sections":
        case "prompt.render":
         case "config.load": {
          const serviceId = String(params["serviceId"] ?? "");
          const maps = {
            "agent.list": agentServices,
            "command.list": commandServices,
            "skill.list": skillServices,
            "systemContext.sections": systemContextServices,
            "prompt.render": promptServices,
            "config.load": configServices,
          };
          const service = maps[method].get(serviceId);
          if (!service) throw new Error(`unknown ${method} service ${serviceId}`);
          const controller = new AbortController();
          if (typeof id === "number") activeCalls.set(id, controller);
          const result = await (service.handler as (request: unknown, context: ServiceHandlerContext) => unknown)(
            params["request"],
            { host, signal: controller.signal },
          );
          if (typeof id === "number") {
            activeCalls.delete(id);
            reply(id, result);
          }
           return;
         }
         case "provider.metadata":
         case "provider.auth":
         case "provider.route":
         case "provider.media": {
           const serviceId = String(params["serviceId"] ?? "");
           const provider = providers.get(serviceId);
           if (!provider) throw new Error(`unknown provider service ${serviceId}`);
           const operation = method.slice("provider.".length) as "metadata" | "auth" | "route" | "media";
           const handler = provider[operation] as ((request: unknown, context: ProviderContext) => unknown) | undefined;
           if (!handler) throw new Error(`provider ${serviceId} does not implement ${operation}`);
           const controller = new AbortController();
           if (typeof id === "number") activeCalls.set(id, controller);
           const argument = operation === "metadata" ? params["model"] : operation === "auth" ? params["providerId"] : params["request"] ?? params;
           const result = await handler.call(provider, argument, { host, signal: controller.signal });
           if (typeof id === "number") { activeCalls.delete(id); reply(id, result); }
           return;
         }
         case "provider.stream": {
           const serviceId = String(params["serviceId"] ?? "");
           const streamId = String(params["streamId"] ?? "");
           const requestId = Number(params["requestId"]);
           const provider = providers.get(serviceId);
           if (!provider || !streamId || !Number.isSafeInteger(requestId)) throw new Error("invalid provider stream request");
           const controller = new AbortController();
           const values = await provider.stream(params["request"], { host, signal: controller.signal });
           if (typeof id === "number") reply(id, {});
           void pumpStream(streamId, requestId, values, controller);
           return;
         }
         case "route.handle": {
           const routeId = String(params["routeId"] ?? "");
           const route = routes.get(routeId);
           if (!route) throw new Error(`unknown route ${routeId}`);
           const controller = new AbortController();
           if (typeof id === "number") activeCalls.set(id, controller);
           const result = await route.handle(params["request"], { host, signal: controller.signal });
           if (typeof id === "number") { activeCalls.delete(id); reply(id, result); }
           return;
         }
         case "route.websocket.open": {
           const routeId = String(params["routeId"] ?? "");
           const streamId = String(params["streamId"] ?? "");
           const requestId = Number(params["requestId"]);
           const route = websocketRoutes.get(routeId);
           if (!route || !streamId || !Number.isSafeInteger(requestId)) throw new Error("invalid WebSocket open request");
           const controller = new AbortController();
           const session = await route.open(params["request"], { host, signal: controller.signal });
           activeStreams.set(streamId, { controller, ...(session.close ? { close: session.close.bind(session) } : {}) });
           websocketSessions.set(streamId, session);
           if (typeof id === "number") reply(id, {});
           void pumpStream(streamId, requestId, session.outbound, controller, async () => { websocketSessions.delete(streamId); await session.close?.(); });
           return;
         }
         case "route.websocket.message": {
           const streamId = String(params["streamId"] ?? "");
           await websocketSessions.get(streamId)?.message?.(params["message"] as PluginWebSocketMessage);
           if (typeof id === "number") reply(id, {});
           return;
         }
         case "$/cancel": {
          const cancelledId = Number(params["id"]);
          activeCalls.get(cancelledId)?.abort();
          activeCalls.delete(cancelledId);
           return;
         }
         case "$/cancelStream": {
           const streamId = String(params["streamId"] ?? "");
           const active = activeStreams.get(streamId);
           active?.controller.abort();
           activeStreams.delete(streamId);
           websocketSessions.delete(streamId);
           return;
         }
        case "event": {
          await plugin.events?.handler?.(params as unknown as Event);
          return;
        }
        case "shutdown": {
          if (typeof id === "number") reply(id, {});
          process.exit(0);
        }
      }
    } catch (error) {
      if (typeof id === "number") {
        activeCalls.delete(id);
        fail(id, error);
      }
      else console.error(error);
    }
  };

  const lines = readline.createInterface({ input: stdin });
  for await (const line of lines) {
    if (!line.trim()) continue;
    if (Buffer.byteLength(line, "utf8") > MAX_FRAME_BYTES) {
      console.error("neoism-plugin: skipping oversized frame");
      continue;
    }
    let frame: Record<string, unknown>;
    try {
      frame = JSON.parse(line) as Record<string, unknown>;
    } catch {
      console.error("neoism-plugin: skipping malformed frame");
      continue;
    }
    if (typeof frame["method"] !== "string") {
      const replyFrame = frame as unknown as HostReplyFrame;
      const pending = pendingHostRequests.get(replyFrame.id);
      if (!pending) continue;
      pendingHostRequests.delete(replyFrame.id);
      pending.cleanup();
      if (replyFrame.error !== undefined) pending.reject(new Error(replyFrame.error));
      else pending.resolve(replyFrame.result);
      continue;
    }
    const requestFrame = frame as unknown as HostFrame;
    if (activeFrames >= MAX_ACTIVE_CALLS) {
      if (typeof requestFrame.id === "number") {
        fail(requestFrame.id, new Error("plugin request queue is full"));
      }
      continue;
    }
    activeFrames += 1;
    void handle(requestFrame).finally(() => { activeFrames -= 1; });
  }
}
