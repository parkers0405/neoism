# Neoism Agent GUI

A standalone, strict-TypeScript React application for the Neoism Agent V2 API. It uses the local `@neoism/sdk` npm workspace packages directly—no WASM, desktop shell, mock sessions, or browser automation dependency.

## Run

From `neoism-agent/sdk/typescript` (Node **22.13+**):

```sh
npm install
npm run gui:build                  # build SDK first, then packages/gui/dist
neoism-agent web --no-open         # serves the built app; prints its URL
```

`neoism-agent web` opens the URL as well. This command requires the updated Rust executable; older installed binaries report `unrecognized subcommand 'web'`. The development GUI below can connect to an already-running server without rebuilding or restarting it. To attach without starting a server:

```sh
neoism-agent web --server http://127.0.0.1:4096 --no-open
```

Development:

```sh
npm run build                     # compile local SDK packages
npm run dev --workspace @neoism/gui # http://127.0.0.1:5174
```

The production app defaults to its own origin. Vite development defaults to `http://127.0.0.1:4096` regardless of the frontend port; `VITE_NEOISM_AGENT_URL` can override that endpoint. A production preview needs a server URL selected in Settings or provided at build time. Open the profile/settings gear to change the server URL, optional bearer token, or server-side workspace directory. Reverse-proxy URL prefixes are supported by the SDK transport. Cross-origin development requires the agent to allow the frontend's origin; HTTPS pages cannot call insecure remote HTTP endpoints.

Backend asset discovery, authentication and serving details: [`../../../../gui-server.md`](../../../../gui-server.md).

## Features

- ChatGPT-style left navigation: New Chat, Skills, Workflows, searchable cursor-paginated root sessions, rename/delete, and a sticky profile with the deterministic native presence avatar.
- Centered home with the full native **NEOISM** wordmark: original per-letter SVG layers, independent hover/lift/shimmer and click ripple, reduced-motion handling, and the desktop's 44px gap above the floating composer. No slogan or subtitle.
- Native-style composer island: raised input with an attachment/send band, curved lower agent/model/reasoning strip, directory on the left and Tab/command hints on the right.
- The footer project pill opens a searchable recent-project menu. **Add project** and `/cd` open a server-side folder browser: single-click selects, double-click/Right Arrow enters, Back navigates, and **Select folder** confirms. Browsing requires the updated `GET /v2/directories` API and follows the server's existing authentication and directory-scope policy; it never creates a session. An older running agent must be rebuilt/restarted before this browser can work.
- Chat-only sidebar with Agent/Model/Reasoning settings and the native beveled context meter. Subagent controls appear only while children are active; no raw session IDs or empty debug sections.
- Markdown/GFM with language/copy code-card headers. Completed response metadata appears below the answer: agent, model, duration, and streaming tokens/sec when reported. Collapsible reasoning/tools, Pondering/Crafting progress, and abort controls remain available.
- Continuous theme-colored chrome spans the sidebar and chat. Active tabs join the chat background without an underline; the chat composer overlays the canvas with measured clearance, while the home wordmark/input pair stays centered.
- Native Crafting/Pondering text uses the desktop scramble, letter-wave and ocean-dot animation. Background-job and subagent runtime envelopes become collapsed task cards, not user-message dumps; terminal control sequences are removed.
- Code highlighting uses locally bundled Tree-sitter WASM grammars in a lazy worker, with live native syntax-theme variables. Supported: Rust, JavaScript/JSX, TypeScript/TSX, Python, Bash, JSON, CSS, HTML. Unsupported or oversized code stays escaped plain text. Native semantic prose and response-footer color roles are also applied.
- Agent, model and reasoning-effort pickers PATCH the active session and restore its model, variant and provider account on reopen; GPT-5.6 includes `ultra`. Permission and question cards are session-scoped and support allow once/always/reject, multiple-choice and custom answers.
- Ordinary user messages use compact right-aligned Apple-blue bubbles without author headings. Display names are not treated as ownership IDs. Tool/task rows have no outer background boxes; real code and diff contents retain their formatting.
- Recents load additional pages on downward scrolling. Upward conversation scrolling loads older history with anchor preservation and once-per-cursor guards. Fetches show animated contextual skeletons, not visible “Loading more” labels.
- Models use search, Current/Recent selections, and provider groups in one continuously virtualized list, with no Previous/Next paging UI. Clicking an open composer selector toggles it closed.
- Reopening history hides old reasoning/tool details while live parts remain visible. Message ordering uses server creation timestamps and minimal parent-before-child constraints, never ID sorting or forced turn regrouping.
- Top chat tabs keep multiple conversations and unsent drafts open per server. Opening an existing chat reuses its tab; closing a tab does not delete or abort the conversation. Tabs, drafts, and selections restore on reload; attachment files survive tab switches in memory but are not persisted across reloads.
- Agent pickers and Tab cycling show only non-hidden primary/user agents, excluding subagent-only definitions such as Explore/General and internal Summary/Compaction/Title agents.
- New tabs load effective directory config defaults before the first send, with the native connected-provider fallback. Explicit tab choices and saved session metadata take precedence without inheriting another tab's model/account/reasoning.
- Selected sessions restore on refresh using server-scoped storage and encoded session links; browser Back/Forward restores selection. Active chat metadata is independent of filtered root Recents and child-session pickers.
- OpenCode-style two-column settings: dedicated General, Appearance, Servers, and Providers pages; grouped provider rows with right-aligned **+ Connect** actions and a compact back-enabled authentication flow.
- Geist interface text and larger Geist headings, with a separate code-font preference defaulting to **JetBrains Mono**. The exact native **Press Start 2P** face remains for the Crafting/Pondering animation. All **101 native themes** remain available. The default uses native **Pastelbeans**, matching the current desktop configuration; the earlier synthetic gray `neoism` preference is migrated without changing explicitly selected native themes. Licensed font assets are bundled locally; saved preferences persist, bearer tokens do **not**.
- Theme selection opens one searchable in-dialog picker. Providers auto-load, search the whole catalog, and progressively reveal further rows on scroll; the API returns the catalog in one response, so this is client-side windowing rather than network pagination.
- Provider API-key and OAuth account management in Settings and the `/connect` panel above Composer: select, label, rename, set default, or delete an account. Explicit account IDs persist per server/provider and accompany session models, prompts and server commands. Deleting a selected account blocks sending until an account is explicitly chosen—no silent billing fallback. Authentication is performed by the selected server.
- Skills CRUD with name/description/Markdown/scope forms and full-definition JSON, revision-aware updates, and read-only catalog fallback when management is unavailable.
- Workflow CRUD with prompt/schedule forms, weekday and date controls, optional full-definition JSON, activate/pause/run-now, diagnostics and run history with links to resulting chats.

## Commands

The floating `/` picker consumes the generated native catalog: **32 commands / 52 spellings**, plus live server commands. Native aliases retain priority when names overlap.

- Type `/` and filter by name, alias or description.
- **↑/↓** and **Tab/Shift+Tab** navigate the slash and follow-up lists; **Enter** accepts or runs the selection and **Esc** dismisses. Typing a space after a slash command leaves the menu for argument entry.
- Model, agent, session, directory, skill, provider-account, and MCP pickers are anchored above the composer, not centered modals. They clamp to the available viewport height and return focus to the composer when dismissed.
- Outside the picker, **Enter** sends and **Shift+Enter** inserts a newline. IME composition is respected.
- `/models` and `/model` open the same model picker. `/agent`, `/think`, `/sessions`, `/sub-agent`, `/skills`, `/connect`, `/cd`, `/new`, `/exit`, `/sidebar`, `/hints` and aliases have local UI handlers.
- `/skill` (or `/skills`) opens a searchable catalog chooser. `/skill <name>` or a chooser selection inserts `$name` into the draft without sending. `/skill list` and `/skill info <name>` show catalog details.
- `/cd` opens a server-backed searchable directory chooser; `/cd <path>` updates the active session directory without creating a new chat or changing the global directory filter. An open session is required.
- `/goal`, `/mcp`, `/queue`, `/permissions`, `/permit`, `/questions`, `/answer`, `/reject`, `/compact`, `/undo`, `/redo`, and `/abort` call actual SDK operations. A new goal creates a session if needed, sets the goal, then sends its initial prompt using the current selections. Pending interaction and capability failures are shown in the app.
- `/yolo` and its dangerous aliases require explicit confirmation, show a persistent warning, and auto-reply **once** to permission requests only while the current session remains open in this tab. Bypass is never saved or applied globally; use the warning banner to re-enable checks immediately.
- The six native playful visual commands have web-native scenes and timed native model prompts without modifying the draft. Repeating/replacing an effect restarts its timers; changing session/server/directory or unmounting cancels them. Reduced motion is respected; the desktop shader renderer is not required.
- Every unrecognized command is forwarded through `sessions.command` with the separate command/arguments, current agent and selected model. Unknown commands are not silently discarded.

## Security and capabilities

The GUI is public static content; the **API remains authenticated**. Store provider secrets on a trusted server and use HTTPS remotely. API keys/bearer tokens are never included in local preferences. OAuth links are opened with `noopener noreferrer`.

Skill management is explicitly opt-in: enable `NEOISM_AGENT_MANAGEMENT_API=1` on the server **and configure a credential**. Enter that bearer token in Settings. Built-in/discovered skills can be read-only. Workflows require the `neoism.workflows` capability. A disabled plugin, absent route, 401/403, or revision conflict produces an actionable error rather than fabricated success.

Markdown does not render raw HTML. Unsafe link protocols are rejected by the renderer. Remote images are shown as placeholders to avoid automatically contacting tracking URLs. Clipboard failures remain visible. Delete, persistent permission grants and credential removal require confirmation.

## Architecture

- `useAppController.ts`: app state, catalog/preferences, session operations and UI-command coordination.
- `useChat.ts`, `state.ts`: typed SSE reducer, bounded event deduplication, abortable history fetches and cursor pagination.
- `useSessionEvents.ts`: live session metadata/deletion updates without resetting pagination.
- `commands.ts`, `nativeCommands.ts`: picker filtering, alias resolution, confirmation and SDK command handlers.
- `components/`: independently scoped navigation, composer, picker, markdown, timeline, settings, provider auth, library forms, interactions and details.
- `src/generated/*`, `scripts/*`: separately owned native theme/command/logo/avatar/font generators and parity tests. Do not hand-edit generated modules.

The SDK internally reconnects SSE. The GUI reconciles the latest history on window focus and every 15 seconds with a single-flight fetch, without triggering older-page loads. Because history responses do not expose an SSE snapshot watermark, snapshots racing live events are deferred until the next quiet reconciliation rather than replaying deltas and duplicating text. Older pages load only by explicit user action, using the returned cursor or a message-ID boundary fallback when the server omits a cursor. Busy indicators reconcile runtime state as well as live events, so reopening an already-running session does not depend on having observed its initial status event. Recent snapshot reconciliation removes reverted/deleted recent messages while preserving already-loaded older pages.

## Verify

```sh
npm run gui:test                   # reducer/commands/SSR rendering + native asset/font parity
npm run assets:check -w @neoism/gui # native source / generated asset drift checks
npm run gui:build
npm test                          # existing SDK contract/plugin/consumer tests
npm audit
```

Component security tests use server-side React rendering, **not** Playwright or another browser tool. Live browser layout, OAuth completion and server-backed workflow execution should also be exercised against your configured server; those require real credentials and enabled capabilities.

The context meter uses the latest nonzero step's input, output, reasoning, and cache usage against the selected model's reported context capacity. It is not cumulative spending or an account-wide quota, and an unknown capacity is not shown as a guessed percentage. Available models/agents and accepted reasoning variants come from the selected server/provider; unsupported selections surface the server's validation errors. Weekday/date scheduling is available in workflow forms; advanced concurrency, retries, agent/model and permissions are also available in the JSON editor.
