# Hosted control plane and isolated workspace workers

## Current architecture and implementation status

This is the authoritative reading map and status for hosted Agent execution. The same Agent engine runs locally or as a whole cloud workspace worker; a production deployment assigns **one isolated VM per logical workspace**, not one VM per agent, conversation or repository. Multiple agents, project folders, session working directories and Git worktrees share that computer. Cloning another repository stays inside the same VM. The workspace's declared directory remains authoritative; changing a session/terminal cwd does not redefine it. This is not a claim of a separate project-registry feature.

```text
Application actor / pipeline / SDK
  -> application policy + workspace host / connection broker
  -> provider-neutral whole-workspace infrastructure lifecycle
  -> one workspace VM: whole Neoism Agent runtime
       -> multiple agents, sessions, project folders and Git worktrees
```

| Read next | Responsibility and current status |
|---|---|
| [Cloud runtime](../../neoism-cloud-runtime/README.md) | Completed v2 `/v2/runtime/...` bridge/client/server, durable generation-fenced registry, recovery and immutable public launch descriptor; infrastructure-only authority. |
| [Workspace host](../../neoism-cloud-host/README.md) | Completed `WorkspaceHost` manager, controller-only signers, authenticated readiness and connection broker, and policy-injected `/v1/workspaces/...` HTTP API. |
| [Cloud SDK](../sdk/typescript/CLOUD.md) | Completed application `HostWorkspaceClient` / `connectWorkspaceWorker` with bounded credential refresh, plus separate infrastructure-only `CloudRuntimeClient`. |
| [Worker packaging](../../packaging/cloud-worker/README.md) | Stock Agent worker startup, public-key binding, authentication and health checks; no GUI or daemon required. |
| [Development provider and full integration test](../../packaging/cloud-worker/DEVELOPMENT.md) | Real `DockerDevelopmentProvider`, development image and opt-in [manager/Docker end-to-end test](../../neoism-cloud-host/tests/workspace_end_to_end.rs). |

**Recorded verification:** Rust cloud-runtime: 33 tests passed; host: 18 tests passed; Docker adapter: 8 fake-CLI tests passed; 1 actual manager/Docker end-to-end test passed. Service API suites passed with 42 + 3 + 4 tests, and SDK typecheck and full tests passed. These are results from the implementation verification, not a claim that every command runs Docker by default or that a production provider is security-certified. The real container run covers bootstrap, signed admission, repository/worktree creation, multiple sessions, controller restart, stop/resume, persistent data and replacement generations. The portable release image/artifact combination remains separately unvalidated.

Docker development is an explicit opt-in **container** backend with enforced CPU/memory controls and durable workspace/state volumes; disk budgets are **not enforced**. It is not Firecracker or a production VM guarantee. No vendor is required by the core and no Boat-specific implementation ships. A production application's integration hook is a `RuntimeProvider` or the v2 HTTP infrastructure bridge: it must enforce advertised VM isolation, limits and durable data, with validation for that provider. Ordinary users do not need to reimplement the generic bootstrap, signer, lifecycle, readiness broker or SDK credential-refresh machinery.

The application/operator still owns accounts, workspace membership and private-workspace authorization policy, subscriptions, VM billing, cloud/infrastructure and model/connector keys, production gateway/TLS, networking, and backups. Private workspaces are application policy, not a new OS isolation primitive. API scopes, directory admission and shared-data guards do not isolate same-user processes or agents sharing the worker. Guest user filesystems are never implicitly mounted. Private 32-byte signing seeds stay controller-only; only the 32-byte public verifier and public startup identity reach the worker. Authenticated `/v2/runtime` metadata reports the worker contract version, root and runtime identity for broker verification. Full editor/daemon cloud hosting and browser/desktop integration are optional later work, not prerequisites for an initial Agent application.

## Architecture

Hosted execution runs **one isolated VM per workspace**, containing the whole standalone Neoism Agent runtime. File edits, shell commands, Git, PTY, LSP, stdio MCP and process plugins are ordinary local operations inside that VM when their capabilities and prerequisites are installed. Agent permissions still apply. Tools neither provision machines nor dispatch individual operations to a cloud execution adapter. There is no per-tool cloud sandbox and no execution fallback on the application host.

The application owns accounts, membership, SSO, billing, entitlements, project authorization and gateway routing. The [workspace host](../../neoism-cloud-host/README.md) provides reusable controller signing and connection brokerage over the provider-neutral [`neoism-cloud-runtime`](../../neoism-cloud-runtime/README.md) library, managing whole-machine lifecycle **outside Agent tools**. The runtime crate owns the lifecycle API, durable binding/generation ledger, recovery and infrastructure-bridge contract; do not implement a second lifecycle protocol in the Agent API. The real Docker reference adapter is available for development; production deployments supply a native `RuntimeProvider` or compatible bridge for their chosen infrastructure.

The Agent owns sessions, canonical messages, actor control leases, participants, tool permissions and execution state. A shared hosted control-plane process remains **execution-disabled**. It may expose tenant-aware metadata, session control and artifacts through injected services, but it must not run native processes or silently redirect work to a local host. Execution belongs on the current workspace worker. An optional workspace daemon can be added later for collaboration/editor/terminal hosting; it is not required for the standalone Agent worker and is not part of this packaging.

The generic host/controller flow, stock worker and real Docker development integration have shipped in this implementation and passed the verification summarized above. That does not claim a Boat deployment, turnkey support for all vendors, a production application gateway or production security certification.

## Startup binding

The controller provisions workspace storage and an isolated VM, then supplies two read-only files outside the working tree. Start the stock runtime with:

```sh
neoism-agent serve --hostname 0.0.0.0 --port 4096 \
  --worker-bootstrap /run/neoism-worker/bootstrap.json \
  --worker-verification-key-file /run/neoism-worker/verification.key
```

`--worker-bootstrap` and `--worker-verification-key-file` require each other. The environment alternatives are `NEOISM_AGENT_WORKER_BOOTSTRAP` and `NEOISM_AGENT_WORKER_VERIFICATION_KEY_FILE`; supply both. These inputs are startup-only controller configuration, not `/config` values, model arguments or user-selected paths. Do not enable unauthenticated remote access, trusted-loopback bypasses or local/operator credentials as an alternate admission path.

`WorkspaceWorkerBootstrap` in `neoism-agent-service-api` has the following JSON contract (unknown fields are rejected):

| Field | Meaning |
|---|---|
| `version` | Exactly `1` |
| `tenantId` | Nonblank application tenant ID |
| `workspaceId` | Nonblank workspace ID |
| `runtimeId` | Nonblank current runtime ID |
| `runtimeGeneration` | Positive current generation; controller-owned fencing identity |
| `root` | Existing absolute canonical directory, not filesystem root |
| `expiresAt` | Future Unix seconds computed by the controller |

Use `/workspace` as the workspace root for the packaged image. `/run/neoism-worker/verification.key` contains exactly 32 raw Ed25519 public-key bytes. The controller creates `WorkspaceWorkerSigningKey` from a private seed of exactly 32 bytes and derives the verifier with `.verification_key().as_bytes()`. Generate a fresh signing seed per runtime generation and keep it exclusively in the application's controller account/secret store: never send it to the VM, container, workspace, runtime state or SDK clients. The public verifier and bootstrap are not secrets; their read-only placement outside the working tree protects immutable controller identity. Whole-VM containment and network/gateway fencing remain deployment responsibilities. `WorkspaceWorkerBinding::new(profile, verification_key)` accepts the public `WorkspaceWorkerVerificationKey`, not controller signing state.

Binding validation checks identity, generation, expiry and canonical root at startup and admission. It does not create OS isolation: the controller must establish that before launching the Agent. Restore/resume must preserve the correct workspace/state ownership; replacement must fence old routing and credentials. Changing the startup file does not change an in-memory binding. Reprovision/restart deliberately when rotating startup identity or key.

## Worker credentials

The controller signs Ed25519 `WorkspaceWorkerCredentialClaims` using `WorkspaceWorkerSigningKey::new(private_seed)?.issue(&claims)`. The seed is exactly 32 bytes and never leaves the controller. Use the service API rather than hand-encoding the wire token. Claims include `version: 1`, tenant/workspace/runtime IDs, runtime generation, subject, `actorType`, an absolute canonical `directoryPrefix`, scopes, quotas, `issuedAt` and `expiresAt`. `ActorType` is `Human` or `ServiceAccount` in Rust (`human` or `serviceAccount` in JSON).

`MAX_WORKSPACE_WORKER_TOKEN_TTL_SECS` is **300 seconds**. A token must not outlive its bootstrap. The worker verifies signature, matching tenant/workspace/runtime/generation, valid issuance/expiry, canonical existing directory containment and the resolved identity. Scopes and quotas come from application authorization, not model input. Directory strings alone never establish tenant identity. There is no desktop, daemon or local credential fallback at this boundary.

Worker operation scopes are enforced before dispatch: `agent:read` permits ordinary read endpoints, `agent:use` also permits conversation/task mutations, and `workspace:admin` permits workspace configuration and scoped connector management. Missing or unknown scopes grant no operations. Configuration cannot change the immutable worker binding. These scopes are API authorization, not filesystem isolation between agents sharing a computer. State/secret-path guards are application admission checks, not OS isolation: all agents in the workspace share its computer, and same-user processes are not isolated from runtime files by these checks. The controller private signing seed is never present on that computer.

See the [packaging README](../../packaging/cloud-worker/README.md#issue-credentials-in-the-controller) for a concrete Rust issuance function, generated bootstrap example, mount permissions and exact build/run commands. Accounts, billing, entitlements, human/service-account lifecycle and token renewal remain application concerns.

## SDK and actor control

Route the client through the application gateway's TLS endpoint for the authorized workspace and current generation. Do not expose the worker's unauthenticated plain HTTP listener publicly. Use short-lived worker credentials and refresh the client/transport before expiry, including streaming connections.

```ts
import { createHttpClient } from "@neoism/sdk";

const agent = createHttpClient({
  baseUrl: workspaceGatewayUrl, // Application-authorized TLS route.
  token: shortLivedWorkerToken, // Issued for this workspace/runtime generation.
});
const session = await agent.sessions.create({ title: "Workspace task" });
const current = await agent.sessions.control(session.id);
const lease = await agent.sessions.claimControl(session.id, {
  expectedRevision: current?.revision ?? 0,
  leaseSeconds: 60,
});
// Prompt/observe this session using the SDK's session and event APIs.
await agent.sessions.releaseControl(session.id, lease.revision);
```

A human and a service account can operate the same tenant-owned session. Taking control uses the latest lease revision; it does not clone the transcript or transfer ownership. Session, interaction, event, artifact, audit and credential access remain scoped to the authenticated identity and current worker binding. Capability discovery describes what the deployed runtime actually supports, not every feature that a developer workstation happens to have installed.

## Shared hosted control plane

For a shared tenant-aware service, embed `neoism-agent-server` and inject a `TenantResolver`, tenant-scoped provider and MCP credential stores and a shared `ArtifactBlobStore`, then select `for_hosted_control_plane()`. Validate the required service boundaries at startup. This profile is execution-disabled: do not inject a tool execution adapter or promise native process capabilities from the shared process. An authenticated application request must be authorized and routed to its separately provisioned workspace worker for execution.

The resolver is authoritative for tenant, subject, actor, scopes and quotas. Keep local desktop configuration and credentials out of shared tenant resolution. Artifact metadata remains Agent-owned; shared blob payload storage must enforce tenant authorization independently of object-key naming. Use private tenant-qualified object keys, retention policy and quotas. Worker-local persistence and application-wide shared artifact access are different deployments; the thin image does not automatically upload blobs or replicate sessions to the shared control plane.

Tenant-owned workflow persistence/recovery and other shared-mode capability boundaries must remain fail-closed where unsupported. The workspace worker uses installed local capabilities within its isolated VM; that does not make those capabilities safe in the shared process.

## Packaging, health and deployment responsibilities

The [worker package](../../packaging/cloud-worker/README.md) accepts prebuilt native Agent/Lua runner artifacts. It contains no GUI, daemon or renderer. It launches `serve` without `--web`, sets explicit XDG configuration/state/cache/data paths and provider-auth path under private persistent `/var/lib/neoism`, and uses writable `/workspace` plus a read-only container root. Resource limits, nonroot execution, dropped capabilities and no Docker socket are defense in depth; the dedicated VM remains the execution isolation boundary.

`GET /v2/health` does not require authentication. Readiness requires `healthy: true`, `deployment: "workspace-worker"`, `executionAvailable: true`, an unexpired binding and deployment checks for expected identity/generation, storage and required services. A live HTTP process, or VM `running` state, is not sufficient. Server health is expiry-aware; the packaged check also verifies bootstrap expiry as defense in depth. The server becomes unready and exits when its immutable binding expires, handles SIGTERM with bounded shutdown, and closes SSE streams when their credentials expire. The controller must drain routing and reconcile/restart deliberately; Docker does not restart a container merely because its health check fails.

Generic/reference bootstrap and public-verifier preparation, private signing-seed storage, generation-fenced lifecycle, authenticated readiness, connection grants, SDK credential renewal and real Docker lifecycle/worker testing are implemented. Remaining deployment work is the chosen production infrastructure adapter/bridge and its validation, application membership/authorization and billing policy, infrastructure credentials, gateway/TLS and network fencing, and storage backup/restore. The [workspace host](../../neoism-cloud-host/README.md) supplies reusable controller machinery rather than requiring ordinary users to build a second bootstrap or lifecycle system; production providers must honor and verify the advertised isolation, limits and durable storage contract.
