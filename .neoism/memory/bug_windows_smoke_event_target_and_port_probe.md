---
name: "False extra-window smoke failure and blocked Windows service startup"
description: "Native .94 smoke false-positive on zero-size winit window; failed TCP connection incorrectly suppressed service launch; corrections tested"
type: "bug"
scope: "project"
origin: "Native v0.7.94 MSI failure evidence"
created: "2026-09-07"
updated: "2026-09-07"
---

v0.7.94 release run34083058638 failed installed-MSI smoke, Linux/mac builds passed, public release stayed hidden draft. Evidence downloaded /tmp/neoism-v0.7.94-windows-evidence. unexpected-windows.jsonl only flagged neoism class 'Winit Thread Event Target' blank title. neoism-window/src/platform_impl/windows/event_loop.rs create_event_target_window creates0x0 layered toolwindow then sets WS_VISIBLE for WM_PAINT event dispatch. IsWindowVisible alone is NOT evidence a user-visible window exists. Correct smoke Snapshot to require GetWindowRect positive width/height; also select main window from this filtered list instead of Process.MainWindowHandle (can temporarily select same event target). Do not disable extra-window checks or whitelist all neoism windows.

Same native logs: local endpoint labelled occupied/notready, no workspace-service.log/no service child in process snapshot; GUI websocket refused. service_process::probe_tcp treated every failed connect except ErrorKind::ConnectionRefused as NotReady, preventing launch. On native Windows short connect deadline can expire before refusal. Correct rule: successful connection followed by invalid/no daemon health = NotReady/occupied; failed connection permits owned service launch, actual child bind determines collision and retry remains backed off. Added closed_daemon_port_allows_startup. Six service tests passed, Linux + Windows production checks passed, PowerShell script parser+C# compile passed via ConPTY Wine. Native corrected MSI behavior still needs next normal release runner. Preparing corrective release v0.7.95; never move existing .94 tag.
