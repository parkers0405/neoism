---
name: "System settings and explicit SSH workspace"
description: "Clean Systems rows navigate into settings; explicit SSH opens/reuses remote Home without replacing local workspace or enabling background catalog."
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-23"
updated: "2026-10-23"
---

# System settings submenu and explicit SSH workspace

Supersedes the earlier inline Active/Remove layout. Main Systems list now contains navigation rows only; click/Enter emits ShowSystemSettings{id}, never Connect/Enable/SSH. Controls live in nested PaletteMode::SystemSettings: Active switch, safe connection retry/disconnect, SSH to system, Remove. Local computer is read-only with no SSH/activation/removal actions. Add system still saves inactive without networking.

Shared APIs: enter_system_settings_mode(entry), refresh_system_settings(entries), system_settings_id(), system_settings_back_action(). Back header uses a real arrow glyph and restores root query/cursor/selected system/scroll. Escape and empty-query Backspace navigate back. App live status refresh preserves details and selected setting; forgotten profile returns to list. Desktop has a separate custom keyboard handler in router/route.rs: handle Systems/Settings Enter before global ex-address/close parsing, so searching q/$/digits cannot close/jump editors. Shared key-routing has equivalent protection. Parent desktop bridges map ShowSystemSettings/SshToSystem to Screen SystemRequest::Settings/SshWorkspace; mutation confirmations return to details unless profile removed.

SSH to system is explicitly independent of saved enabled intent. It reloads the trusted saved ID and passes FULL stored target + ssh_args, not the rendered address. Inactive devices may open a one-off SSH workspace without becoming Active or entering the managed owner-agent catalog. Background Connect/Add/Active remain workspace-neutral. UI copy distinguishes background connection, live disconnect and forget; no remote files are deleted.

Critical distinction: new Screen::request_ssh_workspace_open queues normal SSH attachment with pending_ssh_workspace_replacement=None and destination id cleared. Existing request_ssh_workspace_attach retains CLI takeover semantics and captures current_workspace_id. Do NOT use the takeover helper for the settings action, or it can replace the local workspace. The established start_ssh_workspace_attach -> begin_ssh_workspace_switch -> remote-resolved ~ CreateWorkspace/adopt pipeline handles authentication, parked/tunnel reuse, generation guards, and stable remote Home workspace identity. Settings action cancels conflicting pending SSH/adoption intent, coalesces matching in-flight authentication, and retains local workspaces. No direct begin-switch shortcut or automatic enabled write.

App helpers are in app/systems.rs (open_system_settings, restore_system_view, open_system_ssh_workspace). Source-level background-only regression now checks Add/Connect/Active/background/completion paths separately, allowing intentional SSH action. New tests assert saved args preserved, no set_enabled/connect_saved_system in SSH helper, no local replacement capture, and opening settings does not dial.

WASM/web handle new actions with the existing honest desktop-only explanation. No fake browser SSH or server-token privilege substitution. This is a remote terminal/files/editor workspace, not graphical desktop/video sharing.

Verification: 124 palette tests, keyboard settings/back regression filter, and 38 desktop system tests passed. Native/shared cargo check --tests, wasm32 renderer check, web typecheck and scoped diff checks passed. No release build or live UI/remote machine actions. Concurrent native Vulkan edits briefly blocked checks but settled; final checks passed.
