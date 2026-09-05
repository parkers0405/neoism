---
name: "Agent semantic token spacing and lower tab ghost drag"
description: "Semantic Agent colors no longer switch font weight; lower tabs cannot ghost-drag"
type: "bug"
scope: "project"
origin: "coding session"
created: "2026-08-30"
updated: "2026-09-04"
---

---
name: "Agent semantic token spacing and lower tab ghost drag"
description: "Semantic Agent colors no longer switch font weight; lower tabs cannot ghost-drag"
type: "bug"
scope: "project"
origin: "coding session"
created: "2026-08-30"
updated: "2026-09-04"
---

Agent GUI colored-first-word overlap root cause: plain-prose semantic highlighting coupled color to an implicit bold face through `PlainTokenStyle.bold`. This affected the shared desktop/web Agent Markdown renderer and was distinct from explicit Markdown emphasis. Exact stored examples contained normal spaces and no `**` markers. Multiple whitespace, run-width, ink-clamp, separator-binding, and absolute-prefix placement experiments did not change the live symptom.

Final fix: semantic `PlainTokenStyle` carries color only. Plain semantic tokens use the paragraph's base font options for measurement, caret geometry, and draw, while retaining the full semantic color classifier. Explicit `Bold`, `BoldItalic`, code, and bold Markdown-link variants still request bold normally. The failed absolute-prefix geometry rewrite and temporary Geist diagnostic test were removed. Regression `semantic_plain_token_keeps_color_separate_from_markdown_emphasis` covers colored `Fail`, its standalone separator before `Outages`, and the distinct `**Fail**` explicit-bold path. All 30 Agent Markdown tests pass; the `neoism-terminal-wasm` web target check passes.

Runtime gotcha: validate only after rebuilding/restarting the actual client. An installed `/home/parkersettle/.local/bin/neoism` process may coexist with `target/debug/neoism`, and Vite can serve a previously generated WASM artifact.

Lower Chrome/buffer tabs had the same lost-release latch as Island tabs: release handling was after many consumers and cursor motion ignored physical LMB state. `BufferTabs::cancel_drag`, exact source/preview cancellation, first-refusal lower-tab release after Island, cross-window source resolution, cancellation on fresh LMB press, physical LMB-up motion, and focus-loss cleanup prevent ghost drags. Existing tests/checks pass.
