export interface ApiMeta {
  apiVersion: string;
  serverVersion: string;
  pluginApiVersion: string;
  eventSchemaVersion: string;
  partSchemaVersion: string;
  generation: number;
}

export interface AuditEntry {
  id: string;
  tenantId: string;
  method: string;
  path: string;
  status: number;
  created: number;
}

export interface CapabilityInfo {
  id: string;
  version: string;
  enabled: boolean;
  disableable: boolean;
  source: "core" | "internal-plugin" | "external-plugin" | string;
  pluginId?: string;
  apiPrefix?: string;
  reason?: string;
}

export interface PluginManifest {
  id: string;
  name: string;
  version: string;
  pluginApi: string;
  internal: boolean;
  enabled: boolean;
  active: boolean;
  disableable: boolean;
  capabilities: string[];
  requires: string[];
  eventNamespaces: string[];
  apiPrefix?: string;
  reason?: string;
  config: Record<string, unknown>;
}

export interface EventSubject {
  kind: string;
  id: string;
}

export interface EventEnvelope<T = unknown> {
  id: string;
  sequence: number;
  type: string;
  source: string;
  schemaVersion: string;
  timestamp: number;
  subject?: EventSubject;
  data: T;
}

export interface PartEnvelope<T = unknown> {
  id: string;
  kind: string;
  schemaVersion: string;
  data: T;
}

export interface ApiErrorBody {
  code: string;
  message: string;
  retryable: boolean;
  requestId?: string;
  details: Record<string, unknown>;
}

export interface Page<T> {
  items: T[];
  cursor: { next?: string; previous?: string };
}

export interface Session {
  id: string;
  title: string;
  directory?: string;
  parentID?: string;
  [key: string]: unknown;
}

export interface MessageWithParts {
  info: Record<string, unknown>;
  parts: unknown[];
}

export interface ArtifactInfo {
  id: string;
  filename: string;
  mediaType: string;
  size: number;
  sha256: string;
  created: number;
  sessionId?: string;
  downloadUrl: string;
}

export interface PermissionRequest {
  id: string;
  sessionId: string;
  messageId: string;
  title: string;
  permission: string;
  patterns: string[];
  always: string[];
  tool?: unknown;
  metadata?: unknown;
}

export interface QuestionRequest {
  id: string;
  sessionId: string;
  messageId: string;
  questions: unknown[];
}

export interface AgentInfo {
  name: string;
  description?: string;
  mode?: string;
  hidden?: boolean;
  color?: string;
  [key: string]: unknown;
}

export interface ToolInfo {
  id: string;
  description: string;
  parameters: unknown;
  outputSchema?: unknown;
}

export interface CommandInfo {
  name: string;
  description?: string;
  template?: string;
  agent?: string;
  model?: string;
  subtask?: boolean;
}

export type PromptPart =
  | { type: "text"; text: string }
  | { type: "file"; url: string; mime?: string; filename?: string }
  | { type: string; [key: string]: unknown };

export interface PromptRequest {
  prompt?: string;
  parts?: PromptPart[];
  delivery?: "steer" | "queue";
  messageID?: string;
  model?: { providerID: string; modelID: string; variant?: string };
  agent?: string;
  noReply?: boolean;
  system?: string;
  tools?: Record<string, boolean>;
  variant?: string;
}