# Plugin Authoring

Neoism exposes parallel plugin tiers rather than a mandatory migration ladder. Choose the lowest-authority tier that can produce the required outcome.

| Target | Tier | Runtime |
|---|---|---|
| Editor | Sandboxed Lua | One isolated embedded Lua VM per package revision on the application-owned executor. |
| Agent | Sandboxed Lua | One isolated VM in the supervised `neoism-agent-lua-runner` process. |
| Agent | `neoism-plugin/2` | Any language in a supervised NDJSON subprocess. |
| Editor | Trusted subprocess/native | Explicitly approved exact artifact revision and ABI. |
| Agent | Native Rust | First-party or fully trusted linked code. |

An editor-only package, Agent-only package, dual-target package, existing `init.lua`, existing process plugin and native integration can coexist. No tier must replace another.

The exhaustive source references are `docs/plugins.md`, `docs/editor-plugin-api.md`, `docs/agent-plugins.md`, and generated `docs/lua-api.lua`. Coding agents should read those files before inventing a manifest field, method, event, contribution or capability name.

## Package shape

```text
~/.config/neoism/plugins/dev.example.shared/
├── neoism-plugin.json
├── editor.lua
├── agent.lua
└── lua/
    └── shared/
        └── utility.lua
```

```json
{
  "id": "dev.example.shared",
  "name": "Shared example",
  "version": "1.0.0",
  "apiVersion": 1,
  "editor": { "entrypoint": "editor.lua" },
  "agent": {
    "runtime": "lua",
    "entrypoint": "agent.lua",
    "command": [],
    "capabilities": ["workspace-read"],
    "scope": "workspace",
    "eventNamespaces": []
  },
  "platforms": ["linux", "macos", "windows"],
  "dependencies": [],
  "triggers": [],
  "capabilities": ["document.read"],
  "metadata": {}
}
```

Use `editor.entrypoint` for new packages. Existing top-level `entrypoint` manifests continue to work. `editor.entrypoint` wins when both are present. Workspace packages are metadata-only until externally trusted; repository content cannot approve itself.

Editor and Agent capability strings are different contracts. Editor capabilities use dotted operation names such as `document.read`, `document.write`, `job.execute`, `network.execute`, `lsp.read`, `lsp.edit`, `git.read`, `git.write`, `agent.read`, `agent.write`, `debug.execute`, `pty.execute`, and registration capabilities. Agent capabilities use the kebab-case vocabulary listed in [[Plugins]]. Declaration requests authority; host policy and exact-revision trust grant it.

## Editor Lua entrypoint

```lua
local neoism = require("neoism")

neoism.command.register("shared.show-document", function()
  local document = neoism.document.current()
  neoism.notification.show({
    title = "Current document",
    message = document and document.metadata.title or "No document",
  })
end, { title = "Shared: Show current document" })

neoism.keymap.set("editor", "<C-S-d>", "shared.show-document")
```

Lua callbacks never run on render, layout, scrolling, animation, hit-test, input, timer, process-reader, watcher, network, language-server, daemon or GPU threads. They run on the application-owned executor and publish immutable retained data to Rust. Rust owns rendering, authoritative workspace state, CRDT edits, validation, PTYs, LSP transport and privileged mutations.

## Editor API map

`require("neoism")` exposes these authoring families. Exact function signatures and return types are generated in `docs/lua-api.lua`; semantics, DTO fields and examples are in `docs/editor-plugin-api.md`.

| Family | Purpose |
|---|---|
| `setup`, `ui`, `panel` | Transactional config overlays, styles, surface/item layout, declarative chrome and retained panels. |
| `extension`, `extension_host` | Register retained UI and platform contributions; inspect supported tiers and host operations. |
| `command`, `keymap`, `autocmd`, `augroup` | Commands, aliases, completion, mappings, lifecycle events, priority, once and nested dispatch. |
| `option`, `motion`, `operator`, `text_object`, `input` | Scoped editor options and native input/Vim extension points. |
| `buffer`, `document`, `workspace`, `tab`, `panel`, `file_tree`, `notes` | Typed snapshots, opaque handles, reads and validated mutations. |
| `namespace`, `anchor`, `decoration`, `diagnostic` | Exact-owner resources, sticky anchors, highlights, signs, virtual text/lines, widgets, lenses, folds, concealment and diagnostics. |
| `state`, `register`, `mark`, `jumplist`, `macro`, `clipboard` | Bounded plugin/target state and native editor state projections. |
| `scheduler`, `async`, `job`, `watcher`, `network`, `credential` | Timers and brokered asynchronous host services. |
| `notification`, `progress`, `prompt`, `result_list` | Native notifications, progress, input/confirm/select and quickfix/location/general result lists. |
| `lsp` | Structured hover, signature, definition, references, symbols, diagnostics, clients, code actions, rename and no-save format. |
| `completion`, `snippet` | Completion presentation/resolve/accept and snippet sessions. |
| `syntax`, `tree_sitter` | Query execution and parser/query/capture/injection/fold/indent/highlight/text-object declarations. |
| `task`, `test` | Tasks, tests, watch mode, coverage and parsed problems. |
| `debug` | DAP sessions, breakpoints, stacks, variables and controls. |
| `pty`, `terminal` | Exact-owner PTYs and terminal providers. |
| `git` | Status, diff, blame, history, stage, unstage, commit, branch and worktree operations supported by the host adapter. |
| `agent` | Agent status, sessions, bounded messages and approval/checkpoint/subagent/workflow actions. |
| `virtual_document` | Host-rendered read-only virtual documents. |

Compatibility objects also expose `snapshot`, `get`, `query` and `call`; typed host contracts validate capability, scope, target, cancellation and result shape before dispatch. Prefer explicit namespace methods where available and inspect `neoism.contract` rather than guessing operation names.

All text coordinates are zero-based UTF-8 byte columns. Document, range, selection, cursor, pane, tab and workspace handles are opaque revision-bound capabilities. Capture and return them unchanged. A stale handle or expected revision fails instead of retargeting the currently focused pane.

Shared text changes go through the normal `CodeBuffer` and CRDT transaction path. Local focus, hover, scroll, geometry and overlays remain ephemeral. Remote host paths remain opaque host identities and must not be interpreted with guest-local path semantics.

## Async and lifecycle rules

Jobs, watchers, network, credentials, prompts, result lists, PTYs, DAP sessions and other retained resources belong to an exact `{ pluginId, revision }`. Reload, disable, update, removal or scope retirement cancels or removes only that owner's resources. Late worker results cannot cross into a replacement revision.

`AsyncResult` is the common result event. Requests carry a nonempty ID and produce bounded output plus exactly one terminal success, failure or cancellation. Cancellation suppresses late completion. Timers coalesce missed ticks instead of bursting callbacks.

Network is HTTPS-only, does not follow redirects, rejects embedded credentials and direct authorization/cookie/host/proxy-authorization headers, and uses host-owned credential aliases. Jobs and paths are workspace-contained, bounded and process-tree terminated on timeout, cancellation or retirement.

## Retained UI and platform declarations

Retained UI includes picker, tree, table, list, form, modal, inspector, toolbar, breadcrumb, detail, virtual-document, Agent timeline and approval-card roles. Lua declares immutable scene/view records; Rust performs layout, rendering, accessibility, focus and hit testing.

Platform contribution `kind` tags are snake_case while declaration fields are camelCase. Contribution families include retained views, completion sources, snippet providers, language servers, Tree-sitter parsers and queries, tasks, tests, terminals, Git providers, Agent providers and debug adapters. Unknown fields are rejected by strict declaration DTOs where the contract uses `deny_unknown_fields`.

Trusted subprocess and native editor artifacts require host-owned exact approval bound to plugin ID, revision, package digest, artifact digest, ABI, capabilities and scope. A manifest cannot self-approve an executable or native module. Native extension ABI is versioned; Tree-sitter artifacts are checksum, platform, symbol and runtime-ABI validated before publication.

## Installation and compatibility

Local packages are discovered under `~/.config/neoism/plugins/<id>/`. Declarative local/Git specs live in `~/.config/neoism/lua/plugins/*.lua`. Managed Git revisions are immutable and recorded in host-owned `plugins.lock.json`. Acquisition, validation and publication are transactional; failures preserve the previous lock entry and active generation.

Existing personal `~/.config/neoism/init.lua`, editor-only package manifests, compatibility Lua calls, `neoism-plugin/1`, additive `neoism-plugin/2` frames and native plugins remain supported. Porting guidance is optional and exists only for authors choosing newer package identity, trust, contribution or host-service features.

Read [[Plugins]] for Agent Lua, process, TypeScript, native and remote-resource details. Read `docs/plugins.md` and `docs/editor-plugin-api.md` before shipping an editor package, and use `docs/lua-api.lua` as the machine-readable signature inventory.