# Neoism workspace host

`neoism-cloud-host` is a provider-neutral controller for one whole Neoism Agent server per workspace. It wraps the durable, generation-fenced `neoism-cloud-runtime::Runtime`; it does not run the agent in the controller process and has no service-server dependency. The generic v2 bridge is usable with a real provider implementation behind `provider_router`. Docker development provisioning is a separate adapter, not a claim of VM isolation.

## Construction and authority

`WorkspaceHost::new(registry_directory, provider, signing_store, launch_policy, endpoint_policy)` requires a VM provider with CPU, memory and disk limits and durable workspace storage. `new_development` explicitly admits a container provider with CPU/memory limits and durable storage but no disk-limit guarantee. Grants report the capabilities of the actual provider used to construct Runtime; callers cannot supply separate optimistic grant capabilities.

The signing store is controller-only. `FileSigningKeyStore::new` requires an absolute private directory with an existing trusted parent. On Unix it creates the directory with mode 0700, validates effective ownership, rejects symlink ancestry and opens regular singly-linked 0600 files without following final symlinks. Its per-identity file lock serializes creation. A seed record is atomically renamed and fsynced (including its directory) before initialization can return; failed/cancelled provider creation never regenerates the key. Corrupt records fail closed rather than being overwritten. Never put this directory in an image context, workspace, worker state mount or a VM. Do not delete seed records during routine lifecycle operations. Windows hosts must inject an ACL-secured secret-manager implementation of `SigningKeyStore`; the filesystem implementation deliberately refuses unsupported permission enforcement rather than silently claiming private storage.

`SigningIdentity` hashes length-delimited provider/tenant/workspace plus generation, so identifiers containing colons never become filenames. `SigningAuthority` is not serializable and Debug is redacted. An external secret manager must implement atomic, durable `load_or_create` and deterministic `load`, including the original creation timestamp. It should also implement compare-and-swap `renew_uncommitted` for expired abandoned preparations (the default fails closed). The timestamp anchors descriptor expiry on precommit replay. If this reservation expires before allocation commit, Runtime's new-allocation initializer, while holding the workspace lock, is proof that no provider side effects could have happened: only this path may replace the abandoned authority with a fresh seed and creation timestamp. The old seed is not reused or silently extended. Once an allocation is committed, Runtime never invokes its initializer again, so committed descriptors/authorities remain immutable and expired workers require confirmed destroy/new generation. Corrupt registry data must fail rather than being treated as a clean absence; loss or manual deletion of the registry is outside this recovery guarantee and requires external recovery/fencing, not initializer-based renewal. Signing seeds are never part of allocations, bridge DTOs, responses, events or VM bootstrap.

The allocation initializer runs under Runtime's workspace lock and only talks to the key store; it never reenters Runtime. Its bounded runtime ID is derived from immutable generation identity. The public descriptor carries contract version 1, runtime ID, normalized VM root and state root, expiry and public verification key. Generation is inherited from Allocation, not duplicated in the descriptor. Root and state root are validated lexically in the VM namespace, without resolving them on the controller filesystem. State must be outside workspace root. No customer-controlled commands or mutable environment are accepted as launch configuration.

The provider installs the stock worker using this descriptor: `WorkspaceWorkerBootstrap { version, tenant_id, workspace_id, root, runtime_id, runtime_generation, expires_at }`, plus the public verification key, and keeps credential/state files under the independent state root. The provider must isolate the whole agent server, enforce its declared limits, own generation/idempotency tombstones, and install the same immutable bootstrap on retries. Private controller signing authority is never injected into the worker.

## API and lifecycle

- `ensure_worker(key, spec)` allocates only once, preserves the saved public descriptor on retries, and waits for authenticated readiness. An expired committed allocation is recovered through inspect-only `Runtime::recover_handle`, then its owned compute is destroyed and confirmed retired before a fresh generation/authority is allocated. Allocation, handle, intent and revision guards reject concurrent binding changes; unknown, absent or foreign inspection fails closed. Already-pending destroy intents are resumed rather than treating inspection absence as proof of retirement. Recovery, destruction, replacement and authenticated readiness share one bounded deadline. It does not implicitly start an unexpired stopped machine.
- `status(key)` reads durable binding state with `verification: null`. Provider `status.ready` is only a hint, never a verified route.
- `reconcile(key)` replays Runtime's durable intent or inspects, returning an unverified snapshot. No extra multi-stage journal exists.
- `start(handle)` starts the exact owned generation and waits for readiness; `stop(handle)` and `destroy(handle)` delegate fenced lifecycle operations and return unverified snapshots.
- `replace_expired(expected_handle, spec)` is an explicit guarded operation: it refuses an active lease, recovers a missing saved handle through inspect-only Runtime recovery, validates the exact caller-supplied owner/provider/generation/machine handle, confirms retirement through provider destruction before allocating the next generation, and generates a fresh signer/bootstrap. It never extends an expired identity. Transitional destruction is polled within the shared launch deadline; cancellation or uncertain replies leave the durable destroy intent for the next call. The host never interprets missing/failed inspection as confirmed destruction.
- `connect(key, host_approved_access)` refuses retired, pending, uncertain or expired bindings, inspects the current machine, verifies its authenticated identity, issues a narrowed token, and rechecks binding revision/handle/descriptor immediately before returning a grant. Caller authorization must already bind actor and workspace.

`durable_workspace` is a provider contract that destroy removes compute while preserving the logical workspace AND state for the next generation. Without this guarantee Runtime/host admission fails. Stale lifecycle handles are fenced. Old credentials cannot authenticate to a new generation's worker. A grant is a short-lived capability, not a revocable controller-side routing lease: stop/destroy enforcement also depends on the provider actually disabling the old compute/endpoint. Clients must request a fresh connection instead of caching routes.

Ensure/start readiness deadlines encompass initial provider calls and all polling. Connect is also deadline-bounded. Dropping a future cancels polling and releases Runtime's file lock; committed intents remain available to the next call. No non-idempotent mutation is automatically retried outside Runtime's provider contract. A verification snapshot is ephemeral and linked to the exact allocation, handle and revision; no bearer or verification strings are persisted. Verification is not a durable readiness claim and can become stale after return.

## Readiness and connection safety

The trusted endpoint policy runs before sending credentials to the exact provider URL. `AllowedOrigins` is an exact scheme/host/port allowlist; deployments can implement `TrustedEndpointPolicy` to enforce stricter network and binding admission. Production connections require HTTPS with normal certificate validation; numeric loopback HTTP is available only through the development transport. Host network egress controls and trusted DNS are still required: an origin allowlist is not protection against a compromised resolver or a provider lying about its machine binding. Redirects and environment proxies are disabled. Each HTTP probe has a short timeout and a streamed 16 KiB response cap and requires successful JSON, not an HTML health page.

A short `agent:read` credential authenticates `GET /v2/runtime`. The response must have deployment `workspace-worker`, live `executionAvailable`, and exact worker version 1, tenant, workspace, runtime ID, allocation generation, equivalent VM root and immutable expiry. Root equivalence requires shared `worker_vm_path_contains` checks in both directions: alternate Windows drive/verbatim spellings are accepted, but child roots and other drives are not. Launch configuration uses the shared VM validator and is converted to the bridge's canonical POSIX or uppercase-drive/forward-slash spelling without consulting the controller filesystem. Grants consistently carry that declared canonical root. Optional expected image/package version is checked through `/v2/health` (`healthy` and `version`). Provider `ready: false` may still expose a candidate Running endpoint for this probe; provider `ready: true` never bypasses it. Returned grants use the service API's filesystem-independent bound credential issuer, restrict prefixes to the VM root, cap TTL to 300 seconds and the worker lease, and redact bearer values in Debug. Serialization is explicitly for the authenticated host response; never log the serialized grant.

## Optional HTTP host boundary

`host_router(Arc<WorkspaceHost>, Arc<dyn HostPolicy>)` serves:

| Route | Method | Request | Response |
|---|---|---|---|
| `/v1/workspaces/{workspace}/runtime/ensure` | POST | `{}` | `HostStatus` |
| `/v1/workspaces/{workspace}/runtime/status` | GET | none | `HostStatus` |
| `/v1/workspaces/{workspace}/runtime/start` | POST | `LifecycleRequest` | `HostStatus` |
| `/v1/workspaces/{workspace}/runtime/stop` | POST | `LifecycleRequest` | `HostStatus` |
| `/v1/workspaces/{workspace}/runtime/destroy` | POST | `LifecycleRequest` | `HostStatus` |
| `/v1/workspaces/{workspace}/runtime/connection` | POST | `{}` | `ConnectionGrant` |

`LifecycleRequest` requires `expected_handle`. The policy receives the opaque Authorization bearer, URL workspace and explicit `HostAction`. It must authenticate the actor, bind the tenant/workspace, approve the immutable spec and authorize the particular verb, returning a `TrustedApproval`. The API independently verifies the approval's workspace and the lifecycle handle's owner before Runtime fences generation/machine ID. No tenant, spec, scopes, signing key or endpoint is accepted from a connection/ensure body. Unknown fields and malformed bodies are rejected with sanitized errors and 16 KiB body bounds. Duplicate Authorization headers and whitespace-bearing tokens are refused. Grants and error responses use `Cache-Control: no-store`. Accounts, billing and production authentication belong to the embedding application.

Reference wiring (deny-all is intentional; replace with the application's actual authentication/authorization policy before serving traffic):

```rust,no_run
use std::{sync::Arc, time::Duration};
use neoism_cloud_host::{
    runtime::{Capabilities, HttpProvider, IsolationKind},
    AllowedOrigins, DenyAllPolicy, FileSigningKeyStore, LaunchPolicy,
    WorkspaceHost, host_router,
};

fn build_host(
    bridge_url: &str,
    bridge_credential: &str,
    trusted_worker_origins: Vec<String>,
) -> Result<axum::Router, Box<dyn std::error::Error>> {
    let provider = Arc::new(HttpProvider::new(
        "vm-bridge", bridge_url, bridge_credential, Duration::from_secs(10),
        Capabilities {
            isolation: IsolationKind::VirtualMachine,
            cpu_limit: true, memory_limit: true, disk_limit: true,
            stop_start: true, durable_workspace: true,
        },
    )?);
    let signer = Arc::new(FileSigningKeyStore::new("/var/lib/neoism-controller/signers")?);
    let host = Arc::new(WorkspaceHost::new(
        "/var/lib/neoism-controller/registry", provider, signer,
        LaunchPolicy {
            root: "/workspace".into(), state_root: "/worker-state".into(),
            lease_seconds: 86400,
            readiness_deadline: Duration::from_secs(60),
            probe_timeout: Duration::from_secs(2),
            expected_image_version: None,
        },
        Arc::new(AllowedOrigins::new(trusted_worker_origins)?),
    )?);
    Ok(host_router(host, Arc::new(DenyAllPolicy)))
}
```

A production `HostPolicy` implementation must reject unauthorized lifecycle roles, not merely check that a bearer exists. The included deny-all policy is a reference for secure composition, not a fallback login mechanism. The bridge URL and worker origin list must come from trusted deployment configuration, never user request fields.

## Wire contract and verification

`canonical_openapi()` is the authoritative host contract. It imports bridge/runtime schemas from their owning crate and defines `HostStatus`, `Verification`, `ConnectionGrant`, `EmptyRequest`, `LifecycleRequest` and `HostApiError` in one place. Generate SDK inputs with `cargo run -p neoism-cloud-host --example openapi`; no handwritten TypeScript copy is maintained here. `ConnectionGrant` is camelCase; Runtime bindings and `HostStatus`/`Verification` use the exact snake_case fields shown in the schema.

Run `cargo check -p neoism-cloud-host` and `cargo test -p neoism-cloud-host`. The [lifecycle tests](tests/lifecycle.rs) use a fake idempotent VM provider and real HTTP worker endpoints that verify Ed25519-signed credentials and exact runtime metadata; the fake is confined to tests and is not a production provisioning backend. They cover restart/key/descriptor reuse, cancellation after provider side effects, stop/start/destroy, expired fenced replacement, strict capabilities, policy admission, redacted Debug, wrong identity fields, HTML, redirects, oversized responses and timeouts. The [Docker tests](tests/docker.rs) exercise the real development adapter with a fake CLI. The opt-in [stock-worker manager/Docker end-to-end test](tests/workspace_end_to_end.rs) has passed against the actual development image, including repository/worktree creation, multiple sessions, restart, persistence and generation replacement. See [development provider construction and the integration command](../packaging/cloud-worker/DEVELOPMENT.md); this is not production VM or gateway certification.
