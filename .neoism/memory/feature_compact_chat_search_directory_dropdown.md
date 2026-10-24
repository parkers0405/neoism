---
name: "Compact chat search and directory dropdown"
description: "Chat Search full-width titles, no Neoism side label, matched-sentence second line, fixed eight-row loading/result viewport; directory stays compact"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-18"
updated: "2026-10-18"
---

Scope is chat CONVERSATION SEARCH MODAL and SIDEBAR DIRECTORY/PROJECT DROPDOWN, not agent/model/provider selectors. User wants simple Alt+S Files Finder chrome, BUT explicitly requires skeleton loading and a second matched-sentence line. No generic 'Neoism' source/project branding beside the conversation title, and no resizing when skeletons become results. Global InlinePicker and other selectors stay unchanged.

Navigator-specific renderer: shared/widgets/conversation_navigator/render.rs. Finder frame helper, opaque panel_bg/surface, 8px top-only radius,4px padding,40px input at14px with14px horizontal padding,1px separator,hover selection radius3,no accent stripe. Searchwidth480 stays centered. Projectskeep sidebaranchor,6px gap,320–420px width clamp,six-row cap. No title/esc strip or bottom instruction/range footer. KeepoverlayORDER180.

CURRENT Search48px rows:13px normal title gets FULL FIRST-LINE WIDTH. The old `available*0.6` reservation and right-side footer/source rendering were removed after user screenshot complaint.12px matched sentence gets FULL SECOND LINE. Source/footer metadata stays in host row data but is NOT painted beside Search titles. Projectsremain32px single-line with path/context. Measured Unicode-safe fit_excerpt retains query visibility with12 leading context chars. All scrolling/reveal/cursor/refresh/hit calculations use mode-specific row_height; shared Scroll/direct touch preserved.

Search modal reserves SEARCH_VISIBLE_ROWS=8 (clamped by viewport capacity) IN EVERY STATE: skeleton loading,1/few/many real hits,empty anderror. Bounds AND position are identical across states. Skeletons fill the same capacity, one title bar plus one sentence bar per Search row, with animated safe-phase alpha. No fake clickable blank rows. Projects loading stays three compact skeleton rows and its normal compact sizing. Existing real hits remain usable during refresh, status only in input-right.

Sidebar alignment in shared panels/agent_pane/view/side_panel/draw.rs: removed leftover8px spacer beforeSearch after divider removal. Newchat/Search/Projects all34px high, shared catalog_action_layout for20px icon slots,7px icon inset,29px icon-to-label offset and centered labels; hover/focus styles match. Directory-bottom divider remains. Regression covers multiple scales.

Latest29 navigator tests plus8 Chrome navigator tests passed; previous sidebar alignment testpassed. Desktop cargo check -p neoism --bin neoism and wasm32 --features web --tests checkpassed; gitdiffcheck and scopedrustfmt passed. No app launch/restart/releasebuild. Supersedes older46px InlinePicker geometry and interim32px single-line Search / three-row Search loader / right-side-source-label designs.
