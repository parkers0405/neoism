# External agent orchestration

Neoism remains the primary agent. It can delegate scoped work to OpenCode, Claude Code, or Codex as child tasks, review their results, and integrate the work. This increment does not add a direct external-runtime picker or Antigravity support.

## Using external children

Ask for a particular provider, such as "Have Codex review this change," or authorize Neoism to delegate scoped work to the supported external agents. Ordinary work continues to use Neoism's native agent unless external delegation is requested or authorized.

External children appear in the existing left agent sidebar and timeline with the saved provider icon and their task description. Native Neoism children use the saved Neoism mark in both places. Provider identity comes from confirmed metadata or the canonical task target, never from words in a title or model name.

| Agent | Task target | Execution host requirements |
| --- | --- | --- |
| OpenCode | `subagent_type: "opencode"` | OpenCode CLI with its own configured authentication. |
| Claude Code | `subagent_type: "claude"` | Node/npx and the Claude ACP adapter's authentication; an installed Claude CLI is resolved when available. |
| Codex | `subagent_type: "codex"` | Node/npx and Codex authentication. The adapter prefers the host-resolved Codex CLI, respects explicit `CODEX_PATH`, and falls back to its bundled CLI if no host executable is available. |

Agents run on the workspace execution host, with that host's credentials and the child's inherited workspace permissions. Neoism API-model selection does not select the external child's model. Provider-native operations do not all pass through Neoism's client-side permission bridge, so this is not an OS sandbox.

## Timeline and tree presentation

Timeline task headers show the provider logo next to `Task @provider` and the task description. Status remains separate: running activity, a green completion marker, or a red error marker/caption. Follow-ups retain the continuation arrow and provider logo. Optional phase captions can compact away without removing the identity icon.

The left tree shows the Neoism parent followed by indented provider-branded children. Each child has its provider label, task description, and working/done/stopped state. Normal completed live rows clear after seven seconds; failed/stopped rows have a longer sixty-second grace period. A viewed child retains return navigation until it is left. Opening children reuses the family tab and preserves the parent's draft.

Selecting a running external child uses the same transcript path as a native child: incremental assistant text, provider-reported reasoning, and normal expandable tool cards with inputs, output, and status. Updates continue into parked child caches while viewing the parent or another sibling. Opening a child after work has started reveals its unfinished turn instead of treating that activity as archived history.

Provider identity is carried through saved history. Completed archived turns retain the existing decluttering policy; revealing an in-flight child does not unhide every old tool call. During the live visit, tool cards remain visible after completion and when returning from a child transcript.

## Task lifecycle

- `task` starts foreground or background work. Omit `task_id` or leave it empty for new work; strict tool clients may send the empty placeholder. A non-empty `task_id` means continuation. Background work reports its result back to the parent Neoism session.
- `task_result` inspects a child's current status and result.
- `stop_task` cancels a child and clears queued work, without cancelling its parent session.
- Continue a finished or stopped child with the same `task_id` and external provider to resume its provider-owned conversation when the adapter supports continuation.
- A follow-up to a running external child is explicitly rejected, not silently dropped. Wait for completion or stop the child before continuing; live external steering is not implemented in this increment.
- An unknown continuation ID, another parent's child, a different provider, or a provider-native projected child cannot be silently substituted for a managed external conversation.

Provider-native nested agents may be displayed as observed child activity. They are distinct from the external children Neoism creates and cannot be independently resumed through `task` without a provider-supported identity.

## Boundaries

Children currently share their parent's checkout. They have their own provider conversation, but not automatic worktree, credential, process, or network isolation. External setup/login failures are reported as failures rather than switching silently to another provider.

The existing pinned ACP launchers are reused. No provider installation or sign-in is triggered merely by displaying an external child in the sidebar. Real execution still requires the appropriate host setup and permission policy.

The deeper architecture and remaining work are documented in [ACP plumbing deep dive](agent-acp-plumbing-deep-dive.md). Direct runtime selection, external model/options targeting, active-child steering, automatic worktree isolation, generic ACP registration, and Antigravity remain separate work.

## Known live Codex output limit

The real tool-using regression distinguishes successful launch and live events from complete output fidelity. Fresh creation with an empty continuation placeholder, assistant streaming, and a running command were exercised successfully. However, the complete-stdout assertion still fails: the captured ACP updates contain the ending stdout chunk but not the command's initial chunk. This remained reproducible with the installed Codex CLI, modern output-delta negotiation, and an ACP-v1-compatible bridge upgrade experiment; the unhelpful version bump was reverted.

The ignored-by-default test `live_codex_new_task_streams_tools_and_retains_stdout` preserves that failing completeness assertion. The new child view displays the text, tools, and output actually reported by the provider, but this is not a claim of full Codex-native transcript fidelity. Bytes never captured in Neoism's history cannot be reconstructed from its saved transcript.

## Verification

The automated ACP suite passes 46 tests, with the opt-in live test ignored by default. The fixtures cover all three providers, empty-placeholder creation, provider-owned continuation, background parent-result delivery, rejected incompatible or unknown continuations, foreground cancellation, and interleaved reasoning/text/tool output that can be observed before completion and recovered from saved snapshots. The rebuilt debug core passes 18 focused child-streaming/family/branding tests, including late-selected running tools, parked child caches, raw part routing, and actual decoded logo comparisons between timeline and tree. TypeScript checking passes.

A real native App exercise uses an isolated protocol fixture, not provider accounts. It opens all four child transcripts, checks the real family-navigation buttons, preserves the parent draft and single family tab, captures running/completed/error states, and captures tree cleanup after eight seconds. The screenshot's deliberate Codex error is fixture data, not the live Codex result.

Earlier no-tools live smoke prompts returned the expected Codex result and continued the same session; those were not sufficient proof of tool-transcript fidelity. The subsequent real tool-using Codex test successfully creates a fresh child and observes live assistant/tool activity, but its complete-output assertion remains failing as described above. Earlier Claude Code testing reached prompt execution but failed on expired OAuth; earlier OpenCode testing did not finish its tiny response and was cancelled. Neither of those live paths is newly certified by the automated fixtures. No manual credentials changes or sign-in flow were initiated.

The native core was built with the `dev` profile, the display addon uses a current-pin verified debug artifact, and the daemon/agent/helper were built with the dev-derived `activation` profile. An isolated source-run debug app was launched without replacing the existing app or services. Self-contained packaging still deliberately rejects debug display bindings; no verification guard was bypassed and no release build was used.

The repeatable native scenario is `neoism-frontend/gpuix/scripts/native-orchestration-check.tsx`. It requires the debug core/display and Hyprland with `grim` for screenshot capture; it captures only its own window and closes that test window afterward. It uses fixture-only terminal transport during ordinary app startup and asserts that it sends no model prompts or thread-creation requests.

The additional daemon history-transport regression build did not finish within its verification bounds, including the warm-profile attempt. No passing daemon-test result is claimed; the production daemon build, shared mapping regression, native rendering tests, and real native UI exercise are separate successful checks.

## Debug artifacts

The verified dev core is preserved at `neoism-frontend/gpuix/artifacts/libneoism_gpuix_core.so`. The built daemon, agent CLI, and update helper are staged under `neoism-frontend/gpuix/artifacts/agent-orchestration-debug/`, outside disposable Cargo target directories. The source-run debug launcher accepts explicit core/daemon paths and creates a private profile without replacing the normal app or its services.

From `neoism-frontend/gpuix`, the verified Linux debug launch is:

```sh
env -u LD_LIBRARY_PATH \
  NEOISM_GPUIX_DISPLAY_DEBUG=1 \
  NEOISM_GPUIX_CORE="$PWD/artifacts/libneoism_gpuix_core.so" \
  NEOISM_GPUIX_WORKSPACE_DAEMON="$PWD/artifacts/agent-orchestration-debug/neoism-workspace-daemon" \
  ./node_modules/@oven/bun-linux-x64-baseline/bin/bun \
  scripts/launch-block-debug.ts /home/parkersettle/projects/neoism
```