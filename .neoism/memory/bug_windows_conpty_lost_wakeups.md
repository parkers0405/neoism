---
name: "Windows ConPTY lost wakeups and shutdown"
description: "ConPTY lost wakeups fixed; bounded pipe shutdown, EOF/readiness, Wine regression coverage; related shell and markdown fixes"
type: "bug"
scope: "project"
origin: "Windows terminal and markdown deep dive"
created: "2026-09-06"
updated: "2026-09-06"
---

Windows terminal deep dive found independent stalls beyond the older LocalPty writable-edge fix. teletypewriter/src/windows/pipes.rs checked empty/full before acquiring condvar mutex; notifications could arrive before wait and disappear, stranding command bytes or output. Fixed predicate/queue/notification synchronization under same mutex; no mutex across blocking I/O. EOF/errors wake pollers and drain buffered output; idle readiness rearmed. Drop repeatedly cancels synchronous IO (cancellation is not sticky), joins up to one second then background reaper; tolerates worker failure/missing handle. spsc backing Rc -> Arc so cross-thread destruction/reaping is safe.

Validation: teletypewriter Windows suite under Wine 12 passed (8 new regressions); 10 repeat passes of all eight. Parent added neoism-terminal-pty/tests/windows_interactive.rs: 32 idle-separated cmd computed-output submissions interspersed with cls pass under Wine. Also contains native PowerShell5/pwsh7 raw and integrated clear/sleep/error tests, cross-compiled but not run (Windows PowerShell absent in Wine). Existing build-mac-win PTY --include-ignored lane will include these. No release build or CI dispatch performed.

Related fixes this run: see bug_windows_terminal_shell_identity.md (daemon shared PowerShell hook, actual shell metadata creation/attach/reconnect desktop/web/wasm, CR framing, lifecycle completion rules); bug_cmd_prompt_lifecycle_repaint.md (cmd prompt D/A separate row avoids echoed wrapped-input D replay); perf_windows_markdown_prefix_geometry.md (exact bounded caret-prefix cache plus skip solo presence FS queries). Native Windows GUI and actual Windows FPS still not measured. Wine binary /nix/store/6qyy46wlrz886xg9cxb7f80i8gn0y78f-wine-wow64-11.0/bin/wine; cargo-xwin installed in ~/.cargo/bin but not necessarily PATH. WINEPREFIX must be under user-owned parent, e.g. ~/.cache/neoism-windows-terminal-regression, not a not-yet-created immediate child of /tmp (Wine refuses).
