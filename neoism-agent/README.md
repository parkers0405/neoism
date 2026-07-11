# neoism-agent

Headless Rust agent framework for Neoism.

This is intentionally separate from the Neoism terminal. The crate layout is SDK-first: frontends, terminals, editors, services, and other companies can embed or call the same headless engine over Rust APIs or HTTP/SSE.

## Goals

- Match OpenCode's proven headless shape: HTTP API, SSE event bus, sessions, messages, parts, MCP, provider auth, and OAuth flows.
- Match OpenCode's agent architecture at the system level: provider/model catalog, primary agents, subagents, steps, tool permission flow, plugin-style extension points, MCP tools, session compaction, and syncable event history.
- Use Neoism naming for the product and protocol surface.
- Keep the CLI thin. It should drive the server, not own the agent runtime.
- Make every public type serializable so SDKs can be generated or consumed directly.

## Architecture Rule

This project should copy OpenCode's proven headless and agentic systems, but implement them as idiomatic native Rust. Do not twist Rust code into TypeScript-shaped internals or hardcode temporary catalogs when OpenCode uses a real service. The target is architectural parity with Rust-quality boundaries: focused modules, explicit state, typed SDK surfaces, durable storage, and clean runtime services.

## Crates

- `neoism-agent-core`: stable SDK types, IDs, session/message/event models, auth config shapes.
- `neoism-agent-server`: headless HTTP/SSE server and in-process engine boundary.
- `neoism-agent-cli`: CLI for `serve`, `run`, ACP, auth, MCP, sessions, tools, and smoke testing.

## Current Status

Neoism is now a usable private-alpha headless runtime, not just a scaffold. It has OpenCode-compatible ID formats, session/message primitives, SQLite-backed session storage, a server event stream, broad OpenCode-compatible route coverage, provider streaming, a multi-step tool loop, queue/interrupt handling, snapshot-backed undo/redo-style revert flows, MCP tool integration, internal Rust plugin hooks, and a practical streaming CLI.

Provider metadata follows OpenCode's `ModelsDev` approach: `neoism-agent-server` loads a provider/model catalog from `https://models.dev/api.json`, caches it under the Neoism cache directory, honors `NEOISM_AGENT_MODELS_URL`, honors `NEOISM_AGENT_MODELS_PATH`, and can disable fetching with `NEOISM_AGENT_DISABLE_MODELS_FETCH`. Provider generation supports models.dev-driven OpenAI-compatible chat completions, native Anthropic Messages streaming, ChatGPT/Codex OAuth through the Responses endpoint, GitHub Copilot device auth, request transforms for major OpenAI-compatible edge cases, and the local stub runtime used by tests. Use `neoism-agent auth login-codex` to authenticate OpenAI with a ChatGPT/Codex subscription, `neoism-agent auth set-api openai <key>` for API-key auth, `neoism-agent models openai` to list OpenAI model IDs, or pass `neoism-agent run --model openai/<model>` to choose a model for a prompt. `NEOISM_AGENT_OPENAI_BASE_URL` can point at another OpenAI-compatible API.

## Local Test Flow

Build the agent binary from the repo root:

```bash
cargo build -p neoism-agent
```

Start the HTTP/SSE server:

```bash
cargo run -p neoism-agent -- serve --port 4096 --hostname 127.0.0.1
```

In another terminal, verify the runtime:

```bash
cargo run -p neoism-agent -- doctor --dir "$PWD"
```

Authenticate with a ChatGPT/Codex subscription:

```bash
cargo run -p neoism-agent -- auth login-codex
```

That command prints the OpenAI device URL/code and waits while you finish the browser flow. Browser callback mode is also available:

```bash
cargo run -p neoism-agent -- auth login-codex --browser
```

Check auth status without printing secrets:

```bash
cargo run -p neoism-agent -- auth status openai
```

List available OpenAI models:

```bash
cargo run -p neoism-agent -- models openai
```

Run a workspace prompt:

```bash
cargo run -p neoism-agent -- run --dir "$PWD" --model openai/<model-id> --agent build --variant low "Inspect this repo and summarize what Neoism can do."
```

Start the basic streaming CLI:

```bash
cargo run -p neoism-agent -- chat --dir "$PWD" --model openai/gpt-5.5 --agent build
```

Inside `chat`, normal lines are sent to the current workspace session and streamed back from the server event bus. The raw terminal UI keeps a Codex-style bottom composer with a model/effort/path/agent footer, opens the `/` command menu as soon as `/` is typed, opens file/agent references as soon as `@` is typed, accepts menu items with Tab, and cycles agents with Tab when no menu is open. Slash commands include `/help [query]`, `/model`, `/models`, `/think`, `/agent`, `/agents`, `/session`, `/messages`, `/undo`, `/redo`, `/tools`, `/skills`, `/mcp`, `/providers`, `/auth`, `/doctor`, `/abort`, `/new`, `/resume <session-id>`, `/queue`, `/permissions`, `/questions`, `/permit`, `/answer`, `/reject`, `/clear`, and `/quit`. The chat renderer streams reasoning in the main transcript as dim secondary text, keeps `Working` in the bottom overlay while the model is busy, collapses exploration tools into an `Explored` tree summary, renders shell commands as `Ran` cells, renders edits as red/green line-numbered diff blocks, renders markdown, and displays fenced code blocks with line numbers.

The CLI code is intentionally split by responsibility: `main.rs` is command dispatch, `cli_direct_commands.rs` and `cli_commands.rs` own HTTP subcommands, `acp.rs` is the editor/ACP bridge, `chat.rs` is chat orchestration, `chat_command_handlers.rs` owns slash command execution, `chat_input.rs` owns prompt state, `chat_input_completion.rs` owns completion scoring, `chat_ui.rs` owns raw terminal rendering, and `chat_tool_render.rs` owns command/edit transcript cells.

Inspect sessions and messages:

```bash
cargo run -p neoism-agent -- session list --dir "$PWD"
cargo run -p neoism-agent -- session messages <session-id>
cargo run -p neoism-agent -- session undo <session-id>
```

Inspect tools and MCP:

```bash
cargo run -p neoism-agent -- tool list --dir "$PWD"
cargo run -p neoism-agent -- mcp status --dir "$PWD"
cargo run -p neoism-agent -- mcp connect <server-name> --dir "$PWD"
cargo run -p neoism-agent -- mcp tools <server-name> --dir "$PWD"
```

Use ACP from an editor by pointing it at:

```bash
neoism-agent acp --cwd /path/to/workspace
```

## Database Backend

The agent database runs on one of two engines, selected by `NEOISM_AGENT_DB_BACKEND`:

- `turso` (default) — [Turso Database](https://github.com/tursodatabase/turso), the Rust rewrite of SQLite with MVCC concurrent writes. State file: `$XDG_STATE_HOME/neoism/agent.turso.db` (a separate file on purpose: Turso is beta and must never rewrite the SQLite-managed database in place, so switching backends starts with an empty session history). Turso has no FTS5, so transcript search falls back to a bounded case-insensitive LIKE scan — same `>>match<<` excerpt markers, no stemming, recency order instead of bm25. Turso reports `Busy` immediately instead of waiting like SQLite's busy_timeout, so the store wraps every turso operation in a bounded exponential-backoff retry (`turso_busy_retry`) — concurrent writers queue instead of failing.
- `sqlite` — the bundled SQLite via sqlx, WAL mode, FTS5-backed message search. State file: `$XDG_STATE_HOME/neoism/agent.sqlite3`. Session history recorded before the turso default landed lives here; run with `NEOISM_AGENT_DB_BACKEND=sqlite` to see it.

This is an environment variable rather than a config key because the database opens at server startup, before any global or per-directory config is loaded. Both engines run the identical schema and SQL; every other feature (sessions, events, sync replay, prompt queues, undo) behaves the same on either backend.

## Config Layout

Neoism uses Neoism-named config paths while matching the same concepts: global agent config from `~/.config/neoism/agent/{config,neoism}.json{,c}`, project config from `neoism.json` / `neoism.jsonc` discovered upward through the git worktree, and config directories from `~/.config/neoism/agent`, project `.neoism` directories, `~/.neoism`, and `NEOISM_AGENT_CONFIG_DIR`.

Config directories load `agent(s)/**/*.md`, `mode(s)/**/*.md`, `command(s)/**/*.md`, local `skill(s)/**/SKILL.md`, configured `skills.paths`, OpenCode-style remote `skills.urls` indexes cached under the Neoism cache directory, declarative plugin manifests from `plugin(s)/*.json`, and custom tools from `tool(s)/*.json`. Markdown frontmatter defines metadata; the markdown body becomes the agent prompt, command template, or skill content. JSON config supports `default_agent`, `agent`, `mode`, `command`, `plugin`, `plugins`, `skills.paths`, `skills.urls`, `permission`, `tools`, `model`, `small_model`, `instructions`, `formatter`, `lsp`, `watcher`, `share`, `autoupdate`, `username`, `experimental`, provider filters, and extra fields. Local `instructions` files plus OpenCode-style `AGENTS.md`, `CLAUDE.md`, and `CONTEXT.md` project instructions are injected into model context; nested instruction files are also surfaced when reading nearby files. Deprecated `tools` maps are normalized into permission rules, including write/edit/patch collapsing into `edit`. `formatter: true` enables discovered built-in formatters, while `formatter.<name>.command/extensions` can add project-specific formatters that run after `write`/`edit`/`apply_patch`. The native `build` and `plan` agents are seeded from the user's OpenCode home-manager prompts, and global permissions merge before per-agent overrides so specific agent config can win.

The Rust server exposes the same headless API families as OpenCode, excluding UI implementation work: sessions/messages/parts, config, provider auth, MCP management/OAuth/runtime routes, files/search, project/VCS, permission/question queues, PTY websocket route shape, sync route shape, experimental tools/resources/worktrees, and v2 `/api/session` routes. The runtime now includes an OpenCode-style continuing tool loop with no implicit default step cap, cancellable provider retry status, durable prompt queues, coarse persisted run lifecycle, durable event history with SSE catch-up plus OpenCode-shaped aggregate `/sync/history` and `/sync/replay`, replay projectors/ownership for sessions/messages/parts/todos/statuses/permissions/questions/queues/compaction/PTY lifecycle, durable permissions, local stdio MCP tools/resources/prompts, remote HTTP/SSE MCP responses with Streamable HTTP session-id persistence, OAuth token refresh, stale credential invalidation, long-lived SSE notifications, bounded reconnect, reconnect diagnostics, and tools-changed refresh events, dynamic MCP OAuth client registration, snapshot-backed undo/unrevert, VCS status/diff fields compatible with OpenCode's `file`/`additions`/`deletions`/`patch` shape, OpenCode-style tagged `read` output for files/directories, typed user and read-tool media attachments through OpenAI Chat image parts, OpenAI Responses image/PDF parts, and Anthropic image blocks, expanded provider transforms for Gemini/Kimi/Moonshot/Claude/Anthropic/Bedrock/Mistral/OpenRouter-style edge cases plus catalog option/header merging and native Anthropic streaming, LSP status/configured-status/hover/definition/references/implementation/call-hierarchy/diagnostics/document-symbol/workspace-symbol/formatting/code-action/touch routes and agent tool operations through a persistent stdio manager with configured env/init options, broken-client eviction, explicit shutdown, and diagnostics cache, configured post-write format hooks plus LSP diagnostic metadata for `write`/`edit`/`apply_patch` and `lsp.updated` notifications, stable completed-tool render metadata with snapshot-derived diff summaries, bundled FFF-powered `ffgrep`/`fffind`/`fff_multi_grep` first-party search tools plus ripgrep-backed OpenCode-style grep/glob grouping/sorting/truncation metadata with Rust fallback, Unix OS PTY websockets with replay, resize, login-shell defaults, foreground process-group setup, process-group stop/SIGWINCH, and exit-status events, model-backed title/compaction summaries with local fallbacks, local and URL-backed skill loading plus an agent-facing `skill` tool with sampled skill files, declarative plugin manifests for provider headers/options and shell env, config-directory custom tools, ACP fork/model/mode/variant config integration with config-option and available-command updates, image-capable prompt metadata, resource/image prompt file parts, load/fork replay, prompt completion updates, live assistant/reasoning/tool/todo/status/queue/compaction/error/usage updates, and permission/question request routing, config validation through `/config/validate` and `doctor`, internal Rust plugin hooks for events/chat/tool execution/shell env, a repo smoke script, and safe `read`, `list`, `ffgrep`, `fffind`, `fff_multi_grep`, `grep`, `glob`, `write`, `edit`, `apply_patch`, `webfetch`, `websearch`, `bash`, `skill`, `lsp`, `task`, `question`, and `todowrite` execution. Remaining large gaps are full native provider adapter breadth beyond OpenAI-compatible and Anthropic, richer process/WASM/SDK plugin execution, protocol-complete MCP edge-case parity against more real servers, multi-client sync bootstrap/order audits, ACP real-client hardening/active resume diagnostics, non-Unix/deep PTY job-control edge cases, and install/update/setup polish.