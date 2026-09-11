# Local Tree-sitter assets

Grammar WASMs: npm tree-sitter-wasms@0.1.13 (Unlicense distribution; upstream grammar licenses retained here). Built with Tree-sitter 0.20; compatible with web-tree-sitter 0.25.10. No CDN requests. The distribution records version ranges, not exact grammar build commits.

Highlight queries from these upstream npm packages (MIT):
- tree-sitter-rust@0.20.4
- tree-sitter-javascript@0.20.3
- tree-sitter-typescript@0.20.5
- tree-sitter-python@0.21.0
- tree-sitter-bash@0.20.5
- tree-sitter-json@0.20.2
- tree-sitter-css@0.20.0
- tree-sitter-html@0.20.0

TS includes JavaScript queries; TSX additionally includes JSX queries. Rust appends Neoism RUST_NVIM_CONTEXT_QUERY from shared/src/syntax.rs. No injection queries are executed; HTML embedded scripts and nested fenced Markdown are not separately parsed. Markdown fences remain plain text.

SHA-256:
- bash.wasm: `807dcdb1380a59befb112ed8fbd3d3872c7fadaf5903a769282b50973b30696d`
- css.wasm: `5fc615467b1b98420ed7517e5bf9e1f88468132dd903d842dfb13714f6a1cb0c`
- html.wasm: `11b3405c1543fb012f5ed7f8ee73125076dce8b168301e1e787e4c717da6b456`
- javascript.wasm: `63812b9e275d26851264734868d27a1656bd44a2ef6eb3e85e6b03728c595ab5`
- json.wasm: `fdb5219abe058369e16897aaa11eecf47ef4f546752c3ddbac339cdd89e1e667`
- python.wasm: `9056d0fb0c337810d019fae350e8167786119da98f0f282aceae7ab89ee8253b`
- rust.wasm: `4409921a70d0aa5bec7d1d7ce809a557a8ee1cf6ace901e3ac6a76e62cfea903`
- tsx.wasm: `6aa3b2c70e76f5d48eafef1093e9c4de383e13f2fdde2f4e9b98a378f6a8f1b6`
- typescript.wasm: `8515404dceed38e1ed86aa34b09fcf3379fff1b4ff9dd3967bcd6d1eb5ac3d8f`

Queries use `.scm.txt` so the existing standalone GUI static server serves them as text/plain (its allowlist does not accept .scm).
