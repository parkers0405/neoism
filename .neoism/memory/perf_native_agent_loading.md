---
name: "Native agent loading latency and instant-first-paint work"
description: "Immediate scoped loading/caches, recent transcript previews; classic Alt+A animation restored on one shared clock (no replay); 47 targeted tests + cargo check pass"
type: "perf"
scope: "project"
origin: "neoism-agent"
created: "2026-10-22"
updated: "2026-10-22"
---

# Native agent loading latency

Alt+A uses the native Rust renderer (`neoism-frontend/desktop`), not the React agent GUI or a Tauri wrapper. Do not fix only `sdk/typescript/packages/gui` for native chip/picker latency.

Confirmed avoidable dependency structure: `fetch_config_defaults` originally waited for config GET, providers GET, and optionally configured-models GET; model picker waited for configured-models and providers serially; every picker open refetched; each uncredentialed owned-local request separately probed health with a new runtime/client; labels ran an additional 320ms reveal after hydration.

Implemented: config defaults publish before discovery; fresh defaults seed synchronously across panes; process-wide bounded catalog cache (endpoint/credential/exact directory/resource, 30s fresh, 5m last-good, single flight, successful auth/config mutation invalidation); model fallback and provider readiness run independently even on initial hydration. CatalogScope captures endpoint/credential/directory/invalidation revision and rejects late replies after mutations/auth ABA. Defaults replies also protect pane generation, session metadata and intervening field choices. Discovery has a 15s background budget to avoid false disconnections on cold catalog/account discovery; no UI thread waits on it. Local readiness has 1s owned-local healthy lease, single flight, retained client/runtime and no write replay.

User follow-up explicitly requires the classic Alt+A chip animation to remain, without double playback (see feedback_agent_startup_animation_once.md). ConfigChipHydration now shares one requested_at clock between loading and 320ms Settling: cached defaults play one reveal; fast requests use only the remainder; slow requests already beyond320ms stop at completion without a second reveal. Repeated begin/completion/background replies do not restart an active animation. Existing-conversation draft cancellation and failures call cancel, not complete. No extra data-loading delay is introduced.

Conversation fixes: sidebar existing-chat opening discards queued draft defaults. Warm activation restores valid layout caches and dirty marks while retaining cross-family trace invalidation and rejecting transient expansion/prepend/merge geometry. Added process-only display-only initial history previews (`api/conversation_cache.rs`): 16 entries, 8MiB total keys/payload, 1MiB per entry, 30s TTL, normalized endpoint+credential+session scoped; auth changes clear snapshots and fence inflight responses. Only successful initial pages populate it. New empty/unhydrated panes paint cached history immediately but still fetch authoritative history. Pagination metadata is retained. Untouched preview messages are stripped before authoritative/live merges so deleted/shortened replies are not resurrected, while SSE edits/new parts survive. Preview-only parking never marks history authoritative. No disk transcript persistence.

Remaining boundary: authoritative history still hydrates artifact images sequentially before publishing text, each image request up to30s. Safe decoupling needs identity-scoped image-only patches, not whole-message replay. Preview cache omits downloaded image buffers. Cold first-process provider/ACP startup and genuinely uncached transcripts cannot be promised instant. No GUI end-to-end timing benchmark was run.

Final follow-up validation: `cargo check -p neoism --bin neoism` passed; 41 native readiness/catalog/defaults/conversation regressions passed; six shared startup animation tests passed including deterministic fast/slow/duplicate/cancel/timeout coverage. Native test compilation briefly hit a concurrent LSP missing-method edit; retry passed without changing LSP files. `git diff --check` passed. Existing unrelated warnings remain. Use `cargo test -p neoism --bin neoism`, NOT `--lib`, for native agent tests (desktop lib contains only small service modules). No release build. `NEOISM_AGENT_UI_PERF_LOG=1` logs composer defaults/model defaults and independent provider readiness durations.
