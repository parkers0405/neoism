---
name: "Invite activation wrong-window consumption fixed"
description: "Fixed wrong-window Wayland authority consumption: endpoint-bound tokens, actual target, cold/mapped handling; pure helper for IPC tests; Windows PID-scoped foreground grant"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-13"
updated: "2026-10-13"
---

# Invite activation must follow the actual target

Clickable Fast invites initially used a single App activation slot plus an event-handler fallback to active/focused/any window. This could consume the browser's one-shot Wayland authority on B while the requested workspace was in A behind a modal. FIXED: only accepted invites return a normalized endpoint; one batch token is associated with the first accepted endpoint in a bounded map (pending queue plus displayed invite), never a generic focused-window fallback. Repeated delivery may raise an active prompt only if its endpoint matches.

`app/invite_join.rs` owns Application activation state methods. Queueing matches the exact public endpoint, or maps a locally owned Fast public origin to its local endpoint. It retires prior window targets for that subject, preventing a stale cold window from consuming a replacement token. Existing rendered/minimized surfaces consume authority immediately; cold targets wait for the initial render. Native Window wrapper has `has_rendered_frame`, set by finish_frame_cadence. Destroyed-target metadata is removed BEFORE route existence filtering. Invalid/rejected queue inputs do not retain authority.

An already-open target behind a modal can activate its actual window without switching tabs or overwriting the dialog. A new invite uses one retained candidate window when the current window is busy, dialing, or dirty; it does not repeatedly allocate windows while its candidate is busy. Existing-workspace focus cancels competing generations before returning for busy/dirty UI, preventing an older dial from stealing the subsequent selection.

Keep `app/invite_activation.rs` a PURE portable Window/InviteActivationToken helper, with no Application imports/impls: ipc.rs unit tests include it with #[path], so `super::Application` would resolve under ipc and fail (or duplicated App methods would collide). Application methods belong in invite_join.rs; the App event owns normalized-subject assignment and render callback.

Windows warm URI delivery grants foreground permission only to the connected named-pipe server PID via GetNamedPipeServerProcessId then AllowSetForegroundWindow; reject PID 0/u32::MAX (ASFW_ANY), no arbitrary PID from URL. This is best-effort OS permission, not a runtime focus guarantee. Linux authority is opaque, bounded/redacted, captured before environment clearing, forwarded privately, and consumed by xdg_activation without background minting or state-mutex reentry. No live browser/compositor runtime test was performed. Focused source re-review found no remaining blocking issue.
