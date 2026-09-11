# Tasks integration (isolated implementation)

No App, Timeline, ToolPart, Markdown, useChat, or activity-popover files were changed.

## Native source findings

- `neoism-frontend/shared/src/panels/agent_pane/view/side_panel/sections.rs`:
  `latest_todos` walks backwards to the latest non-empty tool list; Tasks is below
  subagents, after the goal/session sections. Completed tasks remain visible.
- `view/tool_message/render.rs`: todos also have a dedicated inline renderer, a
  thin left rule, no generic tool box, no fold target (explicit comment near line
  300). Inline native limits display to 12 tasks; this GUI keeps the entire plan
  accessible, wraps long text, and has no automatic collapse.
- `view/tool_message.rs::TodoVisualState::from_status`: aliases are matched exactly.
- `neoism-agent/crates/neoism-agent-server/src/tool_runtime.rs`, `todowrite`:
  stores the full array, emits `todo.updated` with `{sessionID,todos}`, returns a
  JSON array in output plus `metadata.todos`. Input is only a proposal until tool
  success, not the authoritative source.
- Generated SDK contract: `client.operations.request("v2.sessions.todos", ... )`,
  `GET /v2/sessions/{session_id}/todos`; `EventTodoUpdated.data.todos`. Not a plugin.
  Native permissions apply to assistant execution; no user toggle API is exposed
  here. Checkboxes are read-only, with no pointer/keyboard mutations.

## Recommended session panel integration

```tsx
import { useSessionTodos } from "./useSessionTodos";
import { TodoPanel } from "./components/TodoPanel";

const tasks = useSessionTodos(client, sessionId, {
  messages: chat.state.messages, // use the parent's actual immutable messages array
  serverKey: serverIdentity,    // URL/account identity, especially for mutable clients
});
// In the session details/activity surface, after subagents; NOT per tool call:
<TodoPanel key={`${serverIdentity}:${sessionId}`} todos={tasks.todos} />
```

`subscribe` defaults to true: one session-filtered SDK stream and one GET per
attach. No timer or token-triggered requests. For the existing useChat SSE stream,
pass `subscribe: false` and forward events to `tasks.onEvent(event)` from that
same **client/session generation**. Capture the callback when subscribing (or
retain the stream's generation guard); do not forward a retired client's stream
through a ref pointing at a new client's callback. The hook rejects old captured
callbacks and old requests after client/session/server changes and cleanup.

Return values: `todos`, `loading`, `error`, `source` (`server` or `messages`),
`latestPartId`, `onEvent`. Show `error` with the parent's existing nonblocking error
UI if desired. SSE updates win over in-flight hydration and older event sequences.
Explicit empty arrays clear the plan; completed items otherwise stay in the full
array, with their DOM keys intact so glyph stroke/scale and text strike transition.
Do not key TodoPanel by tool call, part ID, array identity, or completion count.

Before the first response, and if the endpoint is unavailable, the hook derives
only the latest successful todowrite from immutable messages. Older-page loads
cannot replace a newer plan. No text checklist scraping. Malformed snapshots
are ignored rather than partially clearing good data. A successful endpoint
response (including an explicit empty array) is authoritative. The current Rust
endpoint reads an in-memory map; if server restarts must recover persisted plans,
that backend needs restoration or an explicit hydration marker—an empty response
cannot safely be distinguished from an intentional clear by the GUI.

## ToolPart integration (parent owner must apply)

Before generic raw JSON rendering:

```tsx
import { isTodoTool } from "../todoHelpers";
// When session TodoPanel is mounted, suppress only successful todo output:
if (isTodoTool(part) && part.type === "tool" && part.state.status === "completed")
  return null;
```

Leave genuine failures/pending permission state visible. Do not filter text parts
or Markdown task lists. If choosing native inline placement instead of the side
panel, mount the same session-level `<TodoPanel placement="inline" .../>` once at
a stable transcript slot. `TodoToolPart({part})` is exported for the latest-only
snapshot view; gate with `part.id === tasks.latestPartId`, never render every old
plan. A per-part timeline container may remount on each call, so use the stable
session panel for cross-call animations rather than relying on that wrapper.

## Markdown integration (optional parent-owned edit)

```tsx
import { MarkdownTodoInput } from "./TodoPanel";
<ReactMarkdown remarkPlugins={[remarkGfm]}
  components={{ ...components, input: MarkdownTodoInput }} />
```

This replaces only checkbox inputs with the same custom glyph, keeps all task-list
`li` contents (including genuine user checklists), and leaves other input types
alone. No disabled browser checkbox appearance. CSS is imported by TodoPanel;
UI uses the existing Geist `--font`, no monospace tool container. There is no code
rendering here; existing code remains on the app's JetBrains Mono `--font-code`.

Tests: native-shaped output/metadata; status aliases; malformed arrays; stable
IDs/duplicate content occurrence keys; latest-only history; pending→active→done
DOM identity; read-only semantics; Markdown user list preservation; scoped fetch,
SSE, stale requests/events, client/server/session switches and StrictMode. All are
Vitest/HappyDOM; no browser or Playwright.
