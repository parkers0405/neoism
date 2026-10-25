---
name: "Whole-window frame-time hot paths"
description: "Verified whole-app frame fixes: async Git/notes, cached LSP/roots, global pump budget, bounded LRU, nonblocking retained terminal+host paint, glyph/image GPU stalls; continuous effects intact; native/GPU/test-code+web cargo checks pass; WSI issue explicitly still open"
type: "perf"
scope: "project"
origin: "neoism-agent"
created: "2026-10-23"
updated: "2026-10-23"
---

# Whole-window frame-time hot paths

Follow-up to perf_agent_steady_frames.md. User says agent changes helped but whole app still hitches, including possible LSP/Git. Preserve continuous idle animations, shader/effect quality, rates and timing; never pause or disable them as a performance workaround. See feedback_preserve_continuous_animations.md.

## Implemented

- Chrome/workspace: save_current_workspace_chrome no longer canonicalizes/stats established roots each animation frame. Root snapshots update only on changes. BufferTabs and nested state derive exact PartialEq so unchanged snapshots avoid deep clones but real interaction/animation state persists. active_pane_workspace_root returns established document roots directly for code/Markdown/notebook LSP/blame refresh; explicit new-root validation remains.
- Git status: native per-window GitStatusCache (shared panels/git_branch/status_cache.rs) runs metadata probes on one worker, bounded16-job queue and256-path cache, one in-flight/path, stale-value retention, negative caching and completion wakes. Render getters never probe filesystem; warm requests do not allocate path keys. Explicit Git actions keep immediate semantics. Existing background numstat cadence remains via a root-known helper.
- LSP: CodePane owns private diagnostic summaries and cached counts; local/remote publishes use set_diagnostic_summaries, preserving full popup data. Status reads four counters, not all diagnostics. Exact active-file comparison bypasses repeated canonicalization. Blame pump exits before root/daemon setup with no code pane or when blame is disabled, without disabling any feature.
- Notes: filesystem listing/frontmatter/icons and cached-key sorting run off UI. One in-flight scan and coalesced follow-up; generation guards reject stale vault/source results. Same-vault listing remains while fetching; root switch clears old data. Apply preserves current expansion/selection/notebook state and live icons; borrowed UTF-8 map keys. Changed icon revisions apply current override INCLUDINGNone; pre-existing None does not wipe fresh JSON icons. select_after_refresh/cancel_selection_after_refresh preserves created-note targets across older scans, consumes once found, cancels on later navigation/pointer/focus/hide/source changes, rejects foreign/remote paths. Ordinary select_path misses never queue. Desktop create helper arms selection after intentional blur. Async enabled before initial AND incoming-workspace assignments; completions polled before render early-outs even hidden. Remote sources never scan local paths.
- Global daemon: one2ms inbound-apply budget per event-loop turn, reset only by new_events; one-message FIFO pops, round-robin active/parked sources, stable connection-object IDs and race-safe real-backlog continuation wakes. Removed unnecessary PTY clone. Outbound flush after input at about_to_wait. One apply/maintenance may overrun; same-connection receive-time reconnect-generation tags not newly implemented.
- Retained-cache LRU: replaced ever-growing stale-stamp VecDeque with existing lru::LruCache, one entry/resident chunk, preserving hot protection and recency.
- Terminal screen: Arc previous-good PanelFrame rows plus PanelFrameRef dynamic presentation. Busy snapshots/resize no longer abort entire window or omit cached panes. Cached damage Noop, consumed-but-unpresented damage deferred until fresh snapshot; no duplicate Full/Partial work. Blink/rainbow/focus/scroll/images/prompt/loaders/duration update on retained frames. Cached acknowledged grid dimensions prevent resize churn. Closed/replaced/rich-text/nonterminal identities prune caches. Bounded raw rows and image metadata captured under one successful try-lock; compose after releasing guard. Cached cwd fallback.
- Host paint: host/run.rs splash/scrollbar/damage/snapshot use try-locks; damage+snapshot share one guard. Busy paths preserve flags/damage/splash/reservations and request retry. host/composer.rs retains prompt/cwd metadata. TerminalPaintCache keyed(route,rich_text) stores metadata only, not duplicate rows. Overlay cursor uses paint-only nonblocking helpers; actual key routing unchanged. No blocking terminal lock remains in the reviewed ordinary paint chain. Input-routing locks, documentation-notebook mutex and command/executable-classification filesystem work remain outside scope.
- Vulkan glyphs: remove redundant other-slot CPU fence waits, preserve acquired-slot staging protection and both fragment-read -> copy-write and reverse same-queue GPU barriers. Glyph bytes/packing/shaders/layout unchanged.
- Vulkan images: whole texture/image/view/descriptor pool/set/staging retire through pending plus actual-acquired-slot bins, freed after covered submission completion. Covers deletion, size/background replacement/clear, reset and font-reload clearing. Eviction no device_wait_idle; shutdown/resize protections remain. Individual image upload one-shots still synchronous, batching deferred.

## Final verification

PASS: cargo check --locked -p neoism -p neoism-ui -p sugarloaf --tests --message-format=short . PASS: cargo check --locked -p neoism-terminal-wasm --target wasm32-unknown-unknown --features web --lib --message-format=short . Final combined check ran after host and notes regression fixes. git diff --check passed. Only compile checks: no app/release build, GUI manipulation or test execution in this pass.

Compiled tests cover bounded LRU, metadata coalescing/overload/stale roots, diagnostic replacement/clearing, tab animation equality, notes generations/icons/create selection/cancellation, global pump FIFO/fairness/budget/wake races, terminal contention/damage/blink/metadata and GPU image retirement/barrier contracts. Source reviews found no confirmed GPU/terminal regressions; notes review found icon-reset and create-selection bugs, both fixed before final checks.

No measured live p95/p99 or zero-jitter claim. Passive samples showed no overall CPU saturation/current memory pressure. perf/pidstat unavailable.

## Explicitly not solved

See bug_vulkan_presentation_lifetime.md. Existing WSI graphics-fence/presentation semaphore lifetime gaps remain. A shutdown-only unbounded swapchain-retention experiment was rejected and ALL its WSI changes/tests rolled back, not shipped. Correct bounded runtime presentation retirement needs proper completion support and GPU validation. Do not claim WSI fixed or resurrect unbounded retention/steady-frame idle waits. VulkanContext diff only removes unused other_slot_fences accessor, preserving original acquisition/deadline/WSI behavior.

Also remaining: synchronous image one-shots, indivisible expensive daemon/LSP applications, some explicit filesystem actions/input locks and actual GPU/runtime/presentation validation through the user's normal dev loop. Do not blindly remove synchronization or lower effects to chase FPS.
