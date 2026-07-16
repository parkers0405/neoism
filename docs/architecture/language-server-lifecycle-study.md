# Language server lifecycle: OpenCode, Neoism, and Zed

Studied on 2026-05-08.

- OpenCode checkout: `6e47ae769ed39461b8bce8249a6bf5f2109252ab`
- Zed `main`: `4a3e0af532e4ad89baf634f4b94938b98beaa292`
- Neoism: current workspace checkout

This study treats "servers" primarily as language-server subprocesses. It also
uses each application's process ownership and client/server boundary where that
affects lifecycle management.

## Executive conclusion

Neoism should retain its current architectural advantage: one persistent LSP
service serves both editor features and agent tools. The next step is not to
copy either project wholesale. It is to combine:

- OpenCode's compact per-workspace registry and single-flight spawn handling.
- Zed's explicit host/proxy ownership, lifecycle state machine, status events,
  bounded graceful shutdown, and user-facing restart/stop operations.
- Neoism's shared document and diagnostics cache across human and agent flows.

The central rule should be: **the machine that owns the workspace owns the
language-server process**. Desktop and web clients should consume the same
daemon API and never spawn competing servers for the same workspace.

## OpenCode

### Ownership and discovery

OpenCode stores LSP state in an `InstanceState`, scoped to an OpenCode project
instance (`packages/opencode/src/lsp/lsp.ts:129-167`). The state contains:

- a configured server registry,
- a client array,
- a `broken` server-id set, and
- a `spawning` map of in-flight startup promises.

`getClients()` selects servers by matching file extensions and walking from the
file toward the project root for configured root markers
(`packages/opencode/src/lsp/lsp.ts:211-285`). Thus a client is keyed effectively
by server identity plus discovered root, and different roots can receive
different server processes.

Server definitions are plain launch adapters. They detect/install binaries,
return a process, initialization options, and optionally per-client environment
variables (`packages/opencode/src/lsp/server.ts`). The registry is easy to
extend and configuration can disable or override built-ins.

### Startup and concurrency

Startup is lazy. A file operation requests clients, and only matching servers
are considered. The `spawning` map deduplicates concurrent startup for the same
server/root key (`packages/opencode/src/lsp/lsp.ts:223-266`). Failed launches are
recorded in `broken`, preventing repeated spawn storms for the life of that
instance.

The spawned client performs initialize/initialized, tracks server capabilities,
and owns document synchronization and diagnostics
(`packages/opencode/src/lsp/client.ts`).

### Shutdown and status

Instance disposal stops all active clients and clears the spawning map
(`packages/opencode/src/lsp/lsp.ts:157-167`). Client shutdown sends the LSP
`shutdown` request, sends `exit`, and then kills the process
(`packages/opencode/src/lsp/client.ts:559-567`).

OpenCode exposes only coarse status: connected clients and broken server ids
(`packages/opencode/src/lsp/lsp.ts:324-352`). It does not model starting,
installing, stopping, exited, or retry/backoff as first-class public states.

### General application server boundary

OpenCode also has a strong process boundary outside LSP: the CLI can run an HTTP
server and clients talk to it through a generated SDK. That makes the service
the owner of sessions and tools rather than embedding that state independently
in each UI. This is directionally consistent with Neoism's workspace daemon.

### What is worth copying

- A single registry object with clients, failures, and in-flight starts.
- A startup future/task per server key, shared by concurrent callers.
- Lazy extension/root-marker selection.
- Launch adapters that separate binary resolution/install from protocol I/O.

### What is not enough for Neoism

- `broken` is permanent until instance disposal; there is no controlled retry.
- Status is too coarse for an editor status UI.
- Shutdown is not visibly timeout-bounded in this layer.
- Lifecycle ownership does not address Neoism's local/remote workspace split.

## Zed

### Ownership and local/remote split

Zed explicitly separates three roles in `crates/project/src/lsp_store.rs`:

- `LocalLspStore` runs on the workspace host and owns server processes.
- `RemoteLspStore` runs for guests and forwards requests over RPC.
- `LspStore` gives consumers one interface over either implementation.

This is the strongest pattern for Neoism. A remote web or desktop client should
not care whether the workspace is local, SSH-hosted, or on another paired
machine. The workspace host makes launch/configuration decisions and streams
results and status back.

### State model and startup

Zed stores each server as `LanguageServerState::Starting` or
`LanguageServerState::Running` (`crates/project/src/lsp_store.rs:14754-14783`).
The starting state retains the startup task and pending workspace-folder
changes, so requests and project mutations do not race an invisible spawn.

The startup path (`crates/project/src/lsp_store.rs:4235-4420`) does substantially
more than spawn:

- resolves adapter and launch options,
- allocates a stable server id,
- inserts `Starting` before async work begins,
- records installation/download status,
- captures stderr for actionable failures,
- initializes the server,
- transitions atomically to `Running`, and
- broadcasts add/remove/status events.

Adapters are registered in a language registry and can resolve binaries,
initialization options, workspace configuration, and labels. The project store
owns orchestration rather than allowing adapters to own global lifecycle state.

### Status and observability

Zed treats status as part of the architecture, not only logging. It emits:

- server added/removed events,
- binary installation/checking/downloading/failure status,
- LSP work progress,
- disk-based diagnostic progress,
- error/warning counts, and
- stderr-backed startup failures.

Remote clients receive these updates through the same project protocol. This
supports a language-server status item without inspecting processes locally.

### Stop, restart, and shutdown

Zed has explicit operations to restart and stop the servers associated with
buffers (`crates/project/src/lsp_store.rs:11669-11812`). Restart remembers which
servers were running, stops them, and re-registers relevant buffers so normal
selection starts them again. Stop records intentionally stopped adapter names,
preventing automatic buffer registration from immediately respawning them.

`stop_local_language_server()` handles both `Starting` and `Running`: it cancels
startup or sends graceful shutdown, removes mappings, clears diagnostics and
statuses, unregisters buffers, and emits removal events
(`crates/project/src/lsp_store.rs:11390-11572`).

The protocol layer uses a bounded graceful shutdown. It requests `shutdown`,
sends `exit`, waits for process exit, then kills after a timeout
(`crates/lsp/src/lsp.rs:1070-1148`). This is the correct final safety net.

### What is worth copying

- Workspace-host process ownership and remote proxying.
- Explicit `Starting` and `Running` entries keyed by stable server id.
- Pending workspace-folder/document work retained during startup.
- Structured lifecycle/status events.
- Intentional stop distinct from failure.
- Restart implemented through normal buffer registration, not ad hoc respawn.
- Graceful shutdown followed by bounded wait and kill.
- Captured stderr attached to startup failure state.

### What would be excessive to copy immediately

Zed's store also handles collaboration, language adapters, snippets, formatting,
code actions, diagnostics, worktrees, and RPC in one very large subsystem.
Neoism does not need that breadth before fixing its state and ownership model.

## Neoism today

### Existing strengths

Neoism's persistent service is in
`neoism-agent/crates/neoism-agent-server/src/lsp_service.rs`. It is shared by
editor-side calls and agent LSP tools through `lsp.rs`. It already provides:

- process reuse keyed by workspace root, language, and command,
- a shared open-document/version map,
- cached diagnostics,
- per-language live/broken reporting,
- request timeouts in the stdio client,
- a `spawning` set to reject duplicate synchronous starts, and
- `shutdown_all()` for service teardown.

This is better than spawning a disposable server for every agent request. It
also establishes the correct product behavior: agent edits and editor
diagnostics see the same semantic session.

The workspace daemon already supervises the Neoism Agent HTTP server in
`neoism-workspace-daemon/src/agent.rs`. It performs a health probe, starts a
dedicated server thread/runtime when needed, and uses a generation number so a
stale server cannot clear a newer active slot. This is a useful precedent for
daemon-owned lifecycle state.

### Current lifecycle gaps

1. **Startup state is implicit.** `LspService::client()` inserts a key into a
   `HashSet`, performs blocking spawn/initialize, then removes the key. A second
   caller gets "already starting" rather than joining the in-flight start.

2. **State is fragmented.** Running clients, broken reasons, and spawning keys
   live in separate locks. There is no atomic `Starting -> Running -> Failed`
   transition or stable server id.

3. **Failures are sticky and retry is implicit.** A failed key is retained in
   `broken`; there is no backoff, retry eligibility, or user restart operation.

4. **Process death is discovered late.** The stdio reader records closure and
   pending requests fail, but the service registry does not proactively
   transition and broadcast an exited state.

5. **Shutdown is incomplete.** `StdioLspClient::shutdown()` writes `shutdown`
   and `exit`, and `Drop` calls `start_kill()`, but there is no bounded wait for
   graceful process exit and no structured shutdown outcome.

6. **Status is too narrow.** `LspStatus` presents `Running`, `Broken`, and
   `Unknown`. It cannot represent resolving/installing, starting, stopping,
   exited, retrying, or intentionally stopped.

7. **No public stop/restart controls.** `shutdown_all()` is teardown-only.

8. **Remote ownership is not explicit in the LSP API.** The workspace daemon is
   the natural owner, but clients do not yet consume a Zed-like local/proxy
   abstraction with lifecycle events.

9. **Locking requires care.** The current service uses several mutexes and has
   synchronous call paths. Any redesign must preserve the project rule against
   re-entering a held mutex through helpers.

## Recommended Neoism target

### One owner

The workspace daemon on the workspace host owns an `LspStore`. All desktop,
web, remote, and agent consumers call it through protocol messages. For a local
desktop workspace this may remain an in-process fast path, but it must expose
the same API and event model as a remote proxy.

Do not create one LSP pool in the frontend and another in the agent server.

### Registry state

Use one mutex-protected registry whose entry is one of:

```rust
enum ServerState {
    Starting {
        task: SharedStart,
        pending_documents: HashMap<PathBuf, PendingDocument>,
        started_at: Instant,
    },
    Running {
        client: Arc<StdioLspClient>,
        started_at: Instant,
    },
    Failed {
        reason: String,
        stderr: String,
        attempts: u32,
        retry_at: Instant,
    },
    Stopped,
}
```

The concrete async primitive can differ, but concurrent callers must await the
same startup result. Never hold the registry lock while spawning, initializing,
performing protocol I/O, or calling a helper that can lock the registry again.

Key entries by workspace id/root, adapter id, and resolved LSP root. Keep a
separate stable runtime `ServerId` for protocol/UI events.

### Public operations

The store should support:

```text
ensure_server(document) -> ServerId
open_or_update_document(document, version, text)
request(ServerId, method, params)
status(workspace) -> [ServerStatus]
stop(ServerId | language | workspace)
restart(ServerId | language | workspace)
shutdown_workspace(workspace)
subscribe_status(workspace)
```

Restart should clear `Failed` or `Stopped` and pass through normal document
registration so root and adapter selection remain canonical.

### Status model

Expose at least:

```text
Resolving, Installing, Starting, Running, Stopping,
Stopped, Failed, Exited, RetryScheduled
```

Status payloads should include server id, adapter/language, root, message,
failure count, next retry time, and captured stderr tail where safe. Broadcast
changes to clients; do not make the UI poll process tables.

### Retry policy

- Configuration and missing-binary failures remain failed until configuration
  changes or the user requests restart.
- Unexpected process exits receive limited exponential backoff.
- Reset the failure count after a meaningful healthy interval.
- Never retry an intentionally stopped server.
- Deduplicate retries with the same single-flight entry used for first startup.

### Shutdown contract

1. Stop accepting new requests for the server.
2. Send `shutdown` with a short request timeout.
3. Send `exit`.
4. Close stdin and wait for process exit with a bounded timeout.
5. Kill the process group on timeout.
6. Fail pending requests and emit one terminal status event.

Process-group termination matters because language servers may own helper
processes. Neoism already encountered orphaned process groups elsewhere, so the
LSP path should not regress that invariant.

## Incremental implementation order

1. Replace `clients`/`broken`/`spawning` with one state registry while keeping
   the current public service methods.
2. Make startup single-flight: concurrent calls await the same result instead
   of receiving "already starting".
3. Add exit observation, structured status events, and stderr-tail capture.
4. Add bounded graceful shutdown and process-group kill fallback.
5. Add stop/restart/status daemon protocol operations and a small status UI.
6. Route every desktop/web/agent consumer through the workspace-host owner.
7. Add controlled retry/backoff only after state and observability are stable.

## Verification requirements

- Two concurrent requests for one cold server spawn exactly one process.
- Different LSP roots can spawn separate instances of the same adapter.
- A request arriving during startup waits and receives the initialized client.
- Startup failure includes a useful reason and bounded stderr tail.
- An unexpected process exit changes status and fails pending requests.
- Retry cannot duplicate a concurrently starting process.
- Stop does not auto-restart when an existing buffer emits another update.
- Restart reopens current documents with monotonic versions.
- Workspace shutdown leaves no server or descendant process alive.
- A remote client sees the same statuses and results as a local client.
- Editor and agent requests share one process and one document version stream.

## Source map

### OpenCode

- `packages/opencode/src/lsp/lsp.ts`
- `packages/opencode/src/lsp/client.ts`
- `packages/opencode/src/lsp/server.ts`
- `packages/opencode/src/lsp/launch.ts`
- `packages/opencode/src/lsp/language.ts`

### Neoism

- `neoism-agent/crates/neoism-agent-server/src/lsp_service.rs`
- `neoism-agent/crates/neoism-agent-server/src/lsp_client.rs`
- `neoism-agent/crates/neoism-agent-server/src/lsp.rs`
- `neoism-agent/crates/neoism-agent-server/src/lsp_query.rs`
- `neoism-workspace-daemon/src/agent.rs`
- `neoism-frontend/desktop/src/agent_server.rs`

### Zed

- `crates/project/src/lsp_store.rs`
- `crates/language/src/language_registry.rs`
- `crates/lsp/src/lsp.rs`
