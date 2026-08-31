# Agent management API

The management plane is local-only and disabled by default. Start the Agent process with:

```sh
NEOISM_AGENT_MANAGEMENT_API=1 neoism-agent serve
```

When enabled, `GET /v2/capabilities` advertises `neoism.management`. Existing catalog reads (`/v2/agents`, `/v2/commands`, and `/v2/skills`) are unchanged. Management endpoints live under `/v2/management` and expose effective built-in/discovered resources as read-only alongside writable managed resources.

All management reads and mutations require an authenticated local static token. Trusted-loopback runtime access, workspace-scoped daemon credentials, and hosted credentials do not grant management authority until tenant-isolated resource storage is supported. Every writable resource has a canonical `sha256:` revision. Send that value through `If-Match` or `expectedRevision` when updating, deleting, or restoring a resource.

Agent and command definitions are deterministic frontmatter plus Markdown in the configured installation/workspace discovery roots. Skills use `skills/{id}/SKILL.md`, may include at most 32 bounded regular support files, and never execute install hooks. Skill versions are immutable Turso records and survive deletion of current files.

The TypeScript core client exposes the routes through `client.management.agents`, `client.management.commands`, and `client.management.skills`.

## Workspaces and repositories

`/v2/management/workspaces` registers canonical workspace roots. The standalone
Agent uses a bounded, atomically replaced state-file registry. When the Agent is
embedded by the Neoism daemon, an injected adapter delegates to the daemon's
existing `WorkspaceManager`, persistence, and tree notification paths; the GUI
registry remains authoritative.

`POST /v2/management/repositories` is discriminated by `kind`: `existing`
adopts an existing Git root, while `clone` accepts a bounded `remoteUrl`, `ref`,
and optional `depth`. Clone destinations are confined to the implementation's
managed workspace directory. Existing roots are canonicalized and symlink or
traversal aliases are rejected. Workspace/repository updates and deletes accept
`If-Match` or `expectedRevision`. Delete only unregisters the binding and never
recursively removes a workspace or working tree.

The TypeScript surfaces are `client.management.workspaces` and
`client.management.repositories`. Every returned root is the same root passed
to normal Agent config snapshots and workspace runtime discovery; repositories
are projections of workspace registrations, not a second catalog.

Install only `@neoism/sdk` for the HTTP client, typed management surfaces, and
the optional workflow client. See `sdk/typescript/examples/management.ts`.

## Workflow definitions and runs

When the optional `dev.neoism.workflows` plugin and management capability are
enabled, authenticated local callers can create definitions with `POST
/v2/plugins/dev.neoism.workflows`, or update, patch, and delete
`/v2/plugins/dev.neoism.workflows/{workflow_id}`. Managed definitions are the
canonical `.agent/workflows/{id}.md` files used by discovery, the watcher, and
the scheduler. Writes are deterministic and atomic. Updates/deletes accept the
returned `sha256:` revision through `If-Match` (or `expectedRevision` for
delete); built-in and other discovered definitions remain read-only.

Definitions may set `retry.maxAttempts`, fixed/exponential backoff delays and
retryable error strings, plus `concurrency.mode` (`forbid`, `replace`, or
`allow`) and `maxRunning`. Defaults remain one attempt and
`forbid`/`maxRunning: 1`. Attempts, retry lineage/due times, and lease hooks are
stored in Turso so queued retries survive restart. Runs can be fetched or
retried at `/runs/{run_id}` and `/runs/{run_id}/retry` beneath a workflow.