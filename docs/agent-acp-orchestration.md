# External agent orchestration

Neoism remains the primary agent. It can delegate scoped work to OpenCode, Claude Code, or Codex as child tasks, review their results, and integrate the work. This increment does not add a direct external-runtime picker or Antigravity support.

## Using external children

Ask for a particular provider, such as "Have Codex review this change," or authorize Neoism to delegate scoped work to the supported external agents. Ordinary work continues to use Neoism's native agent unless external delegation is requested or authorized.

External children appear in the existing left agent sidebar and timeline with the saved provider icon and their task description. Native Neoism children use the saved Neoism mark in both places. Provider identity comes from confirmed metadata or the canonical task target, never from words in a title or model name.

| Agent | Task target | Execution host requirements |
| --- | --- | --- |
| OpenCode | `subagent_type: "opencode"` | OpenCode CLI with its own configured authentication. |
| Claude Code | `subagent_type: "claude"` | Node/npx and the Claude ACP adapter's authentication; an installed Claude CLI is resolved when available. |
| Codex | `subagent_type: "codex"` | Node/npx and the Codex ACP adapter's authentication. |

Agents run on the workspace execution host, with that host's credentials and the child's inherited workspace permissions. Neoism API-model selection does not select the external child's model. Provider-native operations do not all pass through Neoism's client-side permission bridge, so this is not an OS sandbox.

## Timeline and tree presentation

Timeline task headers show the provider logo next to `Task @provider` and the task description. Status remains separate: running activity, a green completion marker, or a red error marker/caption. Follow-ups retain the continuation arrow and provider logo. Optional phase captions can compact away without removing the identity icon.

The left tree shows the Neoism parent followed by indented provider-branded children. Each child has its provider label, task description, and working/done/stopped state. Normal completed live rows clear after seven seconds; failed/stopped rows have a longer sixty-second grace period. A viewed child retains return navigation until it is left. Opening children reuses the family tab and preserves the parent's draft.

Provider identity is carried through saved history. This does not override settled-history decluttering: a cold reopened conversation still hides archived tool traces under the existing design. During the live visit, tool cards remain visible after completion and when returning from a child transcript.

## Task lifecycle

- `task` starts foreground or background work. Background work reports its result back to the parent Neoism session.
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

## Verification

The orchestration regressions use fake ACP subprocesses for all three providers and cover provider-owned continuation, background parent-result delivery, rejected incompatible or unknown continuations, and foreground cancellation without cancelling the parent. Both tests pass. The updated debug core also passes fifteen focused GPUIX branding/family/sidebar tests, including comparison of the actual decoded logo pixels between timeline and tree, plus the shared live/reconstructed identity regression.

A real native App exercise uses an isolated protocol fixture, not provider accounts. It opens all four child transcripts, checks the real family-navigation buttons, preserves the parent draft and single family tab, captures running/completed/error states, and captures tree cleanup after eight seconds. The screenshot's deliberate Codex error is fixture data, not the live Codex result.

Separate live smoke prompts were attempted with the installed host CLIs. Codex returned the expected result and continued the same provider session successfully. Claude Code reached prompt execution but failed because its OAuth session had expired and could not refresh. OpenCode did not complete its tiny response and was cancelled; that live path is not certified. No credentials were changed or sign-in flow initiated.

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