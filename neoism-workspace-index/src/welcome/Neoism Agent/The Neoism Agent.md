# The Neoism Agent

Neoism Agent is Neoism's local AI runtime. It is not a hosted chat page embedded in the terminal. It owns model connections, conversations, tool execution, permissions, subagents, skills, MCP clients, compaction, and durable session history for the workspace.

Open an agent pane with `Alt+A`, or choose **New Agent** from the command palette. A pane may start with the model picker when no usable model has been selected. After a model is available, enter a request just as you would in a terminal composer.

## How it fits into Neoism

Neoism has three cooperating parts:

- **Neoism** renders the workspace and the agent timeline.
- **Neoism Agent** runs conversations, models, tools, and permissions.
- **Neoism Daemon** carries workspace and agent events between desktop, browser, and mobile clients.

Agent sessions survive a pane closing because they are server-owned rather than widget-owned. Opening the same session on another connected client replays its stored messages and then follows live events.

## Architecture

The agent is a versioned HTTP server with a plugin-first core:

- **One API.** Everything speaks `/v2/` — the desktop pane, the browser, the SDK, and your own scripts hit the same endpoints. The contract is a committed OpenAPI document. See [[Server and API]].
- **One event bus.** Every event — token deltas, part snapshots, status, permissions — is delivered over SSE in strict publish order, as a typed union.
- **Plugin-first.** Providers, tools, MCP, LSP, PTY, VCS, and workflows are plugins in per-workspace generations that reload live on config changes. Third-party plugins install from npm and register tools and hooks through the same runtime. See [[Plugins]].
- **Durable by default.** State changes and their events commit in one transaction to the local store; runs, queues, and execution activity survive restarts.

## A turn

A normal turn is:

1. Neoism stores your user message.
2. The selected agent assembles instructions and recent session context.
3. The selected model produces text or tool calls.
4. Neoism checks each tool call against the effective permission rules.
5. Allowed tools run; `ask` rules create a visible permission request.
6. Tool output is added to the conversation and sent back to the model.
7. The turn settles as complete, interrupted, or failed.

The timeline is made of messages and typed parts. Parts include text, reasoning, files, tool calls, step boundaries, patches, snapshots, and compaction summaries. This is why a tool can stream progress without becoming a separate chat message.

## Built-in agents

| Agent | Role |
|---|---|
| `build` | Default implementation agent with normal workspace tools. |
| `plan` | Read-oriented planning agent. It can ask questions and enter plan mode but does not edit by default. |
| `general` | General-purpose subagent for multi-step delegated work. |
| `explore` | Fast, read-only subagent for codebase discovery. |

Neoism also uses hidden internal agents for tasks such as title generation, summaries, compaction, and session handoff. They are implementation details and are not normal picker choices.

## Tools and extensions

The native runtime includes shell, file, patch, search, web, notes, skill, LSP, question, planning, and subagent tools. MCP servers can add more tools, resources, and prompts. Tool availability is filtered by global configuration, the selected agent, and permission rules.

See [[Tools and Background Tasks]], [[Permissions]], [[Skills]], and [[MCP Servers]].

## Local data and external data

Session records, permissions, configuration, and stored provider credentials are kept locally by Neoism. Credentials are written to the agent state directory; on Unix the credential file is created with owner-only permissions.

Prompts, attachments, tool results, and any instructions included in model context are sent to the selected model provider. A local agent does not mean the selected model is local. Review the provider's data policy and avoid attaching secrets unnecessarily.

## First useful workflow

1. Open an agent pane with `Alt+A`.
2. Run `/connect`, choose a provider, and complete its API-key or OAuth flow.
3. Pick a model with `/model`.
4. Ask the agent to inspect the workspace before changing it.
5. Review permission requests rather than approving them blindly.
6. Use `/model` to switch models, `/agent` to switch agents, and `/status` to inspect the session.
7. Press `Esc` to interrupt active generation or use a task's **Stop** action for a background command.

## Continue reading

- [[Configure]] for configuration sources and precedence.
- [[Providers]] and [[Models]] for connecting a model.
- [[Agents and Subagents]] for delegation.
- [[Sessions and Sharing]] for persistence and cross-device behavior.
- [[Server and API]] for the HTTP surface, events, and hosted mode.
- [[Plugins]] for extending the runtime, including third-party serve plugins.
- [[SDK]] for typed TypeScript clients.
- [[Scheduled Workflows]] for date-driven and recurring agent automations.
- [[Troubleshooting]] when startup, authentication, or tools fail.