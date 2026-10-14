---
name: "Git panel actions dropdown"
description: "Rich Git panel now Commit + Git actions dropdown; native/web keys, popup scrolling/hit priority; missing advanced backend actions"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-13"
updated: "2026-10-13"
---

# Git actions dropdown

The rich shared GitDiffPanel footer now keeps Commit as the primary button and puts all supported actions in a Git actions dropdown: Commit, Stage all, Unstage all, stage/unstage selected file, Fetch, Pull (fast-forward only), Push, Switch branch, Refresh. The old Stage All and Fetch/Pull/Push grid is removed, freeing one footer row.

Panel files: neoism-frontend/shared/src/panels/git_diff/{state,types,update,render}.rs. Uses widgets::popover::Popover<widgets::menu::Menu<GitAction>>, with opaque overlay rendering above the footer and viewport-clamped, scrollable rows. Availability is checked both for disabled menu rows and again on execution. Commit requires a message and staged files; other actions require repo/provider and no loading. Branch/actions menus are mutually exclusive; blur/close dismiss actions. Wheel/touch must stay within menu, and divider/scrollbar hit tests must not act under its overlay.

Desktop events: neoism-frontend/desktop/src/screen/bridges/git_diff.rs; open menu is routed ahead of global shortcuts and chrome focus navigation in screen/selection/key_event.rs. Shared/web events: shared/src/chrome/events.rs, including commit/branch input parity. F4 opens actions while panel focused; Alt+Down opens from commit input; arrows/Tab/PageUp/PageDown navigate; Enter/Space activates; Escape restores commit input.

Regression coverage: shared/src/panels/git_diff/actions_tests.rs and chrome/events.rs tests. Use cargo check -p neoism-ui --tests and cargo check -p neoism-terminal-wasm --target wasm32-unknown-unknown. During implementation desktop cargo check was blocked by an unrelated missing activation field in a pattern in desktop/src/app/mod.rs (around line 9529); do not confuse that with dropdown errors.

Common operations still absent from GitDiffIo/backend: create/publish branch, remote branch tracking, stash, discard, amend/undo commit, pull --rebase, force push --force-with-lease, tags/remotes. Do not expose these as working actions until all native/host/wasm providers, Rust protocol/daemon permission classification and TypeScript wire types are extended. Destructive actions need confirmation.
