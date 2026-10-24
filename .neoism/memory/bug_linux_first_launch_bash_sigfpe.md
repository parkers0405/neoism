---
name: "Linux first-launch Bash SIGFPE at 1x2 PTY"
description: "Linux first launch closes: config reload resets viewport, sends 1x2 PTY, Bash Readline SIGFPE; geometry/bootstrap/daemon spawn source fixes"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-16"
updated: "2026-10-16"
---

# Linux fresh-profile first launch closes: Bash SIGFPE from 1x2 PTY

Diagnosed against downloaded Linux v0.7.126 in clean Ubuntu 24.04 with runtime libraries only, UID1000, empty HOME/environment, isolated headless Weston. No live user GUI interaction was used for the diagnosis/fix. Stock second launch remains open; first launch reliably closes.

## Proven root cause

First-run `Screen::new` seeded example packs after `Application::new` had started recursive config watching. Pack writes trigger config reload. `Screen::update_config` rebuilt `Renderer`, whose initial surface viewport/content are empty, then derived workspace bottom padding by subtracting that empty content from the real window height. This reserved the entire window as bottom padding. Grid computation falls back to MIN_COLS=2/MIN_LINES=1 when no available height, issuing TIOCSWINSZ 1x2 to the LOCAL shell. Bash/Readline faults with SIGFPE/FPE_INTDIV (`rl_redisplay`, idiv divisor zero). PTY exit closes the sole route and window. There was no incoming Wayland close or compositor error. Exit status 0 was misleading because Application::exiting calls process::exit(0).

Headless strace ordering: renderer rebuild 16:11:54.312375; local PTY TIOCSWINSZ rows1 cols2 .313140; Bash reads size .313301; SIGFPE .313393; shell reaped with WTERMSIG SIGFPE .397910; VulkanContext drop .424644. Restoring 23x62 came too late. A separate daemon PTY was also being spawned with hardcoded 1x2 and its Bash crashed on both first and second launches.

## Source changes

- `neoism-frontend/desktop/src/app/mod.rs`: seed_example_packs before plugin discovery and config watcher startup; removed seeding from Screen::new in screen/mod.rs.
- `screen/chrome_geom.rs`: resolve rebuilt renderer surfaces using real logical window dimensions BEFORE computing margins/resizing PTYs. `surface_bottom_inset` uses matching resolved viewport/content rectangles, not actual current window size minus unmeasured/stale content. Added regression tests for empty initial viewport and top/bottom dock reservations, including resulting noncollapsed terminal dimensions.
- `context/manager/daemon_sessions.rs`: request_daemon_pty_for_route looks up route's actual ContextDimension and sends its measured rows/cols instead of MIN_COLUMNS/MIN_LINES placeholders; refuses nonexistent routes.

Verification: `cargo check -p neoism --bin neoism --tests` passes (existing warnings); regression tests type-checked, not executed. No release build or binary/archive replacement was done, respecting user's own build workflow. Source fix needs inclusion in rebuilt artifact before clean-profile binary retest. glibc 2.39 / Ubuntu22.04 compatibility is a separate unchanged packaging limitation.

Evidence retained inside Docker container `neoism-release-ubuntu24-runtime`, `/tmp/neoism-headless-investigation/`: FINDINGS.txt, first.stdout, first.strace.7617, first.strace.7616, first.stderr, bash-1x2-confirm.gdb, second.strace.* and second.exit. Investigation-owned Weston stopped; prior host test app was not operated. Temporary host logs also under /tmp/neoism-release-clean-test and /tmp/neoism-ubuntu24-release-test-logs.
