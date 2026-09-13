---
name: "Computer wrong-window typing: explicit target admission fix"
description: "Explicit per-call target now mandatory for typing/batches; no implicit focus inheritance, desktop scope only individual key/pointer, foreground loss stops and screenshot stays bound"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-09-12"
updated: "2026-09-12"
---

Wrong-window typing root cause confirmed in source: focus() only confirmed native foreground; run_worker started Check.target=None each call and execute_locked only bound supplied target. Untargeted batch actions + final screenshot stayed unbound (targetWindow:null), so per-character/press checks did not validate foreground. Fixed via explicit-per-call contract, NOT persisted focus: input requires target unless explicit scope:desktop (individual key/pointer/scroll only; text forbidden); batch requires target, no scope/overrides; same target passed to all actions + screenshot. focus result echoes confirmed target and binding:explicit-per-call and warns no inheritance/refocus. Existing Linux transaction checks after map publication ACK and before each character/press; mac/win character/press checks already existed. Never auto-focus during input. Scope parser fails before platform IO. No new global/session binding; explicit short-lived tokens remain existing snapshot handles, not session-owned capabilities. Schema and docs updated, GUI unchanged.
Tests: cargo check -p neoism-agent-server -p neoism pass; computer_use filter 40 pass/1 ignored live probe; keymap exact-dependency harness 24 pass/1 ignored; exact modules crosschecked Win MSVC + ARM macOS using /tmp/neoism-computer-use-platform-check manifest. New regressions cover missing explicit target/no inherited state, desktop constraints, mocked foreground loss before/between/mid-actions with partial progress and screenshot binding, actual Linux transaction mid-Unicode cleanup, stale tokens/epoch and handle owner PID. No live input/focus/capture performed. Unicode map publication changes seat layout but has no focus request; cannot attribute user's observed switch to it from source alone. Global OS input retains TOCTOU; cleanup releases bypass guard. Input on unsupported Linux window compositor fails closed except explicit desktop scope.
