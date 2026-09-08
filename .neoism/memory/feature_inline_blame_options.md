---
name: "Configurable inline blame — immediate by default, optional scroll hiding"
description: "LATEST: editor.git-blame-delay-ms=0 and editor.git-blame-hide-on-scroll=false; optional 250ms cursor debounce or independent 150ms scroll hiding; typed GUI/native/web reload; personal config immediate"
type: "feature"
scope: "project"
origin: "User-approved configurable refinement, resumed after abort"
created: "2026-09-08"
updated: "2026-09-08"
---

# Configurable inline-blame timing (latest semantics)

Supersedes the fixed 250ms behavior in [[feature_inline_blame_settle]]. User wanted the old immediate behavior by default, with optional cursor delay and scroll hiding. Base feature: [[feature_inline_git_blame]].

Exact typed grouped keys:
- `editor.git-blame`: existing boolean, default false; unchanged.
- `editor.git-blame-delay-ms`: u64 milliseconds, default **0** (immediate). Set 250 for prior cursor-settle behavior. Negative/fractional values rejected by typed config.
- `editor.git-blame-hide-on-scroll`: bool, default **false**. True hides while scrolling and restores 150ms after last viewport movement, independently of cursor delay. Actual smooth-scroll residual movement also restarts the scroll window.

Config definitions/defaults/tests in neoism-backend/src/config/mod.rs, commented example in defaults.rs, Settings GUI descriptors in intelligence.rs: Number control 'Inline blame delay (ms)' and Toggle 'Hide inline blame while scrolling'.

Shared CodeBlame.set_options preserves cached snapshots/images; settling() is enabled && (configured cursor timer || optional 150ms scroll timer), using web_time::Instant. Scope reset preserves both options/deadlines. CodePane scroll methods call note_scroll(), which also dismisses hover. Host request pumps call observe_blame_viewport() before needs_request, deferring new requests while hidden; in-flight replies can still populate cache. render.rs retains finite animation ownership until the timers expire, no gutter or wrap-width changes. Hover dismissal/rearm behavior from prior refinement remains.

Native renderer stores both options, initializes from typed config, applies hot reload to all panes, and applies defaults in code_blame.rs pump. Shared Chrome stores both options, reads them in set_settings_values and applies to parked/current panes. Active draw pass applies options before the first frame of a newly opened pane; wasm request pump applies them before requests. Existing web GetConfig hot reload polling (15s) and immediate GUI write refresh remain the transport. Applies to desktop launched by app launcher or CLI and web/joined graphical editor panes. Terminal-only agent CLI has no graphical blame surface.

User explicitly authorized targeted personal config update: `/home/parkersettle/.config/neoism/config.json` editor block now includes git-blame=true, git-blame-delay-ms=0, git-blame-hide-on-scroll=false; all other settings preserved.

Validation passed: cargo check -p neoism-backend -p neoism-ui -p neoism --tests; cargo check -p neoism-terminal-wasm --target wasm32-unknown-unknown; git diff --check. Added/updated compile-checked regressions for immediate defaults, independent 149/150ms scroll timing, 250ms opt-in, hot option changes, scope-reset request gating/image retention, serde defaults/roundtrip/rejection. No tests executed, no release build, no commits; unrelated workspace edits preserved. Running GUI behavior not manually exercised.
