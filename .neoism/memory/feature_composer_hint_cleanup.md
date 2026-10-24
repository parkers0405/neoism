---
name: "Composer hint cleanup and compact tray"
description: "Removed tab-agents hint only; lower tray now32px (3px less top/bottom padding), slash/Tab behavior preserved;306 tests/shared check passed"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-22"
updated: "2026-10-22"
---

User requested remove the `tab agents` hint and reduce top/bottom padding in lower directory/hints section. Removed INPUT_TAB_HINT, show_tab_hint policy field/assignments, painting and width reservation in shared view/user_input.rs; kept `/ commands`, connect/interrupt hints, actual Tab keyboard behavior unchanged. Simplified command-only alignment and obsolete activity_guard final assignment; updated policy tests. Lower tray INPUT_HELP_STRIP_H now32px instead of38px, reduces vertical padding3px per side around unchanged12.5px type; layout reserve follows same constant so no straygap/viewport clipping. Geometry corner tests now derive traybottom ratherthanoldhardcoded155px. BlackmainRGB/directmaterialjoin/floating silhouette unchanged. 306 shared agent viewtests passed,1ignored; cargo check-q-pneoism-ui passed; scopedgitdiffcheck clean. Remaining nonblocking warnings: preexisting neodrawunusedid and test-only ComposerGeometry::contains/rounded_contains in non-test lib, not changed here.
