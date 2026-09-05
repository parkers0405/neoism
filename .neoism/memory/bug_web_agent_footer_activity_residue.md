---
name: "Web Agent footer activity residue"
description: "Web no longer treats waiting_subagents as a standalone streaming verb; authoritative family snapshots and terminal Task results clear aggregate and parked child residue."
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-09-01"
updated: "2026-09-01"
---

---
name: "Web Agent footer activity residue"
description: "Web no longer treats waiting_subagents as a standalone streaming verb; authoritative family snapshots and terminal Task results clear aggregate and parked child residue."
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-09-01"
updated: "2026-09-01"
---

# Web Agent footer activity residue — fixed follow-up

Remaining root cause after the first terminal-family cleanup: the daemon translated `session.status { type: waiting_subagents }` into protocol `StreamingState::WaitingSubagents`, and WASM ingested it with ordinary per-session streaming lifetime. Desktop's event classifier intentionally ignores that status and derives `Sub-agents working` only from child lifecycle. Therefore web could retain the exact aggregate label independently of the authoritative branch set. A second bypass existed for provider paths that emitted only terminal `tool.completed` Task output (`task_id` + explicit terminal `status`) without a later `message.part.updated`, `session.subtask.completed`, or timely `execution.finished`.

Fix:
- daemon no longer promotes `waiting_subagents` status into standalone streaming state (desktop parity);
- WASM defensively treats legacy WaitingSubagents frames as hints and never as independent activity, including parked cache routing;
- shared authoritative family reconciliation clears aggregate waiting state/hold whenever the versioned branch set has zero outstanding children, but clears other root verbs only on `execution.finished`, preserving parent continuations;
- equal-revision reconnect snapshots reapply idempotently to heal late local residue; strictly older snapshots remain rejected;
- terminal child edges clear only that child's parked runtime and clear the parent aggregate after the final active child, preserving siblings and out-of-order completion;
- daemon synthesizes `SubagentUpdate` from Task tool output only when output explicitly says completed/error; `status: running` background launch results remain active;
- WASM has a direct Task-result fallback for compatible protocol producers.

Files: `neoism-frontend/shared/src/panels/agent_pane/state/{streaming.rs,state.rs,tests.rs}`, `neoism-frontend/wasm/src/rendered/catalog.rs`, `neoism-workspace-daemon/src/agent/events.rs`.

Verification: `cargo check -p neoism-ui -p neoism-workspace-daemon`; wasm32 check for `neoism-terminal-wasm --features web`; daemon agent event suite 15 passed; wasm tests compile with `cargo test --no-run`; rustfmt check and targeted `git diff --check` pass. Running wasm tests is unavailable because `wasm-bindgen-test-runner` is not installed. Shared unit execution is blocked by unrelated dirty-worktree test compile error in `shared/src/chrome/events.rs` referring to the removed `agent_pane::NeoismAgentPane` re-export.
