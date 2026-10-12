# Live HTML artifact IPC

`neoism-interactive-artifacts` is the browser-neutral document, input, theme and frame contract. Its optional `host` feature adds a nonblocking native worker facade; `ipc` exposes only the bounded pipe protocol. Neither feature links CEF, Servo, wgpu or a windowing library.

## Desktop host

```rust
use neoism_interactive_artifacts::worker_host::Host;
```

`Host::new(waker)` launches `neoism-cef-runtime` and returns without waiting for Chromium startup. The executable comes from `NEOISM_CEF_RUNTIME`, the Neoism executable directory, or, in debug builds, `../cef-runtime/target/debug`. There is no shell invocation, PATH search, automatic compilation or GUI-thread pipe I/O. `NEOISM_CEF_DIR` or `CEF_PATH` supplies an optional native-library directory for the child without modifying the desktop's loader environment.

The native desktop automatically enables artifacts only when the existing worker lookup resolves to a file (cached until app restart). `NEOISM_HTML_ARTIFACTS=1` bypasses that capability gate and permits unavailable-worker diagnostics; each launch/retry resolves the worker path again. `NEOISM_HTML_ARTIFACTS=0` disables them. Server-side artifact guidance requires explicit `=1`, independently of desktop automatic activation. The worker is invoked with `--stdio --experimental` but Chromium sandboxing stays enabled unless the separate development-only sandbox override is explicitly set. Its application policy and any OS sandbox are separate from the IPC contract; `Host::sandbox_status()` conservatively does not claim verified sandbox protection.

Debug builds also discover the standard `$HOME/.local/share/cef` install automatically. Linux debug desktop compilation prepares the independent worker; no compilation happens inside `Host::new`.

## Documents and frames

`reconcile(ArtifactDocument)` creates or updates a document. The document carries a key, HTML, source revision, viewport, visibility, appearance and validated `ArtifactStyles`. Same-revision updates preserve live state; different source requires a new revision. Geometry, visibility and theme changes must not reload the page.

`Viewport` is physical width/height plus physical pixels per CSS pixel. Viewports and HTML sizes are bounded before engine work. The host assigns request generations independently of source revisions, rejecting obsolete geometry, theme, destroyed/recreated keys and hidden-frame replies.

`pump()` drains the latest-per-key frame mailbox and diagnostics. Frames contain tightly packed, top-down, premultiplied RGBA8 pixels. The desktop unpremultiplies them when importing into Sugarloaf. Continuous worker painting must not own an unconditional desktop redraw loop: accepted frame damage wakes the GUI, and notifications coalesce until drained.

Pipe packets have a bounded length prefix. JSON controls and frame metadata are followed by binary RGBA packets. Frame layouts, generations and packet lengths are checked before pixel allocation. Startup/crash/protocol errors become persistent `Error::Backend` values; ordinary console diagnostics appear in `PumpOutput::diagnostics`.

## Input

Input includes pointer movement/buttons/leave, wheel, focus, keyboard events, native composition events and composition dismissal. Pointer coordinates and pixel wheel deltas are view-local physical pixels. Wheel deltas follow DOM convention: positive means right/down. The engine adapter handles the browser-specific scale and sign conversion.

Keyboard types are re-exported from `keyboard-types`, not the windowing crate. Forward composition start/update/end rather than duplicating a committed string as ordinary key input. Native candidate-window placement and platform-specific text selection feedback require separate validation.

## Lifecycle

`destroy(key)` releases one document; `shutdown(self)` consumes the facade. Drop queues graceful shutdown and schedules a background reaper with a 500ms grace period. No engine teardown or pipe-thread join blocks the GUI. Each newly constructed facade launches an independent worker.

The CEF worker lives in the independent `../cef-runtime` workspace. The previous `../servo-runtime` remains parked for reference and is not selected by the desktop. The browser binary is built and packaged independently, so normal desktop compilation does not compile Chromium.

See [README.md](README.md), [API_THEME.md](API_THEME.md), and [../cef-runtime/README.md](../cef-runtime/README.md).

### Intrinsic content height (worker protocol v2)

`PumpOutput.layout_metrics: Vec<ArtifactLayoutMetrics>` carries latest-per-key `ArtifactLayoutMetrics { key: String, revision: u64, height: f32 }`. Height is in unscaled browser CSS pixels, independent of the physical viewport and scale. The worker wire output is `ContentHeight { key: String, generation: u64, revision: u64, height: f32 }`. The host only accepts finite positive heights at most 8192, matching the current desired revision and generation, for visible live keys. Changed accepted metrics wake the GUI; repeated static measurements do not. Geometry/source transactions and destroy clear pending metrics and reset deduplication, allowing a new-width/revision transaction to report the same height again. Frames and metrics are separate mailboxes: either arrival order is supported.

Cache intrinsic heights in the caller by source revision + layout width + scale, not fitted height. Keep the caller cache across hidden state, never reuse it for a new width, and resize the worker to the final fitted physical viewport. The host does not automatically reapply old layout metrics. Protocol v1 workers are incompatible; the required handshake is `Hello { version: 2 }`. Theme wire schemas and tokens are unchanged.
