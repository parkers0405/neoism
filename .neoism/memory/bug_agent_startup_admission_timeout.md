---
name: "Agent startup and five-second prompt admission timeout hardening"
description: "Removed five-second admission failures; pooled HTTP, durable queue handoff, async GUI interactions, bounded sidebar/navigation; verification and residual gaps"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-19"
updated: "2026-10-19"
---

# Agent startup / HTTP admission / Rust GUI hardening

The desktop error `Neoism request timed out waiting for the server response` originated in the old raw HTTP read-to-EOF path with a 5s timeout, NOT provider retries. Fixed desktop regular transport with pooled reqwest on a dedicated Tokio runtime (transport.rs): local/remote pools, loopback no_proxy, response framing +32MiB cap, no redirects/implicit POST retries, bounded GET retry. api.rs session-create/prompt admission >=120s, default reads15s. Desktop agent_server.rs readiness120s with strict bounded health probes; remote temporary503 retry never acquires local owner.

Embedded daemon uses asynchronous cached Tailscale discovery; late bind advertises via WorkspaceManager::set_local_daemon_url, no CLI startup gate. Daemon shared agent pools separate from provider client; read30s bounded retry, mutations120s no blind replay, SSE no total lifetime. Joined proxy waits local readiness before work, mints60s credential AFTER wait, unknown mutation failures no Retry-After/replay suggestion.

Server session creation offloads filesystem+Git, bounded2s/command64KiB subprocess capture/reaping, discovery singleflight5s TTL, tenant/root plugin init singleflight. Durable prompt queue now owns commit->wake->spawn in detached admission; peeks queue not destructive pop, saved_message_id marker atomic with user turn, durable run+queue-token handoff before model/tools. Exact duplicate wakes missing worker. Initialization failures retained; parked work requires later wake/restart. Post-handoff execution intentionally not replayed.

Desktop send captures origin immediately and dispatches independent of paint. Created session identity published before POST. Reconcile failed response by exact ID; retain uncertain payload; native /retry same ID/body after reconcile (not generic POST retry). Offthread pin/rename/delete/preferences/interactions. Per-pane preload2 active with latest selection queue replacement. First SSE baseline reconciliation. Sidebar batched updates + shared ordered mutation API + bounded retry epochs/reopen recovery.

Verification: production cargo check four packages PASS; desktop test compilation PASS. Direct compiled desktop binary regression groups responsiveness4/readiness8/transport12 PASS; server queue19/coordinator3/manualcompaction12 PASS; shared sidebar152 PASS; project6/runtime30/sessioncreate4 PASS. Broader desktop pane296 had286pass10fail; one migrated paste-echo assertion corrected to compact token, nine other layout/picker/disclosure/snapshot/animation assertions remain not baseline-proven preexisting. Daemon --tests hits existing remote_pty_io nonexhaustive protocol match. Proxy-auth suite ambient auth-policy assumptions inconsistent. Audit docs/agent-startup-responsiveness.md details changes/boundaries. No release build, Windows testing explicitly skipped by user.

Remaining boundaries: legacy sync directory/slash helpers, uncoalesced independent preference PATCHes, no process-global mutation bound, uncertain payloads in-memory/no bytecap/outbox/retrybutton, external-provider /retry routing not verified, no delayed-SSE end-to-end test, per-key history cloning remains. Do not claim universal instant startup or all tests green.
