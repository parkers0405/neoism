# Language servers

Neoism has one language-server platform shared by editor diagnostics, hover,
completion, navigation, code actions, formatting, and agent tools. Neovim is
the text-editing substrate; it does not launch a second set of LSP clients.

## Adapters and packages are different things

The adapter registry is the runtime source of truth. An adapter declares:

- a stable server identity;
- one or more file routes and their exact LSP `languageId` values;
- root markers and supported operations;
- a stdio command or TCP endpoint; and
- optional environment and initialization settings.

The Extensions catalog is only a package source. Mason metadata may tell
Neoism how to download a binary, but it cannot define file routing or protocol
startup on its own. Host-installable packages remain available; a package
without a registered runtime adapter is labeled `adapter required`. Installing
that package never creates a fake attachment, and a custom adapter can make it
usable without changing Neoism's built-in list.

Built-in adapters cover Rust, TypeScript/JavaScript, Python, Go, C/C++, Java,
C#, Ruby, Lua, shell, JSON, Docker, YAML, TOML, HTML, CSS-family languages,
PHP, Zig, Elixir, Haskell, Scala, Kotlin, Svelte, Vue, Nix, and Godot. Shared
servers such as TypeScript/JavaScript have one process identity with separate
document routes.

Each initialized client records the server's negotiated position encoding and
text-document synchronization mode. Neoism uses zero-based UTF-8 byte columns
inside the editor, converts positions exactly once at the protocol boundary,
and honors `None`, `Full`, and `Incremental` sync plus the server's open, close,
and save flags. A changed adapter endpoint or startup configuration replaces
the old client instead of leaving a stale process attached.

## Custom adapters

Project adapters can be declared in `neoism.json` or `neoism.jsonc`. A custom
stdio language looks like this:

```jsonc
{
  "lsp": {
    "acme-language-server": {
      "name": "Acme",
      "routes": [
        {
          "id": "acme",
          "documentLanguageId": "acme",
          "extensions": ["acme"],
          "filenamePatterns": ["Acmefile"]
        }
      ],
      "rootMarkers": ["acme.toml"],
      "transport": {
        "kind": "stdio",
        "command": ["acme-language-server", "--stdio"],
        "env": { "ACME_LOG": "warn" }
      },
      "capabilities": {
        "hover": true,
        "diagnostics": true,
        "formatting": false
      },
      "initializationOptions": { "protocolHandshake": true },
      "settings": { "acme": { "lint": "strict" } }
    }
  }
}
```

TCP uses the same adapter model:

```jsonc
{
  "lsp": {
    "remote-acme": {
      "name": "Remote Acme",
      "language": "acme",
      "documentLanguageId": "acme",
      "extensions": ["acme"],
      "transport": {
        "kind": "tcp",
        "host": "127.0.0.1",
        "port": 7005
      }
    }
  }
}
```

`initializationOptions` is sent only in the initialize request. `settings` is
returned from `workspace/configuration` and sent with
`workspace/didChangeConfiguration`; the two protocol fields are never
silently conflated. Capability keys are `workspaceSymbols`, `completion`,
`hover`, `definition`, `references`, `implementation`, `callHierarchy`,
`diagnostics`, `documentSymbols`, `formatting`, `codeActions`, and `rename`.

Set an entry to `false`, or use `"enabled": false`, to disable that adapter.
Set the entire `"lsp"` value to `false` to disable built-ins for the workspace;
`true` enables the built-in registry. Unknown files remain unclaimed; Neoism
never opens them as `plaintext` behind the scenes.

## Godot

Godot owns its GDScript language server. Neoism's built-in Godot adapter
connects directly to `127.0.0.1:6005`, so the project must be open in the
Godot editor. Override the defaults with `GODOT_LSP_HOST` and
`GODOT_LSP_PORT`, or configure a TCP endpoint on the Godot adapter.

Only `.gd` is routed to the GDScript LSP. `project.godot`, `.tscn`, `.tres`,
and `.gd.uid` are project/resource files, not GDScript documents. Their icons
and Tree-sitter syntax support are independent of LSP attachment; `.gdshader`
and `.gdshaderinc` use the GLSL parser.

## Diagnostics consistency

Diagnostics are owned by `(workspace root, file, server, language)` and carry
document versions when the server provides them. A newer empty publication
clears errors; an older publication cannot resurrect them. Buffer switches
reject late snapshots for the prior file, and scrolling only culls rendering—
it never deletes the cached diagnostic.

Status and diagnostic events are scoped to the active file. Opening a
Dockerfile, Nix expression, or Godot resource cannot inherit a Rust server or
error count merely because another file in the workspace uses Rust. Hover
coordinates are resolved by Neovim from the actual rendered grid cell, which
keeps tabs, wide glyphs, emoji, horizontal scrolling, and UTF-8 byte columns in
agreement.

## Quick fixes and code actions

Invoke Quick Fixes at the cursor with `Ctrl+.` (`Cmd+.` on macOS), the editor
context menu, or a diagnostic detail card. Neoism requests
`textDocument/codeAction` from every matching adapter, including only that
server's diagnostics which overlap the cursor. The picker shows action kind,
the server's preferred marker, and disabled reasons; it never silently runs
the first result.

Choosing an action sends its opaque payload back to the originating server.
Neoism resolves lazy actions only when the server advertises resolution,
validates every WorkspaceEdit target and range before mutation, applies edits,
then sends any returned command through `workspace/executeCommand`. There are
no language-specific Quick Fix branches.

Use the fixtures under `fixtures/editor-diagnostics/` to exercise long Rust,
GDScript, Dockerfile, and Nix buffers with errors near the top, middle, and
bottom.
