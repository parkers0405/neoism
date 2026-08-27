import type {
  Agent,
  ApiError,
  ApiMeta as ContractApiMeta,
  Artifact,
  AuditEntry as ContractAuditEntry,
  Capability,
  Command,
  Event as ContractEvent,
  EventEnvelope as ContractEventEnvelope,
  Message,
  PageCursor,
  PartEnvelope as ContractPartEnvelope,
  PermissionRequest as ContractPermissionRequest,
  PluginManifest as ContractPluginManifest,
  PromptPart as ContractPromptPart,
  PromptRequest as ContractPromptRequest,
  QuestionRequest as ContractQuestionRequest,
  Session as ContractSession,
  Tool,
} from "./generated/contract.js";

export type ApiMeta = ContractApiMeta;
export type AuditEntry = ContractAuditEntry;
export type CapabilityInfo = Capability;
export type PluginManifest = ContractPluginManifest;
export type ApiErrorBody = ApiError;
export type Session = ContractSession;
export type MessageWithParts = Message;
export type {
  Part,
  TextPart,
  CompactionPart,
  AgentPart,
  SubtaskPart,
  ReasoningPart,
  ToolPart,
  ToolState,
  ToolStatePending,
  ToolStateRunning,
  ToolStateCompleted,
  ToolStateError,
  StepStartPart,
  StepFinishPart,
  FilePart,
  PartTime,
  TokenUsage,
  CacheUsage,
} from "./generated/contract.js";
export type ArtifactInfo = Artifact;
export type PermissionRequest = ContractPermissionRequest;
export type QuestionRequest = ContractQuestionRequest;
export type AgentInfo = Agent;
export type ToolInfo = Tool;
export type CommandInfo = Command;
export type PromptPart = ContractPromptPart;
export type PromptRequest = ContractPromptRequest;

export type EventEnvelope<T = unknown> = Omit<ContractEventEnvelope, "data"> & { data: T };
/// Every SSE record, as the typed union discriminated by `type` — narrow with
/// a `switch (event.type)` and `data` narrows with it.
export type Event = ContractEvent;
export type PartEnvelope<T = unknown> = Omit<ContractPartEnvelope, "data"> & { data: T };
export interface Page<T> {
  items: T[];
  cursor: PageCursor;
}