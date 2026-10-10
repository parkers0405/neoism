# neoism-cloud-runtime

Provider-neutral, whole-VM-per-workspace lifecycle library. This crate does not depend on the daemon/server or worker startup types and does not invent a vendor API. `HttpProvider` implements the generic protocol below; an actual infrastructure bridge must implement it. There is no production fake provider, local-machine fallback, Docker dependency, or implicit endpoint.

## Registration and usage

The parent workspace registers:

```toml
# [workspace] members entry
"neoism-cloud-runtime"

# [workspace.dependencies]
neoism-cloud-runtime = { path = "neoism-cloud-runtime", version = "0.7.124" }

# consuming crate's [dependencies]
neoism-cloud-runtime.workspace = true
```

Uses the existing workspace dependencies: serde, serde_json, thiserror, tokio, reqwest, url, sha2, uuid; axum supplies the reusable server; base64 validates public keys. Requires the workspace Rust version (at least 1.89 for standard-library file locks). Crate version/edition/license/rust-version inherit from the parent.

```rust,no_run
use neoism_cloud_runtime::{Capabilities, IsolationKind, HttpProvider, Runtime, WorkspaceKey, WorkspaceSpec};
use std::{sync::Arc, time::Duration};

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let provider = HttpProvider::new(
    "my-bridge",
    "https://trusted-bridge.example/runtime-bridge/",
    &std::env::var("RUNTIME_BRIDGE_TOKEN")?,
    Duration::from_secs(60),
    Capabilities { isolation: IsolationKind::VirtualMachine, cpu_limit: true, memory_limit: true, disk_limit: true, durable_workspace: true, stop_start: true },
)?;
let runtime = Runtime::new("/var/lib/neoism/runtime-registry", Arc::new(provider))?;
let owner = WorkspaceKey::new("tenant-123", "workspace-456")?;
let spec = WorkspaceSpec {
    image: "ubuntu-2404".into(), region: "us-east".into(),
    vcpus: 2, memory_mib: 4096, disk_gib: 40,
};
let mut binding = runtime.ensure(&owner, &spec).await?;
if binding.pending.is_none() {
    if let Some(status) = &binding.status {
        if status.state == neoism_cloud_runtime::MachineState::Stopped {
            binding = runtime.start(&status.handle).await?;
        }
    }
}
// In production reconcile with a bounded deadline/backoff until ready or failure.
if binding.worker_connection().is_none() {
    binding = runtime.reconcile(&owner).await?;
}
if let Some(connection) = binding.worker_connection() {
    let runtime_identity = connection.handle();
    let api_base_url = connection.agent_api_base_url();
    // Pass these to the authorized host router. The external broker obtains a
    // short-lived credential scoped to this identity/origin; never persist it here.
    let _ = (runtime_identity, api_base_url);
}
# Ok(()) }
```

Exports: `Runtime`, `RuntimeProvider`, `ProviderFuture`, `HttpProvider`, `Capabilities`, `WorkspaceKey`, `WorkspaceSpec`, `Allocation`, `MachineHandle`, `MachineStatus`, `MachineState`, `WorkerConnection`, `WorkerTransport`, `Binding`, `Intent`, `ProtocolRequest`, `ProtocolResponse`, `RuntimeAction`, `canonical_openapi`, `Error`, `Result`, `ProviderError`, `FailureCode`.

`Runtime`: `new`, `binding` (local snapshot), `ensure`, `reconcile`, `start`, `stop`, `destroy`. The provider trait offers `id`, `capabilities`, boxed async `ensure/start/inspect/stop/destroy`. The synchronous filesystem work is small and bounded but can block on disk; hosts requiring strict executor latency should isolate the lifecycle coordinator on a dedicated executor/thread. Locks are nonblocking, not asynchronous wait queues.

## Ownership, recovery, and state

- Keys include both tenant and workspace; owner IDs are 1–128 ASCII alphanumeric, underscore, hyphen, colon, or dot characters, preserving existing daemon tenants such as `workspace:daemon.id-123`. No path separators, whitespace, or control characters are allowed. Provider/machine/image/region catalog IDs retain the stricter 1–128 ASCII alphanumeric/underscore/hyphen alphabet. There are no unbounded environment variables, labels, shell commands, URLs, bootstrap payloads, or credential fields in the spec. Catalog IDs are not native image URL strings. Resource sizes have explicit bounds. Treat all catalog IDs as non-secret.
- One current binding is stored per key. Its spec is immutable. A different spec/provider returns `Conflict` until destruction is **confirmed**, at which point ensure allocates the next generation. Tombstones are retained; generations and record revisions never reset. Revision increases on each committed intent/result, including inspection, and is not a VM generation.
- Start/stop/destroy require the entire current handle, including owner, provider, generation, and machine ID. Foreign/stale handles return `Fenced` before any provider call. Handles are fencing tokens, not authentication credentials: the host must authenticate its user, authorize the tenant/workspace, and protect registry access.
- The first provisioning intent is committed **before** ensure. Lifecycle intents are committed before provider calls. Lost replies, cancellation, and restart retain the intent; `reconcile` replays it, otherwise inspects. Ensure on an existing allocation also reconciles its existing intent, so it can return a retirement if an earlier destroy was pending. Call ensure again after confirmed retirement to allocate a replacement. Ensure does not implicitly start a stopped VM.
- Provider call failures persist `last_error`, keep the last known status unchanged, and retain the pending intent. They return `Error::Provider`; `binding` retrieves the persisted failure. `retryable` is advice for the host, not an automatic retry loop. A provider-reported failed machine is instead a successful `Binding` with `state: failed` and a structured `failure`. Hosts must check both state and readiness.
- `ready` may be true only for `running`, and then requires a validated `WorkerConnection`. Running/unready may have connection metadata; every other state must have no connection. VM power state is not worker readiness. The bridge determines actual worker readiness; bootstrap/authentication belongs in a separate integration, not the serialized spec.
- `binding` is an owned, potentially stale snapshot, not a live lease. `Binding::worker_connection()` returns routing metadata only for a validated ready status with no pending intent, no last error, and no retirement. A previous ready endpoint may remain in the last-known status after a failed call, but this accessor suppresses it. A snapshot already held by another caller cannot be revoked by cancellation or an OS lock: the broker and worker must fence actual requests against the exact handle/generation.
- Workspace IDs are canonical opaque, case-sensitive IDs assigned by the host. The library does not normalize aliases or infer a tenant from user-supplied paths. The registry clones immutable allocation input before issuing provider calls; key/spec changes require explicit confirmed replacement.
- Transitional ensure/start/stop/destroy results retain their intent until completion. Start completes at running (which may still be unready) or failed; stop completes at stopped or failed. A known provisioning/transitioning machine can be explicitly destroyed, superseding an earlier intent. Destruction completes **only** on an identity-validated `destroyed` status. Not-found inspection or transport failure never retires a binding or enables replacement.

## Worker routing and bootstrap ownership

`WorkerConnection` is versioned nonsecret endpoint metadata, not an authentication grant. Wire fields are `version: 1`, `handle` (the complete runtime identity), `agent_api_base_url`, and `transport` (`https` or `development_loopback_http`). Its handle must exactly equal the enclosing machine status handle; a different tenant, workspace, generation, provider, or machine ID is rejected. Private fields and read-only accessors keep constructed connections immutable. Deserialized DTOs still require validation before use.

`WorkerConnection::new` accepts HTTPS only. `new_development_loopback` explicitly permits HTTP on numeric loopback IPs only (`127.0.0.0/8` or `::1`); it does not resolve `localhost`, accept private-LAN HTTP, or supply a fallback host. Endpoints are at most 2048 bytes, canonical absolute URLs with a trailing slash. Credentials, queries, fragments, raw whitespace/control characters, and backslashes are rejected. Constructor input is canonicalized; noncanonical wire URLs are rejected. This validates syntax/scope, not infrastructure ownership: **only a trusted provider bridge may choose/verify the endpoint**, and the host must apply its network/origin policy before connecting. User tools must never select arbitrary endpoint URLs. Provider diagnostic messages are still sanitized; the nonsecret endpoint itself is intentionally persisted and visible in Debug/status.

A host bootstrap controller owns canonical workspace identity, the allocation generation, runtime handle, and workspace root. It installs/boots the worker through a trusted native integration; it is not an agent tool accepting arbitrary tenant, generation, root, endpoint, or credentials. Worker root/bootstrap types remain outside this crate. An external auth broker returns a short-lived credential scoped to the validated handle and intended API origin. No bearer key is stored in `WorkerConnection`, `MachineStatus`, `Binding`, the registry, the protocol schema, or serialized bootstrap inputs.

Host integration flow (the controller/broker are host-owned APIs, not APIs invented by this crate):

```text
1. Authenticate actor; authorize canonical (tenant, workspace) from host records.
2. runtime.ensure(canonical_key, immutable_host_spec)
3. Hand binding.allocation + validated machine handle to the bootstrap controller.
   Controller chooses root from trusted workspace records and pins generation/handle.
   Controller boots worker using protected out-of-band configuration, not tool fields.
4. runtime.start(current_handle); poll runtime.reconcile(canonical_key) with deadline.
5. Reject retired/pending/error/failed/unready results.
6. connection = binding.worker_connection(); require Some.
7. Broker authorizes actor + connection.handle() + intended origin; obtains ephemeral auth.
8. Route to connection.agent_api_base_url() with that externally held credential.
   Worker checks canonical workspace identity, runtime handle and generation on requests.
9. On superseded generation, discard the route/credential and re-reconcile; never reuse
   an old snapshot merely because its previous status had ready=true.
```

## Durable registry contract

Use a private, durable directory shared by every coordinator of these workspaces, on a filesystem supporting OS exclusive locks and atomic same-directory rename. Do not use an untrusted directory, NFS, or multiple independent registry directories for the same owner namespace. The host controls directory permissions; newly created files are mode 0600 on Unix. Symlink attacks by an actor with registry write access are outside the trust model.

Each key uses a SHA-256 filename derived from a length-prefixed tenant plus workspace. A stable `.lock` file is exclusively locked across read, intent commit, provider call, and result commit; contention returns `Busy`. Locks release on process exit/cancellation. Lock files must not be removed while coordinators are running. Records are versioned, strictly parsed, size-limited to 64 KiB, and identity/spec validated; corrupt or unsupported data is an error, never treated as a missing workspace.

Writes use a fresh same-directory temporary file, flush and sync its contents, **close its handle**, atomically rename it over the record, and sync the directory on Unix. `std::fs::rename` replaces existing files on Unix and Windows; there is deliberately no delete-before-rename fallback. On unsupported filesystems or sharing violations, replacement fails safely rather than creating a missing-record window. Windows uses synced files and replacement rename but does not promise Unix directory-fsync crash guarantees; platform/filesystem power-loss behavior must be validated by the deployment. Windows-specific execution has not been verified in this crate's Linux test run. A write/fsync error may have committed; callers must reload/reconcile, not assume rollback. Crash-left `*.tmp` files are ignored and may be removed offline. Losing or deleting the registry destroys the local generation ledger: restore it from backup before provisioning again. No unsafe automatic registry repair, garbage collection, or tombstone deletion is performed.

## HTTP bridge protocol v2

`HttpProvider::new` requires an explicit HTTPS base URL, provider ID, bearer token, bounded nonzero timeout (at most 600 seconds), and configured capabilities. `new_plaintext` explicitly opts into HTTP for trusted private/test bridges. Neither supplies a localhost default. URL userinfo, query, and fragment are rejected. Redirects and implicit proxy discovery are disabled so credentials cannot be redirected or silently sent through a proxy. HTTP credentials use a sensitive Authorization header; the provider has no Debug implementation. Response bodies, HTTP errors, URLs, and credentials never appear in public errors/status. The host must not log the token passed to the constructor.

All actions use `POST <base>/v2/runtime/{ensure|start|inspect|stop|destroy}`, JSON, `Authorization: Bearer <token>`. Base paths are retained. Inspect is logically read-only despite POST.

Request:

```json
{
  "version": 2,
  "allocation": {
    "owner": { "tenant": "tenant-123", "workspace": "workspace-456" },
    "generation": 1,
    "provider": "my-bridge",
    "spec": { "image": "ubuntu-2404", "region": "us-east", "vcpus": 2, "memory_mib": 4096, "disk_gib": 40 },
    "launch": null
  },
  "handle": null
}
```

`ensure` sends null handle; `inspect` accepts null to recover a lost ensure response; mutations require the complete handle. Bridge responses must be HTTP 200 or 202 with the following JSON body (202 is not an empty acceptance receipt):

```json
{
  "version": 2,
  "status": {
    "handle": {
      "owner": { "tenant": "tenant-123", "workspace": "workspace-456" },
      "generation": 1, "provider": "my-bridge", "machine_id": "vm-abc123"
    },
    "state": "running",
    "ready": true,
    "connection": {
      "version": 1,
      "handle": {
        "owner": { "tenant": "tenant-123", "workspace": "workspace-456" },
        "generation": 1, "provider": "my-bridge", "machine_id": "vm-abc123"
      },
      "agent_api_base_url": "https://verified-worker.example/agent/",
      "transport": "https"
    },
    "failure": null
  }
}
```

States: `provisioning`, `starting`, `running`, `stopping`, `stopped`, `failed`, `destroyed`. Failed status requires `failure: { "code": "unavailable", "retryable": true }`; other states require null failure. Failure codes: `transport`, `timeout`, `unauthorized`, `not_found`, `conflict`, `rejected`, `unavailable`, `protocol`, `identity`. Unknown protocol versions, malformed/oversized bodies, unknown fields, inconsistent readiness/failure, and foreign identities are rejected. Response bodies are capped at 64 KiB, including chunked bodies; the client timeout covers body consumption.

HTTP errors: 401/403 → unauthorized; 404 → not_found; 409/412 → conflict; 408/504 → retryable timeout; 429/other 5xx → retryable unavailable; remaining statuses (including redirects/204) → rejected. Transport errors are retryable and sanitized. No response error body is exposed. An already-destroyed machine must return a 200 identity-checked destroyed tombstone, **not** a bare 404: absent resource is not sufficient evidence of authorized destruction.

### Bridge responsibilities (mandatory)

1. Authenticate the coordinator and authorize its tenant namespace on every action. Check allocation owner, provider, generation, immutable spec, and handle machine identity **before** native provider operations.
2. Maintain a durable allocation/idempotency ledger keyed by `(provider, tenant, workspace, generation)`. Repeated ensure, even after ambiguous timeouts, returns the same machine; never create two machines. Returning an existing machine with a changed ID is a protocol violation.
3. Keep a highest-generation ledger/tombstones per workspace. Reject stale generations, including delayed ensure/start after destruction. Higher generations are allowed only after confirmed prior destruction; enforce at most one active machine across generations. The HTTP client cannot manufacture these guarantees for a bridge that does not implement them.
4. Make start/stop/destroy idempotent. Report transitional states truthfully. Destroy returns destroyed only once the native provider confirms removal; repeated destroy returns the same owned tombstone. Cancellation of an HTTP request is not cancellation of the remote operation.
5. Translate native IDs to the bounded machine ID space if necessary and maintain that mapping durably. Determine actual VM and worker readiness. Return a provider-verified, nonsecret `WorkerConnection` for ready workers, scoped to the exact handle. Keep credentials, native provider diagnostic messages, and bootstrap tokens out of public status. Unknown machines cannot safely be attributed to an owner or destroyed by guesswork; absent-resource handling requires the bridge's authoritative ownership ledger/tombstone, not provider assurances alone.

Native integrations can later implement `RuntimeProvider` directly, or serve this protocol. This crate intentionally does not pretend the generic bridge is a specific cloud vendor endpoint.

## Canonical Rust-owned protocol schema

`canonical_openapi() -> serde_json::Value` is the authoritative OpenAPI 3.1 source for the generic bridge; SDK DTOs and snapshots should be generated from it, not copied by hand. Export without launching a server:

```sh
cargo run --locked --example openapi -p neoism-cloud-runtime
```

Operation IDs are `cloud.runtime.ensure`, `cloud.runtime.start`, `cloud.runtime.inspect`, `cloud.runtime.stop`, and `cloud.runtime.destroy`; all are POST under `/v2/runtime/`. The request body derives its wire shape from the owned public `ProtocolRequest { version, allocation, handle }`; responses use `ProtocolResponse { version, status }`. `HttpProvider` serializes/deserializes these exact DTOs. `ProtocolRequest::new/validate(RuntimeAction)` requires null/absent handle for ensure an optional matching handle for inspect, and a matching complete handle for other mutations. Deserialization is structural: bridges must call validation before any native operation. Bridge version is exactly 2 (worker connection and launch contracts remain 1), generations are positive, and authentication is modeled only as a header bearer security scheme, never a body field or secret-bearing error schema.

Component names (SDK may expose them with a `Cloud` prefix): `ProtocolRequest`, `ProtocolResponse`, `Allocation`, `WorkspaceKey`, `WorkspaceSpec`, `MachineHandle`, `MachineStatus`, `MachineState`, `WorkerConnection`, `WorkerTransport`, `ProviderError`, `FailureCode`, `Capabilities`, `IsolationKind`, `WorkerLaunchDescriptor`, `Binding`, `Intent`. There is no schemars dependency in the parent manifest/lockfile, so this uses the authorized hand-schema fallback with no schema-generation dependency. Tests compare actual DTO serde field names, required/optional fields, unknown-field handling, and enum names to these schemas; they also check operation-specific handle requirements, version/generation bounds, resolved references, security, and deterministic generation. Cross-object identity equality, canonical URL parsing/trust, authorization, and durable fencing remain runtime checks, not claims made by JSON Schema.

Compatibility note: an old ready status without connection metadata is no longer valid. Fail closed and rehydrate through the trusted bridge/controller; do not invent an endpoint or silently repair registry generations. Registry tombstone and last-known state validation remains strict.

## Verification

```sh
cargo test --locked -p neoism-cloud-runtime
cargo check --locked -p neoism-cloud-runtime --all-targets
```

Tests use an in-memory contract-enforcing adapter only under `tests/`, real local HTTP servers, and a raw chunked HTTP response. They exercise durable lost-reply recovery, restart/idempotency, lock contention, cancelled ensure and stop futures, persisted pending snapshots, stale endpoint suppression, corrupt records, daemon namespaced ownership, tenant/machine identity fencing, replacement generations/revisions, tombstones/late ensure/start, lifecycle failure replay, connection readiness/identity/URL validation, and schema/serde parity. HTTP boundary tests cover all five routes, base path retention, redirects (including an untouched auth-leak trap), auth failures, malformed JSON, mismatched identities, protocol versions, unknown fields, known-length and chunked size limits, correct error status/retryability, and timeout/error redaction. The runtime crate does not ship a vendor-specific production provider. A real `DockerDevelopmentProvider` is implemented in the separate [workspace host crate](../neoism-cloud-host/README.md), with fake-CLI contract tests and a passed real manager/Docker end-to-end run; see [development wiring and integration instructions](../packaging/cloud-worker/DEVELOPMENT.md). It enforces CPU/memory controls but not disk budgets and is explicitly container isolation, not a production VM provider. No Boat-specific implementation ships, and no vendor is required by the core. No workspace-wide formatting or release build is needed.

## Worker launch initialization and server API

`Allocation.launch: Option<WorkerLaunchDescriptor>` is serialized as null for intentional machine-lifecycle-only use. Worker managers must require Some and must never auto-repair a machine-only allocation. `Runtime::ensure_with_initializer(&key, &spec, &initializer)` calls `AllocationInitializer::initialize(&AllocationSeed) -> ProviderFuture<WorkerLaunchDescriptor>` only for new generations under the workspace OS lock, before persistence and provider effects. The seed contains public owner, provider, generation and immutable spec. Initializers must not reenter Runtime locks. External private seed stores must be idempotent by owner/provider/generation: cancellation before persistence can repeat the initializer with the same seed. Once committed, the exact descriptor is replayed without regeneration, even after a lost provider reply.

Construct the immutable descriptor with `WorkerLaunchDescriptor::new(runtime_id, root, state_root, expires_at: i64, verification_key)`. Accessors expose each field; generation comes only from Allocation. Contract version is 1. Runtime IDs are bounded opaque identifiers. Verification keys are canonical unpadded base64url of exactly 32 public bytes. VM paths are normalized POSIX absolute or uppercase Windows drive-root with forward slashes, never host-canonicalized. Traversal, volume roots and overlapping state/workspace paths are rejected. No seeds, tokens, environment or commands belong in the descriptor. Structural validation allows expired records to remain readable for destruction; active ensure/start fail with `ExpiredLaunch` until confirmed destroy allows a new generation. Reconcile may inspect expired pending ensure/start, but never replays those mutations; expired stop/destroy intents remain replayable. Existing machine-only records produce `MissingLaunch` with an initializer.

`Capabilities { isolation: IsolationKind, stop_start, cpu_limit, memory_limit, disk_limit, durable_workspace }` reports actual enforcement. `Runtime::new` requires VirtualMachine isolation and all budget/durability guarantees. Explicit `Runtime::new_development` permits Container with CPU/memory enforcement and durable workspace but without a disk quota. There is no process-local isolation category.

`provider_router(Arc<dyn RuntimeProvider>, Arc<dyn BridgeAuthorizer>) -> axum::Router` registers only the five `/v2/runtime/*` routes, with no v1 aliases. `BridgeAuthorizer::authorize(bearer, owner, action) -> ProviderFuture<()>` must authorize the exact owner and operation; there is no privileged fallback. The router rejects absent/duplicate auth, limits JSON to 64 KiB, validates DTOs and responses, bounds authorization and provider calls to 30 seconds each, and never automatically retries mutations. Error responses are status codes only, not raw provider errors. Native providers still own durable ownership/generation fencing and idempotency. Parent packaging owns generation of the v2 OpenAPI snapshot and hash; this crate only exports `canonical_openapi` and the stdout example.

`reconcile` and `recover_handle` retain the existing binding revision and skip the atomic registry write when the full result binding is unchanged after validation. Revisions advance for actual health/routing changes, pending-intent completion, and changes or clears of uncertainty—not merely for observing the same state. This preserves whole-binding authorization CAS across independent connection probes.

### Recovering an expired allocation after a lost create reply

`Runtime::recover_handle(&WorkspaceKey) -> Result<Binding>` performs only `RuntimeProvider::inspect(&allocation, optional_known_handle)` under the workspace lock, including when the descriptor has expired. It validates the returned owner/provider/generation and any known machine handle before saving the discovered status. It preserves the immutable allocation/descriptor, pending intent, and prior uncertainty; it never recreates compute or renews worker authority. A failed or foreign inspection leaves the registry unchanged. NotFound or an inspection tombstone does not retire the allocation. The caller can use the discovered owned handle with `destroy`, wait for confirmed destruction, then initialize a fresh generation. Expired reconcile follows this inspection-only path for pending ensure/start and allows pending destroy to complete normally. Bridge request validation allows expired descriptors for inspect/stop/destroy only.
