---
name: "Windows terminal + agent startup failures — FIXED"
description: "Windows ConPTY stale-build command stall and daemon/agent startup race fixed; includes root causes, files, and validation"
type: "bug"
scope: "project"
origin: "interactive investigation and implementation"
created: "2026-08-01"
updated: "2026-08-01"
---

# Windows terminal command stall and `/connect` startup timeout

Diagnosed and fixed 2026-08-01.

## Terminal

A Windows ConPTY command could remain queued forever when its input pipe was already writable before edge-triggered poll interest was enabled; no later writable edge arrived. Fixed previously in commit `91f126968` / v0.7.66 by flushing writes immediately after draining worker commands (`neoism-terminal-pty/src/local.rs`). Therefore an affected Windows install older than v0.7.66 or built from stale sources shows exactly: command block timer starts, but no output/completion. Current v0.7.84 source contains the fix. Additional hardening added: `LocalPty::write_reply` now uses a 5-second `recv_timeout` rather than waiting forever while the terminal parser lock is held.

## Agent `/connect`

Desktop embedded-daemon readiness was signaled as soon as port 7878 bound, before workspace bootstrap and before `ensure_agent_server_started_with_services`. A newly added synchronous `tailscale ip -4` startup probe can take ~2 seconds on machines without a responsive tailscaled. The frontend then waited only 1.5 seconds for `/v2/health` and gave the actual request a 900 ms read timeout, accounting exactly for the reported 2-3 second timeout. Also daemon endpoint resolution preferred `NEOISM_AGENT_SERVER`, while desktop only used `NEOISM_SERVER`, permitting split endpoints.

Fixes:
- `embedded_daemon.rs`: send ready only after the daemon-owned agent supervisor has been launched, preserving the existing early Tailscale host advertisement.
- `agent_server.rs`: cold-start health budget is 8 seconds; canonical endpoint prefers `NEOISM_AGENT_SERVER`; publish both endpoint env vars.
- `neoism/agent/api.rs`: ordinary JSON request read timeout increased from 900 ms to 5 seconds for cold store migrations/catalog loads.
- `neoism-workspace-daemon/src/agent.rs`: publish both endpoint env vars.

Verification: native `cargo check -p neoism-terminal-pty` and `cargo check -p neoism-workspace-daemon` passed. Windows cross-check `env -u CI cargo xwin check --target x86_64-pc-windows-msvc -p neoism -p neoism-workspace-daemon` passed. `git diff --check` passed. Real Windows hardware validation remains needed, and affected machines should be updated to a build containing v0.7.66+ for the terminal fix.
