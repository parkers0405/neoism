# neoui core

`neoui` is Neoism's experimental renderer-independent declarative UI core: retained keyed Taffy layout, measurement caching, typed components and ownership, routing, input editing, scrolling/list helpers, and owner-scoped task delivery. It contains no window, GPU, application settings, or concrete executor/event loop, and uses no Zed reference code. Production Settings migration is default-off behind the `neoui` Cargo feature; see `docs/neoui-architecture.md` for checkpoint checks and unfinished boundaries.

## Host API

```rust
use neoui::{Element, Insets, Rect, Role, Runtime, Semantics, Size, TextStyle};

#[derive(Clone)]
enum Action { ToggleWrap }

let mut runtime = Runtime::<Action>::new();
let root = Element::column("settings", [
    Element::row("editor/wrap", [
        Element::column("editor/wrap/copy", [
            Element::text("editor/wrap/title", "Wrap long lines", TextStyle::default()),
            Element::text("editor/wrap/description", "Keep long lines inside the editor viewport.", TextStyle::default()),
        ]).grow(1.0).basis(Size::Px(0.0)).gap(4.0),
        Element::quad("editor/wrap/track", [0.2, 0.6, 0.9, 1.0], 10.0)
            .width(Size::Px(36.0)).height(Size::Px(20.0)).shrink(0.0),
    ]).gap(16.0).padding(Insets::all(12.0))
        .on_pointer_down(Action::ToggleWrap)
        .semantics(Semantics::new(Role::Switch).label("Wrap long lines").checked(true)),
    // Reserve height for a legacy/custom row handled by another adapter:
    Element::spacer("legacy/theme-row").height(Size::Px(40.0)),
]);

// Replace with Sugarloaf's complete-string logical-pixel advance measurement.
let mut measure = |text: &str, style: &TextStyle| text.chars().count() as f32 * style.size * 0.5;
let viewport = Rect::new(20.0, 60.0, 400.0, 500.0);
let frame = runtime.layout(root, [20.0, 60.0], 400.0, viewport, &mut measure)?;
let total_height = frame.bounds.height;
let row_height = frame.bounds_for("editor/wrap").map(|bounds| bounds.height);
let frame_id = frame.id;
// Consume frame.paint in order, honoring every command's clip.
// Translate frame.semantics through the host's accessibility adapter.

if let Some(Action::ToggleWrap) = runtime.dispatch_pointer_down_in(frame_id, 30.0, 70.0) {
    // Mutate the application setting, then rebuild the declarative description.
}
runtime.release_pointer(); // pointer up/cancel
runtime.clear();           // required when the settings surface becomes hidden
# Ok::<(), neoui::LayoutError>(())
```

The same API is also available as a function-implementing `TextMeasurer` trait; no closure is retained. `layout(root, origin, available_width, clip, &mut measurer) -> Result<&Frame<Action>, LayoutError>` consumes the description. Clone the frame only if you need to retain an owned snapshot beyond the runtime's next mutable operation.

### Layout and measuring

- All coordinates, text sizes, radii, gaps and padding use logical pixels. The adapter owns DPI conversion and font selection. Taffy rounding is disabled to preserve fractional geometry.
- The root gets exactly `available_width`, overriding its width/min/max-width styles. A horizontal padding floor larger than that width is rejected with `LayoutError::IncompatibleRootWidth`; the resolved root width is also checked before publishing any frame. Height is unconstrained and automatic unless explicitly styled. The viewport clip is **not** a maximum layout height.
- A column has automatic content height; a row uses flex layout; a stack overlays children in a single intrinsic-height grid cell, in painter order. Quads and spacers have no intrinsic size; supply width/height or flex growth where appropriate. All sizes use border-box semantics.
- A text/copy column alongside a fixed toggle should generally use `.grow(1.0).basis(Size::Px(0.0))`; the toggle should `.shrink(0.0)`. Minimum sizes default to zero so flex children can shrink and wrap. Public `Style` exposes min/max dimensions, alignment, flex basis, padding and rectangular descendant clipping.
- If measuring first is necessary, build the same tree at origin `[0.0, 0.0]` and an ample finite clip, read `Frame::bounds`/`bounds_for`, then build it again at its viewport/scroll origin with the real clip. Clip/origin changes do not alter intrinsic height. Prefer one persistent runtime for the complete settings column, with keyed spacers for mixed legacy rows, rather than persistent per-row runtimes.
- `bounds_for(key)` returns every mounted node's full unclipped box, even offscreen. Paint, hits and semantic output are viewport-culled. Offscreen mounted focus is retained; scrolling focused controls into view is host-owned.

### Text contract

`TextMeasurer::measure(&mut self, text: &str, style: &TextStyle) -> f32` returns the complete single-line advance the renderer actually uses. It must be deterministic during one layout and finite/nonnegative. Text size, color and line height are explicit; `line_height` is a logical-pixel line-box height, not a multiplier.

Wrapping measures complete candidate strings, preserving kerning/shaping advances rather than adding character widths. Emitted `PaintCommand::Text` strings are the actual wrapped lines and their `bounds.width` is their measured advance. The adapter must draw each line **without wrapping it again**, using the same font/measurement configuration. `bounds.y` is the **top of the line box**; baseline conversion follows the adapter's existing top-origin conventions. Empty lines emit no text command but still consume one line height.

Explicit newlines, empty paragraphs, trailing newlines and CRLF are supported. Within a paragraph whitespace is normalized to a single space with leading/trailing whitespace removed. Long words fall back to Unicode scalar boundaries; narrow/zero width terminates even when one scalar is wider than the available width. Text is clipped to its content box, including when given a fixed height. This milestone does not implement grapheme-cluster line breaking, bidi reordering, text selection, font fallback or a shaped glyph cache. An adapter may shape the emitted strings, but it must measure the same shaped strings. Rebuild after font or DPI changes.

### Painting, input and lifecycle

`Frame` owns `paint: Vec<PaintCommand>`, `hits: Vec<HitRegion<Action>>`, `semantics: Vec<SemanticNode>`, an opaque `id`, and root `bounds`. No frame refers to Taffy node ids, a measurer, the description tree, or callbacks. Both command variants have explicit `bounds` and `clip`. The adapter must enforce clip, not merely cull whole commands. Rounded quad radii are clamped to half the shorter dimension; clipping and pointer targets are rectangular, not curved masks.

Commands and targets use the same preorder traversal: parents precede children and later siblings are on top. `Frame::hit_test` searches this order backwards. Every target's half-open rectangle is intersected with the same inherited clip used for paint/accessibility. Decorative nodes without handlers do not intercept input. Enabled targets return cloned typed actions; disabled handler targets remain blockers to prevent clicking through an overlapping disabled control. Disabled semantics propagate to descendants. The compatibility pointer-down helpers shown here return typed actions directly. The `routing` module additionally exposes capture/target/bubble dispatch, pointer capture, focus traps, and scoped keybindings; hosts must explicitly connect their event and lifecycle policy.

`Runtime::dispatch_pointer_down(x, y)` always uses the most recent frame. For events queued from a previously rendered frame, use `dispatch_pointer_down_in(frame.id, x, y)`; stale frame tokens are rejected without changing interaction state. Tokens are local to a runtime, not globally interchangeable. Always relayout when the viewport or origin changes. `clear()` invalidates the frame token and removes all geometry, focus and pressed state; call it when hiding, closing, or replacing the surface. Layout failure does the same, never leaving stale hits installed.

Keys must be nonempty and globally unique within the root, including spacers and decorative nodes. They are explicit strings with no positional fallback. Focus and pointer-down identity survive successful rebuilds by key; missing or newly disabled controls lose both. `focus`, `blur`, `focused_key`, and wrapping `focus_next(backwards)` provide deterministic preorder focus. Interactive semantic roles can be focused even without pointer handlers. Pointer down focuses an enabled target and records `pressed_key`; call `release_pointer()` on pointer up/cancel. Clicking empty space blurs; clicking a disabled blocker does not. Keyboard activation, hover, pointer-up actions and focus-ring styling remain host responsibilities.

Semantic nodes include the nearest emitted semantic ancestor key, full bounds, clip, metadata, and synchronized focus state. This is a portable accessibility snapshot, not an OS accessibility bridge. Only visible semantic nodes are emitted. `checked` and labels are application-supplied; the runtime does not mutate application state.

### Failure and scope

All layout errors are returned as `LayoutError` (invalid dimensions, colors, measurements, incompatible exact root width, duplicate/empty keys, depth/node limits, or Taffy failures). Description depth is capped at 128 and node count at 100,000. Rejected descriptions are disposed iteratively on all preflight errors, including input errors that precede depth validation, so arbitrarily deep rejected trees do not recursively overflow the stack during destruction. Accepted descriptions have bounded depth before layout construction or ordinary destruction. Inputs must be finite; lengths nonnegative, percentages fractional in `0..=1`, RGBA components in `0..=1`, and font size/line height positive. The host chooses how to display/log failures; there is no panic-based layout error handling.

The runtime retains its private Taffy nodes and text caches across reconciliations. `set_tree` updates the description, `present` supports origin/clip-only changes, and `invalidate_presentation` requests new presentation output; measurement reuse is controlled by `MeasurementKey` and `TextMeasurer::cache_key`, with work exposed through `RuntimeStats`. Separate modules provide typed component ownership, grapheme input/undo/IME state, basic widgets, variable-height list indexing, and cancellable owner-scoped mailboxes with host-supplied executors. These APIs do not establish full shaping/bidi, application-wide scheduling, native/browser accessibility, or renderer/backend conformance.

## Verification

`cargo test -p neoui` exercises intrinsic sizing, wrapping, Unicode fallback, newline/empty-line height, padding, flex distribution, fractional geometry, stack z-order, clipping, semantics/disable inheritance, keyed focus, stale events, clear/failure lifecycle, and mixed production-style boolean rows. No app or GPU is needed.
