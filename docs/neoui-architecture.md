# NeoUI architecture and quality roadmap

## Status and purpose

This document specifies a framework direction and acceptance gates, not a claim of GPUI parity. The goal is GPUI-quality or better developer ergonomics, correctness, accessibility, and responsiveness, demonstrated through reproducible evidence rather than visual resemblance or a renderer rename. Do not copy Zed source; design and implement against Neoism's requirements and independently specified contracts.

The first end-to-end slice is implemented in `neoui/`, with the Sugarloaf adapter in `neoism-frontend/shared/src/neoui_renderer.rs` and declarative Settings boolean rows in `neoism-frontend/shared/src/panels/settings_page/neoui.rs`. The verification record below distinguishes automated coverage from unperformed visual, profiling, and platform-runtime checks. Later maturity gates remain open; a checked build is not evidence of runtime or accessibility parity.

## Clean checkpoint: opt-in only

Production migrations are disabled by default. Shared Settings selects `legacy_state.rs` and `legacy_view.rs` unless the Cargo feature `neoui` is enabled; native editing/IME and shared clipboard/composition bridges are gated with it. Enable the experimental desktop path with `cargo check -p neoism --features neoui`; enable the browser path with `cargo check -p neoism-terminal-wasm --target wasm32-unknown-unknown --features web,neoui`. The standalone `neoui` workspace crate remains directly buildable with `cargo check -p neoui`. No merge into main is part of this checkpoint.

Implemented foundations include typed component ownership and teardown, retained Taffy nodes and measurement caches, presentation-only invalidation, runtime counters, capture/target/bubble routing APIs, pointer capture, focus traps/keybindings, grapheme editing with undo and composition state, simple widgets, variable-height list indexing, and owner-scoped event delivery/cancellation with host-supplied executors. Experimental Settings uses `ComponentHost<String>`, owned boolean-row/toggle components, canonical pane values, and the text-input model; native and shared hosts translate editing, clipboard, and composition events. These are current APIs and a narrow integration, not framework parity or a completed migration of all Settings controls.

Unfinished boundaries: Sugarloaf still submits shapes and text in separate passes, so arbitrary interleaving is not supported; no production GPU ordering changes were made here. Full shaping/bidi editing, native/browser accessibility bridges, candidate-window geometry, general application scheduler/executor integration, backend conformance, and live UI/performance validation remain open. The new routing, task, and virtualization APIs are not claims that every application surface uses them.

### Checkpoint compile record

- Passed: `cargo check -p neoui`.
- Passed: `env -u CI cargo check -p neoism-ui --features neoui`; default shared UI also compiled during the default desktop check.
- Passed: `env -u CI cargo check -p neoism-terminal-wasm --target wasm32-unknown-unknown --features web` and the same command with `--features web,neoui`.
- Blocked outside NeoUI: both `env -u CI cargo check -p neoism` and `env -u CI cargo check -p neoism --features neoui` fail in concurrent, excluded agent-server edits at `external_agent/helpers.rs:46`, referencing missing `crate::tool_support::truncate`. Desktop host type-check completion is therefore not claimed.
- No new tests, full-suite rerun, release build, live UI validation, GPU conformance, or performance measurement was performed for this checkpoint.

## Evidence and provenance

| Inspected path | Relevant evidence and implication |
| --- | --- |
| `LICENSE` | Root MIT terms retain Rio and Alacritty lineage and point to `NOTICE`; do not treat the root summary as replacing per-file terms. |
| `NOTICE` | Identifies Sugarloaf as Rio-derived, describes original extensions, and distinguishes the Apache-2.0 Alacritty and winit-derived portions. |
| `sugarloaf/LICENSE` | MIT license explicitly retains both Neoism contributors and Raphael Amorim copyrights. |
| `sugarloaf/src/sugarloaf/primitives.rs` | Raphael Amorim copyright header; contains primitive types including `Corners` and `SugarCursor`. This is direct source-level provenance, not just historical branding. |
| `sugarloaf/src/lib.rs` | Exposes `Rect`, `Quad`, `RichText`, `Object`, graphics, text/layout types, and virtual-surface APIs; these are existing integration seams, not evidence that a general-purpose component framework exists. |
| `sugarloaf/src/components/core/shapes.rs`, `image.rs`, `svg.rs` | Existing shape, image, and SVG implementation areas under `sugarloaf/src/components/core/`. |
| `sugarloaf/src/renderer/batch.rs`, `sugarloaf/src/renderer/compositor.rs` | Existing batching and compositing implementation areas to audit when defining ordering and adapter behavior. |
| `sugarloaf/src/renderer/batch/tests.rs`, `sugarloaf/src/renderer/batch/ring_tests.rs` | Existing clip/order regression-test sources. Their presence is not a claim that they were run or cover the new adapter. |
| `sugarloaf/src/context/mod.rs` | Selects feature-gated WebGPU, macOS Metal, Linux Vulkan, and native CPU contexts; browser canvas construction is asynchronous and WebGPU-only in this module. |
| `sugarloaf/src/context/{metal,vulkan,webgpu,cpu}.rs` | Concrete context implementations; renderer work must preserve backend distinctions rather than assume one GPU API. |
| `sugarloaf/src/renderer/renderer.{metal,wgsl}`, `sugarloaf/src/renderer/vulkan.rs`, `sugarloaf/src/renderer/cpu.rs` | Existing renderer implementations/shaders that require contract-level conformance checks. |
| `sugarloaf/src/font/{linux,macos,windows}.rs`, `sugarloaf/src/font/text_run_cache.rs`, `sugarloaf/src/text.rs` | Existing platform font and text infrastructure to evaluate and reuse, not a certification of complete Unicode or IME behavior. |
| `sugarloaf/src/virtual_surface/` | Existing retained/virtual-surface infrastructure; coordinate its identities and cache policy with NeoUI rather than create competing document virtualization. |
| `neoism-window/src/platform_impl/mod.rs` | Dispatches to Linux, macOS, Windows, web, and Orbital modules. Linux has separate `wayland/` and `x11/` directories. These are window/event backends, not Sugarloaf renderer backends. |
| `neoism-window/LICENSE`, `neoism-window/LICENSE-APACHE`, `NOTICE` | The local MIT notice coexists with Apache terms and NOTICE's winit-fork attribution; release auditing must preserve applicable upstream terms rather than infer MIT-only coverage from a crate name. |
| `neoism-frontend/shared/src/panels/settings_page/{state,view}.rs` | Existing settings state and painter integration area. The inspected painter uses Sugarloaf directly and explicit ordering constants; migration must account for existing overlays and hit geometry. |
| `neoism-frontend/{desktop,shared,wasm,web}/` | Existing host/shared frontend separation; platform-neutral framework policy must not become desktop-only through its first integration. |

The root license's brief Alacritty wording and NOTICE's explicit Apache-2.0 description warrant reconciliation in a separate licensing audit; this document does not resolve that discrepancy or change any legal file. Preserve source headers, bundled licenses, and distribution notices during moves, extraction, and packaging. New first-party code does not extinguish upstream obligations in derived code. Any public renderer rebranding must explicitly retain the Rio lineage and applicable MIT attribution; a cosmetic rename is not an architectural deliverable.

## First milestone: a narrow end-to-end slice

The initial implementation provides a platform-neutral `neoui` crate with Taffy-backed layout, a keyed declarative tree, typed actions, and frame output containing a clipped display list, hit regions, and semantic nodes. The core has no Neoism application or GPU dependency. Shared Settings boolean rows and a Sugarloaf adapter exercise that boundary without replacing configuration persistence or host rendering loops.

The slice should cover a boolean row's label, current value, enabled/disabled state, stable key, layout, visible control, semantic checked state, and action mapping to the existing settings model. Application code remains the authority for configuration persistence, error reporting, and external changes. A UI action requests a change; it does not prove that persistence succeeded. Read the resulting model state back into the next view, including failed writes and configuration changes originating outside the panel.

The first milestone was not a full settings rewrite, editor framework, native accessibility bridge, replacement event loop, or new renderer. Subsequent foundation APIs are summarized in the checkpoint above; their existence does not establish lifecycle, routing, text, or cross-platform parity. Other Settings controls retain their existing paths unless explicitly integrated.

### Current implementation limits

`Runtime` retains keyed Taffy nodes and measurement caches. `set_tree` reconciles structure, `layout` resolves layout and output, and `present`/`invalidate_presentation` support presentation-only changes. `MeasurementKey` and the measurer's optional cache key define measurement reuse; `RuntimeStats` exposes work counters. `ComponentHost` separately owns typed state, event delivery, and teardown. Keys are currently globally unique within a surface. Host text measurement preserves exact measured strings, but fallback wrapping is Unicode-scalar-based, not a full grapheme, bidi, or Unicode line-breaking implementation. Clips are rectangular. Semantic nodes are metadata, not an operating-system accessibility bridge.

The first Sugarloaf adapter supports the non-overlapping row composition used by Settings. It preserves original rounded-quad bounds with GPU fragment clipping and explicitly occludes text under dropdowns. Shapes and text still occupy separate renderer passes: arbitrary interleaved text/shape stacking is not a supported adapter contract. The batching regression pins track-before-knob ordering for equal-order clipped quads. General stacking requires a renderer-contract milestone, not a claim that the current display-list replay already solves it.

Settings boolean controls use NeoUI's resolved geometry for paint and input, keyed by configuration path. Tab/Shift+Tab traversal and Enter/Space activation are connected in native and shared/browser hosts, without activating on repeated keypresses. Search, dropdowns, text editors, navigation, and scrolling retain their ownership rules. Keyboard traversal reveals offscreen controls; wheel scrolling relinquishes toggle keyboard focus so an invisible control cannot change.

### Dependency direction

Arrows mean “depends on”; names in brackets are architectural roles rather than asserted source filenames.

```text
Neoism shared settings integration ───────> neoui core
             │                                │
             │                                └──> Taffy
             v
Existing configuration/model services

Native / browser host ──> [NeoUI host bridge] ──> neoui core
                                  │                 │
                                  │                 └── emits frame description
                                  v
                         [Sugarloaf adapter] ──> neoui frame types
                                  │
                                  v
                              Sugarloaf
                         /       |       \
                     Metal    Vulkan    WebGPU    (+ native CPU fallback)

[future accessibility adapter] ──> neoui semantic snapshot
              └──> AccessKit/native host or browser DOM accessibility bridge
```

The core owns identity, layout policy, action descriptions, and platform-neutral frame data. The host owns scheduling, OS/browser events, clipboard, cursors, scale changes, and platform services. The adapter consumes frame data and submits existing renderer primitives; it must not own configuration or silently mutate component state. Text measurement is an injected service with explicit metrics and revisions, not a backdoor dependency from the core into Sugarloaf. AccessKit belongs behind a future host adapter, not in the layout engine.

## Core contracts

### Identity, ownership, subscriptions, and tasks

Keyed reconciliation is necessary but insufficient for a mature component model. Define sibling-key uniqueness, component-type changes, reparenting, mount/update/unmount order, and replacement semantics. Use generation-checked handles so an old asynchronous result cannot update a newly mounted component occupying the same slot. Test reorder, removal, duplicate keys, and replacement; reject duplicate keys diagnostically rather than silently transfer state.

Introduce an explicit owner scope for each stateful component and window. State belongs to an owner, with documented mutation and notification rules; rendering should read a coherent snapshot, not trigger arbitrary model writes. Track subscriptions with disposable handles owned by that scope. Dependency reads should invalidate only affected consumers, with notification coalescing and a deterministic order. Define behavior for listener removal during notification and recursive updates; never invoke user callbacks while holding internal state locks.

Tasks need cancellation, ownership, executor boundaries, and result delivery rules. Unmounting cancels owned work or causes late results to be discarded by generation checks. Background I/O must not block input/layout/paint; application of results is marshalled to the UI scheduler. Cancellation, errors, panics, and shutdown must not leak subscriptions or wake a destroyed window. Add lifecycle diagnostics and tests before describing the framework as having managed components.

### Retention and invalidation

A keyed declarative input can still rebuild and lay out everything every frame. Separately retain component state, reconciled nodes, layout results, shaped text, paint output, hit data, and semantic data. Track dirty reasons: structure, measure, layout, paint, hit testing, semantics, and resources. A checked-color change should not remeasure an unrelated paragraph; scrolling should update transforms/visible ranges without reconstructing the whole model.

Specify cache keys and dependency revisions before adding caches. Layout keys include constraints, style, child/measure revisions, and font metrics; text keys include content, font/fallback resolution, size, language/direction, and shaping features. Scale, theme, font availability, and resource changes invalidate the appropriate stages. Taffy supplies layout computation, not the application's complete dependency/invalidation model. Add counters for reconciled, measured, laid-out, painted, and semantically updated nodes to make bounded work testable.

### Renderer contract

- **Units:** Core layout, pointer locations, clips, and text metrics use logical pixels, top-left origin, positive x right and y down. The host supplies device scale separately; the renderer boundary converts exactly once to physical pixels. Existing callers with pre-scaled geometry require explicit normalization, not another implicit multiplication.
- **Geometry:** Document rectangles as x/y/width/height with half-open hit bounds. Reject nonfinite geometry; empty clips draw and hit nothing. Preserve fractional layout until rasterization; define outward scissor rounding and any pixel-snapping policy in one place. A scale change must not leave old hit or clip coordinates active.
- **Clipping:** Maintain a nested clip stack and intersect child clips with ancestors and the viewport. Restore clips after each scope. Clip rendered pixels and hit regions consistently. Preserve a rounded rectangle's original bounds/radii when partially clipped; do not implement clipping by resizing the shape and moving its corners. Rounded/path clips require an explicit capability or mask strategy, not a silent rectangular approximation.
- **Stacking:** Use stable paint order within explicit stacking contexts; define overlays/portals separately. A renderer may batch only where that cannot reorder overlapping content, text, transparency, or clips. Hit testing resolves reverse visual order subject to enabled state, clipping, capture, and modal scope. Do not silently squeeze arbitrary tree depth into the legacy painter's small integer order bands.
- **Text and resources:** Frame commands reference stable text/image resources with explicit revisions and lifetimes, not borrowed application pointers. Measured text and drawn text must use compatible shaping results and baselines. The adapter owns renderer resource translation and reports unsupported commands or missing resources rather than silently omitting controls.
- **Color and compositing:** Specify color space, alpha convention, and blending at the adapter boundary; convert once to each backend's requirements. Test translucent overlaps, text edges, and images. Group opacity, effects, and offscreen layers need explicit capability and cost policies; they are not assumed first-slice features.
- **Submission:** Produce display commands on the CPU and submit geometry, glyphs, and resources directly to the selected renderer. No GPU-to-CPU full-frame readback or screenshot-to-texture round trip in normal presentation, hit testing, or layout. Explicit screenshot tests may read pixels outside the interactive path; a native CPU renderer is a separate raster backend, not a GPU readback fallback.
- **Atomicity:** Publish layout, display, hit, and semantic snapshots under a shared frame revision. Input must not target geometry from a different revision without an explicit reconciliation policy. Surface loss, resize, and resource rebuilds must not require rebuilding application state.

Extend Sugarloaf where these contracts expose real gaps. Prefer a small adapter plus backend-neutral conformance fixtures over duplicating shader logic in the UI crate. Audit clipping, batching, and color across Metal, Vulkan, WebGPU, and CPU individually. Existing terminal, editor, markdown, and agent surfaces must retain their direct optimized rendering paths while embedding into a common scene contract; do not flatten them into thousands of generic controls solely for API uniformity.

### Hierarchical input and focus

Define routing phases and propagation explicitly: hit target selection, ancestor capture, target handling, bubble, and default behavior. Typed actions must distinguish an intent from a raw key and declare whether handling stops propagation. Provide scoped key contexts so settings, editors, terminal passthrough, and application shortcuts do not all consume one keystroke. A pointer click should activate a boolean once, not once on press and again on release.

Track pointer capture per pointer ID, with release on cancellation, unmount, window deactivation, and modal changes. Treat mouse, pen, touch, wheel, and gesture events as distinct inputs. Nested scrolling needs an explicit handoff rule at bounds, not competing independent handlers. Capture does not bypass modal security or justify keeping a dead node alive.

Focus needs stable identities, scopes, tab order, directional policy, focus-visible state, and restoration when a scope closes. Modal scopes constrain pointer and keyboard dispatch, prevent background activation, and restore a valid previous focus target. Disabled/hidden/clipped controls need documented focus behavior. Test removal of the focused node and opening a dialog during capture. The first boolean-row gate requires keyboard access without implying that this full router already exists.

### Text, Unicode, IME, and accessibility

Build on the existing Sugarloaf font/text implementation only after auditing its contracts. A complete text service must account for fallback fonts, grapheme clusters, ligatures, script/language shaping, bidirectional runs, line breaking, baselines, and stable glyph-to-source mapping. Do not equate byte offsets, Unicode scalar indices, graphemes, glyphs, terminal cells, or UTF-16 offsets. Define conversions at API boundaries and test Arabic/Hebrew mixed with Latin, Indic scripts, CJK, combining marks, emoji sequences, and missing glyphs.

Text editing requires visual/logical caret movement, selection geometry, deletion by grapheme, hit testing within bidi runs, and compatible measurement/painting. An estimated width callback can support the initial row fixture but is not proof of a production text system. Cache shaping independently from placement, with fonts and fallback revisions included in invalidation. Terminal cell layout remains a separate policy from proportional UI text.

IME composition is provisional state, not a stream of ordinary committed keypresses. The host bridge must deliver preedit, selection, commit, and cancellation, position the candidate window from the caret in platform coordinates, and handle focus/scale changes without committing stale composition. Browser composition and native platform behavior need separate tests, including dead keys and reconversion where supported. No “text input complete” claim before real IME validation.

Semantic nodes are framework output, not screen-reader support by themselves. Define stable semantic IDs, roles, names, descriptions, checked/disabled/expanded states, focus, bounds, relationships, and action routing. The proposed native AccessKit adapter must diff semantic snapshots and map accessibility actions back through the same typed action path. A browser host needs an appropriate DOM accessibility representation and focus/composition bridge; drawing onto a canvas alone is insufficient. Validate with actual assistive technology, including keyboard-only operation and focus/checked announcements. The AccessKit adapter remains future work until implemented and tested.

### Virtualization

Provide a virtual collection contract with stable item keys, viewport plus bounded overscan, estimated/measured extents, incremental height updates, scroll anchoring, and targeted reveal. Work per scroll should scale with visible items and changed measurements, not total item count. Preserve selection and model state when visual rows unmount; keep focus and accessibility navigation well-defined for offscreen items.

Reuse or adapt `sugarloaf/src/virtual_surface/` for document-heavy surfaces instead of introducing a second incompatible source/scroll model. Specify the boundary between framework collection virtualization and document/glyph residency. Test variable-height updates above the viewport, prepend, filtering, resize, deep jumps, and resource eviction. A clipped full list is not a virtualized list.

## Native and web boundary

Keep platform services behind narrow host interfaces: event delivery, redraw scheduling, monotonic time, task spawning, clipboard, IME, accessibility, cursor, file dialogs, and window lifecycle. Use a wasm-safe clock and avoid native blocking I/O assumptions in reachable shared code. Browser initialization and GPU acquisition are asynchronous; background-tab throttling and device loss must not invalidate owner/task lifetimes.

Maintain two separate support matrices: window/input hosts and renderer capabilities. Source availability for an Orbital or web window backend does not certify NeoUI support there. Primary validation targets are Linux Wayland and X11, macOS, Windows, and browser wasm; select concrete OS/browser/GPU configurations in the test manifest. Verify DPI changes, resize/minimize/restore, multiple windows, touch where applicable, and composition per host. Publish unsupported backend capabilities rather than silently substituting different visual or interaction semantics.

## Staged deliverables and gates

Gates below are requirements, not completed checkboxes. Land narrow increments with explicit test evidence; keep existing implementations available until their replacements meet the corresponding gate.

| Stage | Deliverable | Required exit evidence |
| --- | --- | --- |
| 0 — boundary slice | Platform-neutral `neoui`, Taffy layout, keyed tree, typed actions, clipped display/hit/semantic output; shared settings boolean rows and Sugarloaf adapter. | Dependency audit shows no application/GPU edge from core; deterministic layout/action fixtures; sibling reorder identity test; clipped control cannot activate; disabled control cannot change settings; one user activation maps to one intent; keyboard boolean activation; external model changes and persistence errors reflected; mixed old/new settings paint and hit order regression fixture. |
| 1 — ownership and routing | Owner scopes, subscriptions, task cancellation, hierarchy-aware input, focus/capture/modal policy. | Mount/unmount/reorder tests; late-task result rejection; no residual listeners after teardown; nested event order; tab/focus restoration; capture cancellation; modal background suppression; repeated open/close stress fixture. |
| 2 — retained work | Dirty-stage propagation, layout/measure/paint caches, explicit scheduling, virtual collections. | Counters prove unrelated subtrees are not remeasured on a local paint change; idle core requests no frames without an active reason; font/scale/theme invalidation; bounded visible-item work and stable scroll anchors under variable-height edits. |
| 3 — text and semantics | Shaping/measurement contract, editable text, bidi and IME, native AccessKit and browser semantic adapters. | Unicode corpus and source-index round trips; real IME tests on each supported host; caret/candidate geometry across scale changes; accessibility tree/action tests plus manual screen-reader transcripts. No adapter is certified solely by semantic snapshots. |
| 4 — backend conformance | Shared rendering contract across selected Metal/Vulkan/WebGPU/CPU paths and native/browser hosts. | Clip/stack/alpha/text fixtures, device-loss and resize recovery, platform matrix results, GPU capture or instrumentation demonstrating no interactive frame-readback path, terminal/editor rendering regression evidence. |
| 5 — framework release gate | Documented component API, diagnostics, migration guide, benchmark harness, resource limits, and compatibility policy. | Repeatable performance reports, leak/soak tests, explicit unsupported features, licensing/package audit, API examples for a dialog, virtual list, and text field without application-private dependencies. |

Stages may overlap, but later-stage code does not waive earlier gates. Do not label a single integrated settings control a completed general UI framework. Broader migration should follow evidence: boolean row, focusable control group, modal dialog, virtual list, then editable text and richer panels. Each migration removes duplicate application geometry/interaction logic only when the common implementation covers its actual requirements.

## Test strategy and proposed performance budgets

Use deterministic core tests with fake measurement, clock, scheduler, and event streams; property tests for key reconciliation, clip intersection, and finite/nonnegative geometry; model-based tests for focus/capture; and snapshot tests for display, hit, and semantic output. Fake text metrics verify framework policy, not real shaping. Adapter tests should inspect command ordering and clip/resource mapping before backend pixel tests. Pin fonts, scale, and fixtures for image comparisons, with documented raster tolerance rather than large unexplained thresholds.

Performance numbers in this section are proposed acceptance targets, not measurements or assertions about current Neoism, Sugarloaf, or GPUI. Record hardware, OS, backend, power mode, display refresh, resolution, scale, font set, build profile, warm/cold state, fixture size, and sample count. Separate reconciliation, layout, shaping, paint construction, uploads, GPU execution, presentation, and end-to-end input latency; report p50/p95/p99 and trace outliers. Benchmark both an isolated fixture and the real mixed terminal/editor/settings workload.

| Workload | Proposed budget/gate |
| --- | --- |
| 60 Hz interactive reference fixture | p95 UI CPU preparation at most 4 ms and GPU UI execution at most 4 ms, measured separately; end-to-end frame deadline target 16.67 ms. These are not additive guarantees because scheduling/presentation also consume time. |
| 120 Hz reference fixture | Establish a separate 8.33 ms frame-deadline run; do not infer success from the 60 Hz budget. Identify which work must be reduced before enabling a parity claim at this refresh. |
| Local boolean update in a large settings tree | Zero remeasure of unchanged sibling subtrees after warm-up; report reconciled/layout/paint node counts and allocations. Set a bounded ancestor invalidation expectation in the fixture. |
| 100,000-item synthetic collection | Retained visual nodes and per-scroll measurement bounded by visible items plus declared overscan; report bytes/item, cache limits, deep-jump behavior, and latency distributions. No claim that 100,000 items were tested here. |
| Idle window with no animation/caret timer/resource work | Zero core-originated redraw requests over a controlled 10-second interval; classify host/exposure redraws separately. No polling loop merely to discover unchanged state. |
| Lifecycle/resource soak | At least 1,000 mount/unmount or panel-open/close cycles in the fixture; zero live subscriptions/tasks attributable to removed owners; bounded cache growth after warm-up, with an explicit byte cap and eviction test. |

Choose and record reference hardware before turning proposed timing budgets into release blockers. Until then, correctness and work-count gates are enforceable without invented speed claims. Instrumentation must distinguish framework work from document rendering and other application activity. A fast empty demo, screenshots alone, or compilation alone cannot establish mature framework quality.

## First-slice verification record

The following checks were recorded against the earlier initial slice, before the ownership/retention/input additions and default-off feature gate. They are historical evidence only, not a rerun against this checkpoint. Feature-gated tests now require `--features neoui` on `neoism-ui`. Current checkpoint compile results are recorded separately below; no full suite, pixel validation, or performance measurement was run for this stop.

| Command | Result |
| --- | --- |
| `cargo test -p neoui` | 36 unit tests, 2 integration tests, and 1 documentation test passed. Includes padded-row text, exact root-width rejection, and iterative disposal of 50,000-node rejected trees on a 2 MiB stack. |
| `env -u CI cargo test -p neoism-ui --lib panels::settings_page` | 31 tests passed, including mixed-row offsets, wrapped boolean-row parity across widths/scales, typed persistence actions, dropdown ownership, keyboard traversal, viewport clipping, and stale-input cleanup. |
| `env -u CI cargo test -p neoism-ui --lib neoui_renderer` | 2 tests passed for logical units and straight-alpha byte-color conversion. |
| `cargo test -p sugarloaf --features neoism-window/x11 --lib neoui_switch_quads_preserve_equal_order_submission_across_clips` | 1 test passed for track/knob instance order, original geometry, corner radii, and per-instance clips at multiple scales. The window feature is required by Sugarloaf's Linux test dependencies. |
| `env -u CI cargo check -p neoism` | Passed with warnings outside the new framework. |
| `cargo check -p neoui --target wasm32-unknown-unknown` | Passed. |
| `env -u CI cargo check -p neoism-terminal-wasm --target wasm32-unknown-unknown --features web` | Passed with warnings outside the new framework. |
| `cargo clippy -p neoui --all-targets -- -D warnings` | Passed. |
| `cargo fmt -p neoui -- --check` | Passed; changed implementation files also passed scoped rustfmt and whitespace checks. |
| `env -u CI cargo test -p neoism-ui --lib -- --quiet` | 3,525 passed, 13 failed, 1 ignored. The failures are outside the NeoUI/Settings migration; the complete shared suite is not green. |

The remaining full-suite failures occur in markdown wrapping/spellcheck, agent-pane picker/navigation/configuration tests, command-palette routing, and terminal host-completion tests. Those implementations and assertions were not changed for this milestone; this record does not claim a separate baseline checkout was tested. They must be tracked separately rather than silently counted as successful verification.

No interactive visual run, screenshot comparison, live persistence smoke test, Windows/macOS run, assistive-technology validation, or performance baseline was performed in this increment. Those checks and the later framework gates remain required before claiming production-wide migration or GPUI-class maturity.

## Renderer evolution decision

Continue evolving Sugarloaf unless measured correctness, capability, or maintenance constraints justify a targeted replacement. Its existing primitives, text infrastructure, virtual surfaces, and native/WebGPU contexts are concrete starting points. The immediate work is a precise scene/adapter contract, conformance tests, invalidation instrumentation, and backend fixes—not wholesale replacement or branding changes.

If a subsystem cannot meet the contract, record the failing fixture, backend scope, alternatives, compatibility cost, license implications, and migration plan before replacing it. Compare observable behavior and measured workloads rather than claiming parity from architecture vocabulary. The framework is mature only when ownership, input, text, accessibility, retention, platform behavior, and performance gates are evidenced together; the renderer's ancestry is neither a disqualification nor evidence of that maturity.
