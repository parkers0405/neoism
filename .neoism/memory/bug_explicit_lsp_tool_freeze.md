---
name: "Explicit LSP tool freeze"
description: "Explicit LSP tool calls blocked async runtime; fixed with spawn_blocking plus bounded hover output"
type: "bug"
scope: "project"
origin: "project debugging session"
created: "2026-07-11T12:31:58-05:00"
updated: "2026-07-11T12:31:58-05:00"
---

## Symptom
An explicit agent `lsp` tool call could make the entire agent session appear frozen. A tool card might eventually show `completed`, while streaming/UI responsiveness stalled; accidental hover targeting could also return a huge Rust standard-library document.

## Root cause
`lsp_tool` was declared async but executed every synchronous `LspService` query directly on a Tokio worker. The LSP client has request-level timeouts, but while waiting it still blocked an async runtime thread. Hover content was serialized without an operation-specific bound; central generic truncation happens later and permits a much larger payload.

## Fix
- `neoism-agent-server/src/lsp.rs`: `lsp_tool` now hands all explicit LSP operations to `tokio::task::spawn_blocking`, with the existing body in `lsp_tool_blocking`.
- `lsp_parse.rs`: hover documentation is capped at 8,192 Unicode scalar values before serialization, with `[hover documentation truncated]` appended.
- `lsp_tests.rs`: regression test covers oversized Markdown hover output.

## Verification
- `cargo test -p neoism-agent-server --lib parse_hover_caps_oversized_documentation` passes.
- `cargo check -p neoism-agent-server --lib` passes.
- Workspace `cargo fmt` is currently blocked by intentionally incomplete `examples/completion_probe.rs` (`String::`); touched files were formatted directly with `rustfmt`.

## Invariant
Any agent tool that invokes synchronous/blocking LSP service methods must run them on the blocking pool. Bound high-volume semantic payloads before JSON/transcript/UI serialization, not only in central provider truncation.
