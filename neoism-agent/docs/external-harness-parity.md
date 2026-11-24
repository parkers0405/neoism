# External coding harnesses

Neoism's built-in agent framework and external coding harnesses are separate. Selecting an API model changes the model Neoism calls; selecting an external harness launches the provider's own agent process, tools, and authentication. Opening an agent in a terminal does not make it a managed Neoism session.

## T3 Code comparison

| Harness | T3 Code transport | Neoism orchestration |
| --- | --- | --- |
| Codex | Codex App Server | Existing native driver |
| Claude Code | Claude Agent SDK | Existing native driver |
| OpenCode | OpenCode server | Existing native driver |
| Grok Build | ACP with xAI-specific extensions | Installed CLI over ACP v1 (`grok`) |
| Kiro | ACP with Kiro-specific extensions | Installed CLI over ACP v1 (`kiro`) |
| Antigravity | ACP with managed runtime and Google sign-in | Installed `agy` over ACP v1 (`antigravity`) |
| fx | ACP with provider-specific extensions | Installed CLI over ACP v1 (`fx`) |
| Cursor | Cursor SDK | Official Cursor CLI ACP transport (`cursor`), not the SDK |
| Pi | Pi JSONL RPC | Not yet integrated |
| Muse Code | Muse SDK | Not yet integrated |
| ACP Registry / arbitrary local commands | ACP v2 preview with v1 fallback | No registry or arbitrary local command configuration yet |

Sources: [T3 Code provider setup](https://github.com/pingdotgg/t3code/blob/main/docs/user/install.md#providers), [ACP provider docs](https://github.com/pingdotgg/t3code/blob/main/docs/user/providers-acp.md), [provider adapter source](https://github.com/pingdotgg/t3code/tree/main/packages), and [Cursor CLI ACP docs](https://cursor.com/docs/cli/acp).

## Antigravity and subagents

When Neoism delegates a background task to Antigravity, the Task card opens the managed child session using the same path as a built-in Neoism subagent. Text, thinking, and tool updates flow through the child's normal timeline while the turn runs. Neoism streams native command output, waits up to 30 seconds for an awaited non-daemon command that outlasts the prompt, and prefers ACP resume on continuation. This is separate from subagents Antigravity starts inside its own harness. T3 Code's [prerelease adapter](https://github.com/pingdotgg/t3code/blob/856b105fa23f9cb9fdd49a8f102cb66a71b78265/apps/server/src/orchestration-v2/Adapters/AntigravityAdapterV2.ts) recognizes those internal launches as provider events and assigns no child session ID to the batch call. Scheduled wakes, provider-specific steering, image prompts, usage, and approval handling are not at parity.

## Run an installed harness

Install and sign in to the CLI on the machine running the Neoism agent server. The host executable resolver must find `grok`, `kiro-cli`, `agy`, `fx`, or `cursor-agent` respectively. Neoism does not install these CLIs or pass its API credentials to them.

Rune's `/connect-agent` (or `/connect agents`) checks the workspace host's PATH and asks Claude Code and Codex for their existing sign-in status. **Install** runs the vendor installer on that host for supported providers and platforms; others link to the vendor's install guide. Rune rechecks automatically while the picker is open and after setup. Detected installs are marked **Installed**, confirmed logins are marked **Signed in**, and unverified logins are not treated as signed out. This does not modify model accounts in `/connect`.

Select **Sign in** to launch the installed harness's login flow on the workspace host. Use `/connect-agent antigravity` to open directly to Antigravity. Rune opens the Google authorization page locally; enter the returned code in the dedicated sign-in tab, not in chat. Local Codex uses its standard browser login. Remote Codex uses device authorization, which must be enabled for the ChatGPT account. Other harnesses use their native CLI prompts.

- **Direct session:** send `externalProvider` with the provider ID to `POST /v2/sessions`. Send turns through the same session prompt API as other chats.
- **Delegated task:** use the provider ID as `task.subagent_type`. Use the same provider and `task_id` to continue; use `task_result` and `stop_task` to inspect or cancel it.
- **Options:** preview advertised options with `GET /v2/external/options/preview?provider=kiro`. The owning harness validates any supplied `externalOptions` against the session.

These are host-executed text sessions. Neoism projects standard ACP events and mediates permission requests sent to it, but a provider's own sandbox governs actions it performs itself. Continuation requires the provider to advertise ACP load or resume. Provider-specific extensions, native history import for the new providers, managed installs, in-app sign-in, and a new desktop provider picker are not part of this integration.

### Antigravity desktop smoke test

1. Install and sign in to `agy` on the host running the agent server. Start Rune with a workspace open.
2. Ask Neoism: “Delegate this as a background task to Antigravity. Read the workspace README and summarize it without editing files.”
3. Open the Task card before it finishes. Confirm the same tab opens the Antigravity child and that text and tool updates appear there. Return to the parent and confirm it receives the completion.
4. Ask Neoism to continue the same Antigravity task. Confirm it opens the same child transcript rather than creating a new child. Start another background task and stop it while it is running; confirm the child stops without cancelling the parent.