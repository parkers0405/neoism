# External agent support: ACP and the T3 Code comparison

For the deeper lifecycle, ownership, security, verification, and implementation-gap analysis, see [ACP plumbing deep dive](agent-acp-plumbing-deep-dive.md).

The subsequent implementation starts with [external agent orchestration](agent-acp-orchestration.md), not the direct-runtime composer picker. This audit records the pre-implementation source investigation.

## Goal and scope

Keep Neoism's own agent as the default and primary orchestrator. Let users choose Claude Code, Codex, OpenCode, or Antigravity directly from the composer, and let Neoism delegate work to those same runtimes as managed child tasks. External agents are harnesses with their own tools, authentication, sessions, and models, not additional entries in Neoism's API-model catalog.

This is a source investigation and implementation proposal, not a claim that new provider support has shipped. Neoism was inspected at `8282496ddbf44447580b1263aa0dda3efc4b7d3f` plus the current worktree; T3 Code was inspected at `337c5858c33bc7f8f6dfd391a2c279fa0f7e4d17`. No live provider turns, sign-ins, or runtime installations were performed.

## What T3 Code actually does

T3 uses a common provider adapter contract, but does not force all providers through ACP. Its built-in driver registry includes provider-specific integrations alongside a generic ACP Registry integration.

| Runtime | Inspected T3 integration | Implication for Neoism |
| --- | --- | --- |
| Claude Code | `ClaudeAdapterV2.ts` imports `@anthropic-ai/claude-agent-sdk` and calls `query`. | ACP is a viable interoperability route, but is not how T3's main Claude integration gets its richer native features. |
| Codex | `CodexAdapterV2.ts` uses `effect-codex-app-server`. | A native `app-server` adapter can be added behind the same interface if measured ACP limitations justify it. |
| OpenCode | `provider-opencode` selects version-specific native adapters; the v1 runtime uses `createOpencodeClient` and launches `opencode serve`. | Do not describe T3's built-in OpenCode support as just an ACP wrapper. |
| Antigravity | A dedicated driver/auth/installation layer uses the shared ACP adapter with Antigravity-specific behavior. | An ACP transport alone does not complete Antigravity integration. |
| Other ACP agents | Generic ACP Registry/local-command driver and shared ACP runtime. | Generic command registration can extend coverage without a new hardcoded runtime for every agent. |

T3's shared adapter normalizes messages, tools, approvals, plans, subagents, and terminal turn outcomes. Provider instances carry identity, configuration, credentials, capabilities, and continuation identity separately from the model choice. Its ACP runtime requests protocol version 2; its user documentation describes v2-preview support with negotiated v1 compatibility. Neoism currently requests ACP v1, so protocol-version support needs explicit compatibility work, not an unconditional version bump.

### Direct use and orchestration share the provider layer

T3's `OrchestratorMcpService.delegateTask` resolves a target by provider instance or driver and model, verifies availability and project enablement, and dispatches an app-owned child task. It supports shared-checkout or worktree isolation, async or waiting delivery, and idempotent request receipts. The same registered provider adapters back ordinary threads and child tasks.

T3 also supplies its MCP context to ACP runtimes. That lets an external agent use app-provided orchestration tools when enabled. This is distinct from displaying that agent's own native subagents. ACP does not supply a portable native child-lineage/control contract; T3's Antigravity integration explicitly treats its native children as read-only because those events lack child session IDs.

The useful pattern to adopt is shared runtime management and explicit capabilities, not copying T3's whole orchestration architecture or replacing Neoism's own agent.

### Antigravity is a separate integration project

T3 uses Google's official Antigravity ACP distribution, not an unofficial OpenCode authentication plugin. Its documentation requires the ACP executable and matching `localharness_external` helper together, with a separate Antigravity-agent sign-in. Models are discovered from the authenticated account, not hardcoded from the IDE's marketing catalog.

Its driver handles installation, per-instance profiles, account checks, model/thinking selection, approval/question translation, and provider-specific event repair. Remote browser sign-in needs a validated callback handoff because Google redirects to loopback on the agent's host. T3 documents limitations including no conversation rewind, no manual ACP compaction, and read-only native child inspection. Neoism should expose actual capabilities rather than promise uniform behavior across agents.

## What Neoism already has

| Capability | Existing implementation | Current boundary |
| --- | --- | --- |
| ACP client | `neoism-agent/crates/neoism-agent-server/src/external_acp.rs` | Subprocess stdio JSON-RPC, timeouts, events, diagnostics, cancellation/lifetime cleanup; ACP v1. |
| Three runtime launchers | `external_agent/runtime.rs:4-81` | OpenCode: `opencode acp --cwd`; Codex: pinned `@agentclientprotocol/codex-acp@1.13.1`; Claude: pinned `@agentclientprotocol/claude-agent-acp@0.81.1`. No Antigravity or user-defined runtime registration. |
| Direct external chats | `session_routes.rs:320-367`, `session_prompt.rs` | Explicit `externalProvider` persists runtime ownership. Changing a model or agent name must not implicitly change the runtime. Host-credential chats are rejected in hosted/shared-control-plane contexts. |
| External subagents | `plugins/subagents.rs:84-143`, `external_agent/lifecycle.rs` | `task` dispatches `claude`, `codex`, and `opencode` to the external runner; background completion and child continuation exist. Native instructions currently require an explicit user request for these runtimes. |
| Options and commands | `external_agent/options.rs` | Provider-confirmed select options, model/thinking configuration, and commands. Legacy mode fallback exists; legacy model selection needs a compatibility audit. |
| Session continuation | `external_agent/acp_run.rs:250-299` | Capability-gated load/resume with persisted external session IDs; refuses silent replacement of a conversation. |
| Native history import | `external_agent/catalog.rs` | Capability-gated discovery/import; imported transcript is text-only, and replay refuses execution. Resume does not imply import support. |
| Permissions | `external_agent/requests.rs` | Provider approval requests and client-served files/terminals use Neoism interaction/policy paths. This does not sandbox every operation executed internally by a provider. |
| TypeScript contract | `neoism-agent/sdk/typescript/packages/core/src/generated/contract.ts:31,265-266,400-403` | Creation accepts `externalProvider`/`externalOptions`; typed operations cover option previews, session option get/set, native catalog, and import. |

Server paths in the table are relative to `neoism-agent/crates/neoism-agent-server/src/` unless otherwise qualified. These are inspected implementation paths, not live-provider certification.

## The current desktop gap

The older shared agent-pane renderer already has external option UI in `neoism-frontend/shared/src/panels/agent_pane/state/external_options.rs`. That is not proof of current GPUIX support.

The current GPUIX composer has agent, model, and thinking choices in `neoism-frontend/gpuix/src/ui/App.tsx:316-365`. The agent choice maps the native agent catalog; model choice maps configured API providers. In `neoism-frontend/gpuix/src/model.ts:1466-1468`, new-thread creation sends `directory`, `agent`, and `model`, but no `externalProvider` or `externalOptions`. Searches of this frontend found external ownership handling in session catalog presentation, but no corresponding composer execution selection or external-option client wiring.

Consequently, choosing an Anthropic or OpenAI model is not equivalent to running Claude Code or Codex with the user's installed CLI/subscription. Completing the GPUIX path is the first useful increment; rebuilding the existing Rust ACP client is not.

## Recommended product and execution model

1. **Runtime first, model second.** Add an execution choice with Neoism selected by default, then Claude Code, Codex, OpenCode, and eventually Antigravity/custom ACP instances. In Neoism mode retain native agent/model/reasoning behavior. In external mode show options discovered from that runtime, not Neoism's API model catalog.
2. **Stable conversation ownership.** Persist runtime and external session identity independently of mutable model choices. Initially, choosing another runtime starts a new conversation; do not imply that native conversation context can transparently migrate between harnesses.
3. **One runner for direct and delegated work.** Reuse the existing external execution lifecycle for both entry points. Keep parent/child ownership, cancellation, permissions, completion delivery, and transcript persistence in Neoism.
4. **Explicit delegation targets.** Extend external task configuration to select a runtime instance and provider-confirmed options/model. Keep current opt-in policy until a user-configurable delegation policy authorizes automatic external use. The user's request to orchestrate agents should not mean silently spending another provider's subscription on every task.
5. **Capabilities, not fabricated parity.** Expose support for continuation, steering, attachments, import, model switching, rewind, compaction, and native-child inspection. Disable unsupported controls with a useful explanation.
6. **Environment-owned execution.** Run agents where the workspace/server lives, with that environment's installation and credentials. Do not forward local credentials to an arbitrary remote or hosted worker. Keep existing host-execution authorization checks.

Conceptually, both paths converge on the same runtime implementation:

```text
Composer -> Neoism native session -> native model/tools
Composer -> external session -----+
Neoism task -> external child ---+-> managed external runtime -> provider session
```

The distinction between the second and third paths is ownership and completion routing, not a separate integration for each provider.

## Proposed implementation order

### 1. Complete GPUIX direct use for the existing three runtimes

Add typed runtime selection to draft/session state and a composer picker, with Neoism first/default. Call existing option-preview operations before creation, pass `externalProvider` and `externalOptions` on create, and fetch/update session-scoped options after creation. Restore runtime ownership when reopening existing chats. Keep native provider/model/account state separate from external options. Surface missing executable, authentication, option-preview, and resume errors without silently falling back to Neoism.

Verify one existing provider end to end: create, stream text/tools, answer an approval, change an advertised option, cancel, and continue after reconnect. Test observable routing and state restoration, not only picker labels. Preserve directory/workspace authorization on every operation.

### 2. Complete external orchestration controls

Reuse the existing `task` branch rather than adding a parallel task manager. Add explicit external option targeting and provider availability discovery. Fix the active-child follow-up boundary: `external_agent/lifecycle.rs:90-101` currently returns "running" without accepting the supplied new prompt, unlike native child steering. Either implement supported queue/steer semantics or explicitly reject an undelivered follow-up. Validate parent completion delivery, stop, restart/continuation, depth/concurrency limits, and permission inheritance. Use existing worktree support where applicable and verify external tasks actually receive the intended checkout.

Allow Neoism to delegate to external agents under an explicit user/project policy. A scoped MCP bridge for external agents to call back into Neoism is a separate follow-on feature: setup currently sends `mcpServers: []` in `external_agent/acp_run.rs:260-264`. Do not hand child agents a master credential or unrestricted recursive orchestration capability.

### 3. Add Antigravity using the same execution contract

Start with an explicit local executable configuration and the official ACP runtime/helper. Check protocol compatibility, session continuation, auth, models/options, questions/approvals, cancellation, and process cleanup before expanding the supported-provider API/SDK/UI union. Add provider-specific handling only for demonstrated incompatibilities. Integrated sign-in, isolated accounts, remote callback handoff, and managed installation are additional deliverables, not hidden assumptions of an enum entry.

### 4. General ACP registration and native adapters where justified

Support user-registered commands as executable plus literal argument array and explicit environment overrides, never an implicitly shell-expanded command string. Registry-managed installation should require a deliberate user action, version pinning, integrity checks where available, and review of third-party code. Negotiate protocol/capabilities and keep credentials provider-owned.

Keep an adapter boundary that permits a native Codex `app-server` or Claude SDK integration later, but do not replace working ACP paths without a concrete feature, correctness, or performance reason. No blanket switch to ACP v2 preview is proposed.

## Reference source

The following links are pinned to the inspected T3 commit so this comparison does not drift with `main`:

- [Built-in driver registry](https://github.com/pingdotgg/t3code/blob/337c5858c33bc7f8f6dfd391a2c279fa0f7e4d17/apps/server/src/provider/builtInDrivers.ts)
- [Shared provider adapter contract](https://github.com/pingdotgg/t3code/blob/337c5858c33bc7f8f6dfd391a2c279fa0f7e4d17/packages/provider-core/src/server/ProviderAdapter.ts)
- [Claude native adapter](https://github.com/pingdotgg/t3code/blob/337c5858c33bc7f8f6dfd391a2c279fa0f7e4d17/apps/server/src/orchestration-v2/Adapters/ClaudeAdapterV2.ts) and [Codex native adapter](https://github.com/pingdotgg/t3code/blob/337c5858c33bc7f8f6dfd391a2c279fa0f7e4d17/apps/server/src/orchestration-v2/Adapters/CodexAdapterV2.ts)
- [OpenCode driver](https://github.com/pingdotgg/t3code/blob/337c5858c33bc7f8f6dfd391a2c279fa0f7e4d17/packages/provider-opencode/src/server/driver.ts)
- [ACP runtime](https://github.com/pingdotgg/t3code/blob/337c5858c33bc7f8f6dfd391a2c279fa0f7e4d17/packages/provider-acp/src/server/AcpSessionRuntime.ts) and [shared ACP adapter](https://github.com/pingdotgg/t3code/blob/337c5858c33bc7f8f6dfd391a2c279fa0f7e4d17/packages/provider-acp/src/server/adapter.ts)
- [App-owned task delegation](https://github.com/pingdotgg/t3code/blob/337c5858c33bc7f8f6dfd391a2c279fa0f7e4d17/apps/server/src/mcp/OrchestratorMcpService.ts#L2360)
- [Antigravity adapter](https://github.com/pingdotgg/t3code/blob/337c5858c33bc7f8f6dfd391a2c279fa0f7e4d17/apps/server/src/orchestration-v2/Adapters/AntigravityAdapterV2.ts), [auth coordinator](https://github.com/pingdotgg/t3code/blob/337c5858c33bc7f8f6dfd391a2c279fa0f7e4d17/apps/server/src/provider/AntigravityAuth.ts), and [setup documentation](https://github.com/pingdotgg/t3code/blob/337c5858c33bc7f8f6dfd391a2c279fa0f7e4d17/docs/user/providers-antigravity.md)
- [Generic ACP setup and limitations](https://github.com/pingdotgg/t3code/blob/337c5858c33bc7f8f6dfd391a2c279fa0f7e4d17/docs/user/providers-acp.md)