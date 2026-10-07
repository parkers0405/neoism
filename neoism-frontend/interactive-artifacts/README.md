# Neoism interactive artifacts

Platform-neutral artifact contract plus an optional asynchronous **process facade** for the real Servo engine. This crate has **no Servo dependency at all**, even optional. Cargo resolves native `links` constraints for optional dependencies too: Servo v0.5 requires FreeType 0.23, whereas Neoism/Sugarloaf's font-kit requires 0.20. The engine must live in the separate `../servo-runtime` workspace. No font-kit patch, fake engine or native-link hack is used.

## Features and integration

Default features are empty; only keyboard-types is compiled. `servo` enables the frontend IPC facade, serde/serde_json and keyboard event serialization, not any engine or graphics dependency. `ipc` exposes the common bounded pipe protocol for the worker without enabling the facade. Native `servo_host::Host` is absent on wasm. Platform-neutral shared contract types use no `std::time` calls. Neither crate depends on wgpu or neoism-window. See [API.md](API.md) for the stable desktop contract and `examples/smoke.rs` for an executable proxy-to-real-engine pixel check.

Parent owns workspace/dependency/desktop wiring and must gate Host construction behind explicit experimental runtime opt-in. Package the separately built `neoism-servo-runtime` executable beside Neoism or set `NEOISM_SERVO_RUNTIME` to its exact executable path. The frontend never builds, downloads, PATH-searches or shell-launches a helper. Missing helpers fail clearly. Engine initialization and crash/protocol failures arrive as persistent `Err(Error::Backend(...))` from pump; ordinary diagnostics remain in its mailbox. No GUI thread waits for an engine response.

`reconcile` validates document/key/viewport, creates keys and changes revisions; same revision with changed HTML is an error. Unchanged requests are no-ops. Theme, visibility and resize updates are supported. All pipe writes/reads run on dedicated threads. Request admission uses `try_send`, a 32-command queue and a 64MiB queued-byte budget (plus one bounded in-flight packet); backpressure is a retryable error. Destroy removes local state immediately and retries worker destruction if the queue is full. Reader handoff keeps only the latest frame per registered key, at most 16 views. GUI `pump` drains received frames only and never performs pipe I/O; snapshots are owned top-down premultiplied RGBA8 `Arc<[u8]>` suitable for existing Sugarloaf Vulkan/Metal uploads. Old request generations (including theme/scale/visibility changes and destroy/recreate under the same key/revision), viewport, hidden and destroyed-key frames are discarded. Worker frame sequence numbers increase across WebView replacements.

Frames/control use protocol version 1, little-endian packet type/length headers, bounded JSON metadata/control (32MiB) and separate tightly packed binary RGBA packets. Layout checks, checked arithmetic and advertised byte-length validation occur before allocating frame bodies. Viewports are at most 8192 per side, at most 64MiB/frame, finite scale 0.25–8.0; HTML is at most 4MiB, keys at most 1024 bytes. These limits bound transport, not page memory/CPU usage. Native input includes pointer/mouse/wheel, complete keyboard-types key events and IME composition/dismissal; candidate/selection UI feedback is not implemented.

Drop/shutdown queues graceful shutdown, closes stdin when the background writer drains, and schedules background reaping with a 500ms grace before kill/wait. It never waits for upstream Servo teardown on the GUI thread. Multiple windows should ordinarily share a facade; each newly constructed facade starts an independent worker and does not inherit Servo's single-process-lifetime initialization restriction.

## Security status

`SANDBOX_STATUS` / `Host::sandbox_status()` explicitly report `os_sandbox: false`, `in_process: false`. Separate processes solve dependency collisions and allow engine termination, **not** filesystem/network privilege isolation. A malicious engine exploit still runs with the user's privileges. No bwrap/Seatbelt/job restrictions, resource quotas or audited security boundary are supplied. The worker is experimental trusted-content only; parent must not label it a sandbox or automatically render hostile chat HTML.

The real engine creates unique synthetic HTTPS `.invalid` document origins; no file URLs, local web server or privileged bridge are used. Its resource delegates serve the exact initial document GET once and cancel every other intercepted request, including unassociated/worker loads. Navigation, new windows, permissions and native clipboard are denied. CSP permits inline JS/CSS for interaction while denying external resources, connections, frames, workers, forms, objects, fonts and media. Interception precedes native protocol handlers (including file/data/blob) in the pinned Servo source; CSP additionally restricts direct connection paths such as WebSocket. This coverage remains unaudited and is not proof of containment. Image data URLs, CDNs and external assets are deliberately unsupported; inline SVG/CSS works. No privileged bridge or height message channel exists. Theme uses the actual selected `ArtifactStyles` palette/fonts/radius snapshot as standard CSS variables, plus `prefers-color-scheme`. Initial variables/default body and font CSS are installed before author code; theme/styles/viewport/visibility updates preserve source revision, DOM and JS state. A host-controlled style-only script dispatches `neoism-theme-changed` on `window` for canvas/SVG charts to re-read colors. Loading updates wait for the committed synthetic-origin document and retry a false evaluation result rather than losing the latest desired snapshot. See [API_THEME.md](API_THEME.md).

## Verification

On working Rust 1.92, isolated frontend checks pass **7 default tests and 20 `servo`/IPC tests**, including malformed/oversized packet headers, frame layout/length/key checks, stale revisions and hidden/destroyed-key handoff. No release build was used. Worker verification is recorded in `../servo-runtime/README.md`.

After parent wiring:

```sh
cargo test -p neoism-interactive-artifacts --no-default-features
cargo test -p neoism-interactive-artifacts --features servo
cargo check -p neoism-interactive-artifacts --features servo --all-targets
# Requires a separately installed worker and functioning native software adapter:
NEOISM_SERVO_EXPERIMENTAL=1 NEOISM_SERVO_RUNTIME=/absolute/path/neoism-servo-runtime cargo run -p neoism-interactive-artifacts --features servo --example smoke
```

The smoke driver opts in explicitly, serves a synthetic red HTML page through the real helper, and checks returned RGBA pixels. It is not an automatically executed mock-engine test. Servo/native prerequisite failures remain worker diagnostics, not a fallback renderer.
