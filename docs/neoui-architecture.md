# NeoUI architecture on the GPUIX branch

GPUIX is the default desktop frontend. SolidJS and Effect own the frontend
application, GPUIX owns the native window and GPU submission, and Ghostty supplies
the terminal emulator. The daemon owns PTYs, Agent services, files, LSP, Git,
search, collaboration, and workspace lifetime. See the [frontend guide](../neoism-frontend/gpuix/README.md)
and its [verification gates](../neoism-frontend/gpuix/parity.json).

## Retained UI and paint boundaries

`neoui` remains a renderer-independent declarative UI core. It provides retained
keyed Taffy layout, measurement caches, typed component ownership and teardown,
capture/target/bubble routing, pointer capture, focus traps, keybindings,
grapheme editing with undo and composition state, basic widgets, variable-height
list indexing, and owner-scoped task delivery through host-supplied executors.
It creates no window or GPU device and owns no application settings or executor.

`neoism-frontend/shared` retains reusable editor and UI behavior. Its optional
`neoui` feature selects declarative Settings controls and the adapter at
`src/neoui_renderer.rs`. The adapter accepts the shared `Paint` interface;
`neoism-paint` supplies CPU font measurement, text shaping, image decoding, and
virtual-surface/layout data. The GPUIX native bridge consumes those routines.
The retired desktop, browser workspace frontend, renderer, and Lua host are not
build targets or alternate runtime paths.

The dependency direction is:

```text
GPUIX native bridge -> shared editor/UI -> neoui (optional)
                                      -> neoism-paint
                                      -> protocol/customization/terminal data
GPUIX frontend -> daemon/Agent SDK -> daemon services
```

## Host contract

All core geometry uses logical pixels. The host supplies complete-string text
measurement and performs device-scale conversion. Measurements must match the
font and shaping configuration used to paint; font and DPI changes invalidate
measurement caches. The core fallback wrapper splits long words at Unicode
scalar boundaries, so it does not establish full grapheme or bidi line breaking.

Frames contain owned paint commands, clipped hit regions, semantic nodes, and an
opaque frame token. Hosts must honor command clipping, reject input from stale
frames, and clear the runtime when a surface is hidden or replaced. Configuration
actions request a change through the application model; persistence and error
reporting remain application responsibilities.

Layout validates finite dimensions, colors, text metrics, unique keys, and root
width. Description depth and node count are bounded. Invalid deep descriptions
are disposed iteratively, including failures detected before tree validation,
to avoid recursive stack overflow. Layout failure clears stale interaction state.

The [core API guide](../neoui/README.md) describes layout, lifecycle, routing,
measurement, and semantic contracts in detail. Semantic snapshots and composition
state are portable data; native accessibility bridges, complete shaping/bidi
editing, candidate-window geometry, and full application-wide adoption require
separate integration and verification. Existing frontend visual, IME, notebook,
and performance gaps remain recorded in the frontend verification gates.

## Validation

From the repository root, use the pinned compiler:

```sh
bash scripts/blaze.sh check --locked -p neoui -p neoism-ui --features neoism-ui/neoui
bash scripts/blaze.sh test --locked -p neoui
node scripts/test-workspace.mjs --locked
```

The core tests cover layout, wrapping, fractional geometry, stacking, clipping,
semantics, disabled controls, focus, stale events, teardown, and deep invalid-tree
disposal. Headless core tests do not prove native rendering, accessibility,
visual parity, or refresh-rate performance. Preserve the root license, NOTICE,
and retained per-file upstream attributions when changing or redistributing
extracted code and font assets.
