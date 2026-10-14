---
name: "Unified Add Server and Fast tunnel switch"
description: "One Add Server Host/Join modal with Fast tunnel switch; typed drafts, locked invites, single picker action, short-form scrolling fix"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-13"
updated: "2026-10-13"
---

# Unified Add Server modal

User replaced separate Add Server / Add Fast Server picker actions with ONE Add Server form. Shared palette now appends only ServerAdd; removed the unshipped FastServerAdd row/action and wasm action-name case. Existing CreateServer command opens the same unified form in Host mode. HTTPS invite-page instructions now say Add Server -> Join with Fast tunnel enabled; CSP hashes are computed from script/style, so wording updates do not leave stale hashes.

Desktop form builder/state is in screen/bridges/palette.rs. Standard indices are Host=0 / Join=1. Ordinary Add Server defaults Join/Fast OFF (persistent saved connection); Fast ON uses existing ephemeral OpenTunnel lifecycle. Typed ServerFormDraft retains common name/raw password and independent host directory, saved address, and Fast invite link across transitions. Host directory default remains workspace_root_for_new_shell, identical to existing Create Server. Mouse tabs and Ctrl+Tab use the same mode transition; switch clicks and Space/Enter use the same transport transition. Saved Edit remains unchanged and gets no Fast switch/tabs.

Clickable invites initialize the unified Add Server form with Join/Fast ON, visibly locked switch and locked Host/Join selector. The fast_invite_address discriminator, capability fragments and raw/untrimmed passwords remain intact. Screen state and submit validation reject transport/mode mismatches, so an invitation cannot silently register persistently or create a host. Empty persistent create_dir is validated by field presence, not accidentally sent to Join.

Shared widgets/modal.rs adds an optional real Boolean switch row (Settings-style 40x22 track/round knob), geometry, hover, focus, checked/enabled state and locks. APIs: set_form_switch, form_switch_checked/hit/focused, activate_form_switch, set_form_tabs_enabled, lock-aware set_form_active_tab, form_values (nonconsuming/unmasked; never log passwords), focus_previous_form_field. All open/close paths reset switch/locks. Web Chrome emits non-closing Generic form-switch / form-tab host actions; desktop lifecycle/route keyboard invokes typed Screen hooks.

A short viewport/high scale exposed an existing form overflow bug: panel height was capped while fields/buttons retained natural positions, causing visible controls to register as outside clicks. Fixed via authoritative FormGeometry: pinned title/tabs/switch/footer, scrollable clipped description/field body, bounded hit rectangles/backgrounds/carets/scrollbar, keyboard focus reveal, Wheel/PageUp/PageDown scrolling. Very short layouts compress spacing/adapt bounded scale and can move switch into the scroll prefix. Existing desktop/web wheel routes use modal.scroll_at.

Focused source review found no transition/keyboard/invite-lock/credential-cache/Edit regressions; it identified overflow, now fixed. Modal/picker test targets and wasm web check passed before concurrent unrelated diff-card edits. Latest overall check blocked by others' missing CardSpec.horizontal_scroll initializers in agent tool-message/git-diff renderers; prior desktop checks also hit transient unrelated Composition preflight/ResultAlias edits. Do not revert or edit those owners' changes. Tests were compiled, not executed; no release build or live visual popup verification was performed.
