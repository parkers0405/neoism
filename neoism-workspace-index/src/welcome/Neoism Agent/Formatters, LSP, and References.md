# Formatters, LSP, and References

Neoism Agent can query the same language intelligence used by the editor. It also preserves oversized tool output as artifacts so an agent can inspect exact details without replaying megabytes into every model request.

## LSP

The Agent server exposes these language-server operations:

- Server status.
- Hover information.
- Go to definition and implementation.
- Find references.
- Document and workspace symbols.
- Diagnostics.
- Document highlights.
- Folding ranges.
- Selection ranges.
- Formatting.
- Code actions.
- Call hierarchy preparation, incoming calls, and outgoing calls.
- Document touch/update and server shutdown.

The native `lsp` tool wraps these operations behind one typed interface. It accepts a file, zero-based line, and zero-based UTF-8 byte column where a position is required.

## Install language servers

Open Neoism's hamburger menu in the top chrome and choose **Extensions**. The **Language Servers** tab is backed by the same runtime adapter registry used by the editor and Agent tools. It shows built-in integrations, installed servers, and servers Neoism can install from its extension catalog.

Choose **Install** on a supported language server. Neoism downloads it into its managed extension directory, so it does not modify your system package manager or require the executable to be added globally to `$PATH`. The same page provides uninstall and retry actions.

When you open a file whose matched server is missing, Neoism can also present an install prompt. That prompt and the Extensions page use the same runtime registry and installer; they are two entrances to the same system.

The Extensions page also contains **MCP Servers**, built-in **Syntax Parsers**, and **Kernels**. Syntax parsers compiled into Neoism are marked built-in and require no download.

Neoism resolves an LSP executable from its managed extensions, an explicit configured command, or the host environment as supported by the adapter. Live server state appears in the editor status area; multi-server files can report more than one attached server.

## Advanced language-server configuration

Most users should install servers through **Extensions**. The `lsp` block is for disabling an adapter, overriding one, or defining a custom server that is not in the catalog.

```jsonc
{
  "agent": {
    "lsp": {
      "company-rust": {
        "name": "Company Rust Analyzer",
        "command": ["/opt/company/bin/rust-analyzer"],
        "language": "rust",
        "documentLanguageId": "rust",
        "extensions": ["rs"],
        "markers": ["Cargo.toml"],
        "env": { "RUST_LOG": "error" }
      }
    }
  }
}
```

Project `neoism.json` uses the direct `"lsp"` block.

Set an adapter ID to `false`, or use `"enabled": false`, to disable it. A configured definition can reference and override a built-in adapter with `adapter`; custom definitions must provide a valid route and stdio command or TCP endpoint.

## Diagnostics and code actions

LSP diagnostics are published into session/client events after relevant tools run. An agent can inspect current diagnostics and request code actions, but applying an edit still goes through normal edit tools and permissions.

## Formatting

Agent-side formatting currently uses the attached language server's formatting capability. Install the language server through **Extensions**, then use editor formatting or the Agent `lsp` tool. The Agent server does not currently expose a separate formatter inventory.

## Tool-output references

Neoism centrally truncates oversized tool output. When output exceeds the safe inline budget, it writes the complete result to a tool-output artifact and returns:

- A bounded preview.
- `truncated: true` metadata.
- An `outputPath`.
- Artifact metadata and an `artifact://` reference when available.

The model receives the compact reference rather than repeatedly ingesting the full output. It can use artifact-reading/search tools to inspect only the relevant region.

This applies to large shell, search, web, and other tool results. A tool's own pagination `truncated` flag is distinct from Neoism spilling the complete output to disk.

## Source references

Message file parts can carry path ranges, selected text, symbols, and diagnostics. These references let editor context flow into a prompt without flattening every item into anonymous text.

## Troubleshooting

- If LSP status is empty, open **hamburger menu → Extensions → Language Servers**, install the matching server, and reopen or touch the file.
- If installation fails, use the extension card's retry action and verify the required download toolchain/network is available.
- If hover/references fail on a newly edited document, touch/update the document or save it so the server sees current text.
- A zero-based byte column differs from a visual glyph column for non-ASCII text.
- If formatting returns no edits, the language server may not advertise formatting support.
- Read an artifact's `outputPath` or `artifact://` reference when a tool preview says it was truncated.

See [[Attachments]] and [[Troubleshooting]].