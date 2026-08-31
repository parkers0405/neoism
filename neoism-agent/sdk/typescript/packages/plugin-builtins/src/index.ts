import {
  capabilityEnabled,
  createContractClient,
  type NeoismClient,
  type OperationInput,
  type OperationResponse,
  type PluginSdk,
} from "@neoism/sdk-core";

type DirectoryOptions = { directory?: string };
type PositionOptions = DirectoryOptions & { file: string; line: number; character: number };

function optionalPlugin<TClient>(
  id: string,
  capability: string,
  create: (core: NeoismClient) => TClient,
): PluginSdk<TClient> {
  return {
    id,
    capability,
    supported: (capabilities) => capabilityEnabled(capabilities, capability),
    client: create,
  };
}

type Compact<T> = {
  [Key in keyof T as undefined extends T[Key] ? never : Key]: T[Key];
} & {
  [Key in keyof T as undefined extends T[Key] ? Key : never]?: Exclude<T[Key], undefined>;
};

function query<T extends object>(value: T): Compact<T> {
  return Object.fromEntries(
    Object.entries(value).filter(([, item]) => item !== undefined),
  ) as Compact<T>;
}

function directoryQuery(directory?: string): { directory?: string } {
  return directory === undefined ? {} : { directory };
}

export const agents = optionalPlugin<NeoismClient["catalog"]["agents"]>(
  "dev.neoism.agents",
  "neoism.agents",
  (core) => core.catalog.agents,
);

export const commands = optionalPlugin<NeoismClient["catalog"]["commands"]>(
  "dev.neoism.commands",
  "neoism.commands",
  (core) => core.catalog.commands,
);

export const providers = optionalPlugin<NeoismClient["catalog"]["providers"]>(
  "dev.neoism.providers",
  "neoism.providers",
  (core) => core.catalog.providers,
);

export const skills = optionalPlugin<NeoismClient["catalog"]["skills"]>(
  "dev.neoism.skills",
  "neoism.skills",
  (core) => core.catalog.skills,
);

export interface GoalsClient {
  get(sessionId: string): Promise<OperationResponse<"v2.plugins.goals.get">>;
  set(sessionId: string, input?: OperationInput<"v2.plugins.goals.set">["body"]): Promise<OperationResponse<"v2.plugins.goals.set">>;
  clear(sessionId: string): Promise<OperationResponse<"v2.plugins.goals.clear">>;
  research(sessionId: string, input: OperationInput<"v2.plugins.goals.research">["body"]): Promise<OperationResponse<"v2.plugins.goals.research">>;
}

export const goals = optionalPlugin<GoalsClient>("dev.neoism.goals", "neoism.goals", (core) => {
  const operations = createContractClient(core.transport);
  return {
    get: (sessionId) => operations.request("v2.plugins.goals.get", { path: { session_id: sessionId } }),
    set: (sessionId, body) => operations.request("v2.plugins.goals.set", { path: { session_id: sessionId }, ...(body === undefined ? {} : { body }) }),
    clear: (sessionId) => operations.request("v2.plugins.goals.clear", { path: { session_id: sessionId } }),
    research: (sessionId, body) => operations.request("v2.plugins.goals.research", { path: { session_id: sessionId }, body }),
  };
});

export interface SemanticSearchClient {
  search(text: string, options?: { limit?: number; sessionId?: string }): Promise<OperationResponse<"v2.plugins.semantic.search">>;
}

export const semanticSearch = optionalPlugin<SemanticSearchClient>(
  "dev.neoism.semantic",
  "neoism.semantic",
  (core) => {
    const operations = createContractClient(core.transport);
    return {
      search: (text, options = {}) => operations.request("v2.plugins.semantic.search", {
        query: query({ q: text, ...options }),
      }),
    };
  },
);

export interface WorkflowsClient {
  list(directory?: string): Promise<OperationResponse<"v2.plugins.workflows.list">>;
  create(input: OperationInput<"v2.plugins.workflows.create">["body"], directory?: string): Promise<OperationResponse<"v2.plugins.workflows.create">>;
  get(id: string, directory?: string): Promise<OperationResponse<"v2.plugins.workflows.get">>;
  update(id: string, input: OperationInput<"v2.plugins.workflows.update">["body"], options?: DirectoryOptions & { revision?: string }): Promise<OperationResponse<"v2.plugins.workflows.update">>;
  patch(id: string, input: OperationInput<"v2.plugins.workflows.patch">["body"], options?: DirectoryOptions & { revision?: string }): Promise<OperationResponse<"v2.plugins.workflows.patch">>;
  remove(id: string, options?: DirectoryOptions & { revision?: string }): Promise<void>;
  activate(id: string, directory?: string): Promise<OperationResponse<"v2.plugins.workflows.activate">>;
  pause(id: string, directory?: string): Promise<OperationResponse<"v2.plugins.workflows.pause">>;
  preview(id: string, directory?: string): Promise<OperationResponse<"v2.plugins.workflows.preview">>;
  run(id: string, directory?: string): Promise<OperationResponse<"v2.plugins.workflows.run">>;
  history(id: string, options?: DirectoryOptions & { limit?: number }): Promise<OperationResponse<"v2.plugins.workflows.history">>;
  getRun(id: string, runId: string, directory?: string): Promise<OperationResponse<"v2.plugins.workflows.runs.get">>;
  retryRun(id: string, runId: string, directory?: string): Promise<OperationResponse<"v2.plugins.workflows.runs.retry">>;
}

export const workflows = optionalPlugin<WorkflowsClient>(
  "dev.neoism.workflows",
  "neoism.workflows",
  (core) => {
    const operations = createContractClient(core.transport);
    const path = (id: string) => ({ workflow_id: id });
    return {
      list: (directory) => operations.request("v2.plugins.workflows.list", { query: directoryQuery(directory) }),
      create: (body, directory) => operations.request("v2.plugins.workflows.create", { query: directoryQuery(directory), body }),
      get: (id, directory) => operations.request("v2.plugins.workflows.get", { path: path(id), query: directoryQuery(directory) }),
      update: (id, body, options = {}) => operations.request("v2.plugins.workflows.update", { path: path(id), query: directoryQuery(options.directory), headers: options.revision ? { "If-Match": options.revision } : {}, body }),
      patch: (id, body, options = {}) => operations.request("v2.plugins.workflows.patch", { path: path(id), query: directoryQuery(options.directory), headers: options.revision ? { "If-Match": options.revision } : {}, body }),
      remove: (id, options = {}) => operations.request("v2.plugins.workflows.delete", { path: path(id), query: query({ directory: options.directory, expectedRevision: options.revision }) }),
      activate: (id, directory) => operations.request("v2.plugins.workflows.activate", { path: path(id), query: directoryQuery(directory) }),
      pause: (id, directory) => operations.request("v2.plugins.workflows.pause", { path: path(id), query: directoryQuery(directory) }),
      preview: (id, directory) => operations.request("v2.plugins.workflows.preview", { path: path(id), query: directoryQuery(directory) }),
      run: (id, directory) => operations.request("v2.plugins.workflows.run", { path: path(id), query: directoryQuery(directory) }),
      history: (id, options = {}) => operations.request("v2.plugins.workflows.history", { path: path(id), query: query(options) }),
      getRun: (id, runId, directory) => operations.request("v2.plugins.workflows.runs.get", { path: { workflow_id: id, run_id: runId }, query: directoryQuery(directory) }),
      retryRun: (id, runId, directory) => operations.request("v2.plugins.workflows.runs.retry", { path: { workflow_id: id, run_id: runId }, query: directoryQuery(directory) }),
    };
  },
);

export interface VcsClient {
  get(directory?: string): Promise<OperationResponse<"v2.plugins.vcs.get">>;
  status(directory?: string): Promise<OperationResponse<"v2.plugins.vcs.status">>;
  diff(directory?: string): Promise<OperationResponse<"v2.plugins.vcs.diff">>;
  rawDiff(directory?: string): Promise<OperationResponse<"v2.plugins.vcs.diff.raw">>;
  apply(input: OperationInput<"v2.plugins.vcs.apply">["body"]): Promise<OperationResponse<"v2.plugins.vcs.apply">>;
}

export const vcs = optionalPlugin<VcsClient>("dev.neoism.vcs", "neoism.vcs", (core) => {
  const operations = createContractClient(core.transport);
  return {
    get: (directory) => operations.request("v2.plugins.vcs.get", { query: directoryQuery(directory) }),
    status: (directory) => operations.request("v2.plugins.vcs.status", { query: directoryQuery(directory) }),
    diff: (directory) => operations.request("v2.plugins.vcs.diff", { query: directoryQuery(directory) }),
    rawDiff: (directory) => operations.request("v2.plugins.vcs.diff.raw", { query: directoryQuery(directory) }),
    apply: (body) => operations.request("v2.plugins.vcs.apply", { body }),
  };
});

export interface McpClient {
  status(directory?: string): Promise<OperationResponse<"v2.plugins.mcp.status">>;
  catalog(directory?: string): Promise<OperationResponse<"v2.plugins.mcp.catalog">>;
  add(input: OperationInput<"v2.plugins.mcp.add">["body"]): Promise<OperationResponse<"v2.plugins.mcp.add">>;
  configure(name: string, input: OperationInput<"v2.plugins.mcp.config">["body"], directory?: string): Promise<OperationResponse<"v2.plugins.mcp.config">>;
  connect(name: string, directory?: string): Promise<boolean>;
  disconnect(name: string, directory?: string): Promise<boolean>;
  tools(name: string, directory?: string): Promise<OperationResponse<"v2.plugins.mcp.tools">>;
  callTool(name: string, toolName: string, input: unknown, directory?: string): Promise<OperationResponse<"v2.plugins.mcp.tools.call">>;
  resources(name: string, directory?: string): Promise<OperationResponse<"v2.plugins.mcp.resources">>;
  prompts(name: string, directory?: string): Promise<OperationResponse<"v2.plugins.mcp.prompts">>;
  startAuth(name: string, directory?: string): Promise<OperationResponse<"v2.plugins.mcp.auth.start">>;
  authenticate(name: string, directory?: string): Promise<OperationResponse<"v2.plugins.mcp.auth.authenticate">>;
  completeAuth(name: string, code: string, options?: DirectoryOptions & { state?: string }): Promise<OperationResponse<"v2.plugins.mcp.auth.callback.get">>;
  submitAuthCode(name: string, code: string, directory?: string): Promise<OperationResponse<"v2.plugins.mcp.auth.callback.post">>;
  removeAuth(name: string, directory?: string): Promise<OperationResponse<"v2.plugins.mcp.auth.remove">>;
}

export const mcp = optionalPlugin<McpClient>("dev.neoism.mcp", "neoism.mcp", (core) => {
  const operations = createContractClient(core.transport);
  const named = (name: string) => ({ name });
  return {
    status: (directory) => operations.request("v2.plugins.mcp.status", { query: directoryQuery(directory) }),
    catalog: (directory) => operations.request("v2.plugins.mcp.catalog", { query: directoryQuery(directory) }),
    add: (body) => operations.request("v2.plugins.mcp.add", { body }),
    configure: (name, body, directory) => operations.request("v2.plugins.mcp.config", { path: named(name), query: directoryQuery(directory), body }),
    connect: (name, directory) => operations.request("v2.plugins.mcp.connect", { path: named(name), query: directoryQuery(directory) }),
    disconnect: (name, directory) => operations.request("v2.plugins.mcp.disconnect", { path: named(name), query: directoryQuery(directory) }),
    tools: (name, directory) => operations.request("v2.plugins.mcp.tools", { path: named(name), query: directoryQuery(directory) }),
    callTool: (name, toolName, body, directory) => operations.request("v2.plugins.mcp.tools.call", { path: { name, tool_name: toolName }, query: directoryQuery(directory), body }),
    resources: (name, directory) => operations.request("v2.plugins.mcp.resources", { path: named(name), query: directoryQuery(directory) }),
    prompts: (name, directory) => operations.request("v2.plugins.mcp.prompts", { path: named(name), query: directoryQuery(directory) }),
    startAuth: (name, directory) => operations.request("v2.plugins.mcp.auth.start", { path: named(name), query: directoryQuery(directory) }),
    authenticate: (name, directory) => operations.request("v2.plugins.mcp.auth.authenticate", { path: named(name), query: directoryQuery(directory) }),
    completeAuth: (name, code, options = {}) => operations.request("v2.plugins.mcp.auth.callback.get", { path: named(name), query: query({ code, ...options }) }),
    submitAuthCode: (name, code, directory) => operations.request("v2.plugins.mcp.auth.callback.post", { path: named(name), query: directoryQuery(directory), body: { code } }),
    removeAuth: (name, directory) => operations.request("v2.plugins.mcp.auth.remove", { path: named(name), query: directoryQuery(directory) }),
  };
});

export interface LspClient {
  status(directory?: string): Promise<OperationResponse<"v2.plugins.lsp.status">>;
  hover(input: PositionOptions): Promise<OperationResponse<"v2.plugins.lsp.hover">>;
  signatureHelp(input: PositionOptions): Promise<OperationResponse<"v2.plugins.lsp.signatureHelp">>;
  definition(input: PositionOptions): Promise<OperationResponse<"v2.plugins.lsp.definition">>;
  references(input: PositionOptions): Promise<OperationResponse<"v2.plugins.lsp.references">>;
  implementation(input: PositionOptions): Promise<OperationResponse<"v2.plugins.lsp.implementation">>;
  documentHighlights(input: PositionOptions): Promise<OperationResponse<"v2.plugins.lsp.documentHighlights">>;
  documentSymbols(file: string, directory?: string): Promise<OperationResponse<"v2.plugins.lsp.documentSymbols">>;
  diagnostics(file: string, directory?: string): Promise<OperationResponse<"v2.plugins.lsp.diagnostics">>;
  inlayHints(file: string, startLine: number, endLine: number, directory?: string): Promise<OperationResponse<"v2.plugins.lsp.inlayHints">>;
  prepareCallHierarchy(input: PositionOptions): Promise<OperationResponse<"v2.plugins.lsp.prepareCallHierarchy">>;
  incomingCalls(input: PositionOptions): Promise<OperationResponse<"v2.plugins.lsp.incomingCalls">>;
  outgoingCalls(input: PositionOptions): Promise<OperationResponse<"v2.plugins.lsp.outgoingCalls">>;
  codeActions(input: PositionOptions): Promise<OperationResponse<"v2.plugins.lsp.codeActions">>;
  formatting(file: string, directory?: string): Promise<OperationResponse<"v2.plugins.lsp.formatting">>;
  touch(input: OperationInput<"v2.plugins.lsp.touch">["body"]): Promise<OperationResponse<"v2.plugins.lsp.touch">>;
  shutdown(): Promise<OperationResponse<"v2.plugins.lsp.shutdown">>;
}

export const lsp = optionalPlugin<LspClient>("dev.neoism.lsp", "neoism.lsp", (core) => {
  const operations = createContractClient(core.transport);
  const position = <Id extends "v2.plugins.lsp.hover" | "v2.plugins.lsp.signatureHelp" | "v2.plugins.lsp.definition" | "v2.plugins.lsp.references" | "v2.plugins.lsp.implementation" | "v2.plugins.lsp.documentHighlights" | "v2.plugins.lsp.prepareCallHierarchy" | "v2.plugins.lsp.incomingCalls" | "v2.plugins.lsp.outgoingCalls" | "v2.plugins.lsp.codeActions">(
    id: Id,
    input: PositionOptions,
  ) => operations.request(id, { query: input } as OperationInput<Id>);
  return {
    status: (directory) => operations.request("v2.plugins.lsp.status", { query: directoryQuery(directory) }),
    hover: (input) => position("v2.plugins.lsp.hover", input),
    signatureHelp: (input) => position("v2.plugins.lsp.signatureHelp", input),
    definition: (input) => position("v2.plugins.lsp.definition", input),
    references: (input) => position("v2.plugins.lsp.references", input),
    implementation: (input) => position("v2.plugins.lsp.implementation", input),
    documentHighlights: (input) => position("v2.plugins.lsp.documentHighlights", input),
    documentSymbols: (file, directory) => operations.request("v2.plugins.lsp.documentSymbols", { query: { file, ...directoryQuery(directory) } }),
    diagnostics: (file, directory) => operations.request("v2.plugins.lsp.diagnostics", { query: { file, ...directoryQuery(directory) } }),
    inlayHints: (file, startLine, endLine, directory) => operations.request("v2.plugins.lsp.inlayHints", { query: { file, start_line: startLine, end_line: endLine, ...directoryQuery(directory) } }),
    prepareCallHierarchy: (input) => position("v2.plugins.lsp.prepareCallHierarchy", input),
    incomingCalls: (input) => position("v2.plugins.lsp.incomingCalls", input),
    outgoingCalls: (input) => position("v2.plugins.lsp.outgoingCalls", input),
    codeActions: (input) => position("v2.plugins.lsp.codeActions", input),
    formatting: (file, directory) => operations.request("v2.plugins.lsp.formatting", { query: { file, ...directoryQuery(directory) } }),
    touch: (body) => operations.request("v2.plugins.lsp.touch", { body }),
    shutdown: () => operations.request("v2.plugins.lsp.shutdown", {}),
  };
});

export interface PtyClient {
  shells(): Promise<OperationResponse<"v2.plugins.pty.shells">>;
  list(): Promise<OperationResponse<"v2.plugins.pty.list">>;
  get(id: string): Promise<OperationResponse<"v2.plugins.pty.get">>;
  create(input: OperationInput<"v2.plugins.pty.create">["body"], directory?: string): Promise<OperationResponse<"v2.plugins.pty.create">>;
  update(id: string, input: OperationInput<"v2.plugins.pty.update">["body"]): Promise<OperationResponse<"v2.plugins.pty.update">>;
  remove(id: string): Promise<boolean>;
  connectToken(id: string): Promise<OperationResponse<"v2.plugins.pty.connectToken">>;
  connect(id: string, options?: { cursor?: number; signal?: AbortSignal }): Promise<PtyConnection>;
}

export type PtyOutput =
  | { type: "data"; data: string }
  | { type: "cursor"; cursor: number };

export interface PtyConnection {
  write(data: string | Uint8Array): void;
  resize(input: OperationInput<"v2.plugins.pty.update">["body"]): Promise<OperationResponse<"v2.plugins.pty.update">>;
  output(): AsyncIterable<PtyOutput>;
  close(code?: number, reason?: string): void;
}

export const pty = optionalPlugin<PtyClient>("dev.neoism.pty", "neoism.pty", (core) => {
  const operations = createContractClient(core.transport);
  return {
    shells: () => operations.request("v2.plugins.pty.shells", {}),
    list: () => operations.request("v2.plugins.pty.list", {}),
    get: (id) => operations.request("v2.plugins.pty.get", { path: { pty_id: id } }),
    create: (body, directory) => operations.request("v2.plugins.pty.create", { query: directoryQuery(directory), body }),
    update: (id, body) => operations.request("v2.plugins.pty.update", { path: { pty_id: id }, body }),
    remove: (id) => operations.request("v2.plugins.pty.remove", { path: { pty_id: id } }),
    connectToken: (id) => operations.request("v2.plugins.pty.connectToken", { path: { pty_id: id }, headers: { "X-OpenCode-Ticket": "1" } }),
    async connect(id, options = {}) {
      if (!core.transport.connectSocket) {
        throw new Error("This Neoism transport does not support WebSocket connections");
      }
      const token = await operations.request("v2.plugins.pty.connectToken", {
        path: { pty_id: id },
        headers: { "X-OpenCode-Ticket": "1" },
      });
      const socket = await core.transport.connectSocket({
        path: `/v2/plugins/dev.neoism.pty/${encodeURIComponent(id)}/connect`,
        query: query({ ticket: token.ticket, cursor: options.cursor }),
        ...(options.signal ? { signal: options.signal } : {}),
      });
      return {
        write: (data) => socket.send(data),
        resize: (body) => operations.request("v2.plugins.pty.update", { path: { pty_id: id }, body }),
        async *output() {
          const decoder = new TextDecoder();
          for await (const message of socket.messages()) {
            if (typeof message === "string") {
              yield { type: "data", data: message } as const;
              continue;
            }
            if (message[0] === 0) {
              const payload = JSON.parse(decoder.decode(message.subarray(1))) as { cursor?: unknown };
              if (typeof payload.cursor === "number") {
                yield { type: "cursor", cursor: payload.cursor } as const;
              }
              continue;
            }
            yield { type: "data", data: decoder.decode(message) } as const;
          }
        },
        close: (code, reason) => socket.close(code, reason),
      };
    },
  };
});