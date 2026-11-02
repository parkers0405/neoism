---
name: "Agent composer phantom extra row"
description: "GPUIX composer grew phantom third row while prompt still displayed two; fix uses native shaped rows, requires native rebuild"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-11-02"
updated: "2026-11-02"
---

# Agent composer phantom extra row

The GPUIX Agent composer used Rust `measure_prompt_rows_for_host` (legacy font/character wrapping) to size its card while a native GPUI textarea independently shaped/wrapped the displayed text. A single character could increase the Rust estimate while the displayed prompt still occupied two lines, pushing the chips down and creating an empty third-row gap.

Source fix: extend the pinned `caretGeometry` payload with `text`, `layoutWidth`, and `visualRows` derived from the native textarea's full `content_height / line_height` (not the caret row, viewport height, focus, or scroll). Include these fields in geometry equality so same-position changes still report. `ComposerCaret.tsx` forwards layout; `App.tsx` validates draft/width, retains last measured height while typing, and sends rows/textWidth to `agentcomposer.frame`. Native bridge accepts rows only when text width matches current card geometry; initial/resized frames retain legacy fallback. Both home/chat cap visible height at five rows.

Touched display patch and standard/device-preview source pins. Native display and core binaries need rebuilding through user's dev loop. Verification: cargo check --manifest-path neoism-frontend/gpuix/native/Cargo.toml --lib --locked passed; bun run typecheck passed; source-module geometry tests (7 embedded Rust tests) passed; headless real Solid caret event script passed using mocked @gpuix/native window constructors. Normal child/UI tests were already blocked by stale display binding source pin before changes; new core-frame regression needs rebuilt native library. Do not claim live UI verified.

Paths: neoism-frontend/gpuix/native/patches/gpuix-display-frame.patch, native/src/agent_composer.rs, src/agent/ComposerCaret.tsx, src/agent/NativeComposer.ts, src/ui/App.tsx. Regression coverage in tests/agent-composer.test.ts and tests/support/composer-responsive-check.tsx.
