# Agent and LSP resource audit

## Scope and conclusion

Audited Neoism source at `853528742030efc842ac865761eccfc9196ebd9f`, including native subagents, agent tools, editor LSP ownership, process lifecycle, and current local processes. Compared pinned upstream Codex and both the current development and released OpenCode implementations. This is an investigation, not an implemented performance fix or a matched concurrency benchmark.

The evidence does not support "one language server per native subagent." Same-workspace native children share application state and the workspace's agent LSP runtime. However, there are confirmed resource-lifecycle defects, editor/agent ownership duplication, and concurrency controls that do not compose into a laptop-wide budget. These can make a small number of tool-heavy agents expensive. The largest observed local consumer was a Rust language server and its compiler children, not conversation state.

## Live measurements and limits

The installed executable reported `0.7.143-nightly.20261102.3`. Measurements were observational; no existing application, daemon, or language server was stopped or restarted.

| Observation | Measurement | Interpretation |
| --- | --- | --- |
| Active daemon's rust-analyzer, PID 643739 | Initially 9,837,244 KiB RSS and 9,824,222 KiB PSS, approximately 9.4 GiB; later about 10.4 GiB RSS | A genuinely large private working set, not merely virtual-address reservation |
| Rust server process tree | One sample summed to 15,196 MiB RSS across nine processes; a later sample included Cargo and two rustc children | Analysis can launch substantial compiler work too; summed RSS can double-count shared pages and varies during compilation |
| Active workspace daemon, PID 573372 | Approximately 0.5 GiB RSS | Does not establish agent heap as the dominant measured consumer |
| Active desktop, PID 573335 | Approximately 0.8 GiB RSS; about 71% of one logical CPU in a one-second sample | UI work also needs profiling; this was not a controlled idle measurement |
| TypeScript server trees | Five identified trees, approximately 0.5-1.0 GiB summed RSS each | A language-server launcher PID alone substantially understates its tree's cost |
| Workspace daemons | One installed daemon plus four older `dist` preview daemons | Development-preview contamination must be excluded from a normal-user baseline; not evidence of one daemon per native subagent |
| Host memory pressure | Approximately 64 GiB RAM, 32 GiB available, zero recent memory PSI; approximately 6.6 GiB swap occupied | Swap occupancy alone does not prove current thrashing. The sampled machine was not under sustained memory pressure |

The process tree establishes that expensive local compiler/indexer work exists. It does not establish why this user perceives a threshold at precisely three to five agents, nor extrapolate that threshold to every customer's hardware or repository.

## Findings

### 1. High: local-work budgets multiply across agents

`ProviderStreamStepState::new` creates a fresh ten-permit semaphore for each model response step, not a shared daemon/workspace budget. Each composition separately creates a four-permit host budget. Native child admission records execution lifecycle but does not reserve CPU/RAM/process capacity. Detached background jobs return their tool slot while the process continues, with no active-job cap in that path.

Three to five simultaneously tool-active children can therefore permit 30-50 direct tool executions, excluding the parent. Independent compositions can increase the underlying call count further. These numbers describe permitted concurrency, not a measured workload; many tools are cheap or waiting on I/O. Compilers can also create many workers behind one tool call.

Evidence: `neoism-agent/crates/neoism-agent-server/src/provider_stream_processor.rs:33-70,1014-1052`; `tool_composition.rs:983-1001`; `execution_activity.rs:479-554`; `background_job.rs:272-332`.

Recommendation: distinguish logical agents, active model requests, resident runtimes, and expensive local jobs. Add a shared, cancellation-safe admission service for builds/tests/indexing and background processes, with cheap file tools in a separate class. Do not hold a parent permit while it waits for children that require the same permit. A per-daemon limit is not automatically a machine-wide limit across windows or independent daemons.

### 2. High: editor and agent LSP ownership are separate

The workspace daemon constructs an editor `LspRuntime` in `neoism-workspace-daemon/src/standalone.rs:482`. Agent workspace plugin generations construct their own runtime in `neoism-agent/crates/neoism-agent-server/src/workspace_runtime.rs:386,1190`. `LspRuntime::new` creates a separate service with its own clients and caches; clones share, independent construction does not. The client key additionally includes declared workspace root as well as resolved project root and server/configuration identity.

Thus the same project can be indexed independently for editor activity and agent activity, or through distinct declared directory roots. Same-workspace native subagents do share their agent runtime, and do not automatically create worktrees. Separate worktrees genuinely contain different files and cannot safely be merged merely because they share a Git common directory.

Evidence: `neoism-agent/crates/neoism-agent-server/src/lsp.rs:51-82`; `lsp_service.rs:123-174,2430-2459`; `tool.rs:230-235`; `session_actions.rs:265-308`.

Recommendation: one broker per admitted workspace/configuration/security boundary, with editor and agent document leases. Preserve unsaved editor-buffer authority, tenant isolation, environment/configuration differences, and distinct worktree contents. Removing the workspace component from the cache key without redesigning ownership is not a safe fix.

### 3. High: editor close does not close the LSP document

The frontend sends `CloseLspBuffer` during cleanup, but the daemon only removes its `editor_documents` routing entry. `Close` similarly clears that map. Neither path invokes the implemented engine `close_document`, which removes text/version state and sends `textDocument/didClose`.

Agent mutation/query paths also open documents without a corresponding task-scoped close or document-LRU policy. This leaves server-side open-document state resident after the immediate consumer is done. It does not prove that all rust-analyzer memory is attributable to open documents; its project indexes are independently expensive.

Evidence: `neoism-frontend/gpuix/src/editor/lsp.ts:132-136`; `neoism-workspace-daemon/src/server/socket.rs:1031-1047`; `neoism-agent/crates/neoism-agent-server/src/lsp_service.rs:2359-2389`; `tool_support/diagnostics.rs:23-35`.

Recommendation: reference-count document leases across surfaces and agent consumers; issue `didClose` when the last lease expires, not when any one tab closes. Add a bounded idle policy for agent-only documents and server instances. The existing agent workspace TTL is one hour and is checked during later acquisitions, not by an independent LSP idle sweeper (`workspace_runtime.rs:40,1438-1467`).

### 4. High: permanent per-document threads and unbounded full-text queues

`neoism-workspace-daemon/src/language_server/live_sync.rs:28-75` stores senders in a process-static map. Each new document creates an OS thread, clones the entire LSP runtime handle into that thread, and uses an unbounded `mpsc::channel`. There is no remove/close event. The receiver waits until all senders disappear, but the static map retains its sender. The event loop processes every full-text snapshot in order without coalescing (`:163-169`).

This is both a lifecycle leak and a potential queue-amplification mechanism: a slow initializer/request can leave many obsolete snapshots waiting. It is editor-driven, not a separate thread for every plain agent read. The observed active-daemon thread listing did not establish that this editor path was active during the measurement.

An isolated Rust harness at `/tmp/neoism-lsp-lifecycle-audit.rs` includes the exact production `live_sync.rs`, replacing only the language-server engine with a stub. It demonstrated runtime retention after the last external caller dropped its handle, and replay of all 100 snapshots queued behind a blocked engine. Both characterization tests and the production FIFO-order test passed. These tests demonstrate the current undesirable behavior, not a successful fix or whole-application benchmark.

Recommendation: runtime-owned bounded queues, latest-snapshot coalescing between ordering barriers, a bounded worker pool, and explicit release/join paths. Preserve save/query barriers: coalescing must not move an edit across a save or query boundary.

### 5. High: resource-heavy server defaults without a resource policy

The built-in Rust adapter launches ordinary rust-analyzer. Built-in adapters default to no initialization options/settings (`lsp_adapters/types.rs:177-179`), and the inspected runtime does not add a common CPU/RAM budget. The live Rust process tree contained Cargo/rustc children. Three to five agents need not spawn three to five analyzers to be costly: they can activate more languages, projects, background jobs, and compiler work around one large index.

Recommendation: instrument process-tree CPU/RSS/PSS, expose ownership and indexing state, bound concurrent cold starts, and provide configurable analysis/build worker budgets. Do not present diagnostic timeouts as CPU/RAM limits or silently disable correctness features such as proc macros to create a favorable benchmark.

### 6. High, workload-dependent: bash snapshots duplicate dirty-file contents

Before ordinary bash execution, `tool_support/bash.rs:163` synchronously captures a baseline. `snapshot.rs:749-779` visits Git-reported, non-excluded dirty paths and `FileState::from_path` reads, hashes, and base64-encodes full contents (`:70-90`). The baseline is retained through execution (`:259-291`). This Git path has no aggregate-byte/file-size cap. Harmless shell commands from several agents can therefore retain duplicate snapshots of unrelated large dirty files. Snapshot work also occurs before the shell runner's timeout/cancellation boundary.

Recommendation: measure baseline byte totals; preserve undo guarantees using bounded/content-addressed storage rather than one full base64 baseline per concurrent command. Establish explicit behavior for over-budget files instead of silently dropping recovery data.

### 7. Medium: durable event backlog remains unbounded

The historical live-bus amplification has already been fixed: the current live bus has an 8 MiB retention budget and transient deltas bypass durable persistence. The separate durable `mpsc::unbounded_channel` remains, with one sequential append-and-plugin-dispatch consumer. Several agents can enqueue semantic events faster than slow storage/hooks consume them.

Evidence: `neoism-agent/crates/neoism-agent-server/src/state.rs:1023-1029,1200-1226,1744-1763`. No actual queue backlog was measured in this audit.

Recommendation: instrument queued bytes/count/age first; implement bounded durable admission or an explicit durable spool with failure reporting. Do not drop semantic events as a backpressure shortcut.

### 8. High, lifecycle edge cases: background and external process cleanup

Background jobs select shell completion, timeout, or cancellation, then join stdout/stderr readers outside that protected select (`background_job.rs:486-522,982-1029`). A descendant retaining a pipe after its shell exits can strand the job. The helper only escalates group termination when the leader is still alive (`tool_support/process.rs:54-65`). Ordinary bash already uses a more hardened `local_process` path; this is not a claim that all shell cleanup is broken.

External ACP agents are a separate case from native children: each prompt/continuation starts an adapter process (`external_agent/acp_run.rs:340-388`). ACP host terminals create three threads per terminal and direct-child kill does not establish descendant cleanup (`external_acp.rs:621-627,846-889,958-964`). These host-created terminals are not necessarily descendants of the ACP adapter process group.

Recommendation: reuse hardened process ownership, include pipe draining in deadlines, and test leader-exit/TERM-resistant descendants and external-adapter cancellation. These edge cases were source-audited, not reproduced against the running application.

### 9. Medium: queueing and lock waits can look like slow agents

LSP operations serialize on each client's mutex across blocking request/response work. Cold initialization happens while the service client-map lock is held (`lsp_service.rs:1456-1558`). Thus unrelated cold starts can block each other, and the advertised 600 ms post-edit diagnostic budget does not include mutex wait and up-to-30-second initialization. Mutation tools touch changed files sequentially. Request timeout does not send `$/cancelRequest` in the inspected client.

Recommendation: same-key single-flight startup outside the global client-map lock, a bounded startup admission budget, cancellation-aware per-client scheduling, and interactive priority over obsolete background diagnostics. Measure queue wait separately from server response time.

## What the upstream projects actually do

### Codex

Pinned upstream: [`openai/codex@2816088b68069dd5ce1f71a653cd956eb367c28e`](https://github.com/openai/codex/tree/2816088b68069dd5ce1f71a653cd956eb367c28e).

- Native child agents are in-process sessions sharing substantial infrastructure, not complete CLI processes. Neoism already has this important property for native children.
- Default legacy child limit is six and default nesting depth is one. The optional V2 default is four session slots, effectively three children plus the root; V2 has execution/residency admission and idle-runtime eviction, but its feature is not enabled universally. Source: [configuration defaults](https://github.com/openai/codex/blob/2816088b68069dd5ce1f71a653cd956eb367c28e/codex-rs/core/src/config/mod.rs#L260-L270), [residency](https://github.com/openai/codex/blob/2816088b68069dd5ce1f71a653cd956eb367c28e/codex-rs/core/src/agent/control/residency.rs#L35-L109).
- No built-in LSP engine/tool was found in the inspected core. MCP, extensions, dynamic tools, or shells can provide language intelligence, but this is not the same default indexing workload as Neoism's editor-plus-agent stack. Source: [tool handlers](https://github.com/openai/codex/blob/2816088b68069dd5ce1f71a653cd956eb367c28e/codex-rs/core/src/tools/handlers/mod.rs#L1-L37).
- Unified execution bounds retained command output and uses a soft 64-entry process-manager cap, but that manager is per session and the count does not include every descendant. It is not a proven global CPU/RAM limit. Source: [constants](https://github.com/openai/codex/blob/2816088b68069dd5ce1f71a653cd956eb367c28e/codex-rs/core/src/unified_exec/mod.rs#L70-L75).
- Agent/session shutdown explicitly handles descendant agents and command processes. Forked context is opt-in by default; Codex still has per-session histories and an unbounded outgoing event channel. It is not universally memory-bounded.
- Cloud-task submission is a separate remote execution path. Fifty logical/cloud/network-waiting agents is not evidence for fifty simultaneously CPU-heavy local workers on the same laptop.

### OpenCode

Verified active repository: `anomalyco/opencode`, not the archived unrelated Go project. Current development HEAD was [`1bf08ae46a91f7793c756e4ee521e37b6b554064`](https://github.com/anomalyco/opencode/tree/1bf08ae46a91f7793c756e4ee521e37b6b554064). Its replacement runtime has explicit LSP TODOs, so using only that HEAD for this comparison would be misleading. Released `v1.19.5` is pinned at `810c0eba1a7348036f8bdf61e6632d0c36c6876c`; the inspected LSP/task/instance-store blobs are identical to maintained v1 commit [`006d837c5022ea5278162307c04c78412cfae9f3`](https://github.com/anomalyco/opencode/tree/006d837c5022ea5278162307c04c78412cfae9f3), used by the source links below.

- Released v1 shares LSP clients by directory instance, server root, and server ID. Pending starts for the same key join. Ordinary task children inherit that instance instead of launching a complete executable or creating a worktree. Separate directories/worktrees/processes have separate pools. Source: [client lookup/startup](https://github.com/anomalyco/opencode/blob/006d837c5022ea5278162307c04c78412cfae9f3/packages/opencode/src/lsp/lsp.ts#L208-L288).
- Startup is lazy, but successful text reads warm LSP in the background; writes/edits/patches touch and wait for diagnostics. Neoism does not unconditionally start LSP on ordinary reads, avoids unchanged-content resynchronization, and has a shorter post-edit diagnostic wait. Copying OpenCode's read-touch behavior would not automatically reduce local work.
- Released v1 also lacks `didClose`, document LRU, idle-server eviction, and a maximum live-client count. It retains document text and diagnostics until instance disposal. It is not a model for bounded LSP memory. Source: [document synchronization](https://github.com/anomalyco/opencode/blob/006d837c5022ea5278162307c04c78412cfae9f3/packages/opencode/src/lsp/client.ts#L553-L620).
- Its Rust adapter also starts plain rust-analyzer, without a universal lightweight CPU/RAM policy. Source: [Rust adapter](https://github.com/anomalyco/opencode/blob/006d837c5022ea5278162307c04c78412cfae9f3/packages/opencode/src/lsp/server.ts#L890-L932).
- Unix LSP shutdown signals the immediate child; Neoism already has stronger process-group ownership in its main LSP client, although leader-exit edge cases still need scrutiny. Source: [OpenCode stop](https://github.com/anomalyco/opencode/blob/006d837c5022ea5278162307c04c78412cfae9f3/packages/opencode/src/util/process.ts#L149-L162).
- Released v1 loads paginated history into an accumulated array before compaction filtering. Current dev improves SQL history windows and completed-job retention, but still has unbounded live subscriptions. Neither version proves zero-overhead or fifty-way active local scalability.

Claude Code's proprietary implementation was not source-audited. No source evidence or matched benchmark verified the claimed fifty-agent comparison for either requested open-source project.

## Fix order and verification gates

1. Add counters for live servers/documents, runtime owner, process-tree resources, pending sync bytes, local job permits, durable event backlog, and queue wait. Keep this low-overhead and avoid recording source text or credentials.
2. Fix document leases and close/worker teardown; bound and coalesce sync snapshots while preserving save/query ordering. Test multi-surface close, agent/editor coexistence, blocked startup, queued save/query barriers, reconnect, and runtime shutdown.
3. Consolidate editor/agent ownership within compatible workspace/security boundaries. Test that several same-workspace children plus an editor start one server, while different tenant/configuration/worktree identities remain separate.
4. Add shared heavy-local-work admission, including detached jobs and server cold starts. Verify cancellation releases permits and parent/child wait patterns cannot deadlock; keep cheap reads responsive.
5. Bound snapshot retention and durable backlogs without weakening undo or event durability; harden background/ACP process-tree teardown.
6. Run identical 1/3/5/10-agent workloads in isolated installations: read-only search, explicit LSP, edits, and builds separately; same-root versus worktree; cold versus warm. Record CPU/PSS, active processes, runnable threads, disk I/O, PSI/swap-in/out, tool p50/p95 latency, UI frame times, and resource recovery after all tasks finish.

Success should mean bounded expensive local work, no duplicate compatible server ownership, no continued growth from closed documents/jobs, and a responsive editor during admitted agent work. It should not mean merely increasing an agent-count setting or comparing local compilation with remote API wait time.