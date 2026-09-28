---
name: "Streaming Edit preview collapse"
description: "Streaming Edit diff previews briefly collapse due to optimistic live-trace rebase and stale layout"
type: "bug"
scope: "project"
origin: "user screenshot and 2026-08-? investigation"
created: "2026-09-23"
updated: "2026-09-23"
---

Agent Rust GUI transient one-line Edit diff cards during streaming: archived rendering gives diff body height 0, but stale cached timeline rows remain visible after trace boundary shifts, so file-card headers alone appear until next update. Actual fix in desktop pane ingest + shared state: on optimistic empty-id user prompt snapshot, match original prompt before first previously-live part id and keep boundary ahead of already-visible tools; unresolved prompt never reanchors to newest User; older-page prepend shifts optimistic boundary; rebase boundary changes invalidate layout cache. Separate Edit wrapped-title measurement now includes same title offset as renderer. Regressions in desktop pane/tests.rs and shared state/tests.rs. Do NOT 'fix' by hiding archived diff headers: that only changes presentation and was reverted. Preserve fixed-height preview and click-to-scroll.
