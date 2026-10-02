---
name: "Plugin openness: Agent and IDE"
description: "Completed OpenCode-class Agent and Neovim-outcome IDE plugin openness architecture and implementation"
type: "project"
scope: "project"
origin: "agent implementation"
created: "2026-10-06"
updated: "2026-10-06"
---

# Plugin openness: Agent and IDE

Neoism Agent and IDE plugin-openness implementation is complete at the core platform/API level as of 2026-10-06. The full worktree remains dirty and uncommitted; preserve all concurrent changes.

## Agent

`neoism-agent` native `RegistrySnapshot` remains canonical. The versioned `neoism-plugin/2` process protocol now exposes safe native contributions for tools, hooks, events, agents, commands, skills, config, prompts, context, providers, streamed generation, routes, WebSockets, MCP metadata/resources/tools, policy, and custom message parts. Process frames are bounded and exact-instance/owner/generation scoped, with reverse host RPC, stream open/item/end/error, cancellation, one terminal result, stale suppression, revocation, and audit metadata.

Agent Lua runs in packaged `neoism-agent-lua-runner`, a supervised subprocess with an isolated serialized VM, memory/instruction/time/queue/stream budgets, contained require, no io/os/debug/native loading, exact-owner reverse calls, and lifecycle teardown. It is packaged for Linux, macOS, Windows, and MSI. No `mlua` enters agent server, daemon, renderer, worker, or WASM paths.

Plugin packages use JSON-only metadata discovery, one package identity/revision with explicit UI/editor/agent entrypoints, immutable revision/checksum binding, and external trust. Workspace packages are metadata-only until trust stored outside the repository. Activation supports global/user/workspace/session scope, candidate-first publication, exact session pinning, last-known-good rollback, lifecycle diagnostics, and disconnect/reload cleanup.

Host capability brokerage covers config, workspace resources, events, network, process/task execution, secrets, prompts/messages/responses, provider/policy access, dynamic revocation, cancellation, and auditing. The Agent side has scoped runtime composition, native approval routing, TypeScript/Lua bridges, lifecycle routes/DTOs, and cross-platform packaging.

## IDE

`neoism-lua` now has typed action/query/event/async-result contracts, generated annotations, exact `PluginOwner` validation, content-derived revisions, transactional candidates, and compatibility wrappers. It exposes opaque revision-safe document/pane/tab/workspace/selection/cursor/range handles, CRDT-owned edits, commands with aliases/schema/range/count/bang/completion/structured async results/cancellation, and bounded plugin/document/pane/tab/workspace state with atomic persistence.

Owner-scoped namespaces and CRDT sticky anchors feed immutable VM-free render snapshots. Plugins can contribute highlights, signs, virtual text/lines, inline widgets, code lenses, folds, concealment, diagnostics, related information/tags/actions, scoped options, augroups/nested events, key sequences, motions/operators/text objects, registers, clipboard, marks, jumplists, macros, and exact-revision cleanup.

The application-owned async layer supports timers, managed jobs/process streams, filesystem watchers, HTTPS network requests, credential aliases, notifications/progress/input/confirm/select prompts, clipboard reads, quickfix/location/result lists, bounded queues, cancellation, one terminal result, and application-thread Lua delivery. No Lua runs on timer/process/watcher/network/render/daemon threads.

Retained UI contracts cover picker/tree/table/list/form/modal/inspector/toolbar/breadcrumb/detail roles, virtual documents, scene nodes, focus/accessibility/state, Agent timeline/approval cards, and portable desktop/WASM snapshots. Platform contracts and native adapters cover completion/snippet presentation, extended LSP operations/runtime registration, Tree-sitter queries/artifacts, PTY/terminal handles, tasks/tests/watch/coverage, DAP, Git, Agent status/sessions/messages and native approval mutations, trusted subprocesses, and native extension hosting.

Final infrastructure includes an OS-keyring credential broker (Linux Secret Service, macOS Keychain, Windows Credential Manager), opaque aliases and scoped grants, host-owned digest/ABI/capability/scope-bound approvals, supervised versioned native ABI loading, checksum/platform/symbol/runtime validation, daemon-owned opaque remote resources for files/watches/tasks/tests/PTYs/DAP, and immutable Tree-sitter package/runtime ABI validation. Workspace code cannot approve itself or cause untrusted executable/native loading.

## Verification baseline

Delivery-first policy was used after user feedback: no repeated broad suites. Subagents reported passing targeted Cargo checks for affected Agent, protocol, daemon, extension, desktop, and shared crates; process-v2 wire and host checks; Lua runner sandbox/stream checks; TypeScript build/conformance; focused approval serialization/revocation; and diff checks. Existing unrelated warnings remain. Three pre-existing architecture-guard debt locations were reported in PTY leasing, `v2_routes.rs`, and `plugins/subagents.rs`; guard ceilings were not raised.

## Remaining non-core ecosystem work

Representative Neovim-style reference plugin ports, exhaustive stress/adversarial matrices, polished templates/migration guides, and broader ecosystem documentation remain useful follow-up work, but the core extension contracts, safe native adapters, trust model, Agent Lua runtime, IDE Lua runtime, remote resource brokerage, and trusted extension tiers are implemented.
