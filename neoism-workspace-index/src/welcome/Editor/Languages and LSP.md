# Languages & LSP

Neoism has a **Rust-owned LSP engine** that talks to language servers directly. It drives hover, diagnostics, go-to-definition, references, rename, and symbols, and the same engine backs the [[../The Neoism Agent|agent]]'s `lsp` tool.

## Two layers

- **Syntax (Tree-sitter)** — built into the editor, nothing to install. Whole-buffer parsing keeps multi-line strings and comments colored correctly.
- **Semantics (LSP)** — a one-click install per language server.

## Supported servers

Out of the box, Neoism maps file types to these servers:

```text
Rust          rust-analyzer            Go            gopls
TypeScript/JS typescript-language-server   C / C++   clangd
Python        pyright                  Java          jdtls
Ruby          solargraph               C#            omnisharp
Lua           lua-language-server      Bash          bash-language-server
JSON          vscode-json-language-server   YAML     yaml-language-server
TOML          taplo                    HTML/CSS      vscode-html / css servers
PHP           intelephense             Zig           zls
Elixir        elixir-ls                Haskell       haskell-language-server
Scala         metals                   Kotlin        kotlin-language-server
Svelte        svelteserver             Vue           vue-language-server
```

## How you get a server

Open a file in a language you haven't set up yet. Neoism looks for the server binary in its **managed bin dir**, then a config-specified path, then your `$PATH`. If it's missing, you get a prompt:

- If the server is in the package registry, the prompt shows an **Install {package}** button — click it and Neoism downloads the binary into its own managed directory (no plugins, no `$PATH` pollution). If a download fails, a **Retry** prompt appears.
- Or **bring your own** — put the server on your `$PATH` and Neoism uses it.

Live LSP status shows in the editor status-bar pill (and a popup for multi-server buffers, e.g. `ruff + pyright`).

## Advanced

Power users can point Neoism at a custom server command/path via the agent-side `lsp` config (an `lsp` field that is either `enabled` or a map of custom server definitions). Most people never need this — the prompt-driven install covers the common languages.
