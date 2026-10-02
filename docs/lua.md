# Lua customization

Neoism loads `~/.config/neoism/init.lua` and modules from `~/.config/neoism/lua`. Without `init.lua`, native Rust behavior and appearance are unchanged. Reloads are transactional: an invalid config leaves the last working runtime active.

## Canonical plugin documentation

This page is the editor Lua quick reference. Plugin authors and coding agents should use [`plugins.md`](plugins.md) for tier selection, the complete shared package manifest, lifecycle, trust and compatibility; [`editor-plugin-api.md`](editor-plugin-api.md) for every editor namespace, operation and contribution; [`agent-plugins.md`](agent-plugins.md) for Agent Lua, `neoism-plugin/2`, `@neoism/plugin`, native Agent plugins and remote resources; and [`lua-api.lua`](lua-api.lua) for generated Lua language-server signatures. Native Rust, editor Lua, Agent Lua and process plugins are parallel tiers: adopting one does not require migrating plugins written for another tier.

```lua
local neoism = require("neoism")

neoism.setup({ ui = { window = { opacity = 0.96 } } })

neoism.ui.style("file-tree.row.selected", {
  background = "#252a38",
  foreground = "#ffffff",
})
neoism.ui.style("agent.chat.message.user", { background = "surface", accent = "blue" })
neoism.ui.style("status", { background = "#11131a", foreground = "#d8dee9" })

neoism.command.register("notes.today", function()
  neoism.notes.open({ path = "daily/today.md" })
end, { title = "Open today's note" })

neoism.keymap.set("global", "<C-S-n>", "notes.today")
neoism.autocmd("GitChanged", function(event)
  local git = neoism.git.snapshot()
end)
```

## UI

`neoism.ui.style(selector, patch)` styles native Sugarloaf surfaces. Selectors inherit through dotted parents. Stable selectors are `app`, `chrome.top`, `chrome.bottom`, `buffer-tabs`, `breadcrumbs`, `status`, `status.item`, `composer`, `file-tree`, `file-tree.row`, `file-tree.row.selected`, `file-tree.row.hover`, `file-tree.icon`, `notes-tree`, `notes-tree.row`, `notes-tree.row.selected`, `notes-tree.row.hover`, `notes-tree.icon`, `agent.chat`, `agent.chat.message`, `agent.chat.message.user`, `agent.chat.message.assistant`, `agent.chat.tool`, `agent.chat.tool.result`, `agent.sidebar`, `editor`, `markdown`, `terminal`, `git`, `settings`, `palette`, `finder`, `notification`, and `modal`.

Style fields are `visible`, `width`, `height`, min/max sizes, `padding`, `padding_x`, `padding_y`, `gap`, `row_height`, `font_family`, `font_size`, `font_weight`, `line_height`, `foreground`, `background`, `border_color`, `accent`, `muted`, `border_width`, `radius`, `opacity`, `order`, `scroll_multiplier`, `scroll_smooth`, `animation_ms`, `animation_easing`, and `icon`. Colors accept semantic names or `#RRGGBB`/`#RRGGBBAA`.

`neoism.ui.contribute({ id, slot, text, icon, tooltip, command, style, priority })` adds clickable declarative content to `top.left`, `top.right`, `status.left`, `status.right`, `bottom.left`, `bottom.right`, `sidebar.left`, `sidebar.right`, or `git.header`. Lua never runs in a render or pointer-hit-test path.

`neoism.ui.surface(id, patch)` changes the placement of a Rust-owned reserving surface. `neoism.ui.item(id, patch)` hides, aligns, or reorders one of its Rust-owned controls. The action chrome can become a VS Code-style left rail without introducing Lua into layout or input paths:

```lua
neoism.ui.surface("chrome.actions", {
  dock = "left",
  thickness = 44,
})

neoism.ui.item("chrome.notes", { visible = false })
neoism.ui.item("chrome.search", { order = 10 })
neoism.ui.item("chrome.explorer", { order = 20 })
```

Surface fields are `visible`, `dock` (`top`, `bottom`, `left`, or `right`), `thickness`, and `order`. Item fields are `visible`, `surface`, `align` (`start` or `end`), and `order`. The current built-in surface is `chrome.actions`; its items are `chrome.menu`, `chrome.explorer`, `chrome.notes`, `chrome.new-agent`, `chrome.search`, `chrome.presence`, `chrome.agent-details`, `chrome.agent`, and `chrome.servers`. Invalid surface geometry or unknown IDs reject the candidate reload and preserve the last working plugin generation.

`neoism.panel.register(id, { title, icon, location, visible, content, render })` registers a declarative `left`, `right`, `bottom`, `center`, or `overlay` panel. A `render(event)` callback is evaluated only at startup or on events and returns contribution rows.

## Commands, keys, and events

`neoism.command.register(id, callback, options)` adds a command. Options include `arguments_schema`, `result_schema`, aliases, static `completions`, a dynamic `complete(event)` provider, and `accepts_range`, `accepts_count`, and `accepts_bang`. `neoism.command.execute(id, args)` remains the compatibility invocation. `neoism.command.invoke({ command, arguments, range, count, bang })` returns a request ID and emits one exact-owner `CommandResult`; `neoism.command.cancel({ id })` logically cancels a result that has not yet been delivered.

`neoism.keymap.set(mode, chord, command_or_callback, { when })` supports `global`, `terminal`, `editor`, `markdown`, `agent`, `file-tree`, `notes`, `git`, `palette`, `finder`, and editor `normal`, `insert`, or `visual` modes. Predicates include `editorFocus`, `terminalFocus`, `agentFocus`, `normalMode`, `!`, and `&&`.

`neoism.autocmd(event, callback, { pattern, once, scope })` receives `Startup`, `ConfigReloaded`, `LspResult`, and the `BufferChanged`, `WorkspaceChanged`, `TabChanged`, `PanelChanged`, `FileTreeChanged`, `NotesChanged`, `AgentChanged`, `TerminalChanged`, `GitChanged`, `ThemeChanged`, `PluginsChanged`, and `ConfigChanged` state events. `pattern` supports `*` wildcard matching against payload paths.

On desktop, `AgentChanged.payload` includes `composerRevision`, `composerLength`, and `composerEmpty`. The revision advances immediately when the focused Agent composer changes, including typing, deletion, paste, history recall, clearing, and submission. Draft text is intentionally not exposed. Event-driven panel `render(event)` callbacks may use these fields to update retained UI without running Lua on the input or render thread.

`neoism.effect.emit({ kind = "particles", ... })` queues one bounded local vector-particle burst over the active Agent timeline. The plugin supplies normalized sprite polygons, per-vertex flap weights, theme color tokens, a deterministic seed, duration, angle and speed ranges, gravity, wobble, origin spread, size range, and flap motion. Rust validates strict complexity and motion caps, then owns the animation clock, trajectories, drawing, redraw lifetime, and automatic retirement. It requires the `effect.emit` capability and does not create hit regions or expose draft text.

Scopes are `local`, `workspace`, `shared-buffer`, and `presence`.

## Native objects

Every object supports `snapshot()`, `get()`, `query(operation, args)`, and `call(action, args, scope)`. Convenience methods cover common actions.

| Object | Actions |
|---|---|
| `buffer` | `open`, `edit`, `save` |
| `workspace` | `open`, `focus`, `split` |
| `tab` | `create`, `close`, `focus`, `move` |
| `panel` | `show`, `hide`, `toggle`, `focus`, `register` |
| `file_tree` | `open`, `reveal`, `refresh`, `create`, `create_dir`, `rename`, `move`, `delete` |
| `notes` | `open`, `refresh`, `create`, `create_dir`, `rename`, `move`, `delete` |
| `agent` | `open`, `create`, `send` |
| `terminal` | `send`, `run` |
| `git` | `stage`, `commit`, `refresh` |
| `config`, `theme` | `set` |

Shared buffer edits are one daemon-owned CRDT transaction. File and note mutations in joined workspaces use the daemon files plane. Workspace layout and tab moves use serialized daemon layout operations. Viewport, hover, focus, and overlays stay local.

### Typed host contract and document handles

All compatibility calls are resolved through a Rust-owned typed registry before they enter the application queue. Each registered operation declares its capability, execution scope, target kind, cancellation policy, and result schema. Unknown operations, wrong scopes, missing owner revisions, and actions from rejected or retired candidate generations are discarded before privileged dispatch. `neoism.contract` exposes the portable contract metadata and `docs/lua-api.lua` provides Lua language-server annotations.

Host event names and asynchronous results use the same typed registry. Unknown autocmd event names are rejected while loading a candidate. Asynchronous results require both the exact plugin content revision and a non-empty request identity; stale revisions are discarded before callback delivery. `docs/lua-api.lua` is generated deterministically by `neoism_lua::generate_lua_api_annotations`, and a test rejects drift between the registry and the checked-in annotations.

`neoism.document.current()` returns an immutable snapshot with opaque `document`, `pane`, `tab`, and `workspace` handles, text, zero-based UTF-8 cursor/selection positions, metadata, dirty state, and buffer revision. Handle values are host-owned capabilities: plugins must retain and return them, never parse them or treat a remote `hostPath` as a guest-native path.

```lua
local document = neoism.document.current()
local revision = document.revision
local range = {
  handle = "", -- omit/empty for a newly described range
  start = { line = 0, character = 0 },
  ["end"] = { line = 0, character = 0 },
}

neoism.document.edit({
  document = document.handle,
  expectedRevision = revision,
  edits = { { range = range, text = "-- inserted atomically\n" } },
})
```

Document reads include `text`, `lines`, `range`, `cursor`, `selections`, `metadata`, `language`, `dirty`, and `revision`. Mutations currently include atomic non-overlapping range edits, cursor moves, selection/multi-cursor replacement, exact-target focus, save, and close. Every mutation requires the captured document handle and expected revision; a stale handle or revision fails instead of retargeting the focused pane. Text edits use `CodeBuffer::apply_text_edits`, become one undo unit, and flow through the normal Rust-owned CRDT binding as one transaction on collaborative buffers. Cursor, selection, focus, geometry, visible ranges, and scroll remain local view state.

Editor anchors and decorations are Rust-owned resources scoped to an exact plugin revision. Create a namespace with `neoism.namespace.create()`, then anchors with `neoism.anchor.create()` using an exact document handle/revision and zero-based UTF-8 position. The returned queued-action `id` is the opaque namespace or resource handle and may be passed immediately to later calls in the same callback; host actions are applied in order. `neoism.decoration.create()` accepts highlight, gutter-sign, virtual-text, virtual-line, inline-widget, code-lens, fold, conceal, and diagnostic layers. `neoism.diagnostic.publish()` uses the same request with diagnostic severity, related information, tags, and command actions.

CRDT-bound anchors use Yrs sticky indices and are resolved after editor/CRDT service turns. Unbound documents use the same Rust-owned deterministic edit relocation, including local, remote, and plugin edits. The renderer receives an immutable, line-projected snapshot with resolved RGBA values and no callbacks or VM references. Folding is incorporated into the shared wrap/display map, so scrolling and pointer row mapping use the same hidden-line projection; concealment retains source-cell hit testing while suppressing the concealed glyphs. Retiring or reloading a plugin removes only resources owned by that exact content revision.

Autocommands support named augroups through `neoism.augroup(name, { clear = true })`, exact insertion order, integer priority, `once`, and `nested`. Recreating an augroup with `clear` removes the previous generation's declarations; `{ delete = true }` clears and deletes it. Nested event dispatch skips ordinary autocommands and runs only callbacks declared with `nested = true`; a callback already active in the current dispatch chain is not re-entered. Commands may declare an `arguments_schema`, static `completions`, and range/count/bang acceptance metadata. Document and pane lifecycle events include `DocumentOpened`, `DocumentClosed`, `DocumentFocused`, `DocumentChanged`, `SelectionChanged`, `DiagnosticsChanged`, `PaneFocused`, and `PaneChanged`.

Virtual-line decorations reserve synthetic rows in the shared wrap index; scrolling and pointer-to-source mapping therefore use the same layout as paint. Inline widgets, virtual lines, and diagnostic actions publish retained Rust hit regions. Activating one queues a typed `diagnostic.execute_action` carrying the exact owner and resource identity; stale or cross-owner resources are rejected before the command callback runs.

`neoism.state` provides exact-owner plugin/document/pane/tab/workspace state. Non-plugin scopes require a currently published opaque target handle; stale or cross-scope handles are rejected rather than retargeted. Ephemeral state is removed when its exact plugin revision retires, and document/pane/tab/workspace state is cleared when that target closes or changes. Set `persistent = true` only for plugin or workspace scope. Durable values are keyed by plugin ID (and the opaque workspace handle for workspace scope), survive a validated revision replacement, and are committed atomically only after the candidate host becomes active. Keys, values, and per-owner entry counts are bounded.

Callbacks enforce time, nesting-depth, callbacks-per-event, and queued-action budgets. Command aliases resolve to a canonical command before invocation. Argument schemas and range/count/bang acceptance are checked before callback entry; result schemas are checked before an asynchronous result envelope is published. Dynamic completion and command callbacks run on the application Lua executor, never renderer or worker threads. Command request IDs and results are scoped to the requesting `PluginOwner`; cancellation and late delivery cannot cross plugin revisions.

`neoism.option.set` contributes typed `wrap`, `tab_width`, `use_tabs`, or `input_mode` values at document, pane, tab, or workspace scope. Rust-defined defaults and native pane values remain the baseline; active exact-owner contributions overlay them by scope specificity, explicit priority, and insertion order. Stale targets and retired revisions are ignored, and removing the overlay restores the native baseline.

Keymaps and `neoism.motion`, `neoism.operator`, `neoism.text_object`, and `neoism.input` registrations accept single- or multi-key sequences, mode/condition metadata, priority, and an explicit policy for the failing key after a prefix mismatch. Prefix matching is bounded by a 750 ms timeout. Registration callbacks still produce typed host actions on the application executor; they do not run on the input or render path and do not own an editor state machine.

`neoism.register`, `neoism.mark`, `neoism.jumplist`, and `neoism.macro` expose bounded projections of the focused native Vim state and require the exact published document handle for mutation. Lines and UTF-8 byte columns are zero-based. Register and macro writes, jumps, and replay use the native editor, undo, and CRDT paths. Clipboard writes are brokered through the application-owned OS clipboard; synchronous authoritative clipboard reads and changelist navigation are not exposed until those native host services exist.

`neoism.scheduler.after` and `neoism.scheduler.every` schedule registered typed commands on the application executor. Timer handles are exact-owner resources, repeating intervals have a minimum cadence, callback arguments are schema-checked, catch-up bursts are coalesced, and plugin retirement cancels that revision's timers deterministically. `neoism.scheduler.cancel` rejects stale or cross-owner handles.

## Asynchronous host services

`AsyncResult` is the common application-executor delivery event for managed jobs, prompts, clipboard reads, and result-list queries. Its payload is `{ id, kind, ok, cancelled, terminal, result, error }`. Output chunks use `terminal = false`; success, failure, and cancellation produce exactly one terminal event. Requests key by exact plugin ID, content revision, request ID, and originating window. `neoism.async.cancel({ id = request.id })` is logical cancellation: it publishes cancellation once, suppresses late worker results, and never transfers a result to a replacement plugin revision.

`neoism.job.spawn` accepts an executable name, bounded argument vector, workspace-relative `cwd`, explicit environment, timeout, and output-byte limit. It requires the declared and granted `job.execute` capability. Absolute executable paths, workspace escapes, loader-injection environment keys, excessive payloads, and remote paths interpreted with guest semantics are rejected. Jobs run in their own process group; timeout, cancellation, and owner retirement terminate the process tree. Reader and wait threads only enqueue bounded data. `AsyncResult` output records identify `stdout` or `stderr`, and the terminal exit record reports status, aggregate bytes, and truncation. `stdin`, `close_stdin`, and `cancel` require the exact opaque job handle.

`neoism.notification.show` publishes a bounded native notification. `neoism.progress.create/update/finish` uses exact-owner progress handles. `neoism.prompt.input/confirm/select` opens a native modal on the requesting window; replies and cancellation are matched to that exact owner/request/window before `AsyncResult` delivery. Reloading or retiring the owner closes stale prompts and discards replies.

`neoism.result_list` owns bounded quickfix, location, or general result sets. Entries retain opaque document handles plus zero-based UTF-8 positions; Lua cannot reinterpret host paths. Lists support create, replace, append, clear, delete, asynchronous query, and indexed native navigation. Handles and entries are removed when their exact owner revision retires.

`neoism.clipboard.read()` uses the same coordinator and returns through `AsyncResult`; `neoism.clipboard.set()` remains an application-owned write. OS clipboard access never occurs on a Lua or render thread.

## Structured LSP requests

The existing convenience calls such as `neoism.lsp.definition()` and `neoism.lsp.hover()` continue to open Neoism's native finder or popup UI. Plugins that need data instead use `neoism.lsp.request(operation, args)`, which returns `{ id = "lua-N" }` immediately and later emits one owner-targeted `LspResult` event.

```lua
neoism.autocmd("LspResult", function(event)
  local reply = event.payload
  if reply.id ~= pending_definition then return end
  if reply.cancelled then return end
  if not reply.ok then
    print(reply.error.code .. ": " .. reply.error.message)
    return
  end
  for _, location in ipairs(reply.result.items) do
    print(location.path, location.range.start.line, location.range.start.character)
  end
end)

local request = neoism.lsp.request("definition", {})
pending_definition = request.id
```

Local and remote structured reads support `hover`, `signature_help`, `definition`, `references`, `document_symbols`, `workspace_symbols`, `diagnostics`, and `clients`. `workspace_symbols` accepts `{ query = "name" }`. Remote requests preserve host-native path spelling and resolve relative paths lexically inside the remote workspace without interpreting them as guest paths. Read requests require `lsp.read`; `rename`, `format`, `code_actions`, and `apply_code_action` require `lsp.edit` even when invoked through the generic request method.

Structured edits use the same asynchronous result channel. `rename` requires `{ newName = "replacement" }`, while `format` accepts the normal optional target arguments. A successful edit result contains `{ title, changedFiles, ranCommand }`; each changed file reports `{ path, editCount, appliedBy }`. Structured formatting changes the buffer and its normal CRDT state but deliberately does not save, emit `DidSave`, or mark the edited content as saved.

`code_actions` returns only portable summaries: `{ id, requestId, title, kind, preferred }`. Apply one with `neoism.lsp.request("apply_code_action", { requestId = action.requestId, actionId = action.id })`. The IDs form a Rust-owned, owner- and source-request-scoped one-shot capability with a five-minute lifetime. Selection consumes it even if later target validation fails, and Lua cannot override the captured root, path, position, server, command, or edit payload. Raw server IDs, commands, workspace edits, and document hashes never cross into Lua.

Edit preparation never mutates. Immediately before commit, Neoism revalidates the captured target and every touched open-buffer revision, checks closed-file content, rejects files that changed ownership by opening or closing, and preflights workspace containment, symlink escapes, UTF-8 byte boundaries, reversed or overlapping ranges, malformed edits, and resource operations. Open buffers are changed through the owning frontend and normal CRDT plumbing; only closed remote files are patched by the workspace daemon. A stale or invalid plan fails as one terminal result rather than applying a validated subset.

The request target is captured before background work begins and returned as `target = { root, path, line, character, bufferRevision, paneId }`. Omitted `path`, `line`, and `character` values come from the focused code pane. A relative path is resolved inside the focused workspace; a request cannot escape that root. All lines and characters crossing the Lua boundary are zero-based, and characters are UTF-8 byte columns. `bufferRevision` is independent from the plugin owner's revision and is null when the requested file is not the focused open buffer.

A successful reply is `{ id, operation, target, ok = true, cancelled = false, result, error = nil }`. `result.kind` matches the operation and `result.items` contains portable records for locations, hovers, signatures, symbols, diagnostics, or clients. A failed reply has `ok = false` and `error = { code, message }`. Diagnostics are a non-blocking snapshot of the engine's latest published diagnostics.

Cancel a pending request with `neoism.lsp.cancel({ id = request.id })`. Cancellation requires `lsp.cancel`, is restricted to the exact plugin ID and revision that created the request, emits one terminal reply with `cancelled = true`, and suppresses a late worker result. Reloading a plugin also prevents results owned by its old revision from reaching the new runtime. Each owner may have at most 64 pending requests, with a process-wide limit of 512; excess requests receive a `request_limit` error. Lua callbacks always run on the application thread; language-server workers never execute Lua.

## Plugin packages

A local development package lives at `~/.config/neoism/plugins/<id>/` and contains `neoism-plugin.json`, its manifest entrypoint, and optional modules under `lua/`. Each active package receives an isolated Lua VM; `require` can only resolve `<package>/lua/?.lua` and `<package>/lua/?/init.lua`.

```json
{
  "id": "dev.example.notes",
  "name": "Example Notes",
  "version": "1.0.0",
  "apiVersion": 1,
  "editor": { "entrypoint": "init.lua" },
  "dependencies": [],
  "capabilities": ["notes.read", "notes.write"],
  "triggers": [
    { "kind": "command", "value": "example.notes.open" },
    { "kind": "key", "value": "ctrl+shift+n", "mode": "global" },
    { "kind": "filetype", "value": "markdown" },
    { "kind": "surface", "value": "notes" }
  ]
}
```

Declarative specs live in `~/.config/neoism/lua/plugins/*.lua`. A spec can select a local path or an HTTPS Git source and revision. Managed Git installs are checked out into immutable commit-addressed revisions; `plugins.lock.json` records the exact commit, manifest and tree checksums, dependencies, and installed revision. A failed install, update, validation, or reload preserves the prior lockfile and active generation.

Lua packages appear alongside other packages in the Extensions page's `All` view. Rows expose discovered, lazy, loaded, disabled, permission-required, incompatible, blocked, failed, update, restore, and active-job states. Install, update, restore, and remove run on a serialized background worker with stage progress; enable/disable and capability grants update the grouped `plugins` policy. If the full activation candidate rejects an acquired revision, Neoism restores the exact prior lock entry and keeps the last-known-good runtime live.

Capabilities must be both declared by the manifest and granted under `plugins.grants` in `config.json`. `plugins.disabled` disables package IDs, `plugins.trusted-sources` optionally allow-lists Git URL prefixes, and `plugins.update-policy` is `manual`, `notify`, or `automatic`. Personal `init.lua` remains trusted and overlays package defaults after all active plugin snapshots are combined.

### Mash Up Pack editor plugin selection

A Mash Up Pack may select desktop editor Lua packages with a top-level `editor-plugins` object in `packs/<id>/pack.json`:

```jsonc
{
  "pack": {
    "name": "Focused Writing",
    "theme": "focused-writing"
  },
  "editor-plugins": {
    "mode": "only",
    "enabled": ["dev.example.prose", "dev.example.spellcheck"],
    "disabled": ["dev.example.code-minimap"]
  }
}
```

`overlay` starts with the ordinary globally eligible editor package set and subtracts `disabled`. `only` treats `enabled` as roots and admits those packages plus their dependency closure. IDs are trimmed, blank IDs are dropped, and lists are sorted and deduplicated. An ID present in both effective lists rejects the candidate deterministically. A disabled or missing dependency produces the normal plugin graph failure; a missing `only` root also rejects the candidate. Missing IDs in an `overlay` declaration have no effect.

Users can replace individual declaration fields for one pack under the grouped `plugins.mashup-overrides` map. Omitted fields inherit from the pack, while an explicit empty array clears that field:

```jsonc
{
  "appearance": {
    "mashup-pack": "focused-writing"
  },
  "plugins": {
    "disabled": ["dev.example.untrusted"],
    "mashup-overrides": {
      "focused-writing": {
        "mode": "overlay",
        "enabled": [],
        "disabled": []
      }
    }
  }
}
```

Precedence is global policy first as a security boundary, then the resolved pack declaration with user field replacements. `plugins.disabled` is always a hard veto; a pack cannot grant capabilities, approve native artifacts, bypass `plugins.trusted-sources`, or re-enable a globally disabled package. Replacing a pack's `disabled` list can make that pack-controlled package eligible again, but it does not alter the global disabled list. The feature applies only to the native desktop editor Lua manager; Agent and daemon plugins are unchanged. With no active pack, an unknown active pack, or no declaration/override, plugin behavior is byte-for-byte the normal global selection path.

Pack picker and modal input only queue an intent. The application-owned pump resolves that exact pack manifest once, derives the candidate configuration and effective plugin selection, builds the complete editor Lua candidate, and preserves the current configuration, visuals, manager, and snapshot if resolution or discovery fails. It then persists the active pack, pack theme, and pack font in one backend write before atomically committing the accepted Lua generation and resolved visual slots in the same application operation. Deactivation uses the same path with no pack selection, restoring the normal global editor-plugin set. The watcher event caused by persistence rebuilds the same policy and is idempotent.

Fallible wallpaper decoding and shader setup occur before infallible theme, filter, font, and look publication. A synchronous visual asset failure restores the prior wallpaper where necessary, rolls the config fields back in one write, and leaves the prior Lua generation active. A failure in the best-effort rollback itself is logged; a graphics backend failure that occurs only after Sugarloaf has accepted a deferred GPU upload cannot provide a synchronous rollback signal. Lua never runs in Screen, render, layout, hit testing, or input handling.

Triggerless and `Startup` packages load eagerly. Command, event, key, filetype, and surface triggers load dependencies first and activate the package only when needed. A lazy key is intercepted by Rust, the candidate runtime is activated off the input path, the immutable snapshot is republished, and the newly registered mapping is replayed exactly once.

The embedded editor Lua VM has a 64 MiB limit, bounded callbacks, no `io`, `os`, `debug`, native libraries, or unrestricted module path. Agent Lua uses a separate supervised runner with different limits documented in [`agent-plugins.md`](agent-plugins.md). Rust owns rendering, layout, scrolling, animation, validation, CRDT state, and GPU work.