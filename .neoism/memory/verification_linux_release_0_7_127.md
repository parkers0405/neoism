---
name: "Linux v0.7.127 first-launch binary verification PASSED"
description: "Actual downloaded v0.7.127 passes 2 fresh profiles, relaunch, 5 forced reloads in isolated Ubuntu24; no Bash SIGFPE or 1x2 PTY; no developer toolchains"
type: "reference"
scope: "project"
origin: "neoism-agent"
created: "2026-10-17"
updated: "2026-10-17"
---

# Release 0.7.127 first-launch binary verification

Verified the actual newest downloaded archive `/home/parkersettle/Downloads/neoism-linux-x86_64(1).tar.gz`, version `neoism 0.7.127`, SHA256 `4d18fad8ab2baf929cfb7a95cdcdbdf4861a07dcc67b978fa981d8d6ac5b59ae`. This validates the source fix described in [[bug_linux_first_launch_bash_sigfpe]].

Ran downloaded binary in a separate Ubuntu24 container snapshot with desktop runtime libraries and diagnostics, UID1000, fresh HOME, empty environment, only /usr/bin:/bin, isolated headless Weston plus software Vulkan llvmpipe. No host display, config, credentials, development PATH, or live user GUI access. Node/npm/Bun/Cargo/rustc/nvim/Git absent. Python was installed solely to drive the test harness, not as an application dependency. All four packaged binaries resolved their dynamic libraries in this runtime.

Results: first fresh-profile launch initialized and remained alive 30.4 seconds including three forced config-file changes/reloads; second launch of same profile remained alive 13.8 seconds including one reload; independent second fresh-profile launch remained alive 18.8 seconds including one reload. Both interactive Bash shells survived each run and retained identical PIDs through all reloads. strace found NO SIGFPE and NO TIOCSWINSZ 1x2; valid resize sizes were rows/cols 23x62, 23x99, and 26x99. Headless first-run logs explicitly show bundled example packs seeded BEFORE config watcher starts. Test harness terminated apps deliberately after observation, then removed the test container; no unexpected exits.

Full report `/tmp/neoism-release-newest-retest/report.json`; reproducible harness `/tmp/neoism-release-newest-retest/run_test.py`; logs and straces in same directory. Extracted new binary there, left original Downloads archives and old extraction unchanged. Runtime snapshot image `neoism-release-headless-runtime:local` is reusable with appropriate entrypoint (Python was only installed in the disposable test container). No source changes or builds performed for this verification. This proves first-run/reload fix on Ubuntu24 Wayland software rendering; it is not all-distros/all-GPUs/X11 verification, and the separate glibc2.39 baseline restriction remains unchanged.
