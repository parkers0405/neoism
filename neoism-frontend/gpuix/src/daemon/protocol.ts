import { Schema } from "effect";

// Authoritative typed messages live beside the Rust wire protocol.
export type { WorkspaceClientMessage, FilesClientMessage, AgentClientMessage, Attachment,
  SearchClientMessage, ConfigClientMessage, DiagnosticsClientMessage, GitClientMessage,
  CrdtServerMessage, CrdtCursorPosition, CrdtSelectionRange, CrdtPeerPresence,
  CrdtClientMessage, EditorClientMessage, PtyClientMessage as PtyMessage,
} from "../../../../neoism-protocol/typescript/types";
export type Family = "Workspace" | "Files" | "Agent" | "Pty" | "Search" | "Config" | "Editor" | "Diagnostics" | "Git" | "Crdt";
export interface WireEvent { family: Family; requestId: number; message: unknown }
export interface ConnectionState {
  phase: "connecting" | "authenticating" | "connected" | "waiting" | "auth-rejected" | "closed";
  attempt: number;
  reason?: string;
  connectedHostId?:string;
}

export const WorkspaceSchema = Schema.Struct({
  id: Schema.String, host_id: Schema.String, title: Schema.String,
  host_kind:Schema.optionalKey(Schema.String),
  visibility:Schema.optionalKey(Schema.String),
  running_on_host_id:Schema.optionalKey(Schema.NullOr(Schema.String)),
  root_dir: Schema.optionalKey(Schema.NullOr(Schema.String)),
  notes_vault_dir: Schema.optionalKey(Schema.NullOr(Schema.String)),
  linked_vault_dir: Schema.optionalKey(Schema.NullOr(Schema.String)),
  main_session_id: Schema.optionalKey(Schema.NullOr(Schema.String)),
  layout_snapshot: Schema.optionalKey(Schema.NullOr(Schema.String)),
});
export type Workspace = typeof WorkspaceSchema.Type;
export const HostSchema = Schema.Struct({ id: Schema.String, label: Schema.String });
export const TabSchema = Schema.Struct({
  id: Schema.String, workspace_id: Schema.String, title: Schema.String,
  kind: Schema.optionalKey(Schema.NullOr(Schema.String)),
  session_id: Schema.optionalKey(Schema.NullOr(Schema.String)),
  surface_id: Schema.optionalKey(Schema.NullOr(Schema.String)),
  cwd: Schema.optionalKey(Schema.NullOr(Schema.String)),
  active: Schema.optionalKey(Schema.Boolean),
});
export type WorkspaceTab = typeof TabSchema.Type;
export const TreeSchema = Schema.Struct({
  hosts: Schema.Array(HostSchema), workspaces: Schema.Array(WorkspaceSchema), tabs: Schema.Array(TabSchema),
});
export const DirectorySchema = Schema.Struct({
  path: Schema.String,
  entries: Schema.Array(Schema.Struct({ name: Schema.String, is_dir: Schema.Boolean, size: Schema.optionalKey(Schema.NullOr(Schema.Number)) })),
});
export type Directory = typeof DirectorySchema.Type;
export const FileSchema = Schema.Struct({ path: Schema.String, bytes: Schema.Array(Schema.Number) });
export const ThreadSchema = Schema.Struct({
  session_id: Schema.String, title: Schema.String,
  directory: Schema.optionalKey(Schema.NullOr(Schema.String)),
  agent: Schema.optionalKey(Schema.NullOr(Schema.String)),
  model: Schema.optionalKey(Schema.NullOr(Schema.String)),
  busy: Schema.optionalKey(Schema.Boolean),
});
export type Thread = typeof ThreadSchema.Type;
export const ModelSchema = Schema.Struct({ id: Schema.String, name: Schema.String, context_limit: Schema.optionalKey(Schema.NullOr(Schema.Number)) });
export const ProviderSchema = Schema.Struct({ id: Schema.String, name: Schema.String, models: Schema.optionalKey(Schema.Array(ModelSchema)) });
export type Provider = typeof ProviderSchema.Type;
export const AgentSchema = Schema.Struct({ name: Schema.String, description: Schema.String, mode: Schema.optionalKey(Schema.NullOr(Schema.String)) });
export type AgentInfo = typeof AgentSchema.Type;
export const SkillSchema = Schema.Struct({ name: Schema.String, description: Schema.String, path: Schema.optionalKey(Schema.NullOr(Schema.String)) });
export type SkillInfo = typeof SkillSchema.Type;
export const ProviderStateSchema = Schema.Struct({
  agent: Schema.optionalKey(Schema.NullOr(Schema.String)), model: Schema.optionalKey(Schema.NullOr(Schema.String)),
  provider_id: Schema.optionalKey(Schema.NullOr(Schema.String)), thinking: Schema.optionalKey(Schema.NullOr(Schema.String)),
  connection_id: Schema.optionalKey(Schema.NullOr(Schema.String)),
  context_limit: Schema.optionalKey(Schema.NullOr(Schema.Number)),
});
export type ProviderState = typeof ProviderStateSchema.Type;
export const TodoSchema = Schema.Struct({ status: Schema.String, content: Schema.String });
export type Todo = typeof TodoSchema.Type;
export const UsageSchema = Schema.Struct({
  input: Schema.Number, output: Schema.Number,
  reasoning: Schema.optionalKey(Schema.Number), cache_read: Schema.optionalKey(Schema.Number), cache_write: Schema.optionalKey(Schema.Number),
  cost_micros: Schema.optionalKey(Schema.Number),
  total: Schema.optionalKey(Schema.Number),
  context_limit: Schema.optionalKey(Schema.NullOr(Schema.Number)),
});
export type Usage = typeof UsageSchema.Type;
export const HistoryMessageSchema = Schema.Struct({
  id: Schema.String, role: Schema.String, kind: Schema.String,
  title: Schema.optionalKey(Schema.String), text: Schema.optionalKey(Schema.String),
  status: Schema.optionalKey(Schema.String), tool: Schema.optionalKey(Schema.String),
  detail: Schema.optionalKey(Schema.String), lang: Schema.optionalKey(Schema.String),
  todos: Schema.optionalKey(Schema.Array(TodoSchema)),
  usage: Schema.optionalKey(Schema.NullOr(UsageSchema)),
  author: Schema.optionalKey(Schema.NullOr(Schema.String)),
  line_offset: Schema.optionalKey(Schema.NullOr(Schema.Number)),
  attachments: Schema.optionalKey(Schema.Array(Schema.Struct({ filename: Schema.String, url: Schema.String, mime: Schema.String }))),
  created_at: Schema.optionalKey(Schema.Number),
  tool_batch_id: Schema.optionalKey(Schema.NullOr(Schema.String)),
  mcp: Schema.optionalKey(Schema.Unknown),
});
export type HistoryMessage = typeof HistoryMessageSchema.Type;

export const isRecord = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);
export function variant(value: unknown): [string, unknown] | undefined {
  if (typeof value === "string") return [value, {}];
  if (!isRecord(value) || Object.keys(value).length !== 1) return;
  return Object.entries(value)[0];
}
export function parseEnvelope(raw: string): WireEvent | undefined {
  const envelope = variant(JSON.parse(raw));
  if (!envelope) return;
  const [tag, body] = envelope;
  if (["PtyCreated", "PtyOutput", "PtyClosed", "SessionCwd", "SessionProgram", "TerminalDirectoryChanged", "TerminalDirectoryChangeFailed"].includes(tag) && isRecord(body))
    return { family: "Pty", requestId: 0, message: { [tag]: body } };
  const family = tag.replace(/Reply$/, "");
  if (!["Workspace", "Files", "Agent", "Pty", "Search", "Config", "Editor", "Diagnostics", "Git", "Crdt"].includes(family) || !isRecord(body)) return;
  if (typeof body.request_id !== "number" || !Number.isSafeInteger(body.request_id) || body.request_id < 0 || !("message" in body)) return;
  const inner = variant(body.message);
  const requestId = family === "Search" && inner && isRecord(inner[1]) && typeof inner[1].req_id === "number" ? inner[1].req_id : body.request_id;
  return { family: family as Family, requestId, message: body.message };
}

export function expectVariant(reply: unknown, name: string): unknown {
  const pair = variant(reply);
  if (pair?.[0] === name) return pair[1];
  if (pair?.[0] === "Error" && isRecord(pair[1]) && typeof pair[1].message === "string") throw new Error(pair[1].message);
  throw new Error(`Expected ${name} from daemon`);
}
