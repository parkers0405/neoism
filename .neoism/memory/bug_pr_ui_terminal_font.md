---
name: "PR UI must not inherit terminal typography"
description: "PR screenshot failure fixed: explicit regular Geist, inset quiet row, proper form/header hierarchy; native/WASM and focused tests passed"
type: "bug"
scope: "project"
origin: "User screenshot rejected initial PR styling; final visual patch verification"
created: "2026-10-22"
updated: "2026-10-22"
---

User rejected first PR UI screenshot: heavy monospaced 'Create pull request…' flush at sidebar edge with a full-width rule. Root cause DrawOpts::default font_id None inherits terminal PRIMARY font, not explicit bold. A flat layout alone is not Apple-like: explicitly design typography/spacing/hierarchy. Use primitives::prose_font::ui_prose_font_id(sugarloaf.font_library()) for regular bundled Geist; same font options for measure/truncate/wrap/draw. Do not migrate Nerd/PUA icons to prose font.

Fixed sidebar in panels/git_diff/render.rs: Geist12 regular, existing PADDING_X inset + vertical center,26pt allocation retained, measured bounded width, no permanent slab or full-width separator; inset hover-only highlight and chevron. Lookup real caret quad, no terminal │/Enter instruction. Hover tracked through update_latest_commit_hover and suppressed behind menus. Added CPU regression with pixel primary proving Geist/proportional advances + geometry across scale/narrowwidth + hover tests.

Fixed panels/git_pull_request.rs: Geist regular throughout; wrapped14pt title, separate dim11pt metadata; detail-only tabs separated from actionrow; no reviewtabs on creation; dim labels/inset underlined fields/compact accent-filled primary controls; caret geometry; decoration/hits clipped to viewport and occlusions. Dedicated git_pull_request_visual_tests.rs. Flow/intents/read-only/draft safety unchanged. Unrelated Git commit UI formatting preserved.

Final verification:33 PR-filter tests,120 Git tests,28 Chrome events; native cargo check -p neoism --bin neoism --tests exit0; WASM check with --features sugarloaf/web; global git diff --check passed. No screenshot/live GPU/app-launch verification; no release build or commit. Functional architecture remains feature_github_pull_requests.md.
