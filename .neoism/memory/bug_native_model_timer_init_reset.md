---
name: "Native model timer initialization reset"
description: "Crafting timer counted admission wall time then reset at execution init on Linux/macOS; remove Windows-only model-clock/admission gates"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-22"
updated: "2026-10-22"
---

Native Rust Crafting/Pondering timer displayed optimistic `streaming_started_at` wall time on Linux/macOS while prompt/session initialized, then switched to authoritative execution provider time (initially zero), causing visible count-up -> reset -> count-up. DesktopStatusTiming had Windows-only gates on pending_model_clock, retire_execution/accepts_execution, and submit's finished-prior-execution retirement. Removed those gates for admission/model-clock ownership on all desktop platforms, while retaining enabled=cfg!(windows) ONLY for wall-clock correction (`started_at`, `execution_wall_ms`, streaming state change timestamp). Before execution snapshot, locally admitted streaming now displays zero model duration; authoritative model clock then takes over without reset. Finished prior execution retired on all platforms; live family executions preserved. Expanded tests in desktop agent/pane/tests.rs for both policies (initial five-second admission, binding, zero snapshot, completed progression, stale rejection; finished prior reset; ongoing family retained) plus policy lifecycle test. `cargo check -p neoism --tests` passed, existing warnings; tests compiled but not executed. Earlier timeline activity min(attached_y) placement fix untouched.
