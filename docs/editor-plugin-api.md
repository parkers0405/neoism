# Editor plugin API reference

This reference enumerates the editor-exposed families in API version `1`. Use [`docs/lua-api.lua`](lua-api.lua) for generated callable signatures and [`neoism-lua/src/runtime.rs`](../neoism-lua/src/runtime.rs) for the exact installed Lua names; do not infer methods from similarly named host operations.

## Lua entrypoint and common conventions

The runtime installs both the global `neoism` table and `require("neoism")`; both refer to the same module. `neoism.api_version` is an integer and `neoism.contract` is a data-only introspection snapshot.

Every ordinary namespace created by the runtime has `query(operation, args?)` and `call(action, args?, scope?)`. Convenience names are installed broadly, but only namespace/operation pairs registered in [`contract.rs`](../neoism-lua/src/contract.rs) are valid; an invalid combination fails with `unregistered Lua host query` or `unregistered Lua host action`.

Use the named methods documented by [`docs/lua-api.lua`](lua-api.lua) for the core API. For broad extension operations, use explicit `namespace.call`, because names such as `git.status`, `agent.sessions` and `credential.status` also exist as synchronous compatibility queries and are not the asynchronous typed operations.

```lua
local request = neoism.git.call("status", {}, "local")
-- request.id is matched by an AsyncResult event.
```

All request tables cross serde. Fields are `camelCase` unless a table below says otherwise; enum values preserve the explicit serde casing shown here.

### Compatibility object namespaces

The runtime also exposes Rust-owned application objects through the generic namespace mechanism. Valid convenience actions are `buffer.open/edit/save`; `workspace.open/focus/split`; `tab.create/open/close/focus/select/move`; `panel.show/hide/toggle/focus/open/close`; `file_tree.open/reveal/refresh/create/create_dir/rename/move/delete`; `notes.open/reveal/refresh/create/create_dir/rename/move/delete`; `agent.open/create/send`; `terminal.send/run`; `git.stage/unstage/commit/refresh/open/toggle`; and `config.set/apply` plus `theme.set/apply`.

Every object namespace also has `snapshot()` and other broadly installed read conveniences, but only the pairs accepted by `query_contract` are valid and the returned snapshot shape is application-owned. Prefer typed document/platform APIs when a stable DTO exists.

File-tree/note creation and mutation and workspace/tab topology changes use workspace scope; buffer edits/saves use shared-buffer scope; focus, panels, terminal panes and native UI actions remain local. These actions still require the exact capability derived by the registry.

## Setup, configuration and style

### `neoism.setup(config)`

`setup` replaces this runtime snapshot's config patch. It recursively converts `_` in object keys to `-`, so `font_size` becomes `font-size`; it does not mutate array elements except by recursively normalizing contained objects.

Personal `~/.config/neoism/init.lua` is a trusted user overlay loaded only when present. Package defaults are combined first and the personal runtime overlays them; invalid replacement candidates preserve the last working generation.

### `neoism.ui.style(selector, patch)`

Style fields use snake_case and unknown fields are rejected.

| Field | Type |
|---|---|
| `visible` | boolean? |
| `width`, `height`, `min_width`, `max_width`, `min_height`, `max_height` | number? |
| `padding`, `padding_x`, `padding_y`, `gap`, `row_height` | number? |
| `font_family` | string? |
| `font_size`, `line_height` | number? |
| `font_weight` | integer? |
| `foreground`, `background`, `border_color`, `accent`, `muted` | string? |
| `border_width`, `radius`, `opacity`, `scroll_multiplier` | number? |
| `order` | integer? |
| `scroll_smooth` | boolean? |
| `animation_ms` | non-negative integer? |
| `animation_easing`, `icon` | string? |

Selectors inherit from dotted ancestors, broad to specific, while unspecified properties preserve the Rust draw site's default. Stable built-ins are `app`, `chrome.top`, `chrome.bottom`, `buffer-tabs`, `breadcrumbs`, `status`, `status.item`, `composer`, `file-tree`, `file-tree.row`, `file-tree.row.selected`, `file-tree.row.hover`, `file-tree.icon`, `notes-tree`, `notes-tree.row`, `notes-tree.row.selected`, `notes-tree.row.hover`, `notes-tree.icon`, `agent.chat`, `agent.chat.message`, `agent.chat.message.user`, `agent.chat.message.assistant`, `agent.chat.tool`, `agent.chat.tool.result`, `agent.sidebar`, `editor`, `markdown`, `terminal`, `git`, `settings`, `palette`, `finder`, `notification`, and `modal`.

## Declarative UI and retained views

### Contributions and panels

`neoism.ui.contribute(item)` accepts this `camelCase` DTO:

| Field | Type | Default |
|---|---|---|
| `id` | string | required |
| `slot` | string | required |
| `text` | string | `""` |
| `icon`, `tooltip`, `command`, `style` | string? | absent |
| `priority` | integer | `0` |

Known slots are `top.left`, `top.right`, `status.left`, `status.right`, `bottom.left`, `bottom.right`, `sidebar.left`, `sidebar.right`, and `git.header`; handling of other slots is implementation-defined.

`neoism.panel.register(id, panel)` accepts `{ title, icon?, location?, visible?, content?, render? }`. `location` is kebab-case `left`, `right`, `bottom`, `center` or `overlay` (default), `visible` defaults true, and `content` is an array of UI contribution DTOs. `render` is called at registration and retained as an application-executor callback for host event refreshes; it never runs during paint or hit testing.

### Chrome surfaces

`neoism.ui.surface("chrome.actions", patch)` accepts `camelCase` `{ visible?, dock?, thickness?, order? }`, where `dock` is lowercase `top`, `bottom`, `left` or `right`, and thickness must be finite and non-negative.

`neoism.ui.item(id, patch)` accepts `camelCase` `{ visible?, surface?, align?, order? }`, where `align` is lowercase `start` or `end`. Current item IDs are `chrome.menu`, `chrome.explorer`, `chrome.notes`, `chrome.new-agent`, `chrome.search`, `chrome.presence`, `chrome.agent-details`, `chrome.agent`, and `chrome.servers`; current `surface`, when supplied, must be `chrome.actions`.

### `neoism.ui.view`

`neoism.ui.view(id, view)` publishes the compact retained-view DTO; the runtime overwrites `id` and `owner`.

```lua
neoism.ui.view("word-tools.results", {
  kind = "tree",
  title = "Word results",
  state = { expanded = true },
  nodes = {
    {
      id = "root",
      kind = "row",
      role = "treeitem",
      label = "Result",
      value = { count = 1 },
      action = "word-tools.open-result",
      children = {},
    },
  },
})
```

`kind` is snake_case `picker`, `tree`, `table`, `list`, `form`, `modal`, `inspector`, `toolbar`, `breadcrumb`, `detail`, `virtual_document`, `scene`, `agent_timeline`, or `approval_card`. Nodes use `camelCase` `{ id, kind, role?, label?, value?, action?, children? }`.

View IDs are non-empty and at most 256 bytes, titles at most 1024 bytes, depth at most 32, node count at most 10,000, node IDs at most 256 bytes, node kinds at most 64 bytes, labels at most 16 KiB, and the serialized snapshot at most 1 MiB.

### `neoism.extension.register`: platform retained view

The broader platform API uses the adjacently tagged shape `{ kind = <snake_case>, declaration = <DTO> }`.

```lua
neoism.extension.register({
  kind = "view",
  declaration = {
    id = "word-tools.picker",
    title = "Word tools",
    root = {
      {
        id = "sort",
        role = "button",
        text = "Sort lines",
        value = {},
        columns = {},
        children = {},
        events = { activate = "word-tools.sort-lines" },
        disabled = false,
        expanded = false,
        selected = false,
      },
    },
    state = {},
    focus = "sort",
    accessibilityLabel = "Word tools actions",
  },
})
```

`UiRole` is snake_case `picker`, `tree`, `table`, `list`, `form`, `modal`, `inspector`, `toolbar`, `breadcrumb`, `detail`, `row`, `group`, `text`, `markdown`, `code`, `image`, `button`, `toggle`, `input`, `select`, `progress`, `separator`, or `scene`. Node IDs must be valid contribution IDs, unique within a view, and total nodes may not exceed 10,000; `focus`, if set, must name a node.

## Commands, keys, events, augroups, options and input

### Commands

`neoism.command.register(id, callback, options?)` supports Lua-option keys exactly as written below; these option keys are snake_case because the runtime's local deserializer does not apply a rename rule.

| Option | Type | Default |
|---|---|---|
| `title`, `description` | string | `""` |
| `scope` | `local` / `workspace` / `shared-buffer` / `presence` | `local` |
| `arguments_schema`, `result_schema` | JSON Schema-like object | `{}` |
| `completions` | string[] | `[]` |
| `complete` | function? | absent |
| `accepts_range`, `accepts_count`, `accepts_bang` | boolean | false |
| `aliases` | string[] | `[]` |

Schema validation currently enforces `type`, nested `properties`, object `required`, and `enum`; other JSON Schema keywords are implementation-defined. Dynamic completions receive a `Command` event and are merged with matching static completions.

`neoism.command.execute(id, arguments?)` is compatibility invocation. `neoism.command.invoke({ command, arguments?, range?, count?, bang? })` uses `camelCase` range `{ startLine, endLine }`, returns `{ id }`, and emits one exact-owner `CommandResult` with `{ id, command, ok, cancelled, result?, error? }`. `neoism.command.cancel({ id })` is logical cancellation.

### Keymaps and input families

`neoism.keymap.set(mode, key, commandOrFunction, options?)` accepts options `{ when?, priority?, fallback? }`; `fallback` defaults true. Modes include UI modes `global`, `terminal`, `editor`, `markdown`, `agent`, `file-tree`, `notes`, `git`, `palette`, `finder` and editor modes `normal`, `insert`, `visual`.

Known predicates include `editorFocus`, `terminalFocus`, `agentFocus`, `normalMode`, unary `!`, and `&&`; additional context predicates are implementation-defined. Multi-key prefixes have a bounded 750 ms timeout.

`neoism.motion.register`, `neoism.operator.register`, `neoism.text_object.register`, and `neoism.input.register` have the exact signature `(id, callback, options)`, where options are `{ keys: string[], mode?: string, when?: string, priority?: integer, fallback?: boolean }`. At least one non-empty key sequence is required; defaults are `mode="normal"`, `priority=0`, `fallback=true`.

### Events and augroups

`neoism.autocmd(event, callback, options?)` supports `event="*"` or one exact registered event, and options `{ pattern?, once?, scope?, group?, nested?, priority? }`. Event callbacks run by descending priority and then insertion order; `once` removes the callback after its first run.

Registered names are `Startup`, `Command`, `CommandResult`, `AsyncResult`, `ConfigReloaded`, `LspResult`, `BufferChanged`, `WorkspaceChanged`, `TabChanged`, `PanelChanged`, `FileTreeChanged`, `NotesChanged`, `AgentChanged`, `TerminalChanged`, `GitChanged`, `ThemeChanged`, `PluginsChanged`, `ConfigChanged`, `DocumentOpened`, `DocumentClosed`, `DocumentFocused`, `DocumentChanged`, `SelectionChanged`, `DiagnosticsChanged`, `PaneFocused`, and `PaneChanged`.

Every event is `{ name, payload, scope, origin? }`; all currently registered host events require local scope. `pattern` supports host wildcard matching against payload paths. Nested dispatch runs only autocmds declared `nested=true`, and an active callback is not re-entered.

`neoism.augroup(name, { clear?: boolean, delete?: boolean })` returns `name`. The name cannot be blank; `clear` removes declarations in that group, and `delete` also removes the group so later registration against it fails.

### Options

`neoism.option.set({ name, value, scope, target?, priority? })` has a `camelCase` request and rejects plugin scope. Names are snake_case: `wrap` boolean, `tab_width` integer 1 through 16, `use_tabs` boolean, and `input_mode` equal to `standard` or `vim`.

Option contributions overlay Rust defaults by scope specificity, explicit priority and insertion order. Removing or retiring the exact owner restores the native baseline.

## Documents, handles, selections, ranges and state

### Handles and snapshots

Opaque handle types are strings for document, pane, tab, workspace, selection, cursor and range. Never parse, synthesize or transfer them across closed targets or plugin revisions.

`neoism.document.current()` returns:

```lua
{
  handle = "opaque-document",
  pane = "opaque-pane",
  tab = "opaque-tab",
  workspace = "opaque-workspace",
  revision = 42,
  text = "...",
  cursor = {
    handle = "opaque-cursor",
    position = { line = 0, character = 0 },
  },
  selections = {
    {
      handle = "opaque-selection",
      anchor = { line = 0, character = 0 },
      active = { line = 0, character = 4 },
    },
  },
  metadata = {
    title = "file.rs",
    hostPath = "opaque host-native display path",
    language = "rust",
    kind = "code",
    remote = false,
    dirty = false,
    revision = 42,
    lineCount = 10,
  },
}
```

Read methods are `text({document})`, `lines({document,startLine?,endLine?})`, `range({document,range})`, `cursor({document})`, `selections({document})`, `metadata({document})`, `language({document})`, `dirty({document})`, and `revision({document})`.

Mutation methods are `edit`, `set_selections`, `move_cursor`, `focus`, `save` and `close`; `open` and `reload` exist in the typed registry but are not listed in the generated core annotations. Every exact-target mutation must carry `document` and `expectedRevision`; `move_cursor` also accepts `position` and optional `extendSelection`.

An edit is `{ document, expectedRevision, edits = { { range = { handle, start, ["end"] }, text } } }`. Ranges must be ordered, in bounds, on UTF-8 boundaries and non-overlapping; malformed or stale plans fail rather than partially applying.

### State

`neoism.state.get`, `list`, `set`, `delete` and `clear` use `{ scope, target?, persistent?, key?, value? }`, with snake_case scope values `plugin`, `document`, `pane`, `tab`, and `workspace`. Non-plugin scopes require an appropriate currently published target handle.

Persistent state is permitted only for plugin and workspace scope. Persistent values are keyed by plugin ID, and additionally workspace handle for workspace scope, survive a validated revision replacement and commit only after candidate activation; ephemeral state is exact-revision-owned and target state is removed when its target closes or changes.

Keys are non-empty and at most 256 bytes. Values, entries and aggregate state are bounded by the host; limits beyond those explicit in source are implementation-defined.

### Vim state

`register.get/list/set`, `mark.get/list/set/delete`, `jumplist.list/jump`, and `macro.get/list/set/play` require the exact current document handle for document-bound mutation. Register/clipboard text is limited to 1 MiB, marks accept lowercase ASCII names for setting, macro bodies are at most 4096 characters, and macro play count is capped at 100.

`neoism.clipboard.set({text})` writes through the application-owned OS clipboard. `neoism.clipboard.read({})` is asynchronous and emits `AsyncResult`; the generated annotation permits no argument, while the generic runtime also accepts an empty table.

`changelist` is installed as a generic namespace, but no changelist operation is registered in API version `1`; it is not a usable surface.

## Anchors, decorations, diagnostics, folds, conceal, lenses and widgets

Create a namespace with `neoism.namespace.create({ name? })`, then anchors with `neoism.anchor.create({ namespace, document, expectedRevision, position, bias })`, where bias is snake_case `before` or `after`. Returned `{id}` values are opaque resource handles and can be passed to later queued calls in the same callback because host actions preserve order.

`neoism.decoration.create(request)` and `neoism.diagnostic.publish(request)` use this `camelCase`, unknown-field-denying DTO:

```lua
{
  namespace = namespace_id,
  resource = nil,
  start = start_anchor_id,
  ["end"] = end_anchor_id,
  layer = "diagnostic",
  class = "word-tools.warning",
  text = "Repeated word",
  severity = "warning",
  style = {
    foreground = "#e0af68",
    background = nil,
    underline = "#e0af68",
    icon = "warning",
  },
  relatedInformation = {
    {
      message = "First occurrence",
      document = document_handle,
      position = { line = 1, character = 4 },
    },
  },
  tags = { "unnecessary" },
  actions = {
    {
      title = "Remove duplicate",
      command = "word-tools.remove-duplicate",
      arguments = {},
    },
  },
}
```

Layers are snake_case `highlight`, `gutter_sign`, `virtual_text`, `virtual_line`, `inline_widget`, `code_lens`, `fold`, `conceal`, and `diagnostic`; severities are `error`, `warning`, `information`, and `hint`. `decoration.set`, `diagnostic.update`, and `diagnostic.execute_action` exist in the typed registry but are generic-call surfaces rather than generated core signatures.

Delete one anchor/decoration with `{ namespace, resource }`, clear a namespace with `{ namespace }`, and delete a namespace with the same target. Every resource is exact-owner scoped; retirement removes only that owner revision's resources.

CRDT documents use Yrs sticky indices for anchor relocation; unbound documents use deterministic host relocation. Virtual lines reserve synthetic rows in the shared wrap index, folds use the same hidden-line projection as scrolling and pointer mapping, conceal keeps source-cell hit testing while hiding glyphs, and widget/lens/diagnostic actions publish retained Rust hit regions.

The renderer consumes immutable resolved decorations with byte offsets, positions and RGBA colors. It cannot invoke Lua.

## Asynchronous services, timers, jobs, watchers, network, credentials, prompts and results

### Common result envelope

`AsyncResult.payload` is `{ id, kind, ok, cancelled, terminal, result, error }`. Progress/chunks have `terminal=false`; success, failure and cancellation produce exactly one terminal event, with `error={code,message}` on failure.

The coordinator limits pending requests to 64 per owner and 512 process-wide, queues at most 8192 results, and drains at most 256 deliveries per turn. IDs are exact owner/window scoped; retirement suppresses queued and future deliveries from the old revision.

```lua
neoism.autocmd("AsyncResult", function(event)
  local reply = event.payload
  if reply.id ~= pending then return end
  if not reply.terminal then
    -- Handle output/change/message chunks in reply.result.
    return
  end
  pending = nil
  if reply.cancelled then return end
  if not reply.ok then
    error(reply.error.code .. ": " .. reply.error.message)
  end
end)
```

### Timers

`scheduler.after({ delayMillis, command, arguments? })`, `scheduler.every({ delayMillis, intervalMillis?, command, arguments? })`, and `scheduler.cancel({ timer })` use exact-owner timer handles. Timers invoke registered typed commands on the application executor, coalesce catch-up bursts, enforce a host minimum repeat cadence and are cancelled at retirement.

### Jobs and tasks

`job.spawn` request is `{ program, arguments?, cwd?, env?, timeoutMillis?, maxOutputBytes? }`. `program` is a bounded executable name, never a path; `cwd` is workspace-relative and containment checked; arguments are limited to 256 entries and 64 KiB each.

The default timeout is 30 seconds and maximum is 24 hours. Output defaults to 1 MiB and is capped at 4 MiB; stdin chunks are capped at 256 KiB. Environment has at most 64 bounded alphanumeric/underscore keys and rejects `PATH`, `HOME`, `LD_PRELOAD`, `LD_LIBRARY_PATH`, and `DYLD_INSERT_LIBRARIES`.

Progress result is `{event="output",stream="stdout"|"stderr",data}`, and terminal success is `{event="exit",code?,success,outputBytes,truncated}`. `job.stdin({job,data})`, `job.close_stdin({job})` and `job.cancel({job})` require the exact job handle; cancellation and timeout terminate the process tree.

Task and test calls use the explicit generic interface: `task.call("run", args, "local")`, `task.call("cancel", args, "local")`, and `test.call("run"|"watch"|"rerun"|"cancel", args, "local")`. Current desktop `run/watch/rerun` expects either the job request directly or `{process=<job request>}` and uses the same managed-job output envelope.

There is no separate API-version-1 `coverage` namespace or coverage host operation. A test provider may advertise `features={"watch","coverage"}` and define coverage data in its provider `schema`, but interpretation/presentation is implementation-defined and must not be treated as a built-in method.

### Watchers

Use `watcher.watch({path,recursive?})` and `watcher.cancel({id})`. Paths are bounded, workspace-relative, canonicalized and contained; limits are 32 watchers per owner and 256 globally.

Change progress is `{event="change",kind=<debug-form event kind>,resources=<opaque handle[]>}` and watcher errors are progress `{event="error",message}`. Host paths are never returned.

### Network and credentials

`network.request` is `{ url, method?, headers?, body?, timeoutMillis?, maxResponseBytes?, credential? }`. Only HTTPS without embedded credentials is allowed; methods are `GET`, `POST`, `PUT`, `PATCH`, `DELETE`, and `HEAD`; redirects are disabled.

Request body is at most 1 MiB, headers at most 64, and `authorization`, `proxy-authorization`, `cookie`, and `host` headers are forbidden. Timeout defaults to 30 seconds and clamps to 100 ms through 120 seconds; response defaults to 1 MiB and clamps to 1 byte through 4 MiB.

Success is `{status,headers,body}` after filtering authentication/cookie response headers. To use a secret, pass `credential=<alias>` and let the broker inject it; never attempt to pass an Authorization header.

Use `credential.call("status", {alias=<alias>}, "local")` for the asynchronous broker status result `{alias,available}`. Do not use the compatibility `credential.status()` query when you need broker status.

### Notifications, progress and prompts

`notification.show({title?,message,level?})` supports `info`, `warning` or `error`; title is at most 256 bytes and message at most 16 KiB.

`progress.create({message?})`, `progress.update({progress,message?,percentage?})`, and `progress.finish({progress,message?})` use exact-owner handles; current desktop presentation consumes messages and may not render `percentage`, so percentage display is implementation-defined.

`prompt.input/confirm/select` use `{title,message?,default?,options?}` and return asynchronously; `prompt.cancel({id})` cancels. Title is at most 256 bytes, message 16 KiB, options 32, each option 1024 bytes, and a window with an existing modal rejects a new prompt.

### Result lists

Result entries are `{ document, position, label, detail?, severity? }`. Lists support `create`, `replace`, `append`, `clear`, `delete`, asynchronous `query`, and `open`; `open` uses a zero-based `index` and navigates only if the document handle and UTF-8 position are still valid.

Kinds conventionally are `quickfix`, `location`, or `result`. A request has at most 2000 entries, titles 256 bytes, kind 64 bytes, labels 4096 bytes and details 16 KiB; an owner has at most 128 lists and 10,000 aggregate entries.

## Structured LSP

Native-UI compatibility methods such as `lsp.definition`, `hover`, `references`, `format`, `code_actions`, `signature_help`, `document_symbols`, `workspace_symbols`, `diagnostics`, and `clients` enqueue native actions. Data consumers should use `neoism.lsp.request(operation,args?)` and listen for `LspResult`.

Operations are snake_case `hover`, `signature_help`, `definition`, `references`, `document_symbols`, `workspace_symbols`, `diagnostics`, `clients`, `code_actions`, `rename`, `format`, and `apply_code_action`. Reads require `lsp.read`; edits and code actions require `lsp.edit`; cancellation requires `lsp.cancel`.

Optional target arguments are `path`, `line`, and `character`; omitted values come from the focused code pane. `workspace_symbols` adds `query`, `rename` requires `newName`, and `apply_code_action` requires `requestId` and `actionId`.

`LspResult.payload` is `{ id, operation, target?, ok, cancelled, result?, error? }`. Target is `{root,path,line,character,bufferRevision?,paneId}`; successful `result` is a Rust adjacently tagged enum serialized as `{kind=<snake_case>,items=<payload>}`.

| Result `kind` | Item/payload fields |
|---|---|
| `hover` | `{path,contents,kind?,range?,language?}` |
| `signature_help` | `{path,signatures,activeSignature?,activeParameter?,language?}`; signatures have `label`, `documentation?`, `parameters`, `activeParameter?` |
| `definition`, `references` | `{path,range?,language?}` |
| `document_symbols` | recursive `{name,kind,detail?,path,range?,selectionRange?,children,language?}` |
| `workspace_symbols` | `{name,kind,path,line?,language?}` |
| `diagnostics` | `{path,range?,severity,code?,codeDescription?,source?,message,tags,relatedInformation,data?,language?}` |
| `clients` | `{id,name,status,language,command,workspaceRoot,capabilities}` |
| `code_actions` | `{id,requestId,title,kind?,preferred}` |
| `apply_code_action`, `rename`, `format` | `{title,changedFiles,ranCommand}`; changed file is `{path,editCount,appliedBy}` and `appliedBy` is `frontend`, `desktop`, or `daemon` |

Code-action IDs are Rust-owned, owner/source-request-bound, one-shot capabilities with a five-minute lifetime. Applying consumes the capability even if later validation fails; raw server IDs, commands, edits, roots, document hashes and plans never cross into Lua.

Edit preparation does not mutate. Commit revalidates owner and target, open revisions, closed-file digests, ownership transitions, containment, symlinks, URI/resource support, UTF-8 boundaries, reversed ranges, duplicate starts and overlaps; failure is atomic.

Formatting changes the open buffer and CRDT state but does not save, mark saved, queue a save or emit `DidSave`. Cancel with `neoism.lsp.cancel({id})`; limits are 64 pending LSP requests per owner and 512 process-wide.

## Completion and snippets

Register completion with `neoism.extension.register({kind="completion_source",declaration=...})`:

```lua
neoism.command.register("word-tools.complete", function(event)
  return {
    {
      id = "sort",
      label = "sort-lines",
      kind = "function",
      detail = "Sort selected lines",
      documentation = "Sorts lines without leaving the editor.",
      filterText = "sort-lines",
      sortText = "001",
      insertText = "sort-lines",
      snippet = nil,
      score = 1.0,
      deprecated = false,
    },
  }
end)

neoism.extension.register({
  kind = "completion_source",
  declaration = {
    id = "word-tools.words",
    languages = { "markdown" },
    triggers = { ":" },
    requestCommand = "word-tools.complete",
    resolveCommand = nil,
    priority = 10,
  },
})
```

Trigger strings may be at most eight characters. Call `completion.call("request", {source,document,revision,position,trigger}, "local")`; the host invokes the source's registered request command, validates an array of `CompletionCandidate`, presents it only against the exact revision/position, and completes with `{count,presented=true}`.

Resolve uses `completion.call("resolve", {command,candidate}, "local")`; cancellation uses `completion.call("cancel", {id}, "local")`. Candidate fields are `camelCase` `{id,label,kind,detail,documentation,filterText,sortText,insertText,snippet?,score,deprecated}`.

Register snippets with `{kind="snippet",declaration={id,languages?,prefixes,body,description?}}`. At least one prefix is required; body is at most 1 MiB and supports `$1`, `${1}`, and `${1|one,two|}` tab stops. Unsupported, unclosed or numberless placeholders fail validation.

`snippet.call("apply", args, "shared-buffer")` is registered as a shared-buffer mutation, but its frontend payload adapter is currently implementation-defined; do not invent a request DTO beyond the registered `TransactionalTextEdit` `{document,expectedRevision,range,text}` contract without checking the active host.

## Language-server registration

Register with `{kind="language_server",declaration={id,languages,command,initializationOptions?,settings?,customRequests?}}`. Command must be non-empty and bounded, languages must be non-empty, and custom request names cannot be empty or begin with `$/`.

The contract registers `lsp.register_server` and `lsp.unregister_server` as local workspace-targeted actions requiring `lsp.register`. Their exact application payload and dynamic lifetime are implementation-defined in API version `1`; prefer the retained `language_server` declaration and do not invent method-specific tables.

Structured LSP uses the Rust-owned engine. A plugin may register an adapter or request typed operations, but it never owns the editor's document synchronization or runs on an LSP worker.

## Tree-sitter and syntax

Register a parser/query bundle with `{kind="tree_sitter",declaration=TreeSitterRegistration}`:

```lua
neoism.extension.register({
  kind = "tree_sitter",
  declaration = {
    id = "word-tools.markdown-parser",
    language = "markdown",
    parser = {
      resource = "artifacts/tree-sitter-markdown.so",
      sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
      abi = 14,
      platforms = { "linux" },
    },
    highlights = {
      { id = "highlights", query = "(atx_heading) @markup.heading" },
    },
    injections = {},
    folds = {},
    indents = {},
    textObjects = {},
    precedence = 0,
  },
})
```

Language is non-empty ASCII alphanumeric/underscore; each query ID is a valid contribution ID and query text is at most 4 MiB. Parser ABI must be 13 through 15 and the artifact needs exact external approval before supervised validation/loading.

Use `syntax.call("query", {document,expectedRevision,language,query}, "local")` or the equivalent `tree_sitter.call`. Success is `{document,revision,captures}`, where captures serialize `PluginSyntaxCapture` as `camelCase` `{name,start,end,text}`.

## Virtual documents

Register `{kind="virtual_document",declaration={id,title,language,revision,text,editable?,saveCommand?}}`. Text is at most 16 MiB; an editable virtual document requires a non-empty `saveCommand`.

`virtual_document.open`, `update`, and `close` are registered local host-resource actions. Their frontend action payload and presentation adapter are currently implementation-defined; use the retained declaration and probe host support rather than inventing fields.

## Tasks, tests, coverage and watch

Register `{kind="task_provider"|"test_provider",declaration={id,command,features?,schema?}}`. `command` is the registered plugin command callback ID; `features` may advertise provider-defined values such as `watch` or `coverage`, and `schema` describes provider-defined inputs/results.

Runtime operations are `task.run`, `task.cancel`, `test.run`, `test.watch`, `test.rerun`, and `test.cancel`. The current desktop adapter brokers runs as contained managed jobs and therefore accepts the job request shape described above; richer task/test/coverage DTO semantics remain implementation-defined.

Watch mode through `test.watch` is a long-running managed job, not a filesystem watcher. Cancel the returned request ID through the matching test operation or `async.cancel`.

## DAP

Register `{kind="debug_adapter",declaration={id,command,languages?,configurationSchema?}}`. The command vector must be non-empty and bounded.

Start with `debug.call("start", {adapter,cwd?,initialize?}, "local")`. Although the Rust request DTO contains `command`, the desktop host replaces it with the approved retained adapter registration; Lua cannot override the executable through a start request.

Progress emits `{event="message",message=<DAP JSON>}` and `{event="stderr",data}`, then terminal `{event="exit",code?}`. DAP frames are capped at 4 MiB, sessions at four per owner and sixteen globally.

Control operations are `continue`, `pause`, `step_in`, `step_out`, `next`, `evaluate`, and `breakpoints`, all using `{session,message?}`; the host forwards `message` as a framed DAP JSON object. Stop uses the same `{session}` shape and cancels the supervised adapter.

## PTY and terminal

`terminal` is the compatibility/native-pane namespace: `terminal.send(args)` and `terminal.run(args)` target the current pane and are local queued mutations.

The independent brokered PTY API starts with `pty.call("create", {program?,arguments?,cwd?,cols?,rows?}, "local")`; defaults are 80 columns and 24 rows. Dimensions must be 1 through 1000, arguments are bounded, cwd is workspace-relative and contained, and limits are eight PTYs per owner and 32 globally.

Progress is `{event="output",data}`, terminal success is `{event="exit",code?}`, and aggregate output is capped at 8 MiB. Use `pty.call("send", {pty,data}, "local")`, `resize` with `{pty,cols,rows}`, `status` with `{pty}`, and `close` with `{pty}`; writes are capped at 256 KiB and status succeeds as `{pty,running,exitCode?}`.

Register a retained terminal provider with `{kind="terminal_provider",declaration={id,command,features?,schema?}}`; provider-specific schema and invocation are implementation-defined.

## Git

The asynchronous typed operations are `git.call("status"|"diff"|"blame"|"branches"|"history"|"worktrees", args, "local")` for reads and `git.call("stage_hunk"|"unstage_hunk"|"checkout"|"branch"|"worktree", args, "local")` for mutations.

Current desktop support maps to fixed shell-free `git` argv: `diff` optionally accepts workspace-relative `path`, `blame` requires `path`, `history` accepts `limit` capped at 1000, `checkout` requires safe `branch`, and `branch` requires safe `name`. `stage_hunk`, `unstage_hunk`, and worktree mutation explicitly fail because they require native retained-diff or destination-picker capabilities.

Results use managed-job output chunks rather than parsed Git DTOs. Compatibility `git.stage`, `unstage`, `commit`, `refresh`, `open`, and `toggle` are separate native UI/actions; do not confuse them with the typed async broker.

Register a retained Git provider with `{kind="git_provider",declaration={id,command,features?,schema?}}`; provider-specific semantics are implementation-defined.

## Agent bridge

Typed reads are `agent.call("sessions"|"messages"|"checkpoints"|"status", args, "local")`. The current desktop adapter returns the published status snapshot, session list, or up to 1000 recent messages from only the active native Agent session; checkpoints currently return an explicit unsupported failure.

Message results contain implementation-owned fields currently including `id`, `kind`, `title`, `text`, `status`, `tool`, `outputKind`, `language`, `lineOffset`, `detail`, and `author`. Treat unknown/missing fields as forward-compatible.

Mutations are `agent.call("approve"|"checkpoint"|"subagent"|"workflow", args, "local")`, but the current desktop adapter never lets Lua choose a permission answer. It may route a matching `requestId` to the native approval UI and return `{state="permission_required",routedTo="native_agent_approval"}`; without a matching pending card it fails `workflow_required`.

Compatibility `agent.open/create/send` are native-pane actions and not the typed bridge. Register a retained bridge with `{kind="agent_bridge",declaration={id,command,features?,schema?}}`; provider-specific semantics are implementation-defined.

## Native and subprocess extension hosting

Registered host operations are `extension_host.start`, `stop`, and `load_native`, requiring exact external approval and the appropriate execute/cancel/native capability. Their direct Lua payloads are intentionally implementation-defined; package authors declare tiered `entrypoints` and use the Extensions UI approval lifecycle instead of calling undocumented loaders.

Subprocess/native generations are supervised, versioned, checksum/platform/symbol/ABI checked, crash-contained and retired asynchronously. Host paths, file descriptors and process IDs are not capabilities and must not be exposed to sandboxed Lua.

## Capability and scope quick reference

| Family | Typical capability | Registry scope |
|---|---|---|
| document reads | `document.read` | local |
| document edit/save/reload | `document.write` | shared-buffer |
| cursor/selection/focus/close | `document.write` | local |
| anchors/decorations/diagnostics/state | `<namespace>.write` | local |
| commands/timers/jobs | `command.execute`, `scheduler.execute`, `job.execute` | local |
| watcher/network | `watcher.read`, `network.execute` | local |
| credentials | `credential.read` plus alias grant `network.authorize` | local |
| structured LSP | `lsp.read`, `lsp.edit`, `lsp.cancel` | local |
| completion/syntax | `completion.read`, `syntax.read` or `tree_sitter.read` | local |
| task/test/debug/PTY | `<namespace>.execute`, `.write` or `.cancel` by operation | local |
| Git | `git.read` or `git.write` | local |
| Agent | `agent.read` or `agent.write` | local |
| file/note mutation | `file_tree.write` or `notes.write` | workspace |
| workspace split/tab topology | `workspace.write` or `tab.write` | workspace |

This table is explanatory, not a substitute for `neoism.contract`: access strings with dots are preserved, so Vim register reads are `register.editor.read`, marks are `mark.editor.read`, jumplists are `jumplist.editor.read`, and macros are `macro.editor.read`.

## Authoring rules for coding agents

1. Read [`docs/lua-api.lua`](lua-api.lua) and the relevant linked DTO before emitting a call.
2. Preserve serde field casing exactly; do not translate `expectedRevision` to `expected_revision` in Lua.
3. Use only registered namespace/operation pairs from [`contract.rs`](../neoism-lua/src/contract.rs).
4. Use `.call` for broad typed operations when a same-named compatibility query can shadow intent.
5. Carry exact document revisions and opaque handles through every asynchronous turn, and abandon work when either becomes stale.
6. Handle owner-scoped terminal errors and cancellation; never assume a queued `{id}` means the mutation succeeded.
7. Keep callbacks bounded and publish retained data; never implement rendering, hit testing, scrolling or shared text ownership in Lua.
8. Mark host-adapter details as implementation-defined when no public request DTO exists; never invent a method or field.