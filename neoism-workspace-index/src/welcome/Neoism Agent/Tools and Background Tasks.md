# Tools and Background Tasks

Tools let a model inspect or act on the workspace. Neoism validates tool input, checks permissions, executes the operation, stores its result as a message part, and broadcasts progress to every client attached to the session.

## Native tool groups

Neoism's built-in inventory includes:

| Group | Examples |
|---|---|
| Shell | `bash`, `background_task`, `background_task_result` |
| Files | `read`; model-specific `write`/`edit` or `apply_patch` |
| Search | FFF-backed `grep` and `glob` |
| Web | `webfetch`, `websearch` |
| Code intelligence | `lsp` |
| Knowledge | Native `memory`, `docs`, and `skill`; Desktop-provided Notes MCP |
| Workflow | `question`, `plan_enter`, `todowrite` |
| Delegation | `task`, `task_result`, `stop_task` |
| Runtime | `complete_goal`, background-task cancellation routes |

MCP servers are exposed through one `execute` discovery/call gateway so their schemas do not flood every model request. Neoism Desktop preinstalls its vault-backed Notes MCP; standalone Agent hosts do not receive Notes unless their product supplies it. The effective tool list is filtered by global and per-agent `tools` maps.

```jsonc
{
  "agent": {
    "tools": {
      "websearch": false,
      "webfetch": false
    }
  }
}
```

## Foreground shell

`bash` runs a command and waits for completion or timeout. It uses the session project as the default working directory, but a tool call can request another directory subject to `external_directory` permission.

Neoism scans shell text into permission targets instead of treating every command as one opaque string. A compound command can therefore require multiple `bash` decisions and an external-directory decision.

## Background shell tasks

`background_task` starts a long-running shell command and returns a `job_id` immediately. Defaults are:

- Timeout: 30 minutes.
- Retained output: 256 KiB.
- Working directory: session project directory.

The process runs in a new process group. Neoism retains status, output, exit code, signal, truncation, command patterns, and working directory in the live agent server.

Use `background_task_result` with a job ID to inspect one task, or omit the ID to list tasks for the current session.

## Stop a background task

Use the task's **Stop** action. Cancellation is delivered to the runner that owns the live child process; that runner terminates its process tree and reports `cancelled`. Neoism does not later kill an arbitrary reused PID.

Background task state is in-memory and retained only for the current agent-server lifetime. Restarting the server loses the inventory even if an independently detached external process survived.

## Tool output

Large outputs are bounded. Some tool results are spilled to a reference rather than replayed in full to every later model request. The timeline can show a preview while the provider context uses a compact reference.

## File editing

Mutation tools are mutually exclusive for each model:

- GPT/Codex models receive only `apply_patch`, which performs structured multi-region add/update/delete patches.
- Other models receive `edit` for targeted exact replacement and `write` for new files or intentional full replacement.

All map to the `edit` permission. Agents are instructed not to overwrite unrelated concurrent work.

`read` handles both files and directories with bounded streaming output. Agents issue multiple independent `read`, `grep`, `glob`, `webfetch`, or `websearch` calls together; the runtime executes up to ten calls concurrently. This replaces sequential batch wrappers with one strict schema per operation.

## Tool loops and limits

An agent can set `maxSteps`. When the limit is reached, Neoism disables further tools for that turn and requires a text summary. Repeated behavior can trigger the `doom_loop` permission.

## Permissions and side effects

Tool availability is not a safety guarantee. A shell command, browser action, API integration, or MCP tool can create irreversible external effects. See [[Permissions]].

## Subagents are not shell jobs

Delegated agents are child sessions managed through `task`, `task_result`, and `stop_task`. Shell background tasks are OS processes managed through `background_task`. They have different lifecycle and persistence semantics.

## Custom tools

Your own tools register through plugins: a serve plugin declares them at handshake and they join the same permission pipeline as native tools. See [[Plugins]].

See [[Agents and Subagents]] and [[Sessions and Sharing]].
