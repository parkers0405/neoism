---
name: "Shared workspace presence + note opening"
description: "Presence remains across tabs in one shared workspace but clears on workspace switch; shared-note opens use host vault-scoped reads without ENOENT flash"
type: "bug"
scope: "project"
origin: "User report and implementation on 2026-07-26"
created: "2026-07-26"
updated: "2026-07-26"
---

## Invariants

- Presence is scoped to the selected workspace, not the selected tab.
- Switching among editor, terminal, agent, settings, or other tabs inside one collaborative workspace must retain top-chrome membership.
- Switching from that collaborative workspace to a local/different workspace must still publish `ClearPresence`; only the tab-kind transition is special.
- Non-editor tabs publish `workspace://presence` with a zero cursor. It is a membership sentinel, never a file path.
- Desktop and web must use the same sentinel. Desktop selects it in `Screen::drain_daemon_presence_messages`; web selects it in `TerminalPanel.activePresenceTarget`. Web panel `dispose()` remains the true workspace leave.

## Shared notes click routing

- A joined workspace may show the host's linked notes vault even when that vault is outside the daemon's served project root.
- Sidebar listing/mutation requests already use `send_files_with_request_id(..., Some(vault_root))`.
- Opening a shared note must also issue a vault-scoped `ReadFile` and correlate it through `pending_remote_markdown_opens` or `pending_remote_code_opens`; direct local opening alone produces ENOENT on the guest's machine.
- Immediately after pane creation and before dispatching the vault-scoped read, call `mark_remote_loading()` on the pane. Otherwise the synchronous local-open ENOENT renders red for one frame before remote bytes arrive.
- Local/private vault clicks remain direct local opens. EPUB is excluded from the generic text read path.

## Verification

- `cargo test -p neoism-ui remote_presence --lib`: 13 passed, including agent-tab sentinel then workspace-leave clear.
- `cargo check -p neoism --message-format=short`: passed after the loading-state fix.
- Web `npm run typecheck`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
