---
name: "Host terminal no-output spinning timer — fixed"
description: "Host daemon global PTY fan-out could drop a command's output and OSC D under unrelated noisy sessions; fixed with per-socket PTY subscriptions and corrected false SSH badge."
type: "bug"
scope: "project"
origin: "implementation 2026-09-19"
created: "2026-09-19"
updated: "2026-09-19"
---

# Host-owned terminal command output/timer loss

## Symptom
A daemon-backed terminal on the host accepted `ls`, showed no output, and left its block timer spinning. Composer misleadingly displayed `SSH` for a loopback hosted-server workspace. Reproduced symptom affected both current dev and installed production clients because they shared the same external workspace daemon.

## Evidence
- Runtime process inspection showed daemon zsh PTYs alive and idle at their prompts, not stuck in `ls`.
- `~/.zsh_history` recorded the incident-time `ls` (`1789828399`, duration 0), proving input reached and completed in the real shell.
- Live generated zsh wrappers correctly emit OSC 133 C and D/A/B, so the shell integration itself was present.
- The daemon used one bounded process-global PTY broadcast for every live PTY and serialized every PTY's output to every websocket, even though clients discard sessions they do not own. Live daemon PTYs included old/background sessions with multi-GB output accounting. A noisy unrelated session can lag a websocket's bounded receiver; losing the fast `ls` chunk also loses OSC 133;D, exactly producing no output plus a forever-running client block.

## Fix
- `neoism-workspace-daemon/src/server/socket.rs`: each websocket now tracks PTY sessions it explicitly creates, attaches, or addresses, and only forwards live session-scoped PTY broadcasts for those subscriptions. Initial retained backlog discovery remains unchanged. Subscription is installed before handling input to avoid losing output from a fast command that responds before Ack.
- Added focused subscription test and preserved the two-client shared-PTY bidirectional integration test.
- `neoism-frontend/desktop/src/host/composer.rs`: show `SSH` only for actual Quick-SSH workspaces, not generic remote/daemon PTYs such as a host-owned loopback server.
- Preserved the unrelated uncommitted endpoint ownership fixes in app/mod.rs, daemon_sessions.rs, ingest.rs, and tests.

## Verification
- `cargo check -p neoism -p neoism-workspace-daemon` passes (pre-existing warnings only).
- `cargo test -p neoism-workspace-daemon pty_subscription_tests --lib` passes.
- `cargo test -p neoism-workspace-daemon --test remote_pty_io two_clients_share_one_live_pty_bidirectional` passes.
- `git diff --check` passes.

## Runtime uncertainty
No running daemon/client was restarted or altered, so the existing binaries cannot exercise the patch. The daemon log file did not contain the hosted daemon's stdout lag warning; the diagnosis is based on confirmed shell completion, intact OSC wrapper, shared external daemon impact, high-output sibling PTYs, and the exact bounded global fan-out/drop path.
