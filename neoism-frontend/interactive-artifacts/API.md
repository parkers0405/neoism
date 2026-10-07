# Desktop integration contract (worker architecture)

Package: `neoism-interactive-artifacts`; optional feature: `servo` enables **IPC proxy only**, never a Servo/native-library dependency. Parent registers this crate and desktop feature wiring. The actual engine lives in the independent workspace `neoism-frontend/servo-runtime`; it must stay excluded from the root dependency graph because its FreeType links differ from Sugarloaf's.

```rust
use neoism_interactive_artifacts::{ArtifactDocument, ArtifactInput, Viewport};
use neoism_interactive_artifacts::servo_host::Host;
let mut host = Host::new(std::sync::Arc::new(|| { /* enqueue event-loop proxy wake */ }))?;
host.reconcile(ArtifactDocument {
    key: "thread:part".into(), html: "<button>Hello</button>".into(),
    revision: 1, viewport: Viewport { width: 640, height: 320, scale: 1.0 },
    visible: true, theme: neoism_interactive_artifacts::Theme::Dark,
    styles: neoism_interactive_artifacts::ArtifactStyles::default(),
})?;
let update = host.pump()?; // drains received frames, never waits for worker I/O
for frame in update.frames {
    // key, revision, sequence, width, height, stride,
    // rgba: Arc<[u8]> — owned top-down RGBA8, premultiplied alpha
}
host.input("thread:part", ArtifactInput::PointerMove { x: 20.0, y: 30.0 })?;
host.destroy("thread:part");
host.shutdown();
# Ok::<(), neoism_interactive_artifacts::Error>(())
```

**Helper packaging:** `Host::new` launches the explicit path in `NEOISM_SERVO_RUNTIME`, or `neoism-servo-runtime` (`.exe` on Windows) beside the Neoism executable. Debug builds also locate the separately built worker in this checkout's `neoism-frontend/servo-runtime/target/debug`, so cleaning the desktop target does not require another path setting. No PATH search, auto-download, auto-build, shell or local HTTP server. Missing/nonexecutable helpers produce an immediate actionable error. Engine startup, crash and protocol failures arrive asynchronously as persistent `Err(Error::Backend(...))` from `pump`; ordinary diagnostics remain in `PumpOutput::diagnostics`. Parent must visibly gate host creation behind experimental opt-in; the Host passes `--stdio --experimental` to the worker only after the caller has chosen to construct it.

`ArtifactDocument::styles` is a required typed snapshot of the selected renderer palette, fonts and radius; see [API_THEME.md](API_THEME.md). Pass the actual selected `ArtifactStyles`, not merely a light/dark default. The initial parser-blocking bootstrap installs CSS variables and default body background/foreground/sans font before author scripts. Live styles/theme changes update root properties without navigation or JS-state loss, and dispatch `window` event `neoism-theme-changed`; canvas charts should re-read computed `--chart-1` through `--chart-6` on that event. Root inline `colorScheme` overrides normal authored `:root` color-scheme rules. Latest desired styles received during loading are committed after the real synthetic-origin document completes; evaluations against a missing/wrong root are retried.

`reconcile` creates missing keys, updates styles/theme/viewport/visibility in place, and replaces synthetic documents only on changed source revision. Reusing a revision with changed HTML is rejected. Identical requests are no-ops. Requests use bounded nonblocking queues: backpressure returns `Error::Backend` asking for retry on the next tick, not a GUI-thread pipe wait. `destroy` removes local state immediately; full-queue worker destroys are retained and retried. Do not reuse revisions for different documents.

`pump` drains received snapshots and diagnostics only. The worker independently pumps on wakes and 16ms animation ticks, waiting for events while static/hidden. The wake callback can run on reader/writer threads and must only enqueue GUI work, not call Host reentrantly. Host remains thread-affine. `PumpOutput { frames, animating, diagnostics }` contains changed visible frames only; retain the latest upload until replaced or destroyed. `snapshot(key)` clones the latest frame drained by `pump`. Late frames from destroyed keys, old revisions or obsolete viewport sizes are discarded. An internal generation changes on theme/viewport/scale/revision/visibility updates and destroy/recreate, so even equal-key/equal-revision/equal-size old packets are rejected. Worker frame sequences are monotonic across WebView replacements; public API fields stay unchanged. No native GPU handles escape.

Input variants: `PointerMove { x, y }`, `PointerButton { x, y, button: PointerButton, state: ButtonState }`, `PointerLeave`, `Wheel { x, y, delta_x, delta_y, unit: WheelUnit }`, `Key(KeyboardEvent)`, `Ime(CompositionEvent)`, `ImeDismissed`, `Focus(bool)`. Re-exported keyboard-types types do not depend on neoism-window. Pointer coordinates and wheel pixel deltas are view-local physical pixels; scale is physical pixels per CSS pixel. Wheel deltas follow DOM sign: positive scrolls right/down (the worker converts to Servo's opposite sign). IME forwards true start/update/end events; native candidate UI and input-method enable/selection feedback are not implemented yet.

`destroy(key) -> bool`; `shutdown(self)` consumes the facade. Drop closes the request queue and schedules background process reaping: 500ms grace, then kill/wait. No GUI-thread join or Servo teardown wait. The worker itself contains one process-lifetime Servo instance. A new facade can launch a new worker after shutdown.

Runtime remains **experimental, not sandboxed**. `Host::sandbox_status()` reports `os_sandbox: false`, `in_process: false`: process separation avoids native-library collisions and lets the frontend terminate the engine, but does not restrict worker filesystem/network privileges or provide audited exploit containment. Documents use unique synthetic HTTPS `.invalid` origins; interception serves only exact initial registered document GETs. All other intercepted requests are cancelled; navigation, popups, permissions and clipboard access are denied. CSP denies connections, external resources, child frames, workers and forms. No privileged bridge, file URLs, arbitrary JS evaluation API or dynamic-height channel. Fixed 320px-high viewports and theme via CSS `prefers-color-scheme` are supported.

Verification and native packaging details: README.md and ../servo-runtime/README.md.
