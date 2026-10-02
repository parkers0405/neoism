---
name: "Lua E3/E4 progress"
description: "Scoped options complete; input/editor resources partial with precise blockers"
type: "project"
scope: "project"
origin: "coding session"
created: "2026-08-01"
updated: "2026-08-01"
---

Completed and verified the scoped editor-option tranche: closed Rust enum (`wrap`, `tab_width`, `use_tabs`, `input_mode`), exact-owner contributions at document/pane/tab/workspace scope, deterministic specificity/priority/order, stale-target rejection, native baseline restoration, and desktop application into CodePane/CodeBuffer. Started but did not declare complete the broader input/editor-resource tranche: multi-key keymaps (750ms), priority/order/failing-key policy, motion/operator/text_object/input wrappers, native register/mark/jumplist/macro projections/actions, and macro replay through native editor synchronization. Changelist remains blocked by absent Rust-native changelist state; synchronous authoritative clipboard reads need an application-executor coordinator/snapshot. Added generated annotations/docs and explicit default-sandbox test. Verification: cargo check -p neoism passes; neoism-lua suite 36 passed before sandbox test addition; neoism-ui focused vim suite 50 passed; diff checks clean. TASKS marked only scoped editor options and default sandbox complete.
