// Generated from the authoritative canonical Neoism Cloud Host OpenAPI document.
// Run neoism-agent/scripts/cloud-host.sh update. Do not edit by hand.

import type { NeoismTransport, RequestDescriptor } from "../transport.js";

export type Allocation = { generation: number; launch?: (WorkerLaunchDescriptor) | (null); owner: WorkspaceKey; provider: string; spec: WorkspaceSpec; };
export type Binding = { allocation: Allocation; last_error?: (ProviderError) | (null); pending?: (Intent) | (null); retired: boolean; revision: number; status?: (MachineStatus) | (null); };
export type Capabilities = { cpu_limit: boolean; disk_limit: boolean; durable_workspace: boolean; isolation: IsolationKind; memory_limit: boolean; stop_start: boolean; };
export type ConnectionGrant = { baseUrl: string; bearer: string; capabilities: Capabilities; expiresAt: number; handle: MachineHandle; root: string; runtimeId: string; version: 1; workerGeneration: number; };
export type EmptyRequest = Record<string, unknown>;
export type FailureCode = "transport" | "timeout" | "unauthorized" | "not_found" | "conflict" | "rejected" | "unavailable" | "protocol" | "identity";
export type HostApiError = { code: "denied" | "invalid" | "conflict" | "expired" | "unready" | "unavailable"; };
export type HostStatus = { binding: Binding; verification: (Verification) | (null); };
export type Intent = "ensure" | "start" | "stop" | "destroy";
export type IsolationKind = "virtual_machine" | "container";
export type LifecycleRequest = { expected_handle: MachineHandle; };
export type MachineHandle = { generation: number; machine_id: string; owner: WorkspaceKey; provider: string; };
export type MachineState = "provisioning" | "starting" | "running" | "stopping" | "stopped" | "failed" | "destroyed";
export type MachineStatus = { connection?: (WorkerConnection) | (null); failure?: (ProviderError) | (null); handle: MachineHandle; ready: boolean; state: MachineState; } & ((unknown) & (unknown) & (unknown));
export type ProtocolRequest = { allocation: Allocation; handle?: (MachineHandle) | (null); version: 2; };
export type ProtocolResponse = { status: MachineStatus; version: 2; };
export type ProviderError = { code: FailureCode; retryable: boolean; };
export type Verification = { at: number; descriptor: WorkerLaunchDescriptor; expires_at: number; handle: MachineHandle; revision: number; };
export type WorkerConnection = { agent_api_base_url: string; handle: MachineHandle; transport: WorkerTransport; version: 1; } & ((unknown) & (unknown));
export type WorkerLaunchDescriptor = { expires_at: number; root: string; runtime_id: string; state_root: string; verification_key: string; version: 1; };
export type WorkerTransport = "https" | "development_loopback_http";
export type WorkspaceKey = { tenant: string; workspace: string; };
export type WorkspaceSpec = { disk_gib: number; image: string; memory_mib: number; region: string; vcpus: number; };

export interface ApiOperations {
  "host.runtime.connection": { method: "POST"; path: "/v1/workspaces/{workspace}/runtime/connection"; input: { path: { workspace: string; }; body: EmptyRequest; signal?: AbortSignal; }; responses: { "200": ConnectionGrant; }; response: ConnectionGrant; };
  "host.runtime.destroy": { method: "POST"; path: "/v1/workspaces/{workspace}/runtime/destroy"; input: { path: { workspace: string; }; body: LifecycleRequest; signal?: AbortSignal; }; responses: { "200": HostStatus; }; response: HostStatus; };
  "host.runtime.ensure": { method: "POST"; path: "/v1/workspaces/{workspace}/runtime/ensure"; input: { path: { workspace: string; }; body: EmptyRequest; signal?: AbortSignal; }; responses: { "200": HostStatus; }; response: HostStatus; };
  "host.runtime.start": { method: "POST"; path: "/v1/workspaces/{workspace}/runtime/start"; input: { path: { workspace: string; }; body: LifecycleRequest; signal?: AbortSignal; }; responses: { "200": HostStatus; }; response: HostStatus; };
  "host.runtime.status": { method: "GET"; path: "/v1/workspaces/{workspace}/runtime/status"; input: { path: { workspace: string; }; signal?: AbortSignal; }; responses: { "200": HostStatus; }; response: HostStatus; };
  "host.runtime.stop": { method: "POST"; path: "/v1/workspaces/{workspace}/runtime/stop"; input: { path: { workspace: string; }; body: LifecycleRequest; signal?: AbortSignal; }; responses: { "200": HostStatus; }; response: HostStatus; };
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
  "host.runtime.connection": {"method":"POST","path":"/v1/workspaces/{workspace}/runtime/connection","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
  "host.runtime.destroy": {"method":"POST","path":"/v1/workspaces/{workspace}/runtime/destroy","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
  "host.runtime.ensure": {"method":"POST","path":"/v1/workspaces/{workspace}/runtime/ensure","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
  "host.runtime.start": {"method":"POST","path":"/v1/workspaces/{workspace}/runtime/start","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
  "host.runtime.status": {"method":"GET","path":"/v1/workspaces/{workspace}/runtime/status","transport":"http","response":"json","responses":{"200":["application/json"]}},
  "host.runtime.stop": {"method":"POST","path":"/v1/workspaces/{workspace}/runtime/stop","transport":"http","requestMediaType":"application/json","response":"json","responses":{"200":["application/json"]}},
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
