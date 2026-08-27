# Server and API

Neoism Agent is an HTTP server. Every client — the desktop pane, the browser, the SDK, external tools — talks to the same versioned API, so anything the UI can do, your scripts can do.

## One surface: `/v2`

The public API lives entirely under `/v2/`. There are no legacy aliases; a request to an unversioned path returns 404. The full contract is machine-readable:

- `GET /v2/openapi.json` serves the canonical OpenAPI document.
- The same document is committed in the Neoism repository at `neoism-agent/openapi/v2.json`, so API changes are visible in review diffs.
- `GET /v2/meta` reports `apiVersion`, `serverVersion`, `pluginApiVersion`, and the active plugin generation.

Errors use one envelope everywhere: `{ "code", "message", "retryable", "details" }`.

## Route map

| Domain | Routes |
|---|---|
| System | `/v2/health`, `/v2/meta`, `/v2/openapi.json`, `/v2/capabilities`, `/v2/audit` |
| Sessions | `/v2/sessions` CRUD, `/status`, `/fork`, `/children`, `/runtime`, `/todos`, `/diff`, import/export |
| Generation | `/v2/sessions/{id}/prompt`, `/prompt-async`, `/abort`, `/compact`, `/wait`, `/summarize`, `/context` |
| Messages | `/v2/sessions/{id}/messages`, message and part deletion/patching |
| Queue | `/v2/sessions/{id}/queue` (list, clear, pop) |
| History | `/v2/sessions/{id}/undo`, `/redo`, `/revert`, `/unrevert`, `/undo-tree` |
| Interactions | `/v2/interactions/permissions`, `/v2/interactions/questions` with reply/reject |
| Artifacts | `/v2/artifacts` upload/download/list |
| Events | `/v2/events` (SSE) |
| Plugins | `/v2/plugins`, plus every plugin-contributed route (config, providers, agents, skills, commands, LSP, MCP, PTY, VCS, workflows, goals, subagents, semantic search) |

Plugin-contributed routes register through the plugin runtime and are held to the same OpenAPI parity tests as the core routes.

## Prompting

`POST /v2/sessions/{id}/prompt` accepts text or structured parts, an optional client-generated `messageId` (idempotent — retries are deduplicated), a model/agent override, and a `delivery` of `steer` (join the active run) or `queue` (wait for it). The server owns the run: closing the client does not stop generation.

## The event stream

`GET /v2/events?sessionId=<id>&tail=true` is a Server-Sent Events stream carrying everything that happens: token deltas, part snapshots, session status, permissions, questions, queue changes, execution activity, and subagent lifecycle.

- Events arrive in **publish order** from a single ordered bus. A part snapshot can never overtake or lag the deltas around it.
- Every record's `data` is a typed, `type`-discriminated union — the OpenAPI `Event` schema enumerates all event types with their exact payloads, and the SDK exposes it as a TypeScript discriminated union.
- Passing `since=<sequence>` (or a `Last-Event-ID` header) instead of `tail` replays the durable event log from that cursor before going live.
- Live token deltas are transient; durable events are committed transactionally with the state change they describe.

Subscribing with a `sessionId` follows the whole session family: child subagent sessions discovered later join the stream automatically.

## Authentication and hosted mode

With no token configured, the server is open on loopback for local use. Three credential forms enable gating:

| Mechanism | Purpose |
|---|---|
| `NEOISM_AGENT_TOKEN` | A single bearer token for the local server. |
| Daemon-signed credentials | The workspace daemon signs per-client claims (workspace, tenant, directory prefixes). |
| `NEOISM_AGENT_AUTH_CONFIG` | Hosted token file: per-token tenants, directory scopes, rate and concurrency quotas. |

Hosted callers get hard boundaries: session ownership checks on every session-scoped route (validated against route descriptors, never guessed from the path), directory-prefix enforcement, config and credential routes blocked, and an audit log entry per authenticated request (`/v2/audit`).

## Run it standalone

The agent server also runs without the desktop:

```sh
neoism-agent serve            # HTTP server
neoism-agent openapi          # print the OpenAPI document
```

Point any SDK or HTTP client at it. The desktop's embedded server is the same binary surface.

## Embedding the agent in a product

A backend that drives the agent per tenant (a SaaS assistant, a bot, a
pipeline) runs one loop per conversation:

1. Connect with a bearer token. For multi-tenant deployments, use
   `NEOISM_AGENT_AUTH_CONFIG` with one token per tenant, a
   `directoryPrefixes` jail, and per-tenant rate and concurrency quotas.
2. Create or reuse a session rooted in the tenant's directory
   (`sessions.create({ directory })`). Instructions and configuration are
   discovered upward from that directory, so shared rules live at the base
   and per-tenant overrides in the tenant folder.
3. Subscribe to `/v2/events` with the session id and `tail: true`
   **before** prompting, so nothing slips between the two. The SDK
   subscription reconnects automatically and resumes from its sequence
   cursor.
4. Prompt with a caller-generated `messageId` — retrying the same prompt
   after a network failure is idempotent, never a duplicate turn.
5. Stream tokens from `message.part.delta`, watch typed tool and
   step-finish parts (token counts and cost for billing), and finish when
   `session.status` reports idle for the session.

The complete, runnable version of this loop is
`sdk/typescript/examples/headless.ts` in the repository.

Headless runs should preconfigure [[Permissions]] rules (allow and deny
patterns) so a turn never blocks on an interactive approval; the
`permission.asked` event plus `interactions.permissions.reply` is the
interactive fallback, not the plan.

## API stability

The committed OpenAPI document is the contract. Within a major
`apiVersion`: existing routes, fields, event types, and part types are not
removed or repurposed; new ones may be added at any time. Part and event
schemas tolerate additive fields — validating clients must ignore unknown
properties. Event `sequence` values are monotone per connection and safe
to use as a resume cursor. Breaking changes get a new version prefix and a
deprecation window, never a silent change under `/v2`.

See [[SDK]] for typed clients, [[Plugins]] for extending the server, and [[Sessions and Sharing]] for the session model.
