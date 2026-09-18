---
name: "Optional TypeSafe computer MCP mode"
description: "Off-by-default browser_goal runs bounded Jev-selected typed DOM operations; browser_step retained; strict probability/risk/revocation gates and exact caller values."
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-09-17"
updated: "2026-09-18"
---

---
name: "Optional TypeSafe computer MCP mode"
description: "Off-by-default browser_goal runs bounded Jev-selected typed DOM operations; browser_step retained; strict probability/risk/revocation gates and exact caller values."
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-09-17"
updated: "2026-09-18"
---

TypeSafe/Jev integration lives inside built-in computer MCP and remains off by default (`agent.experimental.options.computer-typesafe.enabled`). Preferred tool is now `computer.browser_goal`; shipped `browser_step` remains compatibility single-step. Goal API accepts target/tab/goal, exact caller `text_values`, `select_values`, `navigate_urls`, click/scroll/back toggles, `max_steps` 1..8 and `timeout_ms` 1000..30000. Jev `jev-latest` chooses only server-generated typed candidates. Every action gets a fresh DOM observation; only compact executed trace is carried. Request/response 128 KiB; candidate cap 180 + none; no conversation history.

Goal loop rechecks cancellation, STOP generation, computer MCP and TypeSafe enablement before model/action boundaries. Existing choice confidence >=.85, selected probability >=.75, distribution validation and risk >=.1 `needs_confirmation` remain fail-closed. `possibly_done` is explicitly not application verification. `partial_unknown` immediately stops and never replays. In-flight timeout returns partial_unknown; action budget returns step_budget_exhausted. Normal permissions, focus, attachment, ref freshness and TypeSafe credentials remain required.

DOM adapter now supports guarded `scroll` up/down, `back`, and exact normalized caller HTTP(S) `navigate`, as well as click/fill/select. browser_act ref is required only for element actions. Observation exposes scroll/history capability, filters file and password inputs, and marks disabled select options. No arbitrary/generated JS or text, uploads, screenshots, background automation, or native desktop semantics.

GUI and native settings wording, bundled TypeSafe Browser Mode and Computer Use docs updated to favor browser_goal and disclose page data + compact trace. Tests cover typed candidates, exact inputs, invalid bounds/schemes, risk/done gates, a two-decision goal loop, adapter schema, GUI wording, and bundled docs. Verified cargo check neoism-agent-server and neoism-backend; 8 TypeSafe Rust tests, browser schema test, 3 GUI tests, GUI tsc, product docs test, and git diff --check. Live Chromium adapter test expanded for file exclusion and scroll but not run because NEOISM_TEST_CHROMIUM was not supplied; no live TypeSafe request/key test.
