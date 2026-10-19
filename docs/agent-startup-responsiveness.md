# Agent startup and interaction latency

## Root cause

The desktop's `Neoism request timed out waiting for the server response` error came from HTTP admission, not provider/model retries. Requests had a five-second socket read timeout. Session creation could synchronously run Git discovery and cold plugin initialization; prompt admission could wait for database serialization and busy retries. The server could still be processing an operation after the GUI declared failure.

Readiness budgets also disagreed: desktop requests allowed eight seconds, daemon envelopes nominally allowed 1.5 seconds, and the service supervisor allowed 120 seconds. The daemon's nominal budget did not bound a health request that never returned. Optional Tailscale CLI discovery delayed launching the local agent. Increasing provider retries could not fix these paths.

The GUI had additional latency/correctness problems: synchronous HTTP in pin/rename/delete handlers, prompt ownership captured during a later frame rather than submission, selected history loads bypassing preload concurrency limits, no first-SSE baseline reconciliation, and repeated sidebar rebuilding during catalog bursts.

## Implemented startup and transport changes

- Desktop and daemon readiness now allow the existing 120-second service cold-start window on background workers. Individual probes validate the agent health contract and have a short bounded deadline and body size. Temporary hosted/proxy responses retry without acquiring a local process owner.
- Tailscale startup uses asynchronous cached discovery. A late listener is bound and advertised after bootstrap, without delaying local service startup.
- Session creation offloads filesystem canonicalization and Git discovery from async workers. Git commands have two-second per-command deadlines and 64 KiB output limits, with child termination/reaping. Workspace discovery is cached/single-flight for five seconds, and independent discovery/plugin initialization overlap.
- Plugin runtime initialization is single-flight per tenant/workspace root. Plugin validation and workspace admission checks are retained.
- Regular desktop requests use pooled reqwest clients on a dedicated runtime. Session creation and prompt admission receive at least 120 seconds for the wire operation; ordinary requests default to 15 seconds. HTTP framing replaces EOF-based completion, response bodies are capped at 32 MiB, URL parsing supports IPv6/base paths, and diagnostic errors omit credentials.
- Desktop GETs receive one bounded transient retry. Redirects and implicit mutation replay are disabled. Local clients bypass environment proxies; remote clients retain proxy support. SSE retains its separate polling/cancellation behavior and is not given an ordinary request lifetime limit.
- Daemon agent/proxy traffic shares reusable agent pools, separate from provider traffic. Reads have a 30-second budget with bounded retries; mutations have 120 seconds without blind replay. Proxy header waits are bounded without killing healthy SSE.
- Pre-delivery proxy failures carry `Retry-After`; ambiguous mutation failures report unknown delivery without suggesting automatic replay. Upstream credentials are minted after readiness, preventing their 60-second lifetime from expiring during cold startup.

## Durable prompt admission

Admission owns commit, worker wake and immediate scheduling independently of HTTP caller cancellation. Exact duplicate queued requests wake a missing worker. Queue/status projections cannot turn a committed admission into rejection or strand worker ownership.

Workers retain queue rows during initialization. A `saved_message_id` marker commits atomically with the durable user message. A retained row can recover failed post-save initialization after restart. Handoff confirms the durable run record and removes the recovery token atomically before provider/model/tool work. Fresh duplicate submissions cannot use that token to replay handed-off work. Visible queue cancellation serializes with initialization.

After handoff, interrupted model/tool work is intentionally not automatically replayed: side effects cannot generally be guaranteed exactly once. Store/initialization failures can park retained work until another wake or restart; this is not an unlimited automatic recovery loop. Nonqueued direct admission and external ACP lifecycle recovery were not redesigned.

## Rust GUI changes

- Prompt submission captures endpoint, session/draft, directory, source, options, model, payload and message ID immediately and starts background delivery without requiring a visible frame.
- Session creation publishes its committed identity before prompt admission. Failures retain that identity and the request payload instead of silently creating another chat.
- A failed prompt response performs read-only exact-message reconciliation. Unconfirmed delivery is retained as uncertain. Explicit native `/retry` reconciles first and preserves the original endpoint/session/message ID/payload; it is not generic POST retry.
- Queued-send failure rolls back optimistic queue feedback without stopping the active turn. Completions are endpoint-scoped so they cannot attach to another host's matching session ID.
- Pin/rename/delete, session preference PATCHes, permission/question replies and preference persistence run off-thread. Session mutations are single-flight per owning pane/session, and catalog completions drain even when the sidebar is closed.
- History preloads remain bounded to two in flight per pane; latest selected navigation replaces obsolete queued preloads. Cache/placeholder activation remains immediate.
- First SSE connection now performs baseline reconciliation, as reconnects already did.
- Desktop catalog upserts batch through the shared batch API. Shared ordered mutation support rebuilds once and preserves selection/viewport identity. Catalog/project-page failures have bounded backoff and explicit retry epochs; reopening the sidebar can retry exhausted discovery without discarding cached rows.

## Follow-up: retained admission blocked execution rollover

The first implementation introduced a real regression: the new recovery row remained in `prompt_queue` while the incoming turn initialized, but execution rollover's SQL still treated every queue row as competing old work. After one finished reply, the next prompt could reject itself with `execution ... still has active work`, retain that unexecutable row, and leave the UI busy. Rust quiescence already exempted the admitting session; both SQL settlement and rollover now carry that same narrowly scoped exemption. Active runs, provider segments, subtasks and other family sessions still veto rollover.

The first implementation also delayed `dequeue` until the full assistant reply returned. It now emits at durable run handoff, before model/title/provider work. Later completion updates are snapshots rather than a duplicate dequeue. Acknowledged no-reply/duplicate admissions emit at row retirement too. The recovery row is not deleted early to hide either bug.

Two public-v2-route stub-provider regressions now cover a second distinct prompt in the same settled conversation and a two-prompt burst after settlement, asserting real replies, execution rollover, empty queue, released run/worker ownership, and dequeue before the first provider text delta. The full queue group passes 21 tests. The execution group passed 16 of 17 in one run; the reopen-quiescence fixture's assertion that background recovery has not already finished failed and is not represented as passing.

## Verification and remaining boundaries

Earlier verification (historical, not a full-suite result): production `cargo check` passed for `neoism`, `neoism-ui`, `neoism-workspace-daemon` and `neoism-agent-server`. Desktop test compilation also passed. Targeted tests passed for shared sidebar behavior (152), session queue behavior (19), coordinator behavior (3), manual compaction behavior (12), project discovery (6), workspace runtimes (30), session creation/directory behavior (4), desktop responsiveness (4), isolated HTTP transport and framing, and bounded daemon readiness/retry behavior.

The earlier sampled 296-test desktop pane run had nine failures after correcting a migrated compact-paste assertion. Those failures were not baseline-proven pre-existing and were not represented as passing. Its daemon test compilation also stopped at a nonexhaustive `remote_pty_io` protocol match, and proxy-auth tests assumed inconsistent ambient auth policy. The focused pre-132 recheck below supersedes those specific observations, not the historical queue/execution results above.

### Focused pre-132 recheck

These checks exercised the version-132 working tree initially based on `6ceb0e44c`; they are not full desktop/daemon suite or release validation. The initial verification pass changed only tests and documentation. The follow-up includes the separately implemented empty-snapshot production correction described below; default user updates and animation timing bounds were not changed.

| Command/filter | Actual outcome |
| --- | --- |
| `cargo test -p neoism --bin neoism responsiveness_tests` | 4 passed |
| `cargo test -p neoism --bin neoism request_tests` | 12 passed |
| `cargo test -p neoism-workspace-daemon readiness_wait_tests` | 3 passed |
| `cargo test -p neoism-workspace-daemon agent_proxy_failure_tests` | 1 passed |
| `cargo test -p neoism-workspace-daemon search_error_tests` | 1 passed |
| `cargo test -p neoism-workspace-daemon --test remote_pty_io --no-run` | Compiled; PTY tests were not executed |
| `cargo test -q -p neoism-workspace-daemon --lib agent_proxy_auth_tests` | 9 passed with default parallel execution and again with `-- --test-threads=1` after explicit test auth-policy setup (previously 7 passed, 2 failed in serial execution) |
| `cargo test -q -p neoism --bin neoism neoism::agent::pane::tests` | Final follow-up: 298 passed, 0 failed (earlier stages: 289/9, then 291/7, then 297/1 before explicitly marking the retry fixture) |
| Exact filters for each of the six reconciled pane tests below | Each passed independently: 1 passed, 0 failed per command |
| `cargo test -q -p neoism --bin neoism neoism::agent::pane::tests::live_baseline_replaces_cached_text_before_following_deltas -- --exact` | 1 passed, 0 failed after explicit retry setup |
| `cargo test -q -p neoism --bin neoism late_empty_tests` | 4 passed, 0 failed; separate ingest-nested module, not included in the 298 above |

Test-only corrections: the PTY helper now handles both terminal-directory result variants and still asserts the shared session identity; the two provider-picker assertions now expect the existing `__external_config:<id>` values while retaining selection/commit behavior assertions; auth-required fixtures explicitly enable mandatory auth and the trust-local fixture explicitly disables it, restoring prior environment values afterward. A signing key alone intentionally does not require auth.

The follow-up reconciled six stale fixture expectations against the current implementations, without production edits for those six cases. Each passed using `cargo test -q -p neoism --bin neoism neoism::agent::pane::tests::<name> -- --exact`:

- `clicking_the_active_detail_child_returns_to_main`: the fixture populated the detail rail but omitted `SidePanelMode::Subagents`, which the renderer installs before interaction. Explicitly selecting that mode restores the existing active-child-to-root navigation assertion without changing global/project catalogs or subagent behavior.
- `diff_file_toggle_keeps_position_and_anchors_reader_owned_remeasurement` (formerly `diff_file_toggle_does_not_move_or_reanchor_the_timeline`): diff-section expansion now intentionally captures reader-owned header anchors and cancels inertia, like other tool expansion. The test keeps the unchanged-scroll assertion and additionally checks zero velocity, disabled follow-bottom, exact content/screen anchor coordinates and an instant, nonanimated diff toggle.
- `imported_history_preload_discloses_text_only_without_changing_title_or_source`: external sessions now retain an authoritative, timeline-hidden `todos-snapshot` system row. The test asserts the exact imported reply plus that empty TODO marker, rather than treating every stored row as a visible transcript card; title, provider, disclosure metadata and cache isolation assertions remain.
- `read_group_expansion_tracks_source_and_remeasures_after_animation`: known source rows are remeasured through dirty source indices, not dirty message IDs. The test asserts source index 0 after settling, retaining the original animation deadline, both expansion directions and settled progress checks.
- `tab_mode_switch_rearms_agent_chip_transition`: the newer status-chip activation wave has priority over the concurrent agent-label transition. The test verifies both the activation reason/chip identity and the label reason after advancing only the activation clock to its existing 280 ms boundary.
- `tool_expansion_settled_and_stable_frames_keep_the_clicked_header`: the same dirty-source-index contract applies to ordinary/grouped tools. The test checks source index 0 and retains all clipped/unclipped header-anchor and stable-frame assertions, without changing its animation deadline.

The production empty-snapshot bug was corrected separately in `pane/ingest.rs`: normal late empty snapshots must preserve streamed text, while an explicitly armed retry may clear the previous attempt. The original `late_empty_snapshot_without_retry_never_regresses_streamed_text` now passes in the 298-test group. The related `live_baseline_replaces_cached_text_before_following_deltas` fixture now explicitly arms and verifies consumption of `retry_reset_pending` before its deliberate retry; its normal nonempty-baseline shrink and subsequent delta assertions were preserved. Four additional tests in `pane/late_empty_tests.rs` passed through the actual ingest-nested module filter; together with the original test, this is five-test late-empty coverage, not five newly added tests.

The documented pane test blockers are resolved by the actual passing runs above. No blanket timing increases or full-suite claim is made. Queue, execution-reopen and server conversation-search verification remain separate checks and were not rerun or modified by this pass. Overall release clearance remains the parent's decision across those checks.

Remaining GUI boundaries include legacy synchronous directory/slash helpers, non-coalesced independent preference PATCH workers, per-pane rather than process-global concurrency bounds, in-memory uncertain delivery without an outbox/byte bound/retry-button UI, external-provider `/retry` routing, and input-history cloning. First-subscription reconciliation has unit coverage around reconnect behavior but not a full delayed-SSE/instant-provider integration test. No end-to-end startup latency benchmark was performed. Windows runtime testing was explicitly skipped at the user's request.

The implemented responsiveness contract is immediate local feedback and recoverable background work, not instantaneous completion of disk migration, external process startup or provider requests. The removed five-second first-prompt failure must not be confused with a guarantee that every external dependency is always available.
