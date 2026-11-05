# ACP plumbing deep dive: T3 Code and Neoism

This report records the pre-implementation investigation. The first implementation follows the user's revised priority: [external agent orchestration](agent-acp-orchestration.md) before direct-runtime selection.

## Scope and conclusions

This is a source-backed investigation, not an implementation or a live-provider certification. T3 Code is pinned to `337c5858c33bc7f8f6dfd391a2c279fa0f7e4d17`; Neoism is at `8282496ddbf44447580b1263aa0dda3efc4b7d3f` with the current worktree. No provider was installed, signed in, or given a real prompt. The [initial audit](agent-acp-support-audit.md) contains the provider overview and pinned upstream links.

Neoism does not need a replacement ACP client. It needs current-frontend wiring, explicit runtime ownership/capabilities, stronger external-task controls, and provider qualification. Antigravity is an additional integration, not another model name. Native Neoism should remain the default execution path and the owner of delegated tasks.

References prefixed `N/server/` are relative to `neoism-agent/crates/neoism-agent-server/src/`; `N/gpuix/` means `neoism-frontend/gpuix/src/`; `T/` means the pinned T3 repository root. Line references describe this source snapshot, not permanent API locations.

## 1. T3's architecture is larger than ACP

### Separate six identities

| Identity | Purpose | T3 contract |
| --- | --- | --- |
| Driver | Integration implementation: Claude SDK, Codex app-server, OpenCode native, Antigravity ACP, generic ACP. | `ProviderDriver.driverKind`, `T/packages/provider-core/src/server/driver.ts:25-78`. |
| Configured instance | One configured runtime/profile and its status/models/auth controller. | `ProviderInstance`, `driver.ts:95-115`. |
| Credential domain | Instances whose credential mutations must coordinate. Not necessarily one account per instance. | `ProviderAuthController.credentialBinding`, `T/packages/provider-core/src/server/auth.ts:12-30`. |
| Continuation identity | Whether a saved native conversation can move between instance selections. | `ProviderContinuationIdentity`, `driver.ts:80-92`. |
| Provider thread | The provider-owned native conversation and resumable identity. | `T/packages/provider-core/src/server/ProviderAdapter.ts:458-500`. |
| Turn/run | A particular attempt, live requests, outputs, and terminal outcome. | `ProviderAdapter.ts:83-181,458-500`. |

Neoism already separates native session IDs from ACP session IDs, but its external runtime is currently one of three hardcoded providers, not a general configured-instance/account abstraction. Do not introduce T3's full object graph just to expose those three existing runtimes. Do preserve the identity distinctions when adding accounts, custom commands, or delegation targets.

The default T3 continuation key is instance-based, not a fingerprint of executable/profile/configuration. Even T3's identity contract does not prove that changing the executable under an existing instance preserves conversation compatibility.

### Three registries with different jobs

`ProviderInstanceRegistry` owns live instances and their scopes, `ProviderRegistry` publishes cached status/models, and `ProviderAdapterRegistryV2` serves execution adapters and guards credential changes. Hydration subscribes before reading settings, then reconciles current settings rather than trusting potentially stale event payloads. Disabled instances avoid secret reads; an unreadable configured secret prevents startup rather than falling back to inherited credentials.

Changed/removed entries are unpublished and their scopes closed before replacement construction. Whole-entry comparison can rebuild an instance even for metadata changes. Cached model/auth status may deliberately outlive an unsuccessful probe, so picker readiness is not execution proof. Sources: `T/apps/server/src/provider/ProviderInstanceRegistryHydration.ts:125-189`, `ProviderInstanceRegistry.ts:125-289,312-384,439-532`, `ProviderRegistry.ts:134-296`.

### One adapter boundary, several native transports

`ProviderAdapterV2` exposes capabilities, selection-transition planning, and scoped session creation. Its runtime supports thread activation, turns, steering/interruption, requests, snapshots, and optional compaction/fork/background work/MCP Apps. The common event vocabulary distinguishes reusable versus broken threads, requests awaiting user interaction, transient command output, and final outcomes. A thread-in-use error is not permission to silently create a replacement conversation; an interruption timeout means stopping was not confirmed. Sources: `T/packages/provider-core/src/server/ProviderAdapter.ts:238-253,364-375,607-713`.

This boundary normalizes user-visible behavior; it does not make the transports equivalent. Claude uses the Claude Agent SDK, Codex uses app-server, and OpenCode has version-specific native integrations. Antigravity and generic registry agents use shared ACP compatibility machinery. Replacing Neoism's existing ACP integrations with native bridges would be a separate, evidence-driven decision.

### The actual composer-to-provider path

T3's canonical selection is `ModelSelection { instanceId, model, options? }`; options are string/boolean values with provider descriptors. There is no separate `accountId` in this selection: the instance's auth controller and published auth status describe account binding. Also, T3's `runtimeMode` means permission/approval mode, not agent implementation. Sources: `T/packages/contracts/src/modelSelection.ts:9-49`, `model.ts:7-55`, `server.ts:63-86,207-267`.

```text
ProviderModelPicker / ChatComposer
  -> useComposerSelection stages selection in composer draft
  -> sticky options keyed by instance + model
  -> useComposerSend captures next-turn selection
  -> client startThreadTurn
       draft: launchThread with workspace strategy + initial message
       existing: message.dispatch with modelSelection/context/attachments
  -> WebSocket admission + ThreadMessageIntake
  -> EventSink transaction: receipt + events + projections + queued effects
  -> EffectWorker provider-turn.start
  -> ProviderTurnStartService / RunExecutionService
  -> ProviderAdapterRegistry.get(instanceId)
  -> provider-native session/turn protocol
  -> normalized provider events -> durable projections + transient output
  -> sequence-based snapshot/replay/live tail -> UI
```

Sources: `T/apps/web/src/components/chat/useComposerSelection.ts:93-109,142-214`, `useComposerSend.ts:1270-1368`; `T/packages/client-runtime/src/operations/commands.ts:648-781`; `T/apps/server/src/ws.ts:2008-2044,845-934`; `orchestration-v2/EventSink.ts:635-708`, `EffectWorker.ts:141-153`, `ProviderTurnStartService.ts:1583-1728`, `ProviderSessionManager.ts:1943-2035`.

The draft is not the live runtime or the captured run selection. Existing-chat permission/interaction changes use separate setting operations, not only fields on `message.dispatch`. T3 locks incompatible started-thread provider changes unless handoff is available and explains why; handoff transfers context rather than transplanting native session identity. Source: `T/apps/web/src/components/ChatView.logic.ts:916-939` and `chat/ProviderModelPicker.tsx:76-114`.

### App-owned delegation is above ACP

`delegate_task` targets a provider instance or driver, optional model/options, project/environment, isolation, wait/async mode, and request ID. The server requires a thread-scoped credential and active parent run owned by the calling instance. It resolves an available project-enabled target, validates explicit model/options when catalogs exist, and prevents child permission escalation. A role is task text, not an elevated execution identity. Sources: `T/packages/contracts/src/orchestratorMcp.ts:96-195`; `T/apps/server/src/mcp/OrchestratorMcpService.ts:1330-1468,2360-2475`; `delegatedTaskModes.ts:12-94`.

The orchestrator creates an ordinary durable child thread, `origin: app_owned` subagent projection, execution node, and child `message.dispatch`. It passes task text, not the entire parent history or attachments. Native provider subagents instead use `origin: provider_native` and adapter-owned native refs. Both can appear in one UI, but control authority and recovery differ. Sources: `T/apps/server/src/orchestration-v2/Orchestrator.ts:7748-8016`; `T/packages/contracts/src/orchestrationV2.ts:884-918`.

There is no separate `task_steer`: `t3_thread_send` addresses the child thread using auto/queue/steer/restart semantics. `task_status` includes pending interactions, child activity, actual selection, result transfer, and next-action guidance. Async completion wakes the parent; a waiting call defaults to ten minutes and its timeout does not cancel the task. Sources: `T/apps/server/src/mcp/toolkits/orchestrator/tools.ts:86-123,271-282`; `OrchestratorMcpService.ts:1474-1678,2513-2557,3109-3157`; `orchestration-v2/ThreadManagementService.ts:813-859`.

Outcome and result delivery are separate persisted state. Result transfers deduplicate finalization; delivery tracks pending/claimed/acknowledged/delivered/disposed, with generation-tagged parent cohorts and sibling batching. Completion can steer a live parent only when steering does not interrupt its tools; otherwise it queues. Restart recovery reconciles children and reoffers owed deliveries rather than assuming processes survived. Sources: `T/apps/server/src/orchestration-v2/Orchestrator.ts:5804-5887,12286-12668,12868-13009,13964-14102`; `ProviderRuntimeRecoveryService.ts:811-864`.

T3 idempotency is conditional: command receipts derive from request namespace + operation + supplied request ID. Missing request IDs get fresh identities, and a fresh MCP credential namespace can change deduplication scope. This is not exactly-once external side effects. Its MCP registry hashes bearer tokens and binds thread/instance/capabilities with revocation. Sources: `T/apps/server/src/mcp/OrchestratorMcpService.ts:677-692`; `McpSessionRegistry.ts:97-155,174-191`.

Shared checkout is the default. Worktree isolation uses the resolved parent `HEAD`, not its uncommitted changes; it does not isolate credentials, processes, or network. Cancellation recursively stops child conversations/descendants and disposes outstanding delivery, while preserving an already-terminal result. Remote delegation adds durable remote-child and stop obligations rather than forwarding a fire-and-forget RPC. Sources: `T/apps/server/src/mcp/OrchestratorMcpService.ts:2423-2451,2563-2745`; `T/apps/server/src/peer/RemoteDelegation.ts:63-83,259-326,554-575`.

## 2. T3 runtime lifecycle and compatibility

### Warm processes and explicit transitions

T3 retains live provider runtimes across turns. Its session manager defaults to 30-minute idle release, has bounded background-work pinning, and guards idle teardown by generation/runtime identity so an old timer cannot kill a replacement. Interrupt, unload, and scope-close operations have separate bounds. Sources: `T/apps/server/src/orchestration-v2/ProviderSessionManager.ts:57-66,1114-1233`.

Selection changes are planned as `apply_on_next_turn`, `restart_session`, `create_with_handoff`, or `reject`. A changed driver or continuation identity can require a handoff; a compatible profile/workspace/runtime-mode change may restart and resume. Source: `T/apps/server/src/orchestration-v2/ProviderSessionTransitionPolicy.ts:32-87`. Neoism can initially use a simpler rule: switching runtimes starts a new conversation, never silently rebinds an existing one.

T3 stages replacement ACP runtimes and generation-tags callbacks so stale replies cannot mutate replacement state. This is useful if Neoism later keeps warm processes; the current fresh-process-per-turn design does not require copying that entire manager immediately. Source: `T/packages/provider-acp/src/server/adapter.ts:2357-2453,6886-6978`.

### ACP v2 is not a version-number change

T3 requests protocol version 2, but its compatibility client also inspects initialization response shape. Some Antigravity versions report 2 with a v1-shaped response. Authentication and other operations then route through the appropriate compatibility API. Sources: `T/packages/provider-acp/src/server/AcpSessionRuntime.ts:2085-2099`, `T/packages/effect-acp/src/client.ts:1075-1117`.

Neoism requests and requires version 1 in `N/server/external_acp.rs:15,201-226` and `external_agent/acp_run.rs:148-169`. Supporting another protocol generation needs decoding, method/event compatibility, and fixtures. Blindly changing the constant could break the existing pinned adapters.

### Discovery, selection, and effective state are different

T3 probes ACP agents using disposable sessions, discovers auth/models/options, and combines that with live session updates. Generic discovery is bounded and does not expose client file/terminal execution services. It can still trigger package installation through registry resolution; it is not always a side-effect-free read. Sources: `T/packages/provider-acp-registry/src/server/probe.ts:317-458`, `AcpRegistrySupport.ts:423-457,1827-1917`.

Its option descriptors bound the number of controls/choices and handle legacy modes. Configuration applies model, ordinary options, provider policy, and interaction mode before turns. Agent-pushed updates have generations so a newer notification wins over a stale setter response. However, some unsupported choices or setter failures are skipped/logged: desired selection is not necessarily confirmed effective selection. Sources: `T/packages/provider-acp/src/server/sessionConfig.ts:6-14,55-143`, `adapter.ts:7192-7444,8037-8077`, `AcpSessionRuntime.ts:2030-2082`.

Neoism's existing provider-confirmed options contract should be retained rather than weakened to match that fallback behavior. Its current compatibility gap includes legacy model selection; config select options and legacy mode selection already exist.

### Recovery does not mean replaying a prompt

T3 distinguishes native load/resume from new-session creation. It has bounded cancellation confirmation and provider-specific inactivity recovery that invalidates callbacks, fails open work, closes the runtime, and requires restart. It does not make an unknown-outcome executed turn safe to replay automatically. Sources: `T/packages/provider-acp/src/server/AcpSessionRuntime.ts:1953-2026,2135-2283,2366-2408`, `adapter.ts:7780-7868`.

Capability provenance also matters: base ACP rollback is app-emulated by starting a fresh native session, not proof of native history truncation. Generic ACP has no portable native-child control contract. Source: `T/packages/provider-acp/src/server/adapter.ts:732-738,9259-9300`. Neoism should distinguish native support, app emulation, and observed/read-only projections.

## 3. Neoism's existing execution pipeline

### Direct external conversation

```text
CreateSessionRequest.externalProvider + externalOptions
  -> create_session_in_directory validates host execution and provider
  -> session.extra.externalAgent stores explicit ACP ownership
  -> durable session prompt queue
  -> session_prompt dispatches root_runtime(session)
  -> execute_external_root_prompt
  -> run_external_subtask_prompt_with_cancel
  -> run_acp_prompt
  -> spawn / initialize / new-or-load-or-resume / reapply confirmed options
  -> session/prompt
  -> normalized Neoism message/tool/plan/usage events
  -> persist result, settle nested projections, finish run
```

Sources: `N/server/session_routes.rs:320-367`, `session_prompt.rs:89-91`, `external_agent/lifecycle.rs:361-409,480-609`, `external_agent/acp_run.rs:340-519`.

The GPUIX new-thread request does not currently supply the external fields (`N/gpuix/model.ts:1466-1468`), although the SDK generated contract already admits them. The old shared Rust pane's option UI is a different surface and must not be counted as completed GPUIX wiring.

### Missing frontend and authorization edges

| Layer | What is present | What remains |
| --- | --- | --- |
| Composer state and persisted drafts | Native agent/provider/model/connection/thinking state. | `neoism-frontend/service-client/src/protocol.ts:50-70` has no external ownership/options. Extend canonical state rather than only adding picker labels. |
| New-thread creation | SDK creation accepts external fields; server overrides synthetic native identity appropriately. | `N/gpuix/model.ts:1457-1478` does not send them. Separate native API account/model state from external selection. |
| Existing-session restoration | Sidebar recognizes explicit ACP root source; server retains real execution ownership. | `N/gpuix/daemon/sdk.ts:7-8` drops external metadata; daemon switch returns native state (`neoism-workspace-daemon/src/agent/handlers.rs:111-144`). Hydrate canonical external identity/options in the composer. |
| TypeScript operations | Generated preview/catalog/import/session-option operations exist. | High-level external helper collection and discriminated option DTOs are absent; low-level `operations.request` is usable. Sources: SDK `packages/core/src/client.ts:86-143,193-195`, `generated/contract.ts:265-266,400-403`. |
| Desktop authority routing | Workspace-scoped requests plus daemon-confirmed system workflow access for reopened sessions. | `neoism-agent/crates/neoism-agent-service-api/src/daemon_credential.rs:189-263` does not admit external preview/catalog/import/session-option routes. Choose scope deliberately and add only intended concrete grants; do not bypass authorization or broadly allow plugins. |
| Browser ingress | Session paths allowed subject to upstream checks; GET preview is allowlisted. | `neoism-frontend/gpuix/native/src/fast_servers.rs:759-799` blocks native external catalog/import and POST preview. Browser parity needs an explicit policy, not automatic host-history exposure. |
| Native history import | Backend authorizes canonical cwd, source host, replay, and idempotent import. | GPUIX lacks catalog/import UI. Imported text-only roots are intentionally filtered from normal lists (`N/server/v2_routes.rs:992-997,1052-1058`); use the authorized external catalog path rather than deleting this ownership filter. |

Reopened ACP prompts remain externally routed even if GPUIX sends native model/agent defaults: `session_prompt.rs` dispatches on persisted ownership before processing those defaults. However, native picker changes also PATCH mutable session model/agent metadata (`N/gpuix/model.ts:1348-1373`, `N/server/session_routes.rs:655-684`). That can make restored/displayed state misleading without changing actual runtime ownership. Canonical hydration and server mutation validation are both needed.

### Unsupported native actions must not leak into external sessions

| Current GPUIX action | Boundary to fix |
| --- | --- |
| Native undo/redo/revert | Neoism history/revert markers change without ACP history rewind. Server handlers need external-aware rejection or a real qualified provider operation, not merely a hidden button (`N/server/session_undo.rs:367-429`). |
| Native compact/summarize | Admission queues Neoism compaction, not external compaction (`N/server/session_queue.rs:58-76`). The native compaction path can select a native compaction model/provider; don't expose it as provider `/compact`. |
| Native slash catalog/templates | GPUIX does not consume persisted external advertised commands. Native template/agent resolution can run before ACP dispatch (`N/gpuix/agent/SlashController.ts:390-439`, `N/server/session_actions.rs:3257-3356`). Define command ownership and route only confirmed provider commands or explicitly labeled app actions. |
| Attachments | GPUIX emits file parts; external root handling rejects non-text parts (`N/gpuix/daemon/sdk.ts:20-28`, `N/server/external_agent/lifecycle.rs:377-391`). Disable until negotiated translation exists. |
| Native model/thinking/account controls | Do not set ACP options, and can PATCH misleading native metadata. Bind to provider-confirmed external options instead. |
| Directory change | Stored directory can change without revalidating imported native source identity (`N/server/session_routes.rs:685-740`). Specify whether existing external sessions can move and revalidate host/cwd/continuation accordingly. |
| Direct Neoism shell | An app operation, not proof that the external provider implements a corresponding tool/slash command. Preserve this ownership distinction. |

The first direct-use increment must reject unsafe/unsupported actions server-side as well as gate them in the UI. Implementing additional external actions can follow later; honest rejection cannot.

### Native Neoism delegating to an external runtime

```text
Neoism model emits task(subagent_type = claude | codex | opencode)
  -> plugin task dispatcher checks plugin, execution policy, depth
  -> execute_external_task creates or continues a child session
  -> foreground result OR background tracker
  -> same external prompt runner and ACP transport as direct chat
  -> durable task outcome + parent completion outbox
  -> parent Neoism continuation and/or task_result inspection
```

Sources: `N/server/tool_registry.rs:440-473`, `plugins/subagents.rs:84-143,392-464`, `external_agent/lifecycle.rs:41-185,412-609`.

Child sessions inherit directory, workspace, permissions, tenant, and execution-policy metadata; creation currently clones the parent's checkout. There is no external-task worktree/isolation argument in this dispatch. A depth limit of three exists; generic tool execution concurrency is ten per stream, but that is not a demonstrated global limit on background external processes after task calls return. Sources: `N/server/external_agent/lifecycle.rs:188-256`, `tool_runtime.rs:780`, `provider_stream_processor.rs:33,1014-1052`.

### Completion durability is already substantial

External tasks use the shared generation-keyed completion pipeline. Completion is durable session state, not dependent on a live UI tracker; the pending outbox drives parent continuation, reconciliation, and completion events. Execution activity separately tracks outstanding branches and provider segments. Result reads avoid reusing an older generation's final answer. Sources: `N/server/session_actions.rs:400-505,795-894,947-992`, `execution_activity.rs:479-554`, `tool_runtime.rs:1045-1133`.

This is not missing infrastructure to rebuild. Extend it with explicit external controls and focused tests. A server restart can retain identity/outcomes and reconcile stale work; it does not mean an interrupted external model call resumes mid-generation.

### Event projection is not native child ownership

`external_agent/events.rs` maps text, tools, plans, usage, commands, and option updates into Neoism state. Provider-confirmed selected models are recorded at turn end; this is not proof that a provider never switched models internally. Provider-owned `task`/`spawn_agent` tool calls can produce synthetic nested Neoism sessions with `externalAgent.nested: true` and a parent tool-call ID, rather than a real resumable external child-session ID. Sources: `N/server/external_agent/events.rs:109-280,878-898,947-1039`, `acp_run.rs:521-550`.

Keep these observed provider-native children distinct from Neoism-created external children. Synthetic children are presentation projections; their existence must not imply independent resume, steer, or stop capability.

## 4. Concrete lifecycle risks in Neoism

These are source-level findings to resolve with focused regressions, not failures reproduced against a live provider.

| Priority | Finding | Evidence and required behavior |
| --- | --- | --- |
| P1 | Reopened external sessions lose composer ownership and expose unsupported native operations. | Thread/provider schemas and DTO hydration omit external state; native history/compaction handlers have no matching provider operation. Carry explicit ownership end to end and enforce capabilities on the server. |
| P1 | External option calls encounter a missing desktop capability grant. | System workflow allowlist omits external routes. A picker implementation using session-scoped SDK calls needs deliberate endpoint admission and scoped-route tests. |
| P1 | A follow-up to an active external child is not delivered. | `N/server/external_agent/lifecycle.rs:90-101` returns running without enqueuing `prompt`; native tasks have an explicit queue/steer path in `plugins/subagents.rs:201-242`. Either accept and deliver the prompt with clear semantics or return an explicit rejection. |
| P1 | External continuation validates ancestry, not runtime compatibility. | `lifecycle.rs:55-64` calls `helpers.rs:270-295`, which checks parent ownership only. A saved Claude child can be requested as Codex; background status mutation changes provider metadata before setup, while `external_session_id` trusts that metadata. Reject mismatched provider/origin/continuation identity before any mutation or launch. |
| P1 | Foreground child cancellation can monitor the wrong flag. | `lifecycle.rs:171-181,503-512` passes the parent cancellation flag and selects it instead of the child's new run flag. `session_actions.rs:129-136` aborts the child's run flag. Verify and link both sources so stopping a foreground child actually cancels the provider rather than only marking lifecycle state. |
| P2 | A supplied unknown task ID can create a new external child. | `lifecycle.rs:55-74` falls through to creation on store miss. A continuation request should fail clearly, not silently create new work or avoid the new-child depth check. |
| P2 | Task targets do not include provider-confirmed options/model or configured instance. | Tool schema `tool_registry.rs:440-473`; external dispatcher consumes provider name/prompt/background/task ID. Introduce explicit target options without treating Neoism API models as external models. |
| P2 | Native-child projections need explicit control gating. | `events.rs:979-1007` stores observational metadata, not a native child conversation identity. Reject independent control unless the provider actually supplies and supports it. |
| P2 | Background process/resource budgeting needs qualification. | Per-stream tool concurrency and depth limits do not establish a durable global external-process/account budget. Define admission, queued state, and cancellation behavior before automatic fan-out. |

These findings do not invalidate the root-session ownership checks, options replay, or existing cancellation cleanup. They identify the seams needing qualification before orchestration is advertised as complete.

## 5. Permissions, credentials, and installation boundaries

Neoism bridges `session/request_permission`, client-served filesystem reads/writes, and client terminal execution through its permission paths; terminal execution defaults to ask, and inherited deny rules are tested. It also enforces host-native-execution policy before external root and task execution. Sources: `N/server/external_agent/requests.rs:3-103,135-215,250-438`, `tests.rs:320-380`, `lifecycle.rs:495-501`.

This is not an OS sandbox around every native provider tool. T3 explicitly calls its ACP policy enforcement `client-boundary`; it has the same limitation. Its command approval can grant execution for a turn/session, not prove a subsequent terminal command is exactly the approved command. Source: `T/packages/provider-acp/src/server/clientPolicy.ts:40-199,224-249`.

Neoism's generic ACP setup currently sends `mcpServers: []`, and prompts are text-only (`N/server/external_agent/acp_run.rs:260-264,442-453`). Letting an external root delegate back into Neoism requires a separate scoped MCP bridge with project/session binding, limited tools, recursion limits, revocation, and explicit authority. It is not required for Neoism's native agent to delegate outward.

Current launchers inherit the execution host environment. Runtime identity is not credential isolation. Custom instances/accounts need an explicit profile/home/environment policy and credential-change admission rules; don't assume a different instance ID creates a different account. T3 separates credential domains and gates startup during matching credential mutations (`T/apps/server/src/orchestration-v2/ProviderAdapterRegistry.ts:71-123`), but generic inherited environments are still overlays, not a sandbox (`T/packages/provider-core/src/server/instanceEnvironment.ts:7-35`).

Generic ACP registry support is also a supply-chain feature. T3 requires exact package versions, launches local commands without shell expansion, and verifies binary hashes when supplied; discovery may install artifacts. Neoism should begin with explicit executable plus literal arguments and separate deliberate installation, rather than installing third-party agents as a hidden side effect of opening the picker.

## 6. Antigravity-specific work

T3's Antigravity integration has official runtime/helper acquisition, private per-instance profiles, environment cleanup, selected credential injection, bounded authentication flows, credential-mutation process ownership, model refresh, and remote callback handoff. The callback validator checks exact loopback origin/path/state and code-or-error, avoids redirects/proxies, and bounds forwarding. Sources: `T/apps/server/src/provider/AntigravityInstallation.ts:329-458`, `antigravityAuthSupport.ts:178-255`, `AntigravityAuth.ts:363-516`, `antigravityCallback.ts:12-113`.

Important qualifications from `T/apps/server/src/provider/acp/AntigravityAcpSupport.ts:65-149` and `orchestration-v2/Adapters/AntigravityAdapterV2.ts:96-123,153-187,234-285`:

- Authentication can need eager provider-specific setup and continuation can prefer resume rather than load.
- Client file capability advertisement affects native edit behavior; T3 deliberately does not advertise client writes for this provider.
- Some model access depends on client identity allowlisting; T3 identifies this runtime as `zed`. Neoism must obtain/qualify its supported identity rather than blindly copy impersonation as a product guarantee.
- Version-specific patches disable a workflow operation that can overwrite a workspace skill file and qualify system-prompt/video behavior.
- Native child events lack real child session IDs, so native children remain read-only.
- Manual `/compact` and native history rewind are not uniformly available; a slash word can be ordinary model input on an unsupported version.
- Native tool/read events and outcome reporting need provider-specific repair in T3.

An Antigravity enum entry plus command launch is therefore only a prototype. A supported local integration needs qualified protocol/auth/session/options/approval/cancellation behavior first. Managed installs, multi-account isolation, and remote sign-in are separately scoped deliverables.

## 7. Ordered delivery and acceptance

| Stage | Deliverable | Minimum acceptance |
| --- | --- | --- |
| 1 | GPUIX direct use of existing Claude/Codex/OpenCode runtimes, Neoism first/default, with safe capability guards. | Create with explicit runtime/options; deliberate scope grants; preserve drafts and reopened ownership; provider-confirmed models; stream tools; answer approvals; cancel; continue after reconnect; reject unsupported native mutations/attachments; no silent fallback to native execution. |
| 2 | External command/history discovery and richer qualified controls. | Add authorized external catalog/import and advertised commands; handle text-only import honestly; implement additional options/actions only where supported. Synthetic native children stay read-only unless independently controllable. |
| 3 | Reliable Neoism outward orchestration. | Target external options, reject mismatched/missing task IDs, deliver or reject active follow-ups, stop both foreground/background children, preserve exact-generation parent completion across reload, define shared-checkout versus worktree policy and resource admission. |
| 4 | Qualified official Antigravity local ACP integration. | Verify executable/helper and protocol shape; explicit host auth; model/options discovery; approvals/questions; cancellation/cleanup; persisted continuation; truthful capability exclusions. Expand API/SDK/provider admission consistently. |
| 5 | Configured instances and generic local ACP commands. | Stable instance/continuation/credential-domain ownership; literal argv; deliberate installation; clear unavailable/auth states; no inherited-credential fallback for explicit credentials. |
| 6 | Optional advanced features. | Native Codex/Claude bridges only for measured ACP gaps; warm runtime reuse after benchmarks; scoped inbound MCP orchestration; managed registry install; multi-account and remote OAuth handoff. |

Do not block the first GPUIX increment on a generic registry, native SDK rewrite, warm-session manager, or full T3-style handoff architecture. Do not claim that the picker increment alone completes orchestration.

### Concrete first-increment touchpoints

- State and hydration: `neoism-frontend/service-client/src/protocol.ts`, `N/gpuix/model.ts`, `N/gpuix/daemon/sdk.ts`, and `neoism-workspace-daemon/src/agent/handlers.rs` (or replace its restoration dependency with canonical SDK hydration).
- Composer and command ownership: `N/gpuix/ui/App.tsx`, `N/gpuix/agent/SlashController.ts`.
- Endpoint authority: `neoism-agent/crates/neoism-agent-service-api/src/daemon_credential.rs`; browser ingress separately in `neoism-frontend/gpuix/native/src/fast_servers.rs`.
- Server invariants: `N/server/session_routes.rs` for metadata/directory mutations; `session_undo.rs`, `session_queue.rs`, and `session_actions.rs` for unsupported history/compaction/native command paths.
- Later orchestration work: `N/server/tool_registry.rs`, `plugins/subagents.rs`, `external_agent/lifecycle.rs`, and `external_agent/helpers.rs`, reusing existing completion state rather than creating another task manager.

## 8. Verification evidence and limits

Existing mocked coverage includes root HTTP queue/reload across all three providers (`N/server/external_agent/tests_lifecycle.rs:188`), native import/replay (`catalog.rs:960,1239`), options confirmation/reapply and ephemeral preview (`options_tests.rs:129,282,670,1075`), nested terminal output (`events_tests.rs:4`), permissions/child inheritance (`tests.rs:320`), and root ownership routing (`tests.rs:146`). Shared completion lifecycle/reconciliation tests exist in `session_actions.rs`; they are not automatically proof of external-task steering/cancellation/runtime-identity compatibility.

The attempted command was:

```sh
cargo test -p neoism-agent-server --lib external_agent::tests_lifecycle::three_provider_roots_run_via_real_http_queue_and_reload --offline -- --exact
```

The four-minute bound expired during compilation, after reaching `neoism-agent-server`; no test result was produced. This is neither a passing test nor a demonstrated test failure. No full suite or release build was run. Existing fixture coverage establishes test intent, not live authentication, entitlements, provider-internal policy enforcement, or platform cleanup guarantees.

The targeted GPUIX baseline command was also attempted from `neoism-frontend/gpuix`:

```sh
bun test ./tests/sdk.test.ts ./tests/session-selection.test.ts ./tests/agent-catalog-http.test.ts ./tests/slash-controller.test.ts
```

It could not start because `bun` is not installed/in `PATH`. No runtime was installed to make the research pass runnable. These tests cover current routing/native selection/catalog projection/slash behavior, not the missing external composer path.

Required new coverage should exercise actual operation routing and state transitions: external create/draft/restore, desktop/browser route admission, imported-session discovery, rejection of native mutations on ACP roots, child runtime/origin mismatch, missing continuation ID, active-child follow-up delivery, foreground/background cancellation, and parent delivery across restart. T3's referenced tests were inventoried, not executed.

Before shipping, distinguish four evidence levels: source implementation, passing mocked contracts, real local provider smoke tests, and user-facing end-to-end validation on supported platforms. The present research establishes the first and inventories the second; it does not establish the third or fourth.