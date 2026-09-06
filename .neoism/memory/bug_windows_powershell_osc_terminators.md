---
name: "PowerShell ConPTY OSC terminator failure"
description: "Reproduced PowerShell lifecycle failure: Wine ConPTY strips BEL OSC terminators; ST fixes ls/clear/sleep/error tests without changing Unix hooks"
type: "bug"
scope: "project"
origin: "User persistent Windows terminal stall; actual pwsh runtime reproduction"
created: "2026-09-06"
updated: "2026-09-06"
---

Follow-up after user reproduced ls+Enter forever despite previous Windows terminal fixes: actually ran official portable Windows pwsh 7.4.7 under Wine through production PtySession/ConPTY and shared -NoExit -EncodedCommand hook. Original hook emitted OSC 7/133 with BEL terminators that Wine ConPTY stripped. Raw output had ESC]133;D;0 ESC]133;A ... ESC]133;B with no BELs, so parser lacked complete lifecycle signals. Raw CR command execution itself worked. Console.IsInputRedirected=False and PSReadLine=True. Console.Write instead of returned prompt and explicitly enabling VT output did NOT restore BEL. Fix all six PowerShell hook OSC terminators to ST (ESC backslash). Unix shell hooks unchanged. Do NOT replace CR with LF: LF entered >> continuation under PSReadLine.

Tests neoism-terminal-pty/tests/windows_interactive.rs now check actual ls\r at startup and after clear/cls/Clear-Host in fixture directory, assert filename not present in input before D;0. Includes computed output, 250ms delay no premature D, failed D;1, and no-PSReadLine case explicitly unloaded/autoload disabled. All 3 pwsh tests and 4 shell-hook tests passed actual Wine execution. Windows PowerShell5.1/native hardware not tested, so don't claim Wine proves all native failures fixed.

Portable Windows pwsh ~/.cache/neoism-pwsh-runtime/7.4.7/pwsh.exe. WINEPREFIX=~/.cache/neoism-cmd-lifecycle-wine; WINEPATH='Z:\home\parkersettle\.cache\neoism-pwsh-runtime\7.4.7'; Wine /nix/store/6qyy46wlrz886xg9cxb7f80i8gn0y78f-wine-wow64-11.0/bin/wine. PATH ~/.cargo/bin provides cargo-xwin. cargo xwin test -p neoism-terminal-pty --target x86_64-pc-windows-msvc --test windows_interactive --lib --no-run then run resulting .exe Wine with pwsh --ignored --nocapture --test-threads=1. Logs /tmp/neoism-pwsh-{original,console,st,vt}.log and /tmp/neoism-pwsh-fixture-{build,runtime,hooks}.log.

User also raised potential mismatched ordinary console host vs pseudoconsole attachment; separate audit in progress. Other concurrent fixes cover silent local PTY worker failures and remote stale/disconnected sessions, and Windows-only agent status timing/queued flicker.
