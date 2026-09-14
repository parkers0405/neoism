---
name: "Agent unavailable: stale hosted daemon retains shared database lock"
description: "piss-desktop .104 agent blocked by listenerless old hosted PID25408 holding shared Turso lock; local supervisor diagnostics + bounded runtime teardown; recovery awaits approval"
type: "bug"
scope: "project"
origin: "read-only SSH incident diagnosis"
created: "2026-09-14"
updated: "2026-09-14"
---

## Incident (2026-09-14)
User's piss-desktop Arch Linux 0.7.104 Alt+A agent unavailable. Read-only SSH diagnostics only; NO remote process/config/database mutations authorized or performed.

At 19:24 UTC host port 4096 has no persistent listener (`ss -ltnp 'sport = :4096'`); /v2/health on 127.0.0.1, localhost, ::1 all HTTP000. Embedded daemon PID2143331 binds 127.0.0.1:4096 every2s, exits ~30us after bound before `turso database opened`. Previous daemon PID2141894 was replaced externally during investigation.

Confirmed blocker: PID25408 `/home/parkersettle/.local/bin/neoism-workspace-daemon (deleted)` started Sep8, args `--addr 0.0.0.0:9878 --no-unix-socket --state-dir ~/.config/neoism/hosted-servers/9878 --workspace ~/Docker/Minecraft/neoism`. Holds POSIX whole-file WRITE locks on ~/.local/state/neoism/agent.turso.db and -wal. Read-only F_GETLK on exact DB returns (F_WRLCK, SEEK_SET,0,0,25408). Ownership/permissions normal. No TCP sockets owned by25408; remaining unix sockets + PTY/LSP threads. Main futex wait, three tokio workers hrtimer_nanosleep; consistent with runtime teardown blocked on lingering workers, not proven backtrace. stdout/stderr /dev/null so original shutdown reason unavailable.

Docker PID2141 /usr/local/bin/neoism-agent is DIFFERENT network namespace4026533175; host namespace4026531833. Do not kill Docker or infer port conflict from its command line. User manual19:22:53 AddrInUse is distinct; cannot retrospectively assign binder. Embedded short-lived binds can race manual launch.

## Local targeted fixes
neoism-workspace-daemon/src/agent.rs: preserve nested anyhow source chains and JoinError (panic/cancel) for startup and post-ready exits instead of Ok(Some(_))/let _=task.await. Regression tests plus positive existing-health reuse both credential-store spellings. Existing same-endpoint supervisor remains unchanged.
neoism-agent/crates/neoism-agent-server/src/lib.rs: bind vs post-bind state context.
neoism-workspace-daemon/src/main.rs: runtime.shutdown_timeout(2s) after run returns/persistence flush instead of indefinite runtime Drop, so dedicated process releases DB on shutdown. No per-window ports or stores: user explicitly requires SAME AGENT URL across all things.

## Recovery pending approval
Asked permission to terminate ONLY stale PID25408 (may end lingering Minecraft-workspace PTYs), SIGTERM first; ask before SIGKILL if needed. Current desktop supervisor should retry unchanged DB/4096 automatically once lock released. Do not reset/delete DB/WAL. Local changes not remotely deployed. Check/test jobs running as of note creation; do not claim passed until collected. User GUI files have many unrelated changes; left untouched.
