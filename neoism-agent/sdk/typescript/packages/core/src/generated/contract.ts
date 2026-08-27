// Generated from the authoritative canonical Neoism Agent OpenAPI document.
// Run neoism-agent/scripts/openapi.sh update. Do not edit by hand.

import type { NeoismTransport, RequestDescriptor } from "../transport.js";

export type Agent = { color?: string; description?: string; hidden: boolean; mode: string; model?: ModelRef; name: string; native: boolean; options: { [key: string]: unknown; }; permission: { [key: string]: unknown; }; prompt?: string; steps?: number; temperature?: number; topP?: number; variant?: string; };
export type AgentList = Array<Agent>;
export type AgentPart = { id: string; messageId: string; name: string; sessionId: string; source?: unknown; type: "agent"; [key: string]: unknown; };
export type ApiError = { code: string; details: { [key: string]: unknown; }; message: string; requestId?: string; retryable: boolean; };
export type ApiMeta = { apiVersion: string; eventSchemaVersion: string; generation: number; partSchemaVersion: string; pluginApiVersion: string; serverVersion: string; };
export type Artifact = { created: number; downloadUrl: string; filename: string; id: string; mediaType: string; sessionId?: string; sha256: string; size: number; };
export type AuditEntry = { created: number; id: string; method: string; path: string; status: number; tenantId: string; };
export type AuthInfo = ({ key: string; metadata?: unknown; type: "api"; }) | ({ access: string; accountId?: string; enterpriseUrl?: string; expires: number; refresh: string; type: "oauth"; }) | ({ key: string; token: string; type: "wellknown"; });
export type BackgroundJobStopResponse = { jobId: string; status: "stopping"; };
export type CacheUsage = { read: number; write: number; };
export type Capability = { apiPrefix?: string; disableable: boolean; enabled: boolean; id: string; pluginId?: string; reason?: string; source: string; version: string; };
export type CodeRequest = { code: string; };
export type Command = { agent?: string; description?: string; model?: string; name: string; subtask?: boolean; template?: string; };
export type CommandList = Array<Command>;
export type CompactionPart = { id: string; messageId: string; reason: string; sessionId: string; summary: boolean; tailStartMessageId?: string; type: "compaction"; [key: string]: unknown; };
export type ConfigDiagnostic = { level: "error" | "warning"; message: string; path: string; };
export type ConfigDocument = { [key: string]: unknown; };
export type ConfigProvidersResult = { default: { [key: string]: string; }; providers: Array<Provider>; };
export type ConfigValidation = { diagnostics: Array<ConfigDiagnostic>; ok: boolean; };
export type CreateSessionRequest = { agent?: string; model?: ModelRef; parentId?: string; permission?: Array<PermissionRule>; title?: string; workspaceId?: string; };
export type EmptyObject = Record<string, unknown>;
export type Event = (EventMessagePartUpdated) | (EventMessagePartRemoved) | (EventMessagePartDelta) | (EventMessageUpdated) | (EventMessageRemoved) | (EventMcpToolsChanged) | (EventLspUpdated) | (EventPermissionAsked) | (EventPermissionReplied) | (EventQuestionAsked) | (EventQuestionRejected) | (EventQuestionReplied) | (EventPtyCreated) | (EventPtyUpdated) | (EventPtyDeleted) | (EventPtyExited) | (EventSessionNextCompactionStarted) | (EventSessionNextCompactionDelta) | (EventSessionNextCompactionEnded) | (EventSessionCompacted) | (EventSessionContextUpdated) | (EventSessionCreated) | (EventSessionDeleted) | (EventSessionError) | (EventSessionExecutionUpdated) | (EventSessionBackgroundTaskCompleted) | (EventSessionQueueUpdated) | (EventSessionPromptAdmitted) | (EventSessionStatus) | (EventSessionSubtaskCompleted) | (EventSessionUpdated) | (EventTodoUpdated) | (EventWorkflowUpdated) | (EventWorkflowRunUpdated);
export type EventEnvelope = { data: unknown; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: string; };
export type EventLspUpdated = { data: Record<string, unknown>; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: "lsp.updated"; };
export type EventMcpToolsChanged = { data: { directory: string; server: string; }; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: "mcp.tools.changed"; };
export type EventMessagePartDelta = { data: { delta: string; field: string; messageID: string; partID: string; partType: string; sessionID: string; }; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: "message.part.delta"; };
export type EventMessagePartRemoved = { data: { messageID: string; partID: string; sessionID: string; }; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: "message.part.removed"; };
export type EventMessagePartUpdated = { data: { part: Part; sessionID: string; time: number; }; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: "message.part.updated"; };
export type EventMessageRemoved = { data: { messageID: string; sessionID: string; }; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: "message.removed"; };
export type EventMessageUpdated = { data: { info: { id: string; role: string; sessionId: string; [key: string]: unknown; }; sessionID: string; }; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: "message.updated"; };
export type EventPermissionAsked = { data: { always: Array<string>; id: string; messageId: string; metadata?: ({ [key: string]: unknown; }) | (null); parentSessionID?: string; patterns: Array<string>; permission: string; sessionId: string; sourceAgent?: string; sourceSessionID?: string; sourceTitle?: string; title: string; tool?: { [key: string]: unknown; }; [key: string]: unknown; }; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: "permission.asked"; };
export type EventPermissionReplied = { data: { info?: (PermissionRequest) | (null); reply: string; requestID: string; [key: string]: unknown; }; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: "permission.replied"; };
export type EventPtyCreated = { data: { id: string; info: Pty; ptyID: string; }; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: "pty.created"; };
export type EventPtyDeleted = { data: { id: string; info: Pty; ptyID: string; }; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: "pty.deleted"; };
export type EventPtyExited = { data: { exitStatus: (number) | (null); id: string; ptyID: string; }; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: "pty.exited"; };
export type EventPtyUpdated = { data: { id: string; info: Pty; ptyID: string; }; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: "pty.updated"; };
export type EventQuestionAsked = { data: QuestionRequest; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: "question.asked"; };
export type EventQuestionRejected = { data: { info?: (QuestionRequest) | (null); reason?: string; requestID: string; }; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: "question.rejected"; };
export type EventQuestionReplied = { data: { info?: (QuestionRequest) | (null); reply: { answers: Array<Array<string>>; }; requestID: string; }; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: "question.replied"; };
export type EventSessionBackgroundTaskCompleted = { data: { command: string; cwd: string; exitCode: (number) | (null); jobID: string; parentSessionID: string; result: string; sessionID: string; status: string; taskID: string; title: string; }; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: "session.background_task.completed"; };
export type EventSessionCompacted = { data: { info: Session; sessionID: string; summary: { kind: string; messageID: string; text: string; throughMessageID: string; updated: number; }; }; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: "session.compacted"; };
export type EventSessionContextUpdated = { data: { epoch: { [key: string]: unknown; }; sessionID: string; }; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: "session.context.updated"; };
export type EventSessionCreated = { data: { info: Session; sessionID: string; }; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: "session.created"; };
export type EventSessionDeleted = { data: { sessionID: string; }; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: "session.deleted"; };
export type EventSessionError = { data: { error: ApiError; sessionID: string; }; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: "session.error"; };
export type EventSessionExecutionUpdated = { data: { runtime: SessionRuntimeSnapshot; sessionID: string; snapshot: ExecutionActivitySnapshot; }; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: "session.execution.updated"; };
export type EventSessionNextCompactionDelta = { data: { sessionID: string; text: string; }; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: "session.next.compaction.delta"; };
export type EventSessionNextCompactionEnded = { data: { error?: { [key: string]: unknown; }; kind?: string; messageID?: string; sessionID: string; status?: string; summary?: { [key: string]: unknown; }; text?: string; timestamp?: number; [key: string]: unknown; }; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: "session.next.compaction.ended"; };
export type EventSessionNextCompactionStarted = { data: { messageID: string; reason: string; sessionID: string; timestamp: number; }; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: "session.next.compaction.started"; };
export type EventSessionPromptAdmitted = { data: { delivery: string; request: PromptRequest; sessionID: string; }; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: "session.prompt.admitted"; };
export type EventSessionQueueUpdated = { data: { action: string; delivery?: string; messageID?: string; queue: SessionQueueInfo; removed: number; request?: PromptRequest; sessionID: string; }; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: "session.queue.updated"; };
export type EventSessionStatus = { data: { parentSessionID?: string; queue?: number; runID?: string; sessionID: string; sourceAgent?: string; sourceSessionID?: string; sourceTitle?: string; startedAt?: number; status: SessionStatus; }; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: "session.status"; };
export type EventSessionSubtaskCompleted = { data: { agent?: string; childSessionID: string; parentSessionID: string; result: string; sessionID: string; sourceAgent?: string; status: string; taskID: string; title: string; }; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: "session.subtask.completed"; };
export type EventSessionUpdated = { data: { info: Session; sessionID: string; }; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: "session.updated"; };
export type EventSubject = { id: string; kind: string; };
export type EventTodoUpdated = { data: { sessionID: string; todos: Array<Todo>; }; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: "todo.updated"; };
export type EventWorkflowRunUpdated = { data: { aggregateID: string; run: WorkflowRun; }; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: "workflow.run.updated"; };
export type EventWorkflowUpdated = { data: { active?: boolean; aggregateID: string; error?: string; workflow?: WorkflowProjection; workflowID?: string; }; id: string; schemaVersion: string; sequence: number; source: string; subject?: EventSubject; timestamp: number; type: "workflow.updated"; };
export type ExecutionActivitySnapshot = { activeSegments: { [key: string]: number; }; completedMs: number; executionId: string; finished: boolean; revision: number; rootMessageId: string; rootSessionId: string; };
export type ExportSessionsRequest = { workspaceRoot: string; };
export type ExportSessionsResponse = { bundles: Array<SessionBundle>; };
export type FilePart = { filename?: string; id: string; messageId: string; mime: string; sessionId: string; type: "file"; url: string; [key: string]: unknown; };
export type ForkSessionRequest = { messageId?: string; };
export type GoalResearchNote = { captured: number; content: string; source: string; };
export type GoalResearchRequest = { url: string; };
export type GoalResponse = { goal: (SessionGoal) | (null); researchEnabled: boolean; };
export type HealthResponse = { healthy: true; version: string; };
export type ImportSessionRequest = { bundle: SessionBundle; targetWorkspaceRoot: string; };
export type ImportSessionResponse = { sessionId: string; };
export type LspCallHierarchyCall = { direction: string; item: LspCallHierarchyItem; language?: string | null; ranges: Array<LspRange>; };
export type LspCallHierarchyItem = { kind: string; name: string; path: string; [key: string]: unknown; };
export type LspDiagnostic = { code?: string | null; data?: unknown; language?: string | null; message: string; path: string; range?: (LspRange) | (null); related_information: Array<{ [key: string]: unknown; }>; severity: string; tags: Array<string>; };
export type LspDocumentHighlight = { kind?: string | null; language?: string | null; path: string; range?: (LspRange) | (null); };
export type LspDocumentSymbol = { children: Array<LspDocumentSymbol>; kind: string; name: string; path: string; [key: string]: unknown; };
export type LspHover = { contents: string; kind?: string | null; language?: string | null; path: string; range?: (LspRange) | (null); };
export type LspInlayHint = { character: number; kind?: string | null; label: string; language?: string | null; line: number; padding_left: boolean; padding_right: boolean; path: string; };
export type LspLocation = { language?: string | null; path: string; range?: (LspRange) | (null); };
export type LspPosition = { character: number; line: number; };
export type LspRange = { end: LspPosition; start: LspPosition; };
export type LspShutdownResponse = { shutdown: true; };
export type LspSignatureHelp = { path: string; signatures: Array<{ [key: string]: unknown; }>; [key: string]: unknown; };
export type LspStatus = { command: Array<string>; command_source: string; id: string; language: string; name: string; status: "available" | "connected" | "error"; [key: string]: unknown; };
export type LspTouchRequest = { directory?: string; file: string; text?: string | null; };
export type McpAddRequest = { config: McpConfig; name: string; };
export type McpAuthRemoveResponse = { success: boolean; };
export type McpAuthStartResponse = { authorizationUrl: string; oauthState: string; };
export type McpCatalog = { [key: string]: McpCatalogEntry; };
export type McpCatalogEntry = { configWritable: boolean; enabled: boolean; hasCredentials: boolean; oauthCapable: boolean; runtimeConnected: boolean; status: McpStatus; };
export type McpConfig = { type: "local" | "remote"; [key: string]: unknown; };
export type McpConfigPatch = { enabled: boolean; };
export type McpPrompt = { arguments: Array<{ description?: string; name: string; required: boolean; }>; client: string; description?: string; name: string; };
export type McpResource = { client: string; description?: string; mimeType?: string; name: string; uri: string; };
export type McpStatus = ({ status: "connected" | "disabled" | "needs_auth"; }) | ({ error: string; status: "failed" | "needs_client_registration"; });
export type McpStatusMap = { [key: string]: McpStatus; };
export type McpTool = { annotations?: unknown; client: string; description?: string; inputSchema: unknown; name: string; };
export type McpToolCallResult = { content: Array<{ type: string; [key: string]: unknown; }>; isError?: boolean; };
export type Message = { info: MessageInfo; parts: Array<Part>; };
export type MessageInfo = { id: string; role: "user" | "assistant"; sessionId: string; time: { [key: string]: unknown; }; [key: string]: unknown; };
export type MessageList = Array<Message>;
export type MessagePage = { cursor: PageCursor; items: Array<Message>; };
export type ModelRef = { id: string; providerId: string; variant?: string; };
export type OpenApiDocument = { info: { [key: string]: unknown; }; openapi: string; paths: { [key: string]: unknown; }; [key: string]: unknown; };
export type PageCursor = { next?: string; previous?: string; };
export type Part = (TextPart) | (CompactionPart) | (AgentPart) | (SubtaskPart) | (ReasoningPart) | (ToolPart) | (StepStartPart) | (StepFinishPart) | (FilePart);
export type PartEnvelope = { data: unknown; id: string; kind: string; schemaVersion: string; };
export type PartTime = { end?: number; start: number; };
export type PermissionReply = { message?: string; reply?: "once" | "always" | "reject"; response?: string; };
export type PermissionRequest = { always: Array<string>; id: string; messageId: string; metadata?: unknown; patterns: Array<string>; permission: string; sessionId: string; title: string; tool?: unknown; };
export type PermissionRule = { action: string; pattern: string; permission: string; };
export type PluginManifest = { active: boolean; apiPrefix?: string; capabilities: Array<string>; config?: { [key: string]: unknown; }; disableable: boolean; enabled: boolean; eventNamespaces: Array<string>; id: string; internal: boolean; name: string; pluginApi: string; reason?: string; requires: Array<string>; version: string; };
export type PromptPart = ({ text: string; type: "text"; }) | ({ name: string; source?: unknown; type: "agent"; }) | ({ filename: string; mime: string; type: "file"; url: string; }) | ({ agent: string; command?: string; description: string; model?: UserModel; prompt: string; type: "subtask"; });
export type PromptRequest = { agent?: string; author?: string; delivery?: "steer" | "queue"; messageId?: string; model?: UserModel; noReply?: boolean; parts?: Array<PromptPart>; prompt?: string; system?: string; tools?: { [key: string]: boolean; }; variant?: string; } & (({ prompt: string; }) | ({ parts: Array<PromptPart>; }));
export type Provider = { env: Array<string>; id: string; key?: string; models: { [key: string]: ProviderModel; }; name: string; options: { [key: string]: unknown; }; source: "env" | "config" | "custom" | "api" | "builtin"; };
export type ProviderAuthAuthorization = { instructions: string; method: "auto" | "code"; url: string; };
export type ProviderAuthMethod = { label: string; prompts?: Array<ProviderAuthPrompt>; type: "api" | "oauth"; };
export type ProviderAuthMethods = { [key: string]: Array<ProviderAuthMethod>; };
export type ProviderAuthPrompt = { key: string; message: string; type: "text" | "select"; [key: string]: unknown; };
export type ProviderAuthorizeRequest = { inputs?: { [key: string]: string; }; method: unknown; };
export type ProviderCallbackRequest = { code?: string | null; method: unknown; };
export type ProviderList = Array<{ [key: string]: unknown; }>;
export type ProviderListResult = { all: Array<Provider>; connected: Array<string>; default: { [key: string]: string; }; };
export type ProviderModel = { api: { [key: string]: unknown; }; id: string; name: string; providerId: string; releaseDate: string; status: "alpha" | "beta" | "deprecated" | "active"; [key: string]: unknown; };
export type Pty = { command: Array<string>; cwd: string; id: string; time: number; title: string; };
export type PtyConnectToken = { expires_in: number; ticket: string; };
export type PtyCreateRequest = { command?: Array<string>; cwd?: string; title?: string; };
export type PtyUpdateRequest = { cwd?: string; size?: { cols: number; rows: number; }; title?: string; };
export type QuestionReply = { answers: Array<Array<string>>; };
export type QuestionRequest = { id: string; messageId: string; questions: Array<unknown>; sessionId: string; };
export type QueuedPrompt = { agent?: string; author?: string; messageId?: string; model?: UserModel; noReply: boolean; parts: Array<PromptPart>; system?: string; tools?: { [key: string]: boolean; }; };
export type QueuedPromptBundleItem = { delivery: string; request: QueuedPrompt; };
export type ReasoningPart = { id: string; messageId: string; metadata?: unknown; sessionId: string; text: string; time: PartTime; type: "reasoning"; [key: string]: unknown; };
export type RevertRequest = { messageId?: string; partId?: string; };
export type SemanticSearchHit = { created: number; distance: number; excerpt: string; messageId: string; role: string; sessionId: string; };
export type SemanticSearchResponse = { available: boolean; hits: Array<SemanticSearchHit>; };
export type Session = { agent?: string; directory: string; id: string; model?: ModelRef; parentId?: string; path?: string; permission?: Array<PermissionRule>; projectId: string; slug: string; time: SessionTime; title: string; version: string; workspaceId?: string; [key: string]: unknown; };
export type SessionBundle = { messages: Array<Message>; queuedPrompts: Array<QueuedPromptBundleItem>; session: Session; version: number; workspaceRoot?: string; };
export type SessionCommandRequest = { agent?: string; arguments?: string; command: string; messageId?: string; model?: UserModel; };
export type SessionGoal = { created: number; paused: boolean; research: Array<GoalResearchNote>; status: "active" | "complete" | "blocked"; summary: string; text: string; updated: number; };
export type SessionPage = { cursor: PageCursor; items: Array<Session>; };
export type SessionQueueInfo = { count: number; items: Array<SessionQueueItem>; running: boolean; sessionId: string; worker: boolean; };
export type SessionQueueItem = { agent?: string | null; index: number; model?: (UserModel) | (null); noReply: boolean; partCount: number; text?: string | null; };
export type SessionQueueMutation = { queue: SessionQueueInfo; removed: number; sessionId: string; };
export type SessionRuntimeSnapshot = { branches: Array<SubtaskLifecycleSnapshot>; execution?: (ExecutionActivitySnapshot) | (null); revision: number; rootSessionId: string; };
export type SessionShellRequest = { agent?: string; command: string; messageId?: string; model?: UserModel; };
export type SessionStatus = { type: "idle" | "busy" | "retry"; [key: string]: unknown; };
export type SessionStatusMap = { [key: string]: SessionStatus; };
export type SessionTime = { archived?: number; compacting?: number; created: number; updated: number; };
export type SessionUndoTree = { nodes: Array<{ [key: string]: unknown; }>; sessionID: string; [key: string]: unknown; };
export type SetGoalRequest = { paused?: boolean; researchUrls?: Array<string>; text?: string; };
export type SetPinRequest = { pinned?: boolean; };
export type Shell = { acceptable: boolean; name: string; path: string; };
export type Skill = { description?: string | null; name: string; path?: string; };
export type SkillList = Array<Skill>;
export type StepFinishPart = { cost: number; id: string; messageId: string; reason: string; sessionId: string; snapshot?: string; tokens: TokenUsage; type: "step-finish"; [key: string]: unknown; };
export type StepStartPart = { id: string; messageId: string; sessionId: string; snapshot?: string; type: "step-start"; [key: string]: unknown; };
export type StopSubagentsRequest = { taskId?: string; };
export type StopSubagentsResult = { clearedPrompts: number; stopped: Array<string>; };
export type SubagentTask = { agent: string; childSessionId: string; description: string; id: string; nested: boolean; result?: string; sessionId: string; status: string; };
export type SubtaskLifecycleSnapshot = { parentSessionId: string; sessionId: string; startedAt?: number | null; status: "outstanding" | "completed" | "failed"; };
export type SubtaskPart = { agent: string; command?: string; description: string; id: string; messageId: string; model?: UserModelRef; prompt: string; sessionId: string; type: "subtask"; [key: string]: unknown; };
export type TextPart = { id: string; messageId: string; sessionId: string; synthetic?: boolean; text: string; time?: PartTime; type: "text"; [key: string]: unknown; };
export type Todo = { content: string; priority: string; status: string; };
export type TokenUsage = { cache: CacheUsage; input: number; output: number; reasoning: number; total?: number; };
export type Tool = { description: string; id: string; outputSchema?: unknown; parameters: unknown; [key: string]: unknown; };
export type ToolList = Array<Tool>;
export type ToolPart = { callId: string; id: string; messageId: string; metadata?: unknown; sessionId: string; state: ToolState; tool: string; type: "tool"; [key: string]: unknown; };
export type ToolState = (ToolStatePending) | (ToolStateRunning) | (ToolStateCompleted) | (ToolStateError);
export type ToolStateCompleted = { input: unknown; metadata: unknown; output: string; status: "completed"; time: PartTime; title: string; [key: string]: unknown; };
export type ToolStateError = { error: string; input: unknown; status: "error"; time: PartTime; [key: string]: unknown; };
export type ToolStatePending = { input: unknown; raw: string; status: "pending"; [key: string]: unknown; };
export type ToolStateRunning = { input: unknown; status: "running"; time: PartTime; [key: string]: unknown; };
export type UnknownObject = { [key: string]: unknown; };
export type UnknownValue = unknown;
export type UpdateSessionRequest = { agent?: string; directory?: string; model?: ModelRef; permission?: Array<PermissionRule>; time?: { archived?: number; }; title?: string; };
export type UserModel = { modelId: string; providerId: string; variant?: string; };
export type UserModelRef = { modelId: string; providerId: string; variant?: string; };
export type VcsApplyRequest = { directory?: string; patch: string; [key: string]: unknown; };
export type VcsApplyResult = { error?: string | null; success: boolean; };
export type VcsDiffList = Array<VcsFileDiff>;
export type VcsFileDiff = { added: number; additions: number; deletions: number; file: string; hunks: Array<unknown>; patch: string; path: string; removed: number; status: string; };
export type VcsFileStatus = { additions: number; deletions: number; file: string; path: string; status: string; };
export type VcsInfo = { branch?: string | null; default_branch?: string | null; };
export type VcsStatusList = Array<VcsFileStatus>;
export type WorkflowCatalog = { diagnostics: Array<WorkflowDiagnostic>; workflows: Array<WorkflowView>; };
export type WorkflowDefinition = { active: boolean; agent?: string; directory?: string; id: string; model?: ModelRef; name: string; permissions?: { [key: string]: unknown; }; prompt: string; schedule: WorkflowSchedule; skill?: string; };
export type WorkflowDiagnostic = { message: string; sourcePath: string; };
export type WorkflowHistory = { runs: Array<WorkflowRun>; };
export type WorkflowPreview = { definition: WorkflowDefinition; sourcePath: string; upcoming: Array<{ local: string; scheduledAt: number; }>; };
export type WorkflowProjection = { activatedAt: number; activationId: string; active: boolean; definition: WorkflowDefinition; lastScheduledAt?: number | null; sourceHash: string; sourcePath: string; updated: number; workflowId: string; workspaceRoot: string; };
export type WorkflowRun = { activationId: string; created: number; error?: string | null; finishedAt?: number | null; id: string; scheduledAt: number; sessionId?: string | null; startedAt?: number | null; status: string; trigger: string; workflowId: string; };
export type WorkflowSchedule = { at?: string; date?: string; frequency: string; interval: number; minute?: number; monthDay?: number; time?: string; timezone: string; weekdays?: Array<string>; };
export type WorkflowView = { activationID?: string | null; active: boolean; definition: WorkflowDefinition; lastScheduledAt?: number | null; sourceHash: string; sourcePath: string; };

export interface ApiOperations {
  "v2.agents.get": { method: "GET"; path: "/v2/agents/{name}"; input: { path: { name: string; }; query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": Agent; }; response: Agent; };
  "v2.agents.list": { method: "GET"; path: "/v2/agents"; input: { query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": AgentList; }; response: AgentList; };
  "v2.artifacts.content": { method: "GET"; path: "/v2/artifacts/{artifact_id}/content"; input: { path: { artifact_id: string; }; signal?: AbortSignal; }; responses: { "200": Uint8Array; }; response: Uint8Array; };
  "v2.artifacts.create": { method: "POST"; path: "/v2/artifacts"; input: { headers?: { "Content-Type"?: string; "X-Neoism-Filename"?: string; "X-Neoism-Session-Id"?: string; }; body: Uint8Array | Blob; signal?: AbortSignal; }; responses: { "201": Artifact; }; response: Artifact; };
  "v2.artifacts.delete": { method: "DELETE"; path: "/v2/artifacts/{artifact_id}"; input: { path: { artifact_id: string; }; signal?: AbortSignal; }; responses: { "204": void; }; response: void; };
  "v2.artifacts.get": { method: "GET"; path: "/v2/artifacts/{artifact_id}"; input: { path: { artifact_id: string; }; signal?: AbortSignal; }; responses: { "200": Artifact; }; response: Artifact; };
  "v2.artifacts.list": { method: "GET"; path: "/v2/artifacts"; input: { query?: { sessionId?: string; }; signal?: AbortSignal; }; responses: { "200": Array<Artifact>; }; response: Array<Artifact>; };
  "v2.audit.list": { method: "GET"; path: "/v2/audit"; input: { query?: { limit?: number; }; signal?: AbortSignal; }; responses: { "200": Array<AuditEntry>; }; response: Array<AuditEntry>; };
  "v2.capabilities.list": { method: "GET"; path: "/v2/capabilities"; input: { query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": Array<Capability>; }; response: Array<Capability>; };
  "v2.commands.list": { method: "GET"; path: "/v2/commands"; input: { query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": CommandList; }; response: CommandList; };
  "v2.config.get": { method: "GET"; path: "/v2/config"; input: { query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": ConfigDocument; }; response: ConfigDocument; };
  "v2.config.update": { method: "PATCH"; path: "/v2/config"; input: { query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; body: ConfigDocument; signal?: AbortSignal; }; responses: { "200": ConfigDocument; }; response: ConfigDocument; };
  "v2.config.validate": { method: "GET"; path: "/v2/config/validate"; input: { query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": ConfigValidation; }; response: ConfigValidation; };
  "v2.events.subscribe": { method: "GET"; path: "/v2/events"; input: { query?: { since?: number; tail?: boolean; limit?: number; sessionId?: string; }; headers?: { "Last-Event-ID"?: number; }; signal?: AbortSignal; }; responses: { "200": string; }; response: string; };
  "v2.health": { method: "GET"; path: "/v2/health"; input: { signal?: AbortSignal; }; responses: { "200": HealthResponse; }; response: HealthResponse; };
  "v2.interactions.permissions.list": { method: "GET"; path: "/v2/interactions/permissions"; input: { query?: { sessionId?: string; }; signal?: AbortSignal; }; responses: { "200": Array<PermissionRequest>; }; response: Array<PermissionRequest>; };
  "v2.interactions.permissions.reply": { method: "POST"; path: "/v2/interactions/permissions/{request_id}/reply"; input: { path: { request_id: string; }; body: PermissionReply; signal?: AbortSignal; }; responses: { "200": boolean; }; response: boolean; };
  "v2.interactions.questions.list": { method: "GET"; path: "/v2/interactions/questions"; input: { query?: { sessionId?: string; }; signal?: AbortSignal; }; responses: { "200": Array<QuestionRequest>; }; response: Array<QuestionRequest>; };
  "v2.interactions.questions.reject": { method: "POST"; path: "/v2/interactions/questions/{request_id}/reject"; input: { path: { request_id: string; }; signal?: AbortSignal; }; responses: { "200": boolean; }; response: boolean; };
  "v2.interactions.questions.reply": { method: "POST"; path: "/v2/interactions/questions/{request_id}/reply"; input: { path: { request_id: string; }; body: QuestionReply; signal?: AbortSignal; }; responses: { "200": boolean; }; response: boolean; };
  "v2.meta.get": { method: "GET"; path: "/v2/meta"; input: { signal?: AbortSignal; }; responses: { "200": ApiMeta; }; response: ApiMeta; };
  "v2.openapi.get": { method: "GET"; path: "/v2/openapi.json"; input: { signal?: AbortSignal; }; responses: { "200": OpenApiDocument; }; response: OpenApiDocument; };
  "v2.plugins.get": { method: "GET"; path: "/v2/plugins/{plugin_id}/manifest"; input: { path: { plugin_id: string; }; query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": PluginManifest; }; response: PluginManifest; };
  "v2.plugins.goals.clear": { method: "DELETE"; path: "/v2/plugins/dev.neoism.goals/{session_id}"; input: { path: { session_id: string; }; signal?: AbortSignal; }; responses: { "200": GoalResponse; }; response: GoalResponse; };
  "v2.plugins.goals.get": { method: "GET"; path: "/v2/plugins/dev.neoism.goals/{session_id}"; input: { path: { session_id: string; }; signal?: AbortSignal; }; responses: { "200": GoalResponse; }; response: GoalResponse; };
  "v2.plugins.goals.research": { method: "POST"; path: "/v2/plugins/dev.neoism.goals/{session_id}/research"; input: { path: { session_id: string; }; body: GoalResearchRequest; signal?: AbortSignal; }; responses: { "200": GoalResponse; }; response: GoalResponse; };
  "v2.plugins.goals.set": { method: "POST"; path: "/v2/plugins/dev.neoism.goals/{session_id}"; input: { path: { session_id: string; }; body?: SetGoalRequest; signal?: AbortSignal; }; responses: { "200": GoalResponse; }; response: GoalResponse; };
  "v2.plugins.list": { method: "GET"; path: "/v2/plugins"; input: { query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": Array<PluginManifest>; }; response: Array<PluginManifest>; };
  "v2.plugins.lsp.codeActions": { method: "GET"; path: "/v2/plugins/dev.neoism.lsp/code-actions"; input: { query: { directory?: string; file: string; line: number; character: number; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": Array<UnknownValue>; }; response: Array<UnknownValue>; };
  "v2.plugins.lsp.definition": { method: "GET"; path: "/v2/plugins/dev.neoism.lsp/definition"; input: { query: { directory?: string; file: string; line: number; character: number; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": Array<LspLocation>; }; response: Array<LspLocation>; };
  "v2.plugins.lsp.diagnostics": { method: "GET"; path: "/v2/plugins/dev.neoism.lsp/diagnostics"; input: { query: { directory?: string; file: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": Array<LspDiagnostic>; }; response: Array<LspDiagnostic>; };
  "v2.plugins.lsp.documentHighlights": { method: "GET"; path: "/v2/plugins/dev.neoism.lsp/document-highlights"; input: { query: { directory?: string; file: string; line: number; character: number; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": Array<LspDocumentHighlight>; }; response: Array<LspDocumentHighlight>; };
  "v2.plugins.lsp.documentSymbols": { method: "GET"; path: "/v2/plugins/dev.neoism.lsp/document-symbols"; input: { query: { directory?: string; file: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": Array<LspDocumentSymbol>; }; response: Array<LspDocumentSymbol>; };
  "v2.plugins.lsp.formatting": { method: "GET"; path: "/v2/plugins/dev.neoism.lsp/formatting"; input: { query: { directory?: string; file: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": Array<UnknownValue>; }; response: Array<UnknownValue>; };
  "v2.plugins.lsp.hover": { method: "GET"; path: "/v2/plugins/dev.neoism.lsp/hover"; input: { query: { directory?: string; file: string; line: number; character: number; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": Array<LspHover>; }; response: Array<LspHover>; };
  "v2.plugins.lsp.implementation": { method: "GET"; path: "/v2/plugins/dev.neoism.lsp/implementation"; input: { query: { directory?: string; file: string; line: number; character: number; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": Array<LspLocation>; }; response: Array<LspLocation>; };
  "v2.plugins.lsp.incomingCalls": { method: "GET"; path: "/v2/plugins/dev.neoism.lsp/incoming-calls"; input: { query: { directory?: string; file: string; line: number; character: number; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": Array<LspCallHierarchyCall>; }; response: Array<LspCallHierarchyCall>; };
  "v2.plugins.lsp.inlayHints": { method: "GET"; path: "/v2/plugins/dev.neoism.lsp/inlay-hints"; input: { query: { directory?: string; file: string; start_line: number; end_line: number; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": Array<LspInlayHint>; }; response: Array<LspInlayHint>; };
  "v2.plugins.lsp.outgoingCalls": { method: "GET"; path: "/v2/plugins/dev.neoism.lsp/outgoing-calls"; input: { query: { directory?: string; file: string; line: number; character: number; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": Array<LspCallHierarchyCall>; }; response: Array<LspCallHierarchyCall>; };
  "v2.plugins.lsp.prepareCallHierarchy": { method: "GET"; path: "/v2/plugins/dev.neoism.lsp/prepare-call-hierarchy"; input: { query: { directory?: string; file: string; line: number; character: number; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": Array<LspCallHierarchyItem>; }; response: Array<LspCallHierarchyItem>; };
  "v2.plugins.lsp.references": { method: "GET"; path: "/v2/plugins/dev.neoism.lsp/references"; input: { query: { directory?: string; file: string; line: number; character: number; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": Array<LspLocation>; }; response: Array<LspLocation>; };
  "v2.plugins.lsp.shutdown": { method: "POST"; path: "/v2/plugins/dev.neoism.lsp/shutdown"; input: { signal?: AbortSignal; }; responses: { "200": LspShutdownResponse; }; response: LspShutdownResponse; };
  "v2.plugins.lsp.signatureHelp": { method: "GET"; path: "/v2/plugins/dev.neoism.lsp/signature-help"; input: { query: { directory?: string; file: string; line: number; character: number; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": Array<LspSignatureHelp>; }; response: Array<LspSignatureHelp>; };
  "v2.plugins.lsp.status": { method: "GET"; path: "/v2/plugins/dev.neoism.lsp"; input: { query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": Array<LspStatus>; }; response: Array<LspStatus>; };
  "v2.plugins.lsp.touch": { method: "POST"; path: "/v2/plugins/dev.neoism.lsp/touch"; input: { body: LspTouchRequest; signal?: AbortSignal; }; responses: { "200": Array<UnknownValue>; }; response: Array<UnknownValue>; };
  "v2.plugins.mcp.add": { method: "POST"; path: "/v2/plugins/dev.neoism.mcp"; input: { body: McpAddRequest; signal?: AbortSignal; }; responses: { "200": McpStatusMap; }; response: McpStatusMap; };
  "v2.plugins.mcp.auth.authenticate": { method: "POST"; path: "/v2/plugins/dev.neoism.mcp/{name}/auth/authenticate"; input: { path: { name: string; }; query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": McpStatus; }; response: McpStatus; };
  "v2.plugins.mcp.auth.callback.get": { method: "GET"; path: "/v2/plugins/dev.neoism.mcp/{name}/auth/callback"; input: { path: { name: string; }; query: { code: string; state?: string; directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": string; }; response: string; };
  "v2.plugins.mcp.auth.callback.post": { method: "POST"; path: "/v2/plugins/dev.neoism.mcp/{name}/auth/callback"; input: { path: { name: string; }; query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; body: CodeRequest; signal?: AbortSignal; }; responses: { "200": McpStatus; }; response: McpStatus; };
  "v2.plugins.mcp.auth.remove": { method: "DELETE"; path: "/v2/plugins/dev.neoism.mcp/{name}/auth"; input: { path: { name: string; }; query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": McpAuthRemoveResponse; }; response: McpAuthRemoveResponse; };
  "v2.plugins.mcp.auth.start": { method: "POST"; path: "/v2/plugins/dev.neoism.mcp/{name}/auth"; input: { path: { name: string; }; query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": McpAuthStartResponse; }; response: McpAuthStartResponse; };
  "v2.plugins.mcp.catalog": { method: "GET"; path: "/v2/plugins/dev.neoism.mcp/catalog"; input: { query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": McpCatalog; }; response: McpCatalog; };
  "v2.plugins.mcp.config": { method: "PATCH"; path: "/v2/plugins/dev.neoism.mcp/{name}/config"; input: { path: { name: string; }; query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; body: McpConfigPatch; signal?: AbortSignal; }; responses: { "200": McpCatalogEntry; }; response: McpCatalogEntry; };
  "v2.plugins.mcp.connect": { method: "POST"; path: "/v2/plugins/dev.neoism.mcp/{name}/connect"; input: { path: { name: string; }; query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": boolean; }; response: boolean; };
  "v2.plugins.mcp.disconnect": { method: "POST"; path: "/v2/plugins/dev.neoism.mcp/{name}/disconnect"; input: { path: { name: string; }; query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": boolean; }; response: boolean; };
  "v2.plugins.mcp.prompts": { method: "GET"; path: "/v2/plugins/dev.neoism.mcp/{name}/prompts"; input: { path: { name: string; }; query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": Array<McpPrompt>; }; response: Array<McpPrompt>; };
  "v2.plugins.mcp.resources": { method: "GET"; path: "/v2/plugins/dev.neoism.mcp/{name}/resources"; input: { path: { name: string; }; query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": Array<McpResource>; }; response: Array<McpResource>; };
  "v2.plugins.mcp.status": { method: "GET"; path: "/v2/plugins/dev.neoism.mcp"; input: { query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": McpStatusMap; }; response: McpStatusMap; };
  "v2.plugins.mcp.tools": { method: "GET"; path: "/v2/plugins/dev.neoism.mcp/{name}/tools"; input: { path: { name: string; }; query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": Array<McpTool>; }; response: Array<McpTool>; };
  "v2.plugins.mcp.tools.call": { method: "POST"; path: "/v2/plugins/dev.neoism.mcp/{name}/tools/{tool_name}"; input: { path: { name: string; tool_name: string; }; query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; body: UnknownValue; signal?: AbortSignal; }; responses: { "200": McpToolCallResult; }; response: McpToolCallResult; };
  "v2.plugins.pty.connect": { method: "GET"; path: "/v2/plugins/dev.neoism.pty/{pty_id}/connect"; input: { path: { pty_id: string; }; query: { ticket: string; cursor?: number; }; signal?: AbortSignal; }; responses: {  }; response: void; };
  "v2.plugins.pty.connectToken": { method: "POST"; path: "/v2/plugins/dev.neoism.pty/{pty_id}/connect-token"; input: { path: { pty_id: string; }; headers: { "X-OpenCode-Ticket": "1"; }; signal?: AbortSignal; }; responses: { "200": PtyConnectToken; }; response: PtyConnectToken; };
  "v2.plugins.pty.create": { method: "POST"; path: "/v2/plugins/dev.neoism.pty"; input: { query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; body: PtyCreateRequest; signal?: AbortSignal; }; responses: { "200": Pty; }; response: Pty; };
  "v2.plugins.pty.get": { method: "GET"; path: "/v2/plugins/dev.neoism.pty/{pty_id}"; input: { path: { pty_id: string; }; signal?: AbortSignal; }; responses: { "200": Pty; }; response: Pty; };
  "v2.plugins.pty.list": { method: "GET"; path: "/v2/plugins/dev.neoism.pty"; input: { signal?: AbortSignal; }; responses: { "200": Array<Pty>; }; response: Array<Pty>; };
  "v2.plugins.pty.remove": { method: "DELETE"; path: "/v2/plugins/dev.neoism.pty/{pty_id}"; input: { path: { pty_id: string; }; signal?: AbortSignal; }; responses: { "200": boolean; }; response: boolean; };
  "v2.plugins.pty.shells": { method: "GET"; path: "/v2/plugins/dev.neoism.pty/shells"; input: { signal?: AbortSignal; }; responses: { "200": Array<Shell>; }; response: Array<Shell>; };
  "v2.plugins.pty.update": { method: "PUT"; path: "/v2/plugins/dev.neoism.pty/{pty_id}"; input: { path: { pty_id: string; }; body: PtyUpdateRequest; signal?: AbortSignal; }; responses: { "200": Pty; }; response: Pty; };
  "v2.plugins.semantic.search": { method: "GET"; path: "/v2/plugins/dev.neoism.semantic/search"; input: { query: { q: string; limit?: number; sessionId?: string; }; signal?: AbortSignal; }; responses: { "200": SemanticSearchResponse; }; response: SemanticSearchResponse; };
  "v2.plugins.vcs.apply": { method: "POST"; path: "/v2/plugins/dev.neoism.vcs/apply"; input: { body: VcsApplyRequest; signal?: AbortSignal; }; responses: { "200": VcsApplyResult; }; response: VcsApplyResult; };
  "v2.plugins.vcs.diff": { method: "GET"; path: "/v2/plugins/dev.neoism.vcs/diff"; input: { query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": VcsDiffList; }; response: VcsDiffList; };
  "v2.plugins.vcs.diff.raw": { method: "GET"; path: "/v2/plugins/dev.neoism.vcs/diff/raw"; input: { query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": string; }; response: string; };
  "v2.plugins.vcs.get": { method: "GET"; path: "/v2/plugins/dev.neoism.vcs"; input: { query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": VcsInfo; }; response: VcsInfo; };
  "v2.plugins.vcs.status": { method: "GET"; path: "/v2/plugins/dev.neoism.vcs/status"; input: { query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": VcsStatusList; }; response: VcsStatusList; };
  "v2.plugins.workflows.activate": { method: "POST"; path: "/v2/plugins/dev.neoism.workflows/{workflow_id}/activate"; input: { path: { workflow_id: string; }; query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": WorkflowProjection; }; response: WorkflowProjection; };
  "v2.plugins.workflows.get": { method: "GET"; path: "/v2/plugins/dev.neoism.workflows/{workflow_id}"; input: { path: { workflow_id: string; }; query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": WorkflowView; }; response: WorkflowView; };
  "v2.plugins.workflows.history": { method: "GET"; path: "/v2/plugins/dev.neoism.workflows/{workflow_id}/runs"; input: { path: { workflow_id: string; }; query?: { directory?: string; limit?: number; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": WorkflowHistory; }; response: WorkflowHistory; };
  "v2.plugins.workflows.list": { method: "GET"; path: "/v2/plugins/dev.neoism.workflows"; input: { query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": WorkflowCatalog; }; response: WorkflowCatalog; };
  "v2.plugins.workflows.pause": { method: "POST"; path: "/v2/plugins/dev.neoism.workflows/{workflow_id}/pause"; input: { path: { workflow_id: string; }; query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": WorkflowProjection; }; response: WorkflowProjection; };
  "v2.plugins.workflows.preview": { method: "GET"; path: "/v2/plugins/dev.neoism.workflows/{workflow_id}/preview"; input: { path: { workflow_id: string; }; query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": WorkflowPreview; }; response: WorkflowPreview; };
  "v2.plugins.workflows.run": { method: "POST"; path: "/v2/plugins/dev.neoism.workflows/{workflow_id}/run"; input: { path: { workflow_id: string; }; query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": WorkflowRun; }; response: WorkflowRun; };
  "v2.providers.auth.delete": { method: "DELETE"; path: "/v2/providers/{provider_id}/auth"; input: { path: { provider_id: string; }; signal?: AbortSignal; }; responses: { "200": boolean; }; response: boolean; };
  "v2.providers.auth.get": { method: "GET"; path: "/v2/providers/{provider_id}/auth"; input: { path: { provider_id: string; }; signal?: AbortSignal; }; responses: { "200": (AuthInfo) | (null); }; response: (AuthInfo) | (null); };
  "v2.providers.auth.set": { method: "PUT"; path: "/v2/providers/{provider_id}/auth"; input: { path: { provider_id: string; }; body: AuthInfo; signal?: AbortSignal; }; responses: { "200": boolean; }; response: boolean; };
  "v2.providers.authMethods": { method: "GET"; path: "/v2/providers/auth-methods"; input: { query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": ProviderAuthMethods; }; response: ProviderAuthMethods; };
  "v2.providers.configured": { method: "GET"; path: "/v2/providers/configured"; input: { query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": ConfigProvidersResult; }; response: ConfigProvidersResult; };
  "v2.providers.list": { method: "GET"; path: "/v2/providers"; input: { query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": ProviderListResult; }; response: ProviderListResult; };
  "v2.providers.oauth.authorize": { method: "POST"; path: "/v2/providers/{provider_id}/oauth/authorize"; input: { path: { provider_id: string; }; body: ProviderAuthorizeRequest; signal?: AbortSignal; }; responses: { "200": (ProviderAuthAuthorization) | (null); }; response: (ProviderAuthAuthorization) | (null); };
  "v2.providers.oauth.callback": { method: "POST"; path: "/v2/providers/{provider_id}/oauth/callback"; input: { path: { provider_id: string; }; body: ProviderCallbackRequest; signal?: AbortSignal; }; responses: { "200": boolean; }; response: boolean; };
  "v2.sessions.abort": { method: "POST"; path: "/v2/sessions/{session_id}/abort"; input: { path: { session_id: string; }; signal?: AbortSignal; }; responses: { "200": boolean; }; response: boolean; };
  "v2.sessions.children": { method: "GET"; path: "/v2/sessions/{session_id}/children"; input: { path: { session_id: string; }; signal?: AbortSignal; }; responses: { "200": SessionPage; }; response: SessionPage; };
  "v2.sessions.commands.execute": { method: "POST"; path: "/v2/sessions/{session_id}/commands"; input: { path: { session_id: string; }; body: SessionCommandRequest; signal?: AbortSignal; }; responses: { "200": Message; }; response: Message; };
  "v2.sessions.compact": { method: "POST"; path: "/v2/sessions/{session_id}/compact"; input: { path: { session_id: string; }; signal?: AbortSignal; }; responses: { "204": void; }; response: void; };
  "v2.sessions.context": { method: "GET"; path: "/v2/sessions/{session_id}/context"; input: { path: { session_id: string; }; signal?: AbortSignal; }; responses: { "200": MessageList; }; response: MessageList; };
  "v2.sessions.create": { method: "POST"; path: "/v2/sessions"; input: { query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; body?: CreateSessionRequest; signal?: AbortSignal; }; responses: { "200": Session; }; response: Session; };
  "v2.sessions.delete": { method: "DELETE"; path: "/v2/sessions/{session_id}"; input: { path: { session_id: string; }; signal?: AbortSignal; }; responses: { "200": boolean; }; response: boolean; };
  "v2.sessions.diff": { method: "GET"; path: "/v2/sessions/{session_id}/diff"; input: { path: { session_id: string; }; signal?: AbortSignal; }; responses: { "200": Array<VcsFileDiff>; }; response: Array<VcsFileDiff>; };
  "v2.sessions.directoryOptions": { method: "GET"; path: "/v2/sessions/{session_id}/directory-options"; input: { path: { session_id: string; }; query?: { query?: string; limit?: number; }; signal?: AbortSignal; }; responses: { "200": Array<string>; }; response: Array<string>; };
  "v2.sessions.export": { method: "POST"; path: "/v2/sessions/export"; input: { body: ExportSessionsRequest; signal?: AbortSignal; }; responses: { "200": ExportSessionsResponse; }; response: ExportSessionsResponse; };
  "v2.sessions.fork": { method: "POST"; path: "/v2/sessions/{session_id}/fork"; input: { path: { session_id: string; }; body?: ForkSessionRequest; signal?: AbortSignal; }; responses: { "200": Session; }; response: Session; };
  "v2.sessions.get": { method: "GET"; path: "/v2/sessions/{session_id}"; input: { path: { session_id: string; }; signal?: AbortSignal; }; responses: { "200": Session; }; response: Session; };
  "v2.sessions.import": { method: "POST"; path: "/v2/sessions/import"; input: { body: ImportSessionRequest; signal?: AbortSignal; }; responses: { "200": ImportSessionResponse; }; response: ImportSessionResponse; };
  "v2.sessions.jobs.cancel": { method: "DELETE"; path: "/v2/sessions/{session_id}/jobs/{job_id}"; input: { path: { session_id: string; job_id: string; }; signal?: AbortSignal; }; responses: { "200": BackgroundJobStopResponse; }; response: BackgroundJobStopResponse; };
  "v2.sessions.list": { method: "GET"; path: "/v2/sessions"; input: { query?: { directory?: string; path?: string; roots?: string; start?: number; search?: string; limit?: number; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": SessionPage; }; response: SessionPage; };
  "v2.sessions.messages": { method: "GET"; path: "/v2/sessions/{session_id}/messages"; input: { path: { session_id: string; }; query?: { limit?: number; order?: "asc" | "desc"; slim?: boolean; cursor?: string; }; signal?: AbortSignal; }; responses: { "200": MessagePage; }; response: MessagePage; };
  "v2.sessions.messages.delete": { method: "DELETE"; path: "/v2/sessions/{session_id}/messages/{message_id}"; input: { path: { session_id: string; message_id: string; }; signal?: AbortSignal; }; responses: { "200": boolean; }; response: boolean; };
  "v2.sessions.messages.get": { method: "GET"; path: "/v2/sessions/{session_id}/messages/{message_id}"; input: { path: { session_id: string; message_id: string; }; signal?: AbortSignal; }; responses: { "200": Message; }; response: Message; };
  "v2.sessions.parts.delete": { method: "DELETE"; path: "/v2/sessions/{session_id}/messages/{message_id}/parts/{part_id}"; input: { path: { session_id: string; message_id: string; part_id: string; }; signal?: AbortSignal; }; responses: { "200": boolean; }; response: boolean; };
  "v2.sessions.parts.update": { method: "PATCH"; path: "/v2/sessions/{session_id}/messages/{message_id}/parts/{part_id}"; input: { path: { session_id: string; message_id: string; part_id: string; }; body: Part; signal?: AbortSignal; }; responses: { "200": Part; }; response: Part; };
  "v2.sessions.pin": { method: "POST"; path: "/v2/sessions/{session_id}/pin"; input: { path: { session_id: string; }; body?: SetPinRequest; signal?: AbortSignal; }; responses: { "200": Session; }; response: Session; };
  "v2.sessions.prompt": { method: "POST"; path: "/v2/sessions/{session_id}/prompt"; input: { path: { session_id: string; }; body: PromptRequest; signal?: AbortSignal; }; responses: { "204": void; }; response: void; };
  "v2.sessions.promptAsync": { method: "POST"; path: "/v2/sessions/{session_id}/prompt-async"; input: { path: { session_id: string; }; body: PromptRequest; signal?: AbortSignal; }; responses: { "204": void; }; response: void; };
  "v2.sessions.queue.clear": { method: "DELETE"; path: "/v2/sessions/{session_id}/queue"; input: { path: { session_id: string; }; signal?: AbortSignal; }; responses: { "200": SessionQueueMutation; }; response: SessionQueueMutation; };
  "v2.sessions.queue.list": { method: "GET"; path: "/v2/sessions/{session_id}/queue"; input: { path: { session_id: string; }; signal?: AbortSignal; }; responses: { "200": SessionQueueInfo; }; response: SessionQueueInfo; };
  "v2.sessions.queue.pop": { method: "POST"; path: "/v2/sessions/{session_id}/queue/pop"; input: { path: { session_id: string; }; signal?: AbortSignal; }; responses: { "200": SessionQueueMutation; }; response: SessionQueueMutation; };
  "v2.sessions.redo": { method: "POST"; path: "/v2/sessions/{session_id}/redo"; input: { path: { session_id: string; }; body?: RevertRequest; signal?: AbortSignal; }; responses: { "200": Session; }; response: Session; };
  "v2.sessions.revert": { method: "POST"; path: "/v2/sessions/{session_id}/revert"; input: { path: { session_id: string; }; body: RevertRequest; signal?: AbortSignal; }; responses: { "200": Session; }; response: Session; };
  "v2.sessions.runtime": { method: "GET"; path: "/v2/sessions/{session_id}/runtime"; input: { path: { session_id: string; }; signal?: AbortSignal; }; responses: { "200": SessionRuntimeSnapshot; }; response: SessionRuntimeSnapshot; };
  "v2.sessions.shell": { method: "POST"; path: "/v2/sessions/{session_id}/shell"; input: { path: { session_id: string; }; body: SessionShellRequest; signal?: AbortSignal; }; responses: { "200": Message; }; response: Message; };
  "v2.sessions.status": { method: "GET"; path: "/v2/sessions/status"; input: { signal?: AbortSignal; }; responses: { "200": SessionStatusMap; }; response: SessionStatusMap; };
  "v2.sessions.summarize": { method: "POST"; path: "/v2/sessions/{session_id}/summarize"; input: { path: { session_id: string; }; body: EmptyObject; signal?: AbortSignal; }; responses: { "200": boolean; }; response: boolean; };
  "v2.sessions.todos": { method: "GET"; path: "/v2/sessions/{session_id}/todos"; input: { path: { session_id: string; }; signal?: AbortSignal; }; responses: { "200": Array<Todo>; }; response: Array<Todo>; };
  "v2.sessions.undo": { method: "POST"; path: "/v2/sessions/{session_id}/undo"; input: { path: { session_id: string; }; body?: RevertRequest; signal?: AbortSignal; }; responses: { "200": Session; }; response: Session; };
  "v2.sessions.undoTree": { method: "GET"; path: "/v2/sessions/{session_id}/undo-tree"; input: { path: { session_id: string; }; signal?: AbortSignal; }; responses: { "200": SessionUndoTree; }; response: SessionUndoTree; };
  "v2.sessions.unrevert": { method: "POST"; path: "/v2/sessions/{session_id}/unrevert"; input: { path: { session_id: string; }; signal?: AbortSignal; }; responses: { "200": Session; }; response: Session; };
  "v2.sessions.update": { method: "PATCH"; path: "/v2/sessions/{session_id}"; input: { path: { session_id: string; }; body: UpdateSessionRequest; signal?: AbortSignal; }; responses: { "200": Session; }; response: Session; };
  "v2.sessions.wait": { method: "POST"; path: "/v2/sessions/{session_id}/wait"; input: { path: { session_id: string; }; signal?: AbortSignal; }; responses: { "204": void; }; response: void; };
  "v2.skills.list": { method: "GET"; path: "/v2/skills"; input: { query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": SkillList; }; response: SkillList; };
  "v2.subagents.tasks.list": { method: "GET"; path: "/v2/plugins/dev.neoism.subagents/sessions/{session_id}/tasks"; input: { path: { session_id: string; }; signal?: AbortSignal; }; responses: { "200": Array<SubagentTask>; }; response: Array<SubagentTask>; };
  "v2.subagents.tasks.stop": { method: "POST"; path: "/v2/plugins/dev.neoism.subagents/sessions/{session_id}/stop"; input: { path: { session_id: string; }; body: StopSubagentsRequest; signal?: AbortSignal; }; responses: { "200": StopSubagentsResult; }; response: StopSubagentsResult; };
  "v2.tools.list": { method: "GET"; path: "/v2/tools"; input: { query?: { directory?: string; }; headers?: { "X-Neoism-Directory"?: string; }; signal?: AbortSignal; }; responses: { "200": ToolList; }; response: ToolList; };
}

export type OperationId = keyof ApiOperations;
export type OperationInput<Id extends OperationId> = ApiOperations[Id]["input"];
export type OperationResponse<Id extends OperationId> = ApiOperations[Id]["response"];
export type OperationResponses<Id extends OperationId> = ApiOperations[Id]["responses"];

export interface OperationDescriptor {
  readonly method: "GET" | "POST" | "PUT" | "PATCH" | "DELETE";
  readonly path: string;
  readonly transport: "http" | "sse" | "websocket";
  readonly requestMediaType?: string;
  readonly response?: "json" | "bytes" | "text";
  readonly responses: Readonly<Record<string, readonly string[]>>;
}

export const operationDescriptors = {
  "v2.agents.get": {"method":"GET","path":"/v2/agents/{name}","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.agents.list": {"method":"GET","path":"/v2/agents","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.artifacts.content": {"method":"GET","path":"/v2/artifacts/{artifact_id}/content","transport":"http","response":"bytes","responses":{"200":["application/octet-stream"]}},
  "v2.artifacts.create": {"method":"POST","path":"/v2/artifacts","transport":"http","requestMediaType":"application/octet-stream","response":"json","responses":{"201":["application/json"]}},
  "v2.artifacts.delete": {"method":"DELETE","path":"/v2/artifacts/{artifact_id}","transport":"http","responses":{"204":[]}},
  "v2.artifacts.get": {"method":"GET","path":"/v2/artifacts/{artifact_id}","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.artifacts.list": {"method":"GET","path":"/v2/artifacts","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.audit.list": {"method":"GET","path":"/v2/audit","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.capabilities.list": {"method":"GET","path":"/v2/capabilities","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.commands.list": {"method":"GET","path":"/v2/commands","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.config.get": {"method":"GET","path":"/v2/config","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.config.update": {"method":"PATCH","path":"/v2/config","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
  "v2.config.validate": {"method":"GET","path":"/v2/config/validate","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.events.subscribe": {"method":"GET","path":"/v2/events","transport":"sse","responses":{"200":["text/event-stream"]}},
  "v2.health": {"method":"GET","path":"/v2/health","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.interactions.permissions.list": {"method":"GET","path":"/v2/interactions/permissions","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.interactions.permissions.reply": {"method":"POST","path":"/v2/interactions/permissions/{request_id}/reply","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
  "v2.interactions.questions.list": {"method":"GET","path":"/v2/interactions/questions","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.interactions.questions.reject": {"method":"POST","path":"/v2/interactions/questions/{request_id}/reject","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.interactions.questions.reply": {"method":"POST","path":"/v2/interactions/questions/{request_id}/reply","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
  "v2.meta.get": {"method":"GET","path":"/v2/meta","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.openapi.get": {"method":"GET","path":"/v2/openapi.json","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.get": {"method":"GET","path":"/v2/plugins/{plugin_id}/manifest","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.goals.clear": {"method":"DELETE","path":"/v2/plugins/dev.neoism.goals/{session_id}","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.goals.get": {"method":"GET","path":"/v2/plugins/dev.neoism.goals/{session_id}","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.goals.research": {"method":"POST","path":"/v2/plugins/dev.neoism.goals/{session_id}/research","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.goals.set": {"method":"POST","path":"/v2/plugins/dev.neoism.goals/{session_id}","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.list": {"method":"GET","path":"/v2/plugins","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.lsp.codeActions": {"method":"GET","path":"/v2/plugins/dev.neoism.lsp/code-actions","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.lsp.definition": {"method":"GET","path":"/v2/plugins/dev.neoism.lsp/definition","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.lsp.diagnostics": {"method":"GET","path":"/v2/plugins/dev.neoism.lsp/diagnostics","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.lsp.documentHighlights": {"method":"GET","path":"/v2/plugins/dev.neoism.lsp/document-highlights","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.lsp.documentSymbols": {"method":"GET","path":"/v2/plugins/dev.neoism.lsp/document-symbols","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.lsp.formatting": {"method":"GET","path":"/v2/plugins/dev.neoism.lsp/formatting","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.lsp.hover": {"method":"GET","path":"/v2/plugins/dev.neoism.lsp/hover","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.lsp.implementation": {"method":"GET","path":"/v2/plugins/dev.neoism.lsp/implementation","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.lsp.incomingCalls": {"method":"GET","path":"/v2/plugins/dev.neoism.lsp/incoming-calls","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.lsp.inlayHints": {"method":"GET","path":"/v2/plugins/dev.neoism.lsp/inlay-hints","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.lsp.outgoingCalls": {"method":"GET","path":"/v2/plugins/dev.neoism.lsp/outgoing-calls","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.lsp.prepareCallHierarchy": {"method":"GET","path":"/v2/plugins/dev.neoism.lsp/prepare-call-hierarchy","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.lsp.references": {"method":"GET","path":"/v2/plugins/dev.neoism.lsp/references","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.lsp.shutdown": {"method":"POST","path":"/v2/plugins/dev.neoism.lsp/shutdown","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.lsp.signatureHelp": {"method":"GET","path":"/v2/plugins/dev.neoism.lsp/signature-help","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.lsp.status": {"method":"GET","path":"/v2/plugins/dev.neoism.lsp","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.lsp.touch": {"method":"POST","path":"/v2/plugins/dev.neoism.lsp/touch","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.mcp.add": {"method":"POST","path":"/v2/plugins/dev.neoism.mcp","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.mcp.auth.authenticate": {"method":"POST","path":"/v2/plugins/dev.neoism.mcp/{name}/auth/authenticate","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.mcp.auth.callback.get": {"method":"GET","path":"/v2/plugins/dev.neoism.mcp/{name}/auth/callback","transport":"http","response":"text","responses":{"200":["text/html"]}},
  "v2.plugins.mcp.auth.callback.post": {"method":"POST","path":"/v2/plugins/dev.neoism.mcp/{name}/auth/callback","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.mcp.auth.remove": {"method":"DELETE","path":"/v2/plugins/dev.neoism.mcp/{name}/auth","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.mcp.auth.start": {"method":"POST","path":"/v2/plugins/dev.neoism.mcp/{name}/auth","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.mcp.catalog": {"method":"GET","path":"/v2/plugins/dev.neoism.mcp/catalog","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.mcp.config": {"method":"PATCH","path":"/v2/plugins/dev.neoism.mcp/{name}/config","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.mcp.connect": {"method":"POST","path":"/v2/plugins/dev.neoism.mcp/{name}/connect","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.mcp.disconnect": {"method":"POST","path":"/v2/plugins/dev.neoism.mcp/{name}/disconnect","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.mcp.prompts": {"method":"GET","path":"/v2/plugins/dev.neoism.mcp/{name}/prompts","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.mcp.resources": {"method":"GET","path":"/v2/plugins/dev.neoism.mcp/{name}/resources","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.mcp.status": {"method":"GET","path":"/v2/plugins/dev.neoism.mcp","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.mcp.tools": {"method":"GET","path":"/v2/plugins/dev.neoism.mcp/{name}/tools","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.mcp.tools.call": {"method":"POST","path":"/v2/plugins/dev.neoism.mcp/{name}/tools/{tool_name}","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.pty.connect": {"method":"GET","path":"/v2/plugins/dev.neoism.pty/{pty_id}/connect","transport":"websocket","responses":{}},
  "v2.plugins.pty.connectToken": {"method":"POST","path":"/v2/plugins/dev.neoism.pty/{pty_id}/connect-token","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.pty.create": {"method":"POST","path":"/v2/plugins/dev.neoism.pty","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.pty.get": {"method":"GET","path":"/v2/plugins/dev.neoism.pty/{pty_id}","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.pty.list": {"method":"GET","path":"/v2/plugins/dev.neoism.pty","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.pty.remove": {"method":"DELETE","path":"/v2/plugins/dev.neoism.pty/{pty_id}","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.pty.shells": {"method":"GET","path":"/v2/plugins/dev.neoism.pty/shells","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.pty.update": {"method":"PUT","path":"/v2/plugins/dev.neoism.pty/{pty_id}","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.semantic.search": {"method":"GET","path":"/v2/plugins/dev.neoism.semantic/search","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.vcs.apply": {"method":"POST","path":"/v2/plugins/dev.neoism.vcs/apply","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.vcs.diff": {"method":"GET","path":"/v2/plugins/dev.neoism.vcs/diff","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.vcs.diff.raw": {"method":"GET","path":"/v2/plugins/dev.neoism.vcs/diff/raw","transport":"http","response":"text","responses":{"200":["text/x-diff"]}},
  "v2.plugins.vcs.get": {"method":"GET","path":"/v2/plugins/dev.neoism.vcs","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.vcs.status": {"method":"GET","path":"/v2/plugins/dev.neoism.vcs/status","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.workflows.activate": {"method":"POST","path":"/v2/plugins/dev.neoism.workflows/{workflow_id}/activate","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.workflows.get": {"method":"GET","path":"/v2/plugins/dev.neoism.workflows/{workflow_id}","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.workflows.history": {"method":"GET","path":"/v2/plugins/dev.neoism.workflows/{workflow_id}/runs","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.workflows.list": {"method":"GET","path":"/v2/plugins/dev.neoism.workflows","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.workflows.pause": {"method":"POST","path":"/v2/plugins/dev.neoism.workflows/{workflow_id}/pause","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.workflows.preview": {"method":"GET","path":"/v2/plugins/dev.neoism.workflows/{workflow_id}/preview","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.plugins.workflows.run": {"method":"POST","path":"/v2/plugins/dev.neoism.workflows/{workflow_id}/run","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.providers.auth.delete": {"method":"DELETE","path":"/v2/providers/{provider_id}/auth","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.providers.auth.get": {"method":"GET","path":"/v2/providers/{provider_id}/auth","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.providers.auth.set": {"method":"PUT","path":"/v2/providers/{provider_id}/auth","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
  "v2.providers.authMethods": {"method":"GET","path":"/v2/providers/auth-methods","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.providers.configured": {"method":"GET","path":"/v2/providers/configured","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.providers.list": {"method":"GET","path":"/v2/providers","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.providers.oauth.authorize": {"method":"POST","path":"/v2/providers/{provider_id}/oauth/authorize","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
  "v2.providers.oauth.callback": {"method":"POST","path":"/v2/providers/{provider_id}/oauth/callback","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.abort": {"method":"POST","path":"/v2/sessions/{session_id}/abort","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.children": {"method":"GET","path":"/v2/sessions/{session_id}/children","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.commands.execute": {"method":"POST","path":"/v2/sessions/{session_id}/commands","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.compact": {"method":"POST","path":"/v2/sessions/{session_id}/compact","transport":"http","responses":{"204":[]}},
  "v2.sessions.context": {"method":"GET","path":"/v2/sessions/{session_id}/context","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.create": {"method":"POST","path":"/v2/sessions","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.delete": {"method":"DELETE","path":"/v2/sessions/{session_id}","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.diff": {"method":"GET","path":"/v2/sessions/{session_id}/diff","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.directoryOptions": {"method":"GET","path":"/v2/sessions/{session_id}/directory-options","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.export": {"method":"POST","path":"/v2/sessions/export","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.fork": {"method":"POST","path":"/v2/sessions/{session_id}/fork","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.get": {"method":"GET","path":"/v2/sessions/{session_id}","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.import": {"method":"POST","path":"/v2/sessions/import","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.jobs.cancel": {"method":"DELETE","path":"/v2/sessions/{session_id}/jobs/{job_id}","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.list": {"method":"GET","path":"/v2/sessions","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.messages": {"method":"GET","path":"/v2/sessions/{session_id}/messages","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.messages.delete": {"method":"DELETE","path":"/v2/sessions/{session_id}/messages/{message_id}","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.messages.get": {"method":"GET","path":"/v2/sessions/{session_id}/messages/{message_id}","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.parts.delete": {"method":"DELETE","path":"/v2/sessions/{session_id}/messages/{message_id}/parts/{part_id}","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.parts.update": {"method":"PATCH","path":"/v2/sessions/{session_id}/messages/{message_id}/parts/{part_id}","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.pin": {"method":"POST","path":"/v2/sessions/{session_id}/pin","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.prompt": {"method":"POST","path":"/v2/sessions/{session_id}/prompt","transport":"http","requestMediaType":"application/json","responses":{"204":[]}},
  "v2.sessions.promptAsync": {"method":"POST","path":"/v2/sessions/{session_id}/prompt-async","transport":"http","requestMediaType":"application/json","responses":{"204":[]}},
  "v2.sessions.queue.clear": {"method":"DELETE","path":"/v2/sessions/{session_id}/queue","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.queue.list": {"method":"GET","path":"/v2/sessions/{session_id}/queue","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.queue.pop": {"method":"POST","path":"/v2/sessions/{session_id}/queue/pop","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.redo": {"method":"POST","path":"/v2/sessions/{session_id}/redo","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.revert": {"method":"POST","path":"/v2/sessions/{session_id}/revert","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.runtime": {"method":"GET","path":"/v2/sessions/{session_id}/runtime","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.shell": {"method":"POST","path":"/v2/sessions/{session_id}/shell","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.status": {"method":"GET","path":"/v2/sessions/status","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.summarize": {"method":"POST","path":"/v2/sessions/{session_id}/summarize","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.todos": {"method":"GET","path":"/v2/sessions/{session_id}/todos","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.undo": {"method":"POST","path":"/v2/sessions/{session_id}/undo","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.undoTree": {"method":"GET","path":"/v2/sessions/{session_id}/undo-tree","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.unrevert": {"method":"POST","path":"/v2/sessions/{session_id}/unrevert","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.update": {"method":"PATCH","path":"/v2/sessions/{session_id}","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
  "v2.sessions.wait": {"method":"POST","path":"/v2/sessions/{session_id}/wait","transport":"http","responses":{"204":[]}},
  "v2.skills.list": {"method":"GET","path":"/v2/skills","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.subagents.tasks.list": {"method":"GET","path":"/v2/plugins/dev.neoism.subagents/sessions/{session_id}/tasks","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "v2.subagents.tasks.stop": {"method":"POST","path":"/v2/plugins/dev.neoism.subagents/sessions/{session_id}/stop","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
  "v2.tools.list": {"method":"GET","path":"/v2/tools","transport":"http","response":"json","responses":{"200":["application/json"]}},
} as const satisfies Record<OperationId, OperationDescriptor>;

export function buildOperationRequest<Id extends OperationId>(
  id: Id,
  input: OperationInput<Id>,
): RequestDescriptor {
  const descriptor = operationDescriptors[id] as OperationDescriptor;
  const value = (input ?? {}) as { path?: Record<string, unknown>; query?: Record<string, unknown>; headers?: Record<string, unknown>; body?: unknown; signal?: AbortSignal };
  let path = descriptor.path;
  for (const [name, part] of Object.entries(value.path ?? {})) {
    path = path.replace(`{${name}}`, encodeURIComponent(String(part)));
  }
  if (/\{[^}]+\}/.test(path)) throw new TypeError(`missing path parameter for ${id}`);
  const headers = Object.fromEntries(Object.entries(value.headers ?? {}).filter(([, item]) => item !== undefined).map(([name, item]) => [name, String(item)]));
  if (descriptor.requestMediaType && value.body !== undefined) headers["content-type"] ??= descriptor.requestMediaType;
  return {
    method: descriptor.method,
    path,
    ...(value.query ? { query: value.query as NonNullable<RequestDescriptor["query"]> } : {}),
    ...(Object.keys(headers).length ? { headers } : {}),
    ...(value.body !== undefined ? { body: value.body } : {}),
    ...(descriptor.response ? { response: descriptor.response } : {}),
    ...(value.signal ? { signal: value.signal } : {}),
  };
}

export interface ContractClient {
  request<Id extends OperationId>(id: Id, input: OperationInput<Id>): Promise<OperationResponse<Id>>;
  descriptor<Id extends OperationId>(id: Id): (typeof operationDescriptors)[Id];
}

export function createContractClient(transport: NeoismTransport): ContractClient {
  return {
    request: (id, input) => transport.request(buildOperationRequest(id, input)),
    descriptor: (id) => operationDescriptors[id],
  };
}
