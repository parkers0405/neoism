---
name: "cmd lifecycle PROMPT hook and replay-safe marker row"
description: "Actual cmd PROMPT D/A/B shared native integration; critical Wine-verified $_ prefix row prevents false D on line-editor repaint. Computed-output/sleep/cls interactive test passes."
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-09-06"
updated: "2026-09-06"
---

Follow-up to [[bug_windows_terminal_shell_identity]]: interactive cmd now has actual OSC prompt integration, not just completion heuristics.
- `neoism-terminal-pty/src/shell_integration.rs::{cmd_prompt,apply_cmd_prompt_env}` frame child-only PROMPT, preserving original expanded text (default $P$G) and case-insensitive env semantics. Explicit /C, /K and positional batch invocations left untouched; no argv rewrites. No invented status: D has NO exit argument.
- `LocalPty::spawn` applies after resolving actual Windows shell. Therefore desktop local PTYs, daemon prepared PTYs, and default fallback cmd all go through same last native spawn boundary. Unix path unchanged.
- CRITICAL discovered via real Wine: plain inline `D/A + original prompt + B` is NOT robust. cmd line editor can replay prompt markers while echoing/wrapping a command; short visible prompts reproduced premature D (<100ms while foreground child sleeps 800ms). Long prompt initially hid the issue by clipping prefix during redraw. Correct PROMPT is `$e]133;D$e\$e]133;A$e\$_<original>$e]133;B$e\`. The `$_` puts lifecycle prefix on its own row outside editable-line repaint. Original visible prompt text preserved, with an extra marker-only prompt row. Do NOT remove this separator.
- cmd still has no C; D-generation is completion authority. Old B may be replayed without D and MUST NOT finish submitted command. Native parser accepts ST and no-exit D, resets last_exit_code=None; tested byte-fragmented stream. Shared buffer cmd test holds old awaiting state 400 rows/5sec and stays Running; next D finishes; cls only clears on next D.
- Dedicated owned Wine test: `neoism-terminal-pty/tests/cmd_lifecycle.rs` (NOT parent's windows_interactive.rs). Initial inherited custom short prompt at C:\, short `set /a` computed result, wrapped command invoking test binary foreground child that prints 300 computed rows then sleeps 800ms then computed marker; three cls/child cycles. No literal expected result in submitted source, 20sec per-phase timeout, responds to CPR, checks D precedes fresh A/B not cached B, rejects D before computed result or <700ms. Passed via cargo xwin test + Wine 11, plus three consecutive stress reruns.
- Wine path `/nix/store/6qyy46wlrz886xg9cxb7f80i8gn0y78f-wine-wow64-11.0/bin/wine`; WINEPREFIX=$HOME/.cache/neoism-cmd-lifecycle-wine; WINEDEBUG=-all; CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER=Wine path; env -u CI cargo xwin test --target x86_64-pc-windows-msvc -p neoism-terminal-pty --test cmd_lifecycle -- --ignored --exact interactive_cmd_prompt_lifecycle_survives_output_delay_and_cls --nocapture.
- Verification: Windows desktop/daemon production xwin check --features wgpu PASS; native desktop/daemon check --tests PASS; 4 shared hook tests + 1 ST parser + all 68 terminal buffer tests PASS. No markdown, pipes or parent's tests/windows_interactive.rs edits; no commit.
- Deliberate cmd /Q/echo-off disables prompt output (observed on Wine), and later PROMPT replacement can remove integration; do not silently override user settings. Test uses /D to exclude machine-specific AutoRun; production preserves AutoRun behavior.
