---
name: "Background completion delivered but GUI resurrects old running jobs"
description: "FIXED worktree: runtime empty overwritten by historical launch scan; old .96 monitor ghost, latest checks DID notify parent; shared authority + family/reconnect races + regressions"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-09-08"
updated: "2026-09-08"
---

## Background shell completion delivered, but GUI resurrects historical running job — fixed in worktree

User explicitly authorized runtime/agent UI fix only: NO release, commit, push, or replacement of running application. Preserve large shared-file/LSP/Git worktree.

### Concrete evidence
- Live `/v2/sessions/ses_f8b5f3c30ffetvxBL4jkdlhgAi/runtime` initially returned `runningBackgroundTasks: []`, branches empty, backend 0.7.96.
- Latest job `job_07e60c21b0011057jStjdMhO1t` (final shared files/LSP/Git combined checks) completed exit 0. Persisted `msg_background_completion_job_07e60c21b0011057jStjdMhO1t` exists and parent received/reported completion. NOT missing delivery for this job.
- `/v2/sessions/:id/context` selected operational tool metadata / visible notification headers only. Unmatched historical launch `job_07cee06f3001sWyAZrsDWxC1rG`, title "Monitor remaining Windows release job with resilient status polling", still metadata status running but absent from live in-memory jobs; no corresponding persisted completion. Do not infer its final exit status.

### Reproduction/root cause
1. History contains old unmatched running launch A.
2. Runtime gives running B then authoritative empty revision N when B finishes.
3. B completion card upsert calls desktop `ensure_background_task_activity_clock` / shared `refresh_background_task_activity_clock`, rescanning immutable history and overwriting count with A=1.
4. Same revision N recovery snapshot is rejected, leaving ghost running status/title.
`background_authority_survives_completion_and_delayed_launch_parts` FAILED before fix (actual 1 expected 0), passes after in both panes.

### Fix
- New shared `panels/agent_pane/background_runtime.rs::BackgroundTaskAuthority`: authoritative presence even for empty lists; monotonic per-epoch revisions; retired epochs reject late responses from previously replaced server; live job ID set.
- Desktop/shared transcript refresh/upsert cannot replace authoritative live count. Summary titles derive only from live IDs (child jobs lacking local launch get ID fallback), not historical unmatched jobs.
- Replace old pane/cache epoch+revision pair with authority; preserve it with counts/timer during cached same-family root/child switches. Unrelated families retain separate cached state.
- Desktop ingest: accept versioned jobs from latest matching-session hydration before unrelated execution revision guard. SSE reconnect requests runtime; its subsequent idle/status event used to invalidate the whole fetched response.
- WASM catalog active/cache paths: stale branch snapshot cannot discard independently versioned background jobs for an already established family.
- Backend `publish_background_jobs_updated`: same all-workspace `running_jobs_for_family` source as v2 runtime endpoint, emit family root as sessionID. Previously only inspected completing/launching job's workspace and addressed child; cross-worktree sibling jobs could be omitted / root routing depend on child roster.
- Completion enqueue/durable notification semantics unchanged; tests verify one parent notification, one completion event, collection no duplicate, visible mapped completion card no duplication.

### Changed files
`neoism-agent/crates/neoism-agent-server/src/{background_job.rs,tests_interaction_tools.rs}`;
`neoism-frontend/desktop/src/neoism/agent/{commands.rs,pane.rs,pane/ingest.rs,pane/render_state.rs,pane/session.rs,pane/tests.rs}`;
`neoism-frontend/shared/src/panels/agent_pane/{background_runtime.rs,mod.rs,state.rs,state/caches.rs,state/hit_rects.rs,state/session_cache.rs,state/tests.rs,stream_events.rs}`;
`neoism-frontend/wasm/src/rendered/catalog.rs`;
`neoism-workspace-daemon/src/agent/events.rs` (proxy regression only).
Unrelated formatter churn removed from preexisting tests/session code.

### Validation
- `cargo test -p neoism-ui --lib background_`: 29 pass.
- `cargo test -p neoism --bin neoism background_`: 19 pass.
- `cargo test -p neoism-agent-server --lib background_`: 6 pass.
- `cargo test -p neoism-workspace-daemon --lib background_runtime`: 2 pass.
- Exact reproduction rerun in both UI crates after adding live-title assertion: passes.
- `env -u CI cargo check -p neoism -p neoism-ui -p neoism-workspace-daemon -p neoism-agent-server --tests`: pass.
- WASM `cargo check -p neoism-terminal-wasm --target wasm32-unknown-unknown`: pass.
- Windows `cargo xwin check --target x86_64-pc-windows-msvc -p neoism -p neoism-ui -p neoism-workspace-daemon` production: pass; `-p neoism-ui --tests`: pass.
- Full Windows desktop `--tests` blocked by existing `windows::platform_key_bindings` cfg(not(test)), `DaemonEndpoint::Unix` references in desktop context/manager/test.rs + daemon_client/mod.rs.
- Full Windows daemon `--tests` blocked by existing `Permissions::mode` in daemon_token.rs and Unix shell helpers in sessions.rs. Left untouched.
- Logs `/tmp/neoism-background-windows-{check,production,shared-tests,ui-tests}.log`.
- No native Windows GUI execution. No release builds/commits/pushes. Installed app still old binary; fix only checked/tested worktree.
