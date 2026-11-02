# Agent composer shell mode

Start a composer draft with `!` (for example, `! ls -la`) to select shell mode. The composer shows “Shell · Enter to run · Esc to exit”. Enter or the send button explicitly submits the command; typing, restoring a draft, or pasting never executes it. Escape clears the shell draft and exits; Backspace on an empty shell draft exits too. An exclamation mark elsewhere in prose remains ordinary message text. Remove attachments before submitting a shell command.

The frontend uses the owning session's authenticated SDK connection, including remote workspace routing. A new Agent home creates a conversation first. `POST /v2/sessions/{session_id}/shell` executes the configured shell in the session directory through the ordinary permission-aware Bash tool runtime. It does not ask a model to run the command and does not create a terminal pane. The command and running/completed/error Bash block are persisted and broadcast through the existing timeline protocol; results remain context for a later model turn.

The SDK first checks direct-execution support on the same session endpoint. Older daemons that forwarded shell requests to a model are rejected before submission; update/restart the server rather than falling back to model execution.

Shell commands share the session execution slot. A busy session rejects a new shell submission rather than running concurrently or silently queueing it. The existing Stop/abort action cancels permission waits and running processes. Once admitted, execution is owned by the server, not the HTTP connection; disconnecting the frontend does not orphan the run's bookkeeping. Supplying an already persisted `messageId` returns a conflict without re-executing the command.

This is the existing non-interactive Bash-tool execution path, not an interactive PTY. Commands inherit its configured shell/environment, default 120-second timeout, process-tree cancellation, output truncation/artifact handling, and 1 MiB per-stream capture bound. Output appears when the tool finishes; commands needing terminal input or a full-screen TUI are not supported here. Each command starts in the session directory; a `cd` in one command does not persist into the next. Permission denials, failures, and timeouts appear as normal tool errors.

## Focused verification

From `neoism-frontend/gpuix`:

```sh
./node_modules/bun/bin/bun.exe scripts/build-sdk.ts
./node_modules/bun/bin/bun.exe test tests/shell-mode.test.ts tests/sdk.test.ts
./node_modules/bun/bin/bun.exe x tsc --noEmit
```

From the repository root:

```sh
cargo test -p neoism-agent-server --lib shell_tests -- --nocapture
```

Reference behavior: [OpenCode shell commands](https://opencode.ai/docs/tui/#shell-commands) run a leading-`!` command and add output as a conversation tool result; [Claude Code interactive mode](https://code.claude.com/docs/en/interactive-mode) also provides leading-`!` direct shell input.