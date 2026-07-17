# Hosted Neoism

This document defines the production architecture for the managed service at
`neoism.ai`. The desktop application remains open source. Customers pay for a
managed account, durable data, isolated workspace runtimes, remote access, and
AI API usage.

## Product boundary

A Neoism account owns:

- one personal organization initially, with team organizations later;
- multiple projects;
- multiple workspaces per project;
- notes, drawings, agent conversations, and project metadata;
- desktop and browser devices;
- API keys for external agent access;
- a subscription, usage ledger, and spending limits.

A **project** is the durable product object. It groups a Git repository or
uploaded files, notes, agent history, secrets, and one or more workspaces.

A **workspace** is an executable checkout of a project. It owns a filesystem,
PTYs, managed Neovim processes, agent tool processes, and transient UI state.
Workspaces may be stopped and resumed without deleting the project.

The desktop application subscribes to an account, lists its projects and
workspaces, and connects to a selected workspace through the Neoism gateway.
Its existing server registry and per-server workspace subscriptions are the
starting point for this behavior.

## Trust boundary

Do not host unrelated customers inside one `neoism-workspace-daemon` process.
The daemon controls files, shells, editor processes, Git credentials, provider
credentials, and arbitrary agent tools. Its current token gate authenticates a
connection, but it is not a tenant sandbox.

The minimum safe isolation unit is one Linux container per account. The target
is one container or microVM per running workspace, which provides stronger
resource accounting, sleeping, snapshots, and secret isolation.

Each runtime contains:

- `neoism-workspace-daemon`;
- `neoism-agent-server`;
- the project checkout and workspace volume;
- only the short-lived secrets granted to that workspace;
- CPU, memory, process, disk, and network limits.

The runtime is never directly public. The gateway authenticates the customer,
authorizes access to the workspace, and proxies HTTP, SSE, and WebSocket
traffic to the private runtime network.

## System architecture

```text
Desktop / Browser / API client
              |
              | HTTPS, WSS, SSE
              v
        api.neoism.ai
   +-----------------------+
   | API gateway           |
   | account authentication|
   | workspace authorization|
   | rate and spend limits |
   +-----------+-----------+
               |
       +-------+--------+
       | Control plane  |
       | projects       |
       | subscriptions  |
       | devices/API keys|
       | usage ledger   |
       +---+---------+--+
           |         |
      PostgreSQL   Object storage
                     notes snapshots,
                     drawings, exports,
                     workspace backups

 Private runtime network
       |
       +-- workspace runtime A
       |    daemon + agent + volume
       +-- workspace runtime B
            daemon + agent + volume
```

The first deployment can run the gateway, control plane, PostgreSQL, and a
single runtime worker on one server. Their process and data boundaries should
still match this design so a second worker can be added without changing the
desktop protocol.

## Domains

- `neoism.ai`: marketing and account entry point.
- `app.neoism.ai`: hosted browser client and account UI.
- `api.neoism.ai`: versioned control-plane and external AI APIs.
- `docs.neoism.ai`: documentation.
- `status.neoism.ai`: service status.

Workspace runtimes should use opaque internal addresses. Do not give each
runtime a permanently public hostname.

## Identity and authentication

Human clients use an OAuth-style flow with short-lived access tokens and
rotating refresh tokens. Start with email magic links or a hosted identity
provider; add GitHub login after the account model is stable.

Desktop login uses a browser authorization flow:

1. Desktop generates a PKCE verifier and opens `neoism.ai/device`.
2. The customer signs in and approves the named device.
3. Desktop receives a short-lived authorization code on a loopback callback or
   polls a device-code endpoint.
4. Desktop exchanges it for an access token and rotating refresh token.
5. The control plane records the device so the customer can revoke it.

External integrations use scoped Neoism API keys. Store only an Argon2id hash
of each key. Show the plaintext once. Every key has a prefix, account,
organization, scopes, optional project restriction, creation time, last-used
time, and revocation time.

Runtime credentials are separate from account credentials. The gateway mints
a short-lived, workspace-specific token when it starts or resumes a runtime.
The existing daemon token remains an internal second gate, not the customer's
primary credential.

## Core data model

The control-plane database starts with these tables:

- `users`
- `organizations`
- `organization_members`
- `devices`
- `refresh_tokens`
- `api_keys`
- `projects`
- `project_members`
- `workspaces`
- `workspace_instances`
- `notes`
- `note_updates`
- `agent_threads`
- `subscriptions`
- `entitlements`
- `usage_events`
- `usage_rollups`
- `audit_events`

All customer-owned rows include `organization_id`. Queries must derive that ID
from authenticated context rather than accepting it as an unverified filter.
Use UUIDv7 identifiers so records remain opaque and time-sortable.

## Notes and drawings

Hosted notes are project-owned, not runtime-owned. A note record contains its
project, normalized path, title, current revision, timestamps, and deletion
marker. Content is represented by CRDT updates so desktop, browser, and tablet
clients can work offline and merge later.

Reuse `neoism-sync` for note and drawing documents, but add a server-side
append-only update store. Periodically compact updates into a snapshot and
retain recent updates for reconnecting clients. Store small current snapshots
in PostgreSQL initially; move large snapshots, drawings, and attachments to
S3-compatible object storage.

Runtime file-backed Markdown remains useful. A project can designate a vault
path, and the runtime sync bridge materializes hosted notes into that path.
The hosted note document is authoritative when multiple devices edit it.

## Projects and workspaces

Creating a project accepts one of:

- an empty project;
- a Git URL and optional branch;
- a template;
- a later desktop upload/import flow.

Creating a workspace selects a project and source revision. The control plane
allocates a runtime, attaches a persistent volume, checks out the project, and
starts the daemon and agent server. Workspace state is `creating`, `running`,
`stopping`, `stopped`, `failed`, or `deleting`.

Idle workspaces are stopped after a plan-specific timeout. Stopping preserves
the volume and durable metadata but terminates PTYs and processes. A later
resume starts a new runtime against the same volume. The UI must distinguish
durable workspaces from live sessions so it never promises that a process
survives a stopped runtime.

## External AI API

Expose a stable, authenticated API at `/v1`. Do not expose the current agent
server directly: its routes are broad, permit permissive CORS, assume a local
directory, and do not provide account authorization or billing enforcement.

Initial public resources:

```text
GET    /v1/projects
POST   /v1/projects
GET    /v1/projects/{project_id}

GET    /v1/projects/{project_id}/workspaces
POST   /v1/projects/{project_id}/workspaces
POST   /v1/workspaces/{workspace_id}/resume
POST   /v1/workspaces/{workspace_id}/stop

GET    /v1/projects/{project_id}/notes
POST   /v1/projects/{project_id}/notes
GET    /v1/notes/{note_id}
PATCH  /v1/notes/{note_id}

POST   /v1/agent/runs
GET    /v1/agent/runs/{run_id}
GET    /v1/agent/runs/{run_id}/events
POST   /v1/agent/runs/{run_id}/cancel

GET    /v1/usage
GET    /v1/me
```

`POST /v1/agent/runs` names a project, optional workspace, model, agent, input,
and idempotency key. It returns a run ID. Events are available as SSE and
include text deltas, tool requests, permission requests, completion, usage,
and errors. The control plane translates this stable contract to the internal
`neoism-agent-server` API.

API keys receive explicit scopes such as `projects:read`, `notes:write`,
`agents:run`, and `workspaces:control`. Destructive tools require an interactive
approval or a narrowly scoped service policy. API access must never imply
unrestricted shell execution by default.

## AI credentials and billing

Support two provider modes:

1. **Bring your own key.** Encrypt provider credentials with an envelope key
   from a managed KMS. Decrypt only for the selected runtime. Do not write
   plaintext credentials into project volumes or logs.
2. **Neoism credits.** Neoism pays the provider and charges the customer for
   usage. Add this only after metering and hard spend limits are reliable.

The agent runtime already records token counts and `cost_micros` in message
usage. Treat those values as evidence from the runtime, not the billing source
of truth. The gateway records an immutable usage event with account, project,
workspace, run, provider, model, input/output/cache tokens, provider cost,
billable amount, and idempotency key. A reconciliation job compares these
events with provider invoices.

Use Stripe for subscriptions, Checkout, the customer portal, invoices, and
webhooks. Stripe is not the entitlement database. Webhooks update local
subscription records; local entitlements decide whether an action is allowed.

## Plans

Launch with simple limits rather than many feature gates:

| Plan | Price | Included |
| --- | ---: | --- |
| Free | $0 | local app, one hosted project, limited notes, BYOK, no always-on runtime |
| Pro | $20/month | multiple projects, hosted notes, remote desktop access, workspace hours and storage |
| Team | $35/user/month | shared projects, members, team permissions, pooled usage, audit history |

AI provider spend is separate from the subscription. Start with BYOK and sell
runtime hours/storage. Later offer prepaid Neoism credits with a transparent
margin. Never offer uncapped AI usage at a flat price.

Keep plan values in entitlements such as:

- maximum projects;
- maximum concurrent running workspaces;
- monthly runtime seconds;
- storage bytes;
- note history retention;
- team member count;
- external API availability;
- idle timeout;
- monthly AI spend ceiling.

## Security requirements before public beta

- TLS on every public connection.
- No direct public runtime ports.
- Mandatory account authentication and object-level authorization.
- Container or microVM isolation for unrelated customers.
- Rootless runtime user with no host Docker socket.
- CPU, memory, process, disk, and execution-time limits.
- Restricted metadata-service and internal-network access.
- Encrypted secrets and backups.
- API rate limits and hard cost limits.
- Immutable security and billing audit events.
- Dependency, image, and secret scanning in CI.
- Tested backup restoration and account deletion.
- Redaction of tokens, prompts marked private, and environment values in logs.
- Separate production and development credentials and databases.

## Delivery phases

### Phase 0: paid concierge pilot

- One server and manually provisioned account-isolated containers.
- BYOK only.
- Existing desktop remote-server connection.
- Multiple workspaces inside each isolated account runtime.
- Hosted notes backup and manual account administration.
- Stripe Payment Links or invoices; no automated provisioning promise.

This phase can be sold while the control plane is under construction.

### Phase 1: account control plane

- Users, organizations, projects, workspaces, devices, and API keys.
- Browser/desktop login and token refresh.
- Gateway authorization and runtime routing.
- Runtime create, resume, stop, and idle suspension.
- PostgreSQL migrations and audit events.

### Phase 2: hosted notes and desktop subscription

- CRDT note update API and WebSocket synchronization.
- Snapshot compaction and object storage.
- Project/workspace browser in desktop.
- Offline changes, reconnect, conflicts, and device revocation.

### Phase 3: public agent API

- Stable `/v1/agent/runs` contract.
- SSE events, cancellation, idempotency, and scoped keys.
- Runtime adapter to `neoism-agent-server`.
- Usage events, budgets, and rate limits.

### Phase 4: self-service billing

- Stripe Checkout and customer portal.
- Webhook processing with replay protection.
- Entitlement enforcement.
- Usage dashboard and notifications.
- Automated trial, suspension, and deletion lifecycle.

### Phase 5: teams and scale

- Invitations, project membership, and roles.
- Per-workspace runtimes and multiple workers.
- Queueing, autoscaling, regional storage, and disaster recovery.
- SSO and enterprise deployment options.

## Existing code to reuse

- `neoism-frontend/desktop/src/server_registry.rs`: remote server profiles,
  tokens, subscriptions, and last-active workspace state.
- `neoism-workspace-daemon/src/workspace/manager.rs`: multiple workspace
  metadata and workspace lifecycle inside one isolated runtime.
- `neoism-workspace-daemon/src/auth.rs`: internal daemon token and paired-peer
  primitives. These remain runtime authentication, not account identity.
- `neoism-workspace-daemon/src/server.rs`: HTTP/WebSocket runtime service and
  workspace routes behind the gateway.
- `neoism-agent/crates/neoism-agent-server`: agent sessions, streaming,
  providers, tools, and token/cost observations behind an adapter.
- `neoism-sync/src/note.rs`: CRDT note operations for hosted note documents.
- `neoism-sync/src/net.rs`: synchronization message patterns that can inform
  the hosted sync protocol.

## New components

Add these as separate crates or services rather than embedding tenancy and
billing into the workspace daemon:

- `neoism-cloud-core`: account/project/workspace types, scopes, entitlements,
  and API request/response types.
- `neoism-cloud-api`: Axum control plane, authentication, public `/v1` API,
  Stripe webhooks, and gateway authorization.
- `neoism-cloud-store`: PostgreSQL migrations and repositories.
- `neoism-cloud-runtime`: runtime allocator and lifecycle abstraction, with a
  Docker implementation first.
- `neoism-cloud-worker`: asynchronous provisioning, compaction, usage
  reconciliation, and deletion jobs.

Do not modify the existing daemon protocol to carry subscription or payment
state. The control plane decides whether a customer may reach a runtime; the
daemon continues to own executable workspace behavior.

## First implementation slice

The first code slice should create `neoism-cloud-core` with opaque IDs, API-key
scopes, workspace states, entitlements, and the public project/workspace API
types. Follow it with a small `neoism-cloud-api` exposing authenticated
`/v1/me`, project CRUD, and workspace lifecycle records backed by PostgreSQL.
Runtime provisioning can initially be a fake adapter so account and desktop
flows are testable before production container orchestration is introduced.