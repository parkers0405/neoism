# Workspace runtime and host SDK

The SDK is provider-neutral. Your application owns accounts, authorization and VM billing. Neoism supplies a whole-agent workspace worker, an optional host broker, and ordinary typed agent clients. There are no vendor namespaces, local-filesystem launch helpers, SDK signing keys, or automatic infrastructure credentials.

Start with the authoritative [hosted architecture and implementation status](../../docs/hosted-control-plane.md), then the [workspace host construction and policy boundary](../../../neoism-cloud-host/README.md) and [cloud-runtime infrastructure contract](../../../neoism-cloud-runtime/README.md). One logical workspace hosts the whole Agent runtime, multiple agents and project directories on the same VM; session cwd, clones and Git worktrees do not allocate additional machines. The real [Docker development integration](../../../packaging/cloud-worker/DEVELOPMENT.md) is tested container provisioning with CPU/memory limits, explicitly unenforced disk budgets, and no production VM guarantee.

## Application connection: `HostWorkspaceClient`

```ts
import { HostWorkspaceClient, connectWorkspaceWorker } from "@neoism/sdk";

const host = new HostWorkspaceClient({
  endpoint: "https://app.example/gateway/",
  token: async signal => appAuth.getAccessToken({ signal }),
  // Defaults to the gateway origin. Explicitly trust direct worker origins only
  // when your deployment intentionally routes clients directly to workers.
  trustedWorkerOrigins: ["https://worker.example"],
});
const worker = await connectWorkspaceWorker({ host, workspace: "workspace:one" });
const runtime = await worker.runtime.get();
const capabilities = await worker.capabilities.list();
```

`token` is your external application's gateway bearer or callback, not a lifecycle bridge secret or a model-provider account token. The host's injected policy binds the authenticated actor to the requested workspace and tenant, resource spec, role, scopes and quotas. The SDK sends neither a tenant nor user-selected scopes/roles/spec in request bodies. The current canonical host API accepts `{}` for ensure/connection, and `{expected_handle}` for start/stop/destroy. SDK methods are `status(workspace, signal?)`, `ensure(workspace, signal?)`, `connection(workspace, signal?)`, and `start/stop/destroy(workspace, expectedHandle, signal?)`. Authorization for optional administrative lifecycle operations is enforced by your host policy, not by the SDK.

Requests use endpoint-relative `/v1/workspaces/{workspace}/runtime/{ensure,status,start,stop,destroy,connection}`. Status is GET; the other methods are POST. Responses use generated `HostStatus`, `Verification` and `ConnectionGrant` types from `neoism-cloud-host`'s canonical OpenAPI, not an independently handwritten SDK protocol. Status includes the allocation and optional ephemeral verification. A status read alone is not proof of verified readiness. The connection broker rechecks readiness and issues a short-lived grant with `version: 1`, `baseUrl`, `handle`, `root`, `runtimeId`, `workerGeneration`, `bearer`, `expiresAt` and runtime capabilities. A grant contains only the worker credential; never private signing material.

`connectWorkspaceWorker` reads the current binding, rejects retired/pending/unready/expired workers, obtains a broker grant, correlates its exact handle, generation, runtime, namespace root and endpoint with the allocation, then creates a normal `NeoismClient`. Initial `/v2/runtime` introspection must identify the expected workspace-worker with worker contract `version: 1`, tenant, workspace, runtime, generation, root and allocation lease expiry. Root is a VM namespace; it is never resolved on the client machine. Broker/introspection roots use the service API namespace rules: POSIX roots compare exactly, while Windows drive roots compare with ASCII case folding, slash normalization and canonical `\\\\?\\` verbatim-prefix normalization. UNC/device paths, volume roots, traversal and invalid Windows components are rejected; different drives or different root components never compare equal. The original wire roots are not rewritten. Launch descriptors retain the runtime's stricter canonical path spelling. No ensure/start is performed implicitly.

The helper caches grants privately until shortly before expiry. Refresh is singleflight per fixed connection, keyed by the exact owned handle, full endpoint (including origin/path), runtime identity and immutable launch descriptor. The expiry skew is configurable from 0 to 30 seconds, reduced proportionally for short-lived grants to avoid refresh storms. HTTP and SSE resolve authentication at request/connect time. If a broker returns a replacement machine, runtime, descriptor or endpoint—even on the same trusted origin—the old client fails closed and requires an explicit new `connectWorkspaceWorker` call. It never sends an old bearer to a new target or silently keeps a new bearer paired with an old base URL. Cancelling one caller stops its wait without cancelling other users of a shared broker refresh; broker operations remain deadline-bounded. Initial worker introspection is also deadline-bounded (`connectionTimeoutMs`, default 30 seconds).

The helper supports HTTP and SSE. It rejects WebSocket setup rather than putting a bearer in a URL or relying on ambient cookies. A separately configured product gateway may provide its own explicit WebSocket authentication flow.

## Infrastructure host: `CloudRuntimeClient`

This separate **host-only** client talks to the generic `neoism-cloud-runtime` lifecycle bridge, not the agent API or your application's host broker. Never construct it with browser application credentials, worker bearers or model-provider tokens.

```ts
import { CloudRuntimeClient, type Allocation } from "@neoism/sdk";

const allocation: Allocation = {
  owner: { tenant: "tenant:org.example", workspace: "workspace:one" },
  generation: 1,
  provider: "bridge",
  spec: { image: "worker_v1", region: "east", vcpus: 2, memory_mib: 2048, disk_gib: 20 },
  // Optional immutable public WorkerLaunchDescriptor supplied by the trusted host:
  // launch: { version: 1, runtime_id, root, state_root, expires_at, verification_key },
};
const cloud = new CloudRuntimeClient({
  endpoint: "https://lifecycle.example/bridge/",
  provider: "bridge",
  bearer: hostCredentialFromSecretStore,
  capabilities: {
    isolation: "virtual_machine", stop_start: true,
    cpu_limit: true, memory_limit: true, disk_limit: true, durable_workspace: true,
  },
});
const status = await cloud.ensure(allocation);
await cloud.inspect(allocation); // Handle is optional, matching Rust RuntimeProvider.
await cloud.inspect(allocation, status.handle);
// Start/stop/destroy require the exact authoritative allocation and handle.
```

Every operation sends one POST to endpoint-relative `v2/runtime/{ensure,start,inspect,stop,destroy}` with JSON `{version: 2, allocation, handle}`. There are **no v1 aliases**. Ensure sends `handle: null`; inspect may omit a known handle. A 200/202 response must have a version-2 envelope and a status matching tenant/workspace/provider/generation and any expected machine identity. Production capability admission requires virtual-machine isolation, stop/start, CPU/memory/disk limits and durable workspace. Development containers require explicit `developmentContainers: true` and truthful `disk_limit: false`; they are never represented as production VM parity.

The optional launch descriptor contains only `{version: 1, runtime_id, root, state_root, expires_at, verification_key}`. `verification_key` is the canonical unpadded base64url encoding of 32 public-key bytes. Private seeds/signers are rejected as unknown fields. Generation belongs to the allocation, not the descriptor. VM paths must be normalized non-volume-root POSIX paths or uppercase-drive Windows paths with forward slashes; workspace/state roots must not overlap. Validation does not access local files or canonicalize against the host filesystem. Input identity and descriptors are snapshotted before asynchronous I/O. Expired descriptors are rejected for ensure/start; inspect/stop/destroy remain allowed for recovery and cleanup, matching Rust's lifecycle contract.

Your authoritative host registry supplies allocations and generations. The SDK does not mint generations, keep a durable registry, or orchestrate billing. Rust u64 generations must fit JavaScript positive safe integers. Readiness and power state remain distinct. An identity-checked tombstone, not a 404, proves owned destruction.

## Transport security and failure semantics

All three HTTP paths—application host, lifecycle bridge and regular agent HTTP/SSE—use `redirect: "manual"` and `credentials: "omit"`. Redirects and wrong-origin responses fail closed. Agent descriptors cannot point at another origin or escape their configured base path. Host and lifecycle URLs reject embedded credentials, query tokens and fragments. Host/bridge HTTPS is mandatory unless `allowPlaintext: true` is explicitly selected for development; worker origins must be explicitly trusted (gateway origin by default). Injected fetch implementations must honor redirect/credential options: rejecting an already-redirected response cannot undo leakage caused by a custom fetch that ignored them.

Host/bridge responses and streamed reads are bounded to 64 KiB with total deadlines (default 30 seconds; range 1–600,000 ms), including injected implementations that ignore abort. `CloudRuntimeError` exposes only `code` and `retryable`, with no raw response, endpoint, credential or cause. There are no automatic mutation retries, even for 401 or ambiguous timeout. Reconcile with the authoritative registry before deciding to repeat a mutation; never invent a generation as a retry strategy.

For manually configured agent clients, `HttpTransportOptions.token` accepts a string or callback resolved per HTTP request and per SSE reconnect. Callback failures are sanitized, terminal authentication errors; no mutation is replayed. Explicit authorization headers override a token callback case-insensitively. The broker helper does not accept those overrides, keeping its identity cache authoritative. SSE reconnects retain cursor and event-ID deduplication. The native WebSocket API cannot set authorization headers; regular transports never append callback bearers to WebSocket URLs.

See [`examples/cloud-workspace.ts`](examples/cloud-workspace.ts). Generated files are maintained by the parent canonical OpenAPI generation workflow; SDK-owned tests are `scripts/cloud.test.mjs`, `scripts/http-token.test.mjs` and `scripts/host.test.mjs`.
