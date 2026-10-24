# Changelog

All notable user-facing changes to Neoism are documented here.

## [0.7.138] - 2026-10-24

### Model discovery and scaled text correction

- Includes the model discovery, offline picker recovery, rendering caches, question navigation, and license packaging changes prepared for 0.7.137.
- Fixes scaled canvas text sampling outside its glyph allocation on CPU, Vulkan, WGSL, and Metal, preventing stale or neighboring atlas pixels from contaminating glyph edges.
- Adds grayscale/RGBA contaminated-padding fixtures and strengthens font invalidation coverage with live-raster and final-pixel comparisons.

The 0.7.137 production run was canceled before publication after its renderer regression test failed. Its tag remains unchanged; this release carries the correction and is published only after the complete platform matrix passes. No live GPU or affected-machine Windows runtime validation is claimed.

## [0.7.137] - 2026-10-24

### Model discovery and text rendering

- Expands bundled model metadata and refreshes discovery in the background with bounded retries, preserving successful in-memory results when disk-cache writes fail.
- Updates open model pickers as discovery completes without losing search or selection; keeps useful offline options, bounds hydration waits, and isolates account-authoritative Codex models from API-key and other-account catalogs.
- Adds bounded, collision-checked text shaping and font-aware measurement caches, retained caret geometry, cached diff measurements, horizontal token seeking, and offscreen diff-card culling.
- Highlights the latest question in the navigation rail at the live bottom edge.
- Includes regression fixtures, model catalog provenance, and distributed license notices.

This is a stable production release, available through plain `neoism update` after all platform builds and packaging checks pass. Continuous animations and the existing Servo/CEF worker exclusions are preserved. Slow-network Windows behavior still requires affected-machine runtime verification.

## [0.7.136] - 2026-10-23

### Chat hydration, provider readiness, and frame responsiveness

- Improves chat opening with incremental/cached history, bounded ingestion work, background generated-image hydration, and shared attachment payloads.
- Adds bundled offline provider/model metadata and makes optional discovery nonblocking for connection UI. Keeps warm account-scoped readiness metadata through bounded refresh failures while preserving per-model Codex maxima and effective input budgets.
- Adds capability-verified machine-wide Agent target selection, generation-scoped remote catalogs, explicit activation settings, and reconnect handling without automatic prompt replay.
- Adds a question navigation rail and refines image-preview/modal behavior, composer wrapping/layout, and account-selection reconciliation.
- Presents previous-good terminal frames during lock contention instead of blocking unrelated UI; moves Git/notes work off the critical path and improves cached diagnostic and bounded LRU bookkeeping.
- Defers Vulkan texture retirement until acquired-slot fence reuse and uses GPU-ordered atlas uploads rather than ordinary-path idle/cross-slot CPU waits, preserving teardown synchronization.

This is a stable production release, available through plain `neoism update` after all platform builds and packaging checks pass. Visible continuous animations remain enabled. Existing first-terminal behavior, scoped authorization, trusted-user hosting limits, and Servo/CEF worker exclusions are preserved.

## [0.7.135] - 2026-10-23

### Pull requests, systems management, and Agent presentation

- Adds GitHub pull-request discovery, creation/opening, immutable comparison tabs, Changes/Conversation/Checks views, review drafts, comments/approvals/change requests, thread replies/resolution, and viewer-side browser links.
- Adds a repository-scoped paginated PR inbox with search, labels, state/sort filters, relationship grouping, responsive detail layout, and retained drafts.
- Adds explicit current-workspace or managed-worktree checkout with repository/snapshot verification and authoritative document checks; dirty or collaborative documents refuse unsafe transitions. Checkout is not an untrusted-repository sandbox.
- Adds muted, noninteractive PR loading skeletons while retaining meaningful errors/empty states. PR detail headers and tabs scroll away so Code/Timeline can use the full detail viewport, with clipped input routing and review-sheet scroll ownership.
- Adds native Systems management for saved SSH recipes and connection lifecycle, cancellation/generation guards, shared tunnels, and palette navigation. Browser clients report desktop-only management.
- Refines the floating composer, inset tray, wrapping, snapped seams, silhouette-aware painting/hit regions, timeline scrollbar, and browser drag ownership. Adds deferred inverse clipping while preserving preceding render output and atlas/UV metadata.
- Improves provider readiness caching, history-operation serialization, and undo deletion/search cleanup. Uses proportional Geist for PR presentation and updates provider artwork with attribution notices.
- Includes PR documentation and the intentional removal of the source-only typesafe-AI skill files.

This is a stable production release, available through plain `neoism update` after the full platform matrix passes. Servo and CEF workers remain separate experimental components and are not packaged.

## [0.7.134] - 2026-10-22

### Git review, editor lifecycle, and LSP navigation

- Adds native/shared/browser Git review tabs, canonical inline editing, projected source/carets, precise block stage/discard, full-file baselines, commit review, source navigation, and toolbar/gutter/layout improvements.
- Preserves canonical document ownership through review/close/save transitions and reconciles CRDT restores against remembered baselines. Exact block actions reject stale or unsupported selections instead of falling back to whole-file mutation.
- Serializes daemon Git operations asynchronously with bounded admission and scope-aware protocol handling. Browser Git refresh follow-ups retain their captured workspace and reject retired-context replies.
- Improves rich LSP hover presentation with bundled Geist prose, selection/copy and safe links, declaration/type-definition navigation, negotiated capabilities, asynchronous discovery/caching, and exact-position queries.
- Improves Agent catalog/conversation caching, credential/scope invalidation, initial-history previews, preload reconciliation, loading presentation, and activity handling.
- Refines finder behavior, tab/focus/scroll interactions, overlay occlusion, Git review geometry, and first-terminal measured startup layout. Includes the Geist font and its license attribution.

This is a stable production release, available through plain `neoism update` after platform builds and packaging checks pass. Existing first-terminal directory following, trusted-user hosting limits, and Servo/CEF worker exclusions are preserved.

## [0.7.133] - 2026-10-20

### Windows startup, provider connection, and search scheduling

- Preserves the initial optimistic prompt through Windows session hydration/binding, fences pre-admission idle snapshots, and corrects queue ownership/busy projections. Aligns Windows daemon signing-key location with desktop/Agent conventions.
- Adds provider-connection progress, browser fallback, explicit retry, cancellation and late-result suppression, secret clearing, transport-failure settlement, and account/model reconciliation across native/shared hosts.
- Correlates catalog requests and failures across the protocol, daemon, WASM, and browser. Both HTTP failure paths now terminate their correlated request while retaining legacy uncorrelated forms.
- Improves conversation-search eligibility lookup, ancestry reuse, bounded cursor work, and batched payload hydration without dropping canonical authorization. Removes the fixed two-second search deadline while retaining admission and work caps; idle index optimization runs off-thread.
- Bounds native navigator fetch concurrency and pending-query replacement, with debounce, stale-generation rejection, and interruptible retry.
- Uses shared branded provider callback pages with escaped failure text and accurate authorization-received wording rather than claiming an unverified connection.

This is a stable production release, available through plain `neoism update` after the platform matrix passes. Existing first-terminal behavior, trusted-user hosting boundaries, and Servo/CEF worker exclusions are preserved.

## [0.7.132] - 2026-10-19

### Agent startup, request handling, and queue recovery

- Improves cold-start readiness waits and health validation while moving optional Tailscale discovery off the local-startup critical path.
- Uses pooled desktop HTTP clients on a dedicated runtime, bounded responses, request deadlines, and HTTP message framing rather than waiting for connection closure. SSE retains separate polling/cancellation behavior.
- Moves native prompt admission and session preference/interaction writes off the UI thread, captures request ownership at submission, and preserves uncertain-delivery state for reconciliation and explicit retry instead of blindly replaying writes.
- Improves durable queue admission and run handoff, startup cancellation recovery, retained-prompt execution rollover, and dequeue-before-provider-output ordering.
- Carries directory scope through native/browser conversation search, batches canonical authorization hydration, and classifies expected busy/budget errors without false 500 responses. Improves bounded sidebar history preloads and catalog reconciliation.
- Preserves streamed assistant text when a late empty snapshot arrives without an explicit retry reset; explicit retries still clear their previous baseline once, and nonempty corrections keep their existing merge behavior.
- Includes the committed Windows candidate/fixture MSI checkpoint and standalone validation recovery workflow, allowing future saved-package validation retries without recompiling. This does not establish the cause of previous runner losses.

Readiness and request deadlines are separate budgets, not a single end-to-end latency promise. Agent structured errors and daemon proxy responses have different envelopes; uncertain mutations require reconciliation. This is a stable production release, available through plain `neoism update` after the platform matrix passes. Existing first-terminal behavior, trusted-user hosting boundaries, and browser-worker exclusions are preserved.

## [0.7.131] - 2026-10-19

### Background preparation, framing, and startup presentation

- Adds configurable wallpaper blur, centered aspect-preserving cover/optional contain fitting, normalized positioning, platform overrides, and Settings/Lua/documentation integration. Pathless framing can adjust an inherited pack wallpaper without replacing its opacity.
- Prepares native startup wallpaper decode/blur on a background worker with request coalescing, generation checks, source-aware cache keys, validated cache contents, bounded image dimensions, and optional-cache fallback. GPU upload remains on the render thread; picker/pack transaction paths can still prepare images synchronously.
- Coordinates initial window reveal with successful presentation and background preparation, including completion wakeups, returned-error/cleared-image fallthrough, and X11 hidden-window redraw handling.
- Improves configuration round trips for color arrays/null and named margin sides so Lua patches preserve nonempty serialized appearance settings.
- Preserves existing terminal initialization and browser-worker exclusions. First-frame presentation is platform-sensitive; permanently stalled background preparation is not currently governed by a reveal timeout.

This is a stable production release, available through plain `neoism update` after all platform builds and packaging checks pass. Servo and CEF workers remain separate experimental components and are not packaged.

## [0.7.130] - 2026-10-18

### Production Agent GUI contract alignment

- Aligns the production Agent GUI's runtime/activity snapshot fixtures and consumers with the generated catalog-family contract, including `familyRevision` and required per-session activity maps.
- Carries all search, manual-compaction queue, prompt-cache, presence, evaluation-source, and Nix improvements prepared for v0.7.129. That release did not publish because the production GUI check failed; no feature work is omitted from this patch.
- Validates the production GUI build and tests in addition to the SDK/core, native, WASM, and API contract checks.

This is a stable production release. The failed v0.7.129 tag is not moved or reused; v0.7.130 becomes available through plain `neoism update` after all platform checks pass.

## [0.7.129] - 2026-10-18

### Search, compaction queues, presence, and provider caching

- Adds durable manual-compaction queue controls, serialized admission/cancellation, restart and transfer persistence, and consistent desktop/shared status reconciliation.
- Adds conversation full-text search backed by a durable Turso projection, bounded indexing/backfill, scoped ranking and filters, snippets, and navigator loading/error states across desktop and browser hosts.
- Improves Codex OAuth prompt-cache affinity with account/tenant/workspace/connection scoping and matching request header/body cache keys, while preserving non-Codex behavior and explicit overrides.
- Adds optional client-OS presence metadata through the protocol, daemon, browser publisher, and shared/native rendering while retaining legacy message compatibility.
- Improves subagent activity retention, cached-session/sidebar state, pagination, queue/abort presentation, emphasized links, and input wrapping.
- Includes evaluation-lab source, synthetic suites, and public HARD-51 setup/adapters/locks/tests. Paid/public experiment validation remains separate; generated downloads, environments, and run transcripts are not included. Local asset provenance may require preparation on another machine.
- Includes the committed Nix release-runtime, launcher, source-packaging, shader-tooling, and build/test selection fixes.

This is a stable production release, available through plain `neoism update` after the full platform matrix passes. Existing first-terminal directory following, trusted-user hosting boundaries, contributor credit, and Servo/CEF worker exclusion are preserved.

## [0.7.128] - 2026-10-17

### Conversation navigation and Windows validation hardening

- Adds measured Back / Prev / Next navigation for child conversations, following visible nested-roster order without wrapping and falling back to the authoritative main conversation when a trusted roster is unavailable.
- Shares navigation behavior across native and browser hosts, with clipped, occlusion-aware hit regions, live target revalidation, and stale-geometry clearing on session changes and resets.
- Preserves unsent composer drafts alongside cached conversation state and keeps empty child conversations navigable before their first message arrives.
- Uploads Windows installer/native-test evidence checkpoints before MSI smoke tests, adds timestamped phase/process logging, passes the expected version explicitly, and bounds redirected-output draining and process cleanup waits.
- Retains strict installer checks and reboot suppression. This hardening improves failure bounds and diagnosis; it does not establish that the previous hosted-runner disconnection cause is fixed. Alternate validation workflows do not yet include every release-path safeguard.

This is a stable production release, available through plain `neoism update` after the full platform matrix passes. Existing first-terminal directory following, trusted-user hosting limits, and Servo/CEF worker exclusion are preserved.

## [0.7.127] - 2026-10-16

### MCP authorization, subagent navigation, and rendering

- Adds inline cancellable MCP browser authorization with scope-preserving callbacks, explicit connection retry, and status that distinguishes authorization from an established runtime connection. Coalesces visible catalog refreshes and invalidates stale runtime status.
- Adds bounded MCP presentation metadata and inline raster icons, bundled service artwork, and consistent presentation through tool results, history, and remote timelines. Includes artwork/font provenance and license notices.
- Adds selectable connected-subagent catalog rows with exact child identity and owning-project navigation; parent foreground activity no longer mirrors child execution incorrectly.
- Improves dialog wrapping, clickable links, picker actions, icon cropping, settings layout/navigation, and cursor affordances.
- Adds self-contained branded invite/OAuth handoff pages with embedded Geist font and replayable wordmark animation. Adds analytic antialiased ring rendering across CPU, WebGPU, Vulkan, and Metal backends.
- Improves browser conversation-family handling and hover ownership. Uses measured terminal dimensions for initial PTY creation instead of the placeholder size.

This is a stable production release, available through plain `neoism update` after all platform builds and packaging checks pass. Existing first-terminal directory following, trusted-user hosting boundaries, and Servo/CEF worker exclusion are preserved.

## [0.7.126] - 2026-10-15

### Project conversations, hosting, and native interaction

- Promotes the nightly project-grouped conversation sidebar, global pins, independent pagination, conversation search/navigation, unified directory/image browser, and host-scoped recents to stable.
- Adds bounded Rust tool composition with native/MCP dispatch, schema validation, permissions, cancellation, and visible child-call traces.
- Adds Fast Host invites/deep links, unified server controls, recovery/leave workflows, and persistent-host readiness checks. Improves hosted-family authorization, revocation restoration, namespace continuity, and ordinary/Fast transport contracts.
- Improves remote workspace restoration and document hydration. Explicit directory selection targets the existing terminal, preserves drafts, defers busy-shell operations, and waits for host-observed cwd confirmation; input remains blocked after dispatch until trusted completion is known.
- Adds opt-in `appearance.splash-auto-animate` (off by default), preserves click-triggered wordmark effects, and improves native catalog-picker loading states without dropping drafts or search queries.
- Improves response-footer/stream presentation, activity indicators, pointer ownership, Git actions and diff controls, composer interaction, and file-tree dragging. Updates README links to hosted documentation.

Fast Host gives trusted participants host-user access and is not a sandbox. Remote live directory control currently requires integrated Linux bash/zsh; uncertain completion intentionally stays gated rather than accepting unsafe submissions. Native first-terminal directory following and split isolation are preserved; browser ambient shell cwd does not automatically re-root the workspace. Servo and CEF browser workers remain separate experimental components and are not packaged.

This is a stable production release, available through plain `neoism update` after all required platform builds and packaging checks succeed.

## [0.7.126-nightly.20261015.2] - 2026-10-15

### Directory control, remote hydration, and interaction fixes

- Improves workspace restoration and remote attachment retries, distinguishes scoped inventories from authoritative trees, preserves endpoint ownership, and restores connection-local subscriptions without publishing a guest's workspace selection.
- Hydrates remote code and Markdown documents through their owning workspace's live daemon connection, with activation/attachment refreshes and ordinary/Fast transport contract coverage.
- Applies explicit project/directory selections to the existing focused or primary backing terminal, preserves composer drafts, defers busy-shell operations, and waits for host-observed cwd confirmation before submissions resume.
- Adds correlated PTY directory-control requests with capability/session/scope checks and literal path handling. Remote live control currently supports integrated Linux bash/zsh only; unsupported shells/platforms fail closed.
- Improves cursor ownership, Markdown handle hit-testing, pane containment, tool pointer feedback, and trailing conversation activity indicators.

Native first/root-terminal directory following and split isolation are preserved. Browser ambient shell cwd updates do not automatically re-root the workspace; explicit directory selection uses the confirmed control flow. This is an opt-in nightly: use `neoism update --nightly`. Stable Latest remains v0.7.125, Fast Host is for trusted participants, and browser-engine workers remain unpackaged.

## [0.7.126-nightly.20261015.1] - 2026-10-15

### Project conversations and unified navigation

- Adds project-grouped conversation navigation, global pins, activity ordering, folder collapse/expand, and independent paginated chat lists with retry and stale-request invalidation.
- Adds a shared conversation navigator with project, title, and message search, endpoint-aware opens, and saved-project discovery across desktop and browser hosts.
- Unifies directory and image browsing for workspace selection, Agent execution-directory changes, and attachments, including daemon-backed joined/web browsing and host-scoped recents.
- Improves composer hit targets, cursor behavior, occlusion handling, and blank-draft directory selection without creating a session.
- Adds shared vector wordmark animations and carries font-license files into source packaging.
- Includes the branch checkpoint's Agent submission/activity, provider onboarding, React composer, and file-tree dragging fixes. Preserves first-terminal shell directory following, explicit picker selections, and split-terminal isolation.

This is a real opt-in nightly release. Use `neoism update --nightly`; stable Latest remains v0.7.125. Fast Host is for trusted participants, not a sandbox. Browser-engine workers remain separate experimental components and are not packaged.

## [0.7.126-nightly.20261014.1] - 2026-10-14

### Hosted conversation access and connection startup

- Adopts only complete eligible conversation families into a hosted workspace; out-of-root historical tasks keep their family private without blocking unrelated eligible conversations. Ownership conflicts remain atomic failures.
- Restores local access after authoritative host revocation, preserves namespace continuity on rehosting, and checks session authorization for child routes and linked artifacts.
- Starts ordinary persistent hosts through the running desktop's standalone implementation rather than a potentially stale sibling executable. Verifies nonce/protocol/endpoint readiness and persists sidecar metadata before reporting success.
- Moves host revival off the render thread, preserves persistent host lifetime, and adds bounded startup diagnostics with credential redaction and owned-child cleanup on failed startup.
- Distinguishes ordinary Hello-only WebSocket authentication from explicitly identified Fast Host Bearer upgrades; rejected Fast authentication never downgrades. Includes headless real-router transport contract fixtures and hosting/access regression tests.

This is a real opt-in nightly release: use `neoism update --nightly`. Stable Latest remains v0.7.125. Fast Host is not a sandbox and is intended for trusted participants. Browser-engine workers remain separate and are not packaged.

## [0.7.126-nightly.20261013.2] - 2026-10-13

### Hosting reliability and Agent presentation

- Adds a headless desktop workspace-daemon entry path, versioned nonce/address readiness acknowledgements, bounded startup diagnostics, authenticated WebSocket forwarding, and reuse of the local Agent service instead of competing database ownership.
- Improves Fast Host progress notifications, ready-dialog invite copying, picker/modal coordination, connection failure handling, and shutdown/session cleanup.
- Adds independent response-footer reveal and motion state so completion metadata does not replay answer text. Preserves reader positions and cancels inappropriate footer replay during history navigation and user gestures.
- Renders composition child calls as typed tool rows with bounded retained output/metadata, stable identities, and independent expansion while retaining parent source/results. Interrupted or legacy records identify unavailable/truncated output explicitly.
- Refactors standalone daemon startup into a shared runner while preserving normal bootstrap, persistence, signals, and bounded shutdown. Includes focused hosting, composition, and response-footer regression coverage.

This is a real opt-in nightly release. Use `neoism update --nightly`; stable Latest remains v0.7.125. Fast Host remains for trusted participants and is not a sandbox. Browser workers remain separate experimental components and are not packaged in this release.

## [0.7.126-nightly.20261013.1] - 2026-10-13

### Tool composition, server invites, and Git controls

- Adds a bounded Rust tool-composition interpreter with Oxc parsing, native/MCP dispatch, schema validation, permissions, cancellation, and visible call traces. Includes cross-platform interpreter CI and integration/confinement tests.
- Adds temporary Fast Host servers, authenticated invite/deep-link workflows, TLS forwarding, connection recovery, leave handling, and a unified server modal. Fast Host is not a sandbox: guests' shells and agents execute with the host user's access. Only invite trusted participants and review provider/MCP credential exposure before hosting.
- Adds Git actions dropdown controls and improves panel/diff scrolling, Agent diff presentation, and modal/palette interaction.
- Improves Linux startup activation and registers invite URI handling in desktop and Windows installation metadata.
- Preserves first-terminal directory following, per-model Codex budgets, readable MCP titles, and Logan Settle's Windows fix and contributor credit. Keeps the cleaned contributor history and browser-worker packaging boundaries intact.

This is a real opt-in nightly release with platform artifacts, not a dry run. Use `neoism update --nightly`; stable Latest remains v0.7.125. Servo and CEF browser workers are not built or packaged in this release. Native cross-platform hosting behavior remains subject to platform validation.

## [0.7.125] - 2026-10-12

### Transcript stability, draft scrolling, and provider correctness

- Improves logical reader anchors and lazy timeline measurement so historical height corrections do not masquerade as live output or cause false follow-bottom motion. Includes desktop, shared, and WASM handling.
- Adds independent wheel-scrolling viewports for multiline Agent drafts and terminal composers, preserving manual positions while restoring caret visibility after edits and navigation.
- Fixes Anthropic initial thinking content and thinking-token updates, groups parallel tool results before accompanying media, and preserves replay/compaction ordering.
- Clarifies native-first file-tool guidance while retaining shell fallbacks when native tools cannot perform an operation.
- Adds experimental inline Chromium artifact source and a browser-neutral worker facade, with content-height reporting, fitted geometry, theme preservation, input/focus routing, and stale-frame barriers. Live CEF rendering requires a separately installed matching worker and runtime; this release does not package or validate CEF across platforms.
- Retains experimental Servo source improvements without building or packaging Servo. Preserves first-terminal directory following, model-specific Codex budgets, readable MCP labels, and Logan Settle's Windows upgrade fix and contribution credit.

This is a stable production release. Browser-engine workers remain separate experimental components; the ordinary platform packages do not contain them.

## [0.7.124] - 2026-10-10

### Windows updates and readable tool activity

- Includes Logan Settle's Windows MSI upgrade fix: normal installation selects features before a single bounded repair fallback, with payload verification, separate repair logs, and reboot/failure safeguards.
- Replaces generic MCP execution headers with readable service-and-tool titles such as `Firecrawl Search`. A generic formatter handles new services, duplicate prefixes, and identifier separators without company-specific rules; arguments and output are not exposed in headers.
- Applies the same title policy across shared/native/WASM and React timelines, with status, discovery, and malformed-input regression coverage.
- Adds Logan Settle's contributor credit and preserves his original commit authorship. Removes historical Claude co-author trailers from the default branch without changing source trees or published release tags.

This is a stable production release. First-terminal directory following and Codex model-specific budgeting are preserved; experimental Servo runtime remains excluded.

## [0.7.123] - 2026-10-10

### Codex model-specific context limits

- Replaces the blanket 272k Codex OAuth ceiling with selected-account, model-specific discovery metadata. Each model automatically uses its advertised maximum context, falling back to its advertised default and then a conservative 272k window when metadata is unavailable.
- Resolves usable input from the advertised effective context percentage, preserving smaller explicit user limits. Models advertising 872k total context and 95% allowance resolve to 828,400 input tokens before the server's existing additional reserve.
- Keeps picker and generation/compaction limits consistent with the selected authentication connection and actual wire model ID, without borrowing API or Copilot context limits.
- Bounds explicit automatic-compaction thresholds to the known usable budget and adds malformed-metadata, alias/mode, auth-path, and input-budget regression coverage.

These capacities are backed by model discovery metadata and local tests, not live near-limit request validation. API-key model limits remain separate. First-terminal directory following and release-time Servo exclusion are preserved.

## [0.7.122] - 2026-10-09

### Workspace workers, Agent activity, and rendering

- Adds cloud workspace-worker lifecycle libraries, generation fencing, reconciliation, signed bootstrap credentials, and authenticated readiness/connection grants. Cloud host/runtime deployment and production worker-image validation remain separate infrastructure responsibilities; this release does not launch a public hosted service.
- Adds cloud host/runtime OpenAPI contracts, TypeScript clients, refreshed-token HTTP namespaces, worker CLI support, and integration documentation.
- Replaces the per-tool execution-provider abstraction with workspace-worker/controller execution. Rust embedders using `ExecutionProvider`, `with_execution`, or `HostedAttestation` must migrate to the worker APIs.
- Improves root/child catalog activity, Task follow-up context, scoped connectors, viewed-conversation highlighting, and split-pane focus across desktop, daemon, and web.
- Updates OpenAI/Responses reasoning compatibility and Codex subscription-limit handling without increasing smaller model ceilings.
- Fixes generated-image rows with empty or filtered text, compaction reasoning presentation, reopened tool-trace scrolling, Agent text contrast, sidebar state during configuration reload, footer layout, and rounded-card clipping.
- Includes renderer batching, CPU rendering, and font fixes. Preserves first-terminal directory following, split-terminal isolation, and release-time Servo exclusion.

This is a stable production release, available through plain `neoism update` after the full platform matrix passes. Experimental Servo runtime is not built or packaged.

## [0.7.121] - 2026-10-08

### Search, conversation following, and native appearance

- Adds resumable file reads and query-bound grep pagination with bounded output, Unicode-safe continuation, stale-cursor checks, cancellation, and explicit case/mode handling.
- Smooths live Agent stream-follow while preserving manual scroll positions, tool-expansion anchors, and interruption by user input; history loading no longer behaves like new live output.
- Improves activity placement above the composer, wrapped status geometry, background-task interaction, and generated-image hydration.
- Adds a standalone native background picker with installed backgrounds, explicit images, pack inheritance, previews, and transactional persistence without changing the active Mash Up Pack.
- Supports translucent panel backgrounds while preserving foreground content, fixes style-only Lua configuration patches, and updates appearance documentation.
- Fixes modal preview-image layering and Vulkan multi-layer image handling, and improves web animation redraw parity.
- Adds host-path and emoji-presentation regression coverage. Retains first-terminal directory following, split-terminal isolation, and the release-time Servo exclusion guard.

This is a stable production release, available through plain `neoism update` after all Linux, macOS, and Windows release builds and packaging checks succeed. Experimental Servo runtime is not built or packaged.

## [0.7.120] - 2026-10-08

### Agent UI, tools, and workspace reliability

- Adds animated tool activity, streamed output reveal, smooth expansion transitions, and motion-aware diff/todo widgets while respecting the animation setting and viewport visibility.
- Groups adjacent read/search tools only when the server identifies them as part of the same model-response batch, preserving that identity across desktop, daemon, and web history.
- Makes Task rows open child transcripts and preserves navigation to completed or restored subagents without marking them active.
- Improves usage-picker loading, usage labels, footer branch fitting, and resize/DPI-dependent chrome and status-bar layout.
- Fixes scoped indexed search to honor literal directory names, include/exclude filters, search modes, and limits.
- Promotes the nightly frame-pacing, text-cache, avatar-geometry, streaming-text, tool-row, Codex usage, and Mash Up Pack improvements to stable. Retains the first-terminal directory-following hotfix and split-terminal isolation.
- Keeps experimental HTML/Servo source available without building or packaging the Servo runtime in the release stack. The CI dependency guard remains enabled.

This is a stable YOLO release, available through plain `neoism update` after all Linux, macOS, and Windows release builds and packaging checks succeed.

## [0.7.119-nightly.20261007.2] - 2026-10-07

### Fix first-terminal directory following

- Restores workspace and Explorer directory updates when the first/root terminal changes directory. The previous nightly incorrectly returned the cached workspace root before checking the shell's current directory.
- Keeps split terminals from re-rooting the workspace and preserves joined-workspace host-root behavior.
- Adds regression coverage for repeated first-terminal directory changes, split-terminal isolation, and missing-cwd fallback.
- Servo runtime remains excluded from release builds and packaging, with the existing CI dependency guard retained.

This is a real opt-in nightly release. Use `neoism update --nightly`; plain `neoism update` remains on stable.

## [0.7.119-nightly.20261007.1] - 2026-10-07

### Native Agent UI and frame pacing

- Fixes overdue redraws waiting an extra refresh interval, removes repeated workspace/Git filesystem probes on cache hits, and stops search watchers from feeding back on Neoism's own logs while preserving live workspace indexes.
- Reuses composer/message wrapping, text measurements, and static avatar geometry without disabling visible animations; adds frame-time percentiles and diagnostics for event-loop work and Vulkan waits.
- Adds streaming text reveal, compact tool rows, Codex usage inspection, and Agent navigation/rendering improvements.
- Expands Mash Up Pack background effects and refreshes the terminal splash layout.
- Includes experimental inline HTML artifact and Servo worker source behind explicit opt-in features; the experimental browser worker is not part of the default release stack.
- Retains the stable Windows Lua startup/error-unwinding fix and its release smoke test.

This is a real, opt-in nightly release with production platform artifacts, not a workflow dry run. Use `neoism update --nightly`; plain `neoism update` remains on stable. Assets become public only after the Linux, macOS, and Windows release matrix succeeds. Sustained 144 Hz has not been established by an end-to-end before/after benchmark.

## [0.7.118] - 2026-10-04

### Fixed

- Fixes the Windows release-only startup crash reporting "panic in a function that cannot unwind". Lua callback errors use Windows SEH unwinding; the release profile now preserves unwinding so those errors can be handled instead of aborting the desktop or Agent Lua runner.
- Retains the 8 MiB Windows desktop stack reserve introduced in 0.7.117 for graphics adapter initialization.
- Adds an actual release-executable Lua regression check to both Windows release workflows, covering callback errors, Lua `pcall`, execution-budget errors, and continued VM use. The check reproduces the crash with the old abort profile; a normal Rust test harness would mask that profile difference.

This is a stable release with the full desktop, workspace daemon, agent, and Agent Lua runner payloads for Windows, macOS, and Linux.

## [0.7.117] - 2026-10-04

### YOLO — Windows startup fix

- Fixes a reproduced Windows startup stack overflow during native graphics adapter enumeration by increasing the desktop executable's main-thread stack reserve from 1 MiB to 8 MiB. Stack pages remain committed on demand.
- Applies the linker setting only to the Windows desktop executable, with target-aware MSVC and GNU flags. Other platforms and helper executables are unchanged.

This is a stable release, available through plain `neoism update` after all platform builds and packaging checks succeed.

## [0.7.113-nightly.20261002.1] - 2026-10-02

### Mash Up Packs and plugin effects

- Adds atomic Mash Up Pack activation with reversible theme/font baselines and pack-scoped desktop editor Lua selection.
- Adds the Lucid Blocks showcase with a plugin-defined vector-particle effect: every Agent composer edit emits an independently seeded bird while Rust owns bounded physics, rendering, and retirement.
- Adds capability-gated `neoism.effect.emit`, privacy-safe composer revision metadata, and exact-owner validation without exposing draft text or running Lua on input/render threads.
- Fixes Agent New Chat keyboard navigation, child-to-main pointer switching, and stale workspace ownership reconciliation.

This is an opt-in prerelease. Use `neoism update --nightly`; plain `neoism update` stays on stable. Nightly assets become visible only after the full Linux, macOS, and Windows matrix succeeds.

## [0.7.112-nightly.20261002.1] - 2026-10-02

### Open plugin platform

- Adds sandboxed editor Lua packages, isolated Agent Lua packages, managed package discovery, transactional activation, capabilities and grants, retained plugin UI, commands, diagnostics, decorations, LSP, Tree-sitter, tasks, debugging, Git, Agent tools, providers, routes, hooks, prompts, context, MCP, and custom message parts.
- Keeps Rust authoritative for rendering, input, synchronization, processes, transport, mutations, and GPU work; Lua callbacks remain isolated and event-driven.
- Fixes editor panel registration, startup failure visibility, Agent plugin-generation acquisition, web/WASM plugin command routing, and idle plugin state polling.
- Ships and validates the Agent Lua runner across Linux, macOS, Windows MSI, source/download installers, transactional updaters, macOS bundles and DMGs, and Docker.

This is an opt-in prerelease. Use `neoism update --nightly`; plain `neoism update` stays on stable. Nightly assets become visible only after the full Linux, macOS, and Windows matrix succeeds.

## [0.7.106-nightly.20260917.2] - 2026-09-17

### Nightly validation fix

- Includes the same feature preview as nightly `.1`, with the Windows MSI validation corrected to compare its numeric installer version separately from the full nightly executable version. Exact binary-version and payload-hash checks remain enforced.
- Nightly `.1` stays an unpublished draft; its tag is not moved.

This is an opt-in prerelease. Use `neoism update --nightly`; older Linux/Windows builds can bootstrap with `neoism update --target-version v0.7.106-nightly.20260917.2`. Older macOS bundles should use the DMG once. Plain `neoism update` stays on stable.

## [0.7.106-nightly.20260917.1] - 2026-09-17

### Nightly preview

This is an opt-in prerelease, not the latest stable release. `neoism update --nightly` installs the latest published nightly; plain `neoism update` stays on stable. Older Linux/Windows builds can bootstrap with `neoism update --target-version v0.7.106-nightly.20260917.1`. For an older macOS app bundle, install this release's DMG once, then use `--nightly` for subsequent updates.

- Markdown tables, source/caret layout, links, code-fence pairing, spelling, Notes navigation, and context-menu fixes.
- Drawing creation, world-space text scaling and wrapping, resize behavior, and drag selection.
- Agent GUI/mobile improvements, browser computer-use work, MCP handling, and provider/compaction updates.
- Workspace reconnect and live-message synchronization fixes.
- Notebook creation remains disabled; existing notebook documents remain supported.

Nightlies may contain regressions. Platform assets are published together only after the full Linux, macOS, and Windows matrix succeeds.

## [0.7.62] - 2026-08-27

### Fixed

- Guests in a joined workspace now appear in the transcript under their
  presence name instead of `local-operator` or a device id; the
  credential subject remains the authorization and audit identity.
- Joined agent panes adopt the host's configured model and thinking
  variant instead of showing "server default": session metadata now
  flows through the event stream, and retargeting a pane to a host
  re-hydrates its configuration defaults.

## [0.7.61] - 2026-08-27

### Fixed

- Fixed watched sessions freezing while the activity pill kept moving.
  Three mechanisms were closed: transcript snapshots no longer run on the
  SSE reader thread (a slow store read was stalling token delivery for
  its whole duration), the scoped event stream caches foreign-session
  verdicts instead of walking storage per event, and a lagged event
  subscriber is disconnected for clean reconnect recovery instead of
  continuing with a hole in its timeline. Two client-side watchdogs
  remain as safety nets: a dead-socket detector and a stalled-stream
  force-resubscribe with full reconciliation.
- Fixed joined workspaces failing with daemon-credential errors after a
  token rotation: every place a daemon token is signed or compared (the
  websocket handshake, the agent proxy, credential minting, and agent
  verification) now treats the on-disk token as the live trust root with
  the startup environment as fallback.

### SDK and API

- Every message part now has a typed schema in the OpenAPI contract:
  discriminated part union, status-discriminated tool states, and typed
  step-finish token usage and cost. Part events carry the typed part.
- Live events carry a wire-monotone sequence (durable rows persist the
  same value), fixing SDK event streams that previously dropped every
  event after the first; the SDK dedupes by event id and its event
  subscription yields the typed union.
- Added a runnable headless embedding example and handbook coverage for
  embedding, hosted multi-tenant authentication, and API stability.

## [0.7.60] - 2026-08-27

### Fixed

- A watched agent session no longer freezes after a provider-overload
  retry. The event-stream reader now detects a dead connection (45
  seconds without server keep-alives), reconnects, and reconciles the
  transcript — live token streaming resumes without closing and
  reopening the session.
- Retried responses no longer double their text: the retry's message
  reset is honored so re-streamed tokens replace the failed partial
  reply instead of appending to it. Late empty snapshots outside a
  retry still never regress streamed text.
- Failed release build legs now save their compile caches, so release
  retries start warm.

## [0.7.59] - 2026-08-27

This release ships the Agent V2 replatform: the agent server is now a
plugin-first platform with one versioned API, one ordered event bus, a
third-party plugin runtime, and a typed TypeScript SDK.

### Agent V2 platform

- Rebuilt the agent server around internal plugins: providers, tools, agents,
  MCP, LSP, PTY, VCS, workflows, semantic search, configuration, and the
  system prompt all load as plugins in immutable per-workspace generations
  that reload live when configuration changes.
- Every route is served through `/v2/` and described by a committed OpenAPI
  document; a parity test keeps plugin-dispatched routes and the spec in
  lockstep.
- Replaced the split live/durable event channels with one ordered event bus:
  snapshots and deltas broadcast synchronously in publish order, ending
  freeze-then-double-stream artifacts and out-of-order thinking cards during
  live streams.
- Restored real-time token streaming cadence end to end.
- The session coordinator is the sole in-memory authority on run ownership;
  plugin session access is descriptor-validated.
- Plugin generations retire only after active leases release, so in-flight
  requests never race a configuration reload.
- The Agent supervisor starts with the workspace daemon, and persisted Agent
  settings project into V2 configuration.

### Third-party plugins

- Added the serve-plugin runtime: long-lived `neoism-plugin/2` processes that
  register tools, hooks, and event subscriptions over newline-delimited JSON
  stdio.
- Plugins load from a command, a local entry file, or an npm package; npm
  installs happen in the background and the generation rebuilds live when the
  install completes.
- A failing plugin degrades with a visible reason instead of failing the
  workspace.
- Added the `@neoism/plugin` authoring package (`definePlugin`/`runPlugin`)
  with an SDK client wired to the host server.
- Windows resolves npm and batch shims through `cmd /C` with PATHEXT-aware
  executable resolution.

### SDK and typed events

- Typed the V2 event stream: all 34 published event types form a
  discriminated union in the OpenAPI document, exhaustively tested against
  the server's event vocabulary.
- The TypeScript SDK yields that typed union from its event subscription,
  with automatic reconnect and a sequence cursor that deduplicates replays.
- Session, message, artifact, permission, question, provider, catalog, and
  plugin operations are all exposed through the generated typed client.
- Added a version-locked npm publish pipeline for the SDK packages.
- The desktop and shared frontends now consume `neoism-agent-core` types
  directly for event classification and turn assembly.
- Added TLS to the desktop agent transport for `https://` servers.

### Transcript search

- Session search in the agent side panel now searches full transcripts, not
  just titles: matching excerpt chunks render under each session,
  word-wrapped to the panel width.
- Every occurrence of the search terms highlights inside the excerpts.
- Semantic ranking blends in when an embeddings provider is configured;
  keyword search works without one.
- Multi-word searches fall back to per-term matches when no single message
  contains every word.

### Fixed

- New agent responses no longer inherit the previous response's execution
  timer: quiescence settles at queue-worker exit, exempts the admitting
  prompt's own worker, and reconciles leaked `running` run rows that
  silently blocked executions from ever finishing.
- Manual compaction durably finishes its run record instead of leaking a
  permanently running row.
- Transcript search no longer fails when plugin routes deliver numeric query
  parameters as strings.
- Subagent activity status, timing, and live token streaming stabilized;
  live agent timelines stay chronological and reasoning order survives
  metadata arrival.
- Notes vault tests no longer race process-global state.

### Documentation

- Documented the V2 platform in the bundled handbook: new Server and API,
  Plugins, and SDK pages, an architecture overview, and refreshed Configure,
  Tools, and MCP pages.
- GitHub releases now carry these notes automatically from the changelog.

## [0.7.55] - 2026-08-24

### Added

- Added scheduled agent workflows loaded from project or global `workflow/` and
  `workflows/` directories.
- Added one-time, hourly, daily, weekly, monthly, and interval schedules with
  local or IANA timezones, DST handling, month-end clamping, and missed-run
  coalescing.
- Added workflow activation, pausing, manual runs, previews, diagnostics,
  durable run history, overlap prevention, and filesystem hot reload.
- Added workflow API routes and OpenAPI definitions for listing, inspecting,
  activating, pausing, running, previewing, and reading history.
- Added unattended workflow permission policies with explicit allow and deny
  patterns. Interactive permission requests are denied instead of blocking a
  scheduled run indefinitely.
- Added a bundled Scheduled Workflows guide and linked it from the Neoism Agent
  documentation.
- Added a rectangular presence-avatar rendering primitive for future shared
  presence surfaces.

### Changed

- Redesigned the agent side-panel usage display as a compact System 7-style
  context meter.
- Token totals in the side panel now use the mode indicator's rainbow scramble
  transition when their values change.
- Clicking the side-panel usage meter now opens the complete input, output,
  cache, reasoning, cost, and model breakdown on desktop and web.
- The global status line remains free of token-usage UI.
- Each subagent now notifies its parent immediately when that child finishes,
  without waiting for sibling agents. Workflows that need all results can make
  that waiting policy explicit in their prompt.
- Reused subagent sessions can report each later execution independently.
- Local MCP servers now reconnect after process failure, restart when relevant
  configuration changes, and shut down when disabled or removed.
- MCP discovery now reconciles stale runtime state before listing tools.

### Fixed

- Made subagent completion delivery durable and exactly once per logical child
  execution using generation identities, stable message IDs, and serialized
  child and parent reconciliation.
- Prevented duplicate or quadruple subagent completion notifications from
  wrapper, queue-idle, abort, restart, and concurrent sibling races.
- Prevented queued follow-ups from publishing stale wrapper results; one final
  completion is emitted after the queued drain finishes.
- Delayed completion acknowledgement until the runtime notification is durably
  appended to parent history, while keeping crash recovery idempotent.
- Repaired legacy and malformed persisted subagent completion metadata without
  panicking the agent server or duplicating notifications.
- Prevented late permission request and reply cleanup from resurrecting a
  finished subagent in the sidebar. Genuine task continuations still reactivate
  the child normally.
- Fixed streaming conversation teleports while reading above the bottom by
  replacing ambiguous message-ID anchors with stable source and content
  identities.
- Fixed timeline anchoring across duplicate or empty IDs, optimistic-to-durable
  ID transitions, grouped rows, history prepends, tail appends, row removal,
  and active wheel motion.
- Assigned optimistic user prompts their outbound message IDs before display
  and reconciled identical prompt text one-to-one.
- Made scheduled-run claiming and schedule-cursor advancement atomic for SQLite
  and Turso, preventing a crash from repeating a scheduled occurrence and its
  possible side effects.
- Added bounded MCP process shutdown and exact failed-runtime invalidation.

### Tests

- Added and expanded coverage for workflow parsing, validation, recurrence,
  timezones, DST, permissions, execution directories, hot reload, atomic run
  claims, and restart recovery.
- Added concurrency and recovery coverage for immediate, repeated, and
  exactly-once subagent completion delivery.
- Added desktop and shared regressions for permission terminal locks, optimistic
  prompts, stable timeline anchors, wheel targets, usage animation, hit targets,
  and details presentation.
