---
name: "Restart-free active-browser attachment"
description: "Restart-free computer.browser_attach binds an authorized active debug-enabled browser by native process identity; Linux verifies explicit flag and loopback socket ownership"
type: "feature"
scope: "project"
origin: "Parent review follow-up"
created: "2026-09-18"
updated: "2026-09-18"
---

Follow-up hardening completed after review:
- Linux discovery no longer reads cmdline/exe/fds for every `/proc` process. The global bounded pass retains only PID/PPID/start metadata. Cmdline is read with `File::take(64KiB+1)` only along the selected PID ancestry and cleared for non-flagged entries; traversal stops at the first explicit debugging flag. Executable names and up to 1024 fds are inspected only for the flagged browser root and descendants, with deadline/cancellation checks inside the fd loop.
- Runtime binding mutex guard is dropped before all network work. Already-attached calls validate process/listener lifetime before tab access.
- Binding validation now distinguishes attachment lifetime failures from wrong-target errors: actual process/listener death clears binding and observations; a different native target rejects without discarding a healthy attachment.
- Initial and already-attached tab/protocol failures return structured native fallback when the operation remains admitted. Cancellation/deadline remains an error. Any created owned BiDi connection remains cached for explicit browser_disconnect cleanup; foreign sessions are still never ended.
- Added tests proving wrong-target preservation vs dead-listener invalidation and owned BiDi retention after tab failure, alongside existing explicit-flag/loopback family ownership and foreign-session tests.
Verification after follow-up: browser tests 10 passed, 1 opt-in live test ignored; `cargo check -p neoism-agent-server` passed with existing unrelated warnings. No user browser/environment touched and no release build.
