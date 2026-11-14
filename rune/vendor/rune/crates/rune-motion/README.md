# rune-motion

Rune's animation runtime: springs, tweens, decay, keyframes, retargeting, presence and
transitions (blur-replace included), layout animation, matched geometry and choreography. It is
plain math and data structures with no renderer dependency and no required dependencies (the
optional `serde` feature covers the config types). App code states targets. The render thread
samples them once per frame and gets back what to draw.

```rust
use rune_motion::prelude::*;

let mut scene = Scene::new();
let card = NodeId(1);

// The first build never animates.
scene.insert(card, NodeSpec::new(Rect::new(0.0, 0.0, 320.0, 80.0)).radius(12.0), Mount::Initial);

// Later: a new row blurs in place, the card reflows with width leading, then something leaves.
let row = NodeId(2);
scene.insert(row, NodeSpec::new(Rect::new(0.0, 96.0, 320.0, 44.0))
    .parent(card)
    .transition(Transition::blur_replace()), Mount::Animated);
scene.set_frame(card, Rect::new(0.0, 0.0, 360.0, 160.0), LayoutAnim::new(motion::MORPH).width_leads());
scene.remove(NodeId(3));                       // stays drawn until its exit settles

let out = scene.frame_at(1.0 / 120.0);         // render thread, once per frame
for p in out.changed { /* draw p.rect, p.opacity, p.affine(), p.blur, p.mask … */ }
```

Without a scene graph, the store animates any property directly:

```rust
let mut store = AnimationStore::new();
store.set(id, prop::OPACITY, 0.0);
store.animate(id, prop::OPACITY, 1.0, motion::SETTLE);            // spring, bounce 0
store.animate(id, prop::FRAME, rect, motion::MORPH.delay(0.05));  // edges, delayed
store.animate(id, X, 40.0, Curve::ease_in_out(0.3).completion(7)); // tween, Completed{token: 7}
store.release(id, X, fling_v, Release::DecayClamped { decay: Decay::IOS_NORMAL, min: 0.0, max, spring: motion::SNAP });
store.tick_at(now);
let x = store.get(id, X);
```

## API overview

| Piece | What it is |
|---|---|
| `Spring` | SwiftUI `Spring(duration:bounce:)`, bounce 0 by default, closed form in every damping regime. `sample`, `settling_duration`, `css_linear` (WAAPI export), plus constructors from response/damping fraction, ω/ζ, stiffness/damping/mass and Motion `visualDuration`. |
| `Curve` / `Anim` | `Spring`, `Tween` (CSS cubic-bezier, steps, linear), `Decay` (iOS rates), `Keyframes` (Hermite cubics that keep velocity across keys), `Instant`. `Anim` adds `.delay()`, `.additive()`/`.merge()`, `.velocity()` and `.completion(token)`. |
| `motion::*` tokens | `INSTANT SNAP SETTLE REFLOW GLIDE MORPH POP DRIFT CAMERA`, each with an `_EXIT` partner 10–25 % shorter (copied from Kit's motion presets), plus `motion::pair::*` `SpringPair`s and SwiftUI presets. |
| `Animatable` | `f32`, `Vec2`, `Rect` (four edges), `Color` (premultiplied OKLab), `Corners`, `Sides`, `Transform`, `Blur`, tuples and arrays. Up to `MAX_LANES` f32 lanes. |
| `AnimationStore` | Every animated value is a track. Retargeting keeps position and velocity. Also delays, settle and completion events, gesture `release` (spring, decay, decay-into-bounds), `retune` for live token edits, and reduced motion per `PropClass`. |
| `Scene` | Keyed nodes with presence (exits stay mounted, revive mid-exit without a jump), `Transition`s (`opacity scale offset move push slide blur_replace reveal asymmetric combined`), layout by edges (`Travel::Parent`) or one straight screen path (`Travel::Screen`), `match_geometry`, popLayout. |
| Choreography | `Choreography` steps cue on `Cue::now/at/started/arrived/settled(step).after(s).not_before(t)`. Also `Stagger` (from start, end, centre or an index, eased), `PhaseAnimator` and `Keyframes`. |
| Gestures | `VelocityTracker`, `project`, `rubber_band*`, `deceleration`. |
| `Animated<T>` | A single value outside any store, for one-off widgets. |
| `Clock` | A display clock with pause, scale and seek, or a manual clock for tests and offline renders. |
| `LoopTrack` / `LoopSpec` | Periodic eased opacity or clockwise rotation, sampled at clock time. `retime` preserves phase; `enabled` springs the pulse amplitude toward rest. The renderer owns visibility and scheduling. |
| `MotionTrace` | motion-trace JSONL. `MotionTrace::from_env` opens `$RUNE_MOTION_TRACE`. |

### Kit's motion rules are the defaults

- Springs are duration plus bounce, with bounce 0 by default. Exits run 10–25 % shorter than entrances.
- Interrupting a motion keeps its position and velocity.
- Nothing animates on first mount. A track's first `animate` acts as `set`, and setting an unchanged target does nothing.
- The user's direct manipulation is never animated. Gestures write values with `set` and animate only on `release`.
- Under reduced motion, travel, scale, shape and blur change instantly, and opacity, colour and presence take a 0.18 s ease-in-out fade (`REDUCED_FADE`). Gesture values keep their curve. `match_geometry` turns into a crossfade in place.
- Both edges of a rect snap to 1/64 device px, and settle thresholds scale with the pixel density.
- Blur is limited to 4 px and only applies to things entering or leaving in place. A transition that travels drops its blur. Scale stays within [0.9, 1.1] and never goes to 0; `Transition::allow_small_scale()` lowers the floor to 0.5 for small glyphs. A removed node always fades out (`Effect::kit_rules`).
- Width leads: `.width_leads()` runs the X spring at 0.78× the duration (taken from a launcher app).
- A stationary edge gets no motion at all.
- A node given `Scene::set_ride_trailing` keeps its place against its parent's trailing edge while the parent's width animates; its frame is still given in parent space.

## Design

**Analytic tracks.** Each value lane is `target + Σ offset layers`. A layer is a closed-form
segment: an underdamped, critical or overdamped spring, a decay, a tween or a Hermite cubic. Each
one is evaluated directly at the frame's timestamp. Nothing is integrated step by step, so
results are the same at any frame rate. Sampling at the display link's *target* timestamp is
exact, frames can be skipped, and tests can jump to any `t`. Each layer stores its own settle
time, an upper bound that holds for the whole tail (spring envelopes, or bisection for the
critical and overdamped cases). Once that time passes, the track snaps to `(target, 0)`, leaves
the active list and costs nothing.

**Merge vs additive.** Springs, decay and keyframes *merge* by default: the store samples
`(x, v)` and starts one new segment from there, like SwiftUI spring merging or Compose
`animateTo`. That keeps one layer per lane and O(1) sampling. Tweens *add* by default, Core
Animation style: the running curves continue, and a new layer animates the target delta to 0, so
every curve keeps its shape. A test checks that additive springs give exactly the same result as
merged springs. A lane holds at most 8 additive layers. Past that, they collapse into one merged
segment.

**Struct of arrays.** Track data lives in flat lane arrays (layer, target, value, velocity), with
generational `TrackId`s, a dense `active` list, free-list pools for extra layers and delayed
starts, and an Fx-style hash map from `(node, prop)` to track. A tick walks only the active
tracks, reads contiguous memory and does not allocate. `tests/alloc.rs` checks this with a
counting allocator.

**Deterministic math.** `math::exp` and `math::sin_cos` are custom polynomial versions (Estrin's
scheme, FMA on aarch64 or `+fma`, branchless quadrant fix-up). They don't depend on the
platform's libm, so traces and goldens stay stable, and they're cheap: 5 ns for `exp` and 7.6 ns
for `sin_cos`.

**Presence.** One `PRESENCE` progress (0 means absent, 1 means present) drives the insertion and
removal effects. If a node is reversed mid-flight (it comes back while exiting, or leaves while
entering), the effect endpoints are re-solved at the current visual state so nothing jumps.
`Travel::Screen` interpolates root-space start and end edges with a separate progress per axis.
`match_geometry` starts the target node at the source's exact presented rect, velocity, radius
and fill, then removes the source immediately.

## Name mapping

| SwiftUI | Motion (motion.dev) | rune-motion |
|---|---|---|
| `Spring(duration:bounce:)`, `.smooth/.snappy/.bouncy` | `{ type: "spring", visualDuration, bounce }` | `Spring::new(d, b)`, `Spring::SMOOTH/SNAPPY/BOUNCY`, `Spring::from_motion(vd, b)` (Motion `visualDuration` = `duration / 1.2`) |
| `.spring(response:dampingFraction:)` | `{ stiffness, damping, mass }` | `Spring::from_response`, `Spring::from_stiffness_damping` |
| `.easeInOut(duration:)`, `.timingCurve` | `ease: [x1,y1,x2,y2]` | `Curve::ease_in_out(d)`, `Curve::tween(d, Easing::cubic(..))` |
| `withAnimation(_:completion:)` | `animate(...).then` | `.completion(token)` → `MotionEvent::Completed` |
| `.animation(..).delay(s)` | `delay` | `.delay(s)` |
| `.transition(.opacity/.scale/.move(edge:)/.push(from:)/.blurReplace)` | `initial/animate/exit` | `Transition::opacity/scale/move_edge/push/blur_replace` |
| `.asymmetric(insertion:removal:)`, `.combined(with:)` | separate `initial`/`exit` | `Transition::asymmetric`, `.combined` |
| `if`/`ForEach` removal | `AnimatePresence` (`mode="popLayout"`) | `Scene::remove`, `Transition { pop_layout: true }` (the default) |
| implicit layout animation | `layout` | `Scene::set_frame(id, rect, anim)` |
| `matchedGeometryEffect(id:in:)` | `layoutId` | `Scene::match_geometry(from, to, anim)` |
| `keyframeAnimator`, `KeyframeTrack` | keyframes `[a, b, c]` + `times` | `Keyframes`, `Curve::Keyframes` |
| `phaseAnimator` | `repeat` variants | `PhaseAnimator` |
| `ScrollView` deceleration | `inertia` | `Release::Decay` / `DecayClamped`, `Decay::IOS_NORMAL/IOS_FAST` |
| `accessibilityReduceMotion` | `useReducedMotion` | `set_reduced_motion(true)` |
| — | `stagger(gap, { from })` | `Stagger::new(gap).from(..)`, `Choreography::stagger` |

## Benchmarks

`cargo run --profile perf --example bench`. Every track stays active for all 120 ticks at 120 Hz.
Each figure is the best of 9 runs on an Apple M4 build machine, which was busy with other
jobs at the time, so treat these as upper bounds.

| Scenario | 10k tracks | 100k tracks |
|---|---|---|
| scalar spring, critical (bounce 0) | **16.8 ns** | 17.6 ns |
| scalar spring, underdamped (bounce 0.3) | 23.6 ns | 25.0 ns |
| scalar tween (bezier ease-in-out) | 45.6 ns | 47.0 ns |
| colour (4 OKLab lanes) | 37.1 ns | 38.6 ns |
| rect frame (4 edge lanes, MORPH) | 75.5 ns | 77.7 ns |
| mixed (spring, tween, 2× additive) | 34.3 ns | 35.3 ns |
| scene node, layout reflow → `Presented` | 132 ns (1k nodes) | 144 ns (10k nodes) |
| `Spring::sample` standalone | 27.8 ns/call | |

At these speeds, 10k simultaneously moving rects take 0.76 ms per frame and a 1k-node scene
reflow takes 0.13 ms. A steady-state tick allocates nothing. The kernel micro-bench
(`cargo test --profile perf -- --ignored bench_layer_kinds`) measures one underdamped lane at 17 ns,
one critical lane at 10 ns, `exp` at 5 ns and `sin_cos` at 7.6 ns.

## Renderer integration contract

Run this on the render thread once per display-link callback:

1. `scene.set_time(target_timestamp)`. Use the timestamp the frame will be *shown* at, not the
   current time. `Clock::frame(host)` handles pause, slow motion and seek.
2. Apply the commits that arrived since the last frame: `insert(id, spec, mount)`, `remove(id)`,
   `set_frame(id, rect, anim)`, `animate(id, prop, target, anim)`, `set(...)` and
   `match_geometry(from, to, anim)`. Pass `Mount::Initial` for the first build.
3. `let out = scene.frame()`, which returns a `FrameOutput`:
   - `changed: &[Presented]`: only the nodes whose presented values changed, parents before
     children.
   - `exited: &[NodeId]`: nodes whose exit settled. Stop drawing them and free their resources.
   - `events: &[MotionEvent]`: `Settled` and `Completed { token, finished }`, for app callbacks.
   - `animating: bool`: when false, pause the display link until the next commit.

`Presented` holds everything needed to draw one node:

| Field | Meaning |
|---|---|
| `rect` | Root-space rect. It already includes every ancestor's translation and this node's own, and both edges are snapped to 1/64 device px. Draw shapes, borders and shadows at this size. |
| `opacity` | This node's own opacity (presence × property). Apply ancestors' opacity as group opacity. |
| `transform`, `anchor` | Scale and rotation about the root-space `anchor`, composed down the tree like `CALayer`. `affine()` returns the 2×3 matrix. |
| `blur` | Own plus presence Gaussian sigma in points. 0 means attach no filter. Presence effects clamp to ≤ 4 pt; `blur_replace` peaks at 1.5 pt. Explicit property blur can remain at rest. |
| `radius`, `fill` | Animated corner radii, and the fill as sRGB with straight alpha (interpolated in premultiplied OKLab). |
| `mask` | `Mask { inset: Sides, feather: Sides }` in the node's own coordinates: how far each side is pulled in, and the gradient width on that side. Evaluate it as an SDF ramp. `mask.is_none()` means no clip. |
| `phase` | `Entering`, `Present` or `Exiting { generation }`. |
| `in_flow` | Turns false as soon as a popLayout exit starts. App layout should stop reserving space for the node. |
| `flags` | motion-trace bits (`flags::ANIMATED`, `ANCESTOR_ANIMATED`, `CLIPPED`, `FILTERED`) and intent bits (own `<< 8`, ancestor `<< 16`). |

Call `scene.set_pixel_scale(scale)` on display changes and `scene.set_reduced_motion(on)` when the
system setting changes. Setting `RUNE_MOTION_TRACE=/path.jsonl` and calling
`scene.set_trace(MotionTrace::from_env(&header))` writes motion traces.

## Examples and tests

- `cargo run --example curves [dir]` writes SVG plots of the tokens, SwiftUI
  presets, curve kinds, retargeting and presence/travel paths. The current plots are in
  `demo/motion/`.
- `cargo run --profile perf --example bench` prints the numbers above.
- `cargo test` runs 53 tests and 1 doctest. They check the spring closed form against ODE
  integration in every regime, C¹ continuity under random retargets, that additive and merged
  springs match, delays, decay into bounds, generational reuse, zero allocation, presence revive,
  blur-replace, screen travel, matched geometry, reduced motion, choreography cues and trace
  output.

## Not yet

- `Travel::Screen` uses a separate progress per axis, so a width-leading screen path is not perfectly straight.
- Not implemented: resize modes (mask, scale, relayout), a live token table and `tune.json`,
  live `Bound` expressions, growth caps, flight-layer z-order, and scroll physics for platforms
  other than iOS.
- `Animated<T>` has no delay and no additive blending.
- Multi-lane values (rects, colours) cost more than the ~20 ns target per track. The cost is
  roughly per lane.

Trace diagnostics: `Scene::sampling_error()` compares presented scene edges with unsnapped
analytic samples in device pixels. `AnimationStore::audit_retargets(true)` enables inherited
spatial spring velocity checks; `take_retarget_audit()` drains the counters. These diagnostics
exclude renderer-owned growth/scroll and explicit velocity changes or instant cuts.
