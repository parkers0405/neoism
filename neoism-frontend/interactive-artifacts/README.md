# Neoism interactive artifacts

GPUIX does not currently host this experimental facade. The integration and
automatic desktop-worker behavior described below belong to the retired
desktop; only the contracts and independent workers remain available.

Browser-neutral contracts plus an optional asynchronous process facade for live Chromium HTML surfaces. This crate has no CEF, Servo, native graphics or windowing dependency. The engine lives in the separate `../cef-runtime` workspace; the desktop compiles only the small IPC client.

## Features

Default features are empty. `ipc` exposes the bounded protocol and serde/keyboard serialization. `host` adds the native `worker_host::Host` facade; it is unavailable on wasm. The shared contract uses no native clock calls. See [API.md](API.md) and [API_THEME.md](API_THEME.md).

The desktop's `cef-artifacts` feature is included in its normal defaults. Automatic page execution is enabled only when the existing worker lookup resolves to a file. Worker presence is cached on the first automatic probe (restart Neoism after installing a worker). `NEOISM_HTML_ARTIFACTS=1` explicitly enables execution even without a worker, preserving unavailable-worker diagnostics and fresh lookup on edit-to-retry; `NEOISM_HTML_ARTIFACTS=0` disables execution. The server advertises HTML artifact instructions only with explicit `=1`, since it cannot infer the desktop client’s installed worker. Set the flag for both desktop and server; restart an already-running external server after changing it. The worker is found through `NEOISM_CEF_RUNTIME`, beside Neoism, or in the independent debug worker target directory. `NEOISM_CEF_DIR` / `CEF_PATH` can supply separately installed CEF libraries to the child. With explicit opt-in, missing helpers fail visibly rather than falling back to Servo or silently rebuilding an engine. A file-presence check does not guarantee launchability, CEF libraries, or sandbox readiness. The separately installed worker is currently Linux-only and is not bundled in ordinary platform releases; the default facade feature is not a claim of complete cross-platform runtime support.

## Nonblocking transport

All pipe reads/writes run on background threads. Request admission uses `try_send`, a 32-command queue and a 64MiB queued-byte budget. GUI `pump` drains a latest-frame-per-key mailbox only; no browser response or teardown blocks the GUI. Notifications coalesce until drained, and static pages do not own an unconditional window animation loop.

Documents contain source revision, geometry, appearance and a validated theme snapshot. Source edits create a new revision. Resize, visibility and theme updates preserve the DOM. Host request generations reject obsolete frames even when the source revision did not change. Hidden and destroyed keys cannot publish frames into the timeline.

Frames are owned, top-down premultiplied RGBA8 buffers compatible with the existing Sugarloaf image-overlay path. Protocol version 2 uses length-bounded JSON and separate binary frame packets. Viewports are at most 8192 pixels per side and 64MiB per frame, finite scale 0.25-8.0; HTML is at most 4MiB. These are transport limits, not engine CPU/memory quotas.

## Security

Page execution remains experimental, but is enabled by default in the native desktop. The host's `sandbox_status` conservatively reports no verified OS sandbox guarantee. The worker requests Chromium sandboxing unless explicitly disabled for development; actual platform containment must be tested separately. Process separation and resource/navigation policies are not an audited security boundary. Do not enable untrusted pages based on these restrictions alone.

There is no privileged bridge to the filesystem, credentials, agent tools or daemon. Theme updates modify presentation only and emit `neoism-theme-changed` without reloading the page. Native IME candidate placement, popup controls and cross-platform behavior still need end-to-end validation. See the worker README for its exact asset policy and setup.

## Checks

```sh
cargo test --manifest-path neoism-frontend/interactive-artifacts/Cargo.toml --features host
cargo check --manifest-path neoism-frontend/interactive-artifacts/Cargo.toml --features host --all-targets
cargo run --manifest-path neoism-frontend/interactive-artifacts/Cargo.toml --features host --example smoke
```

The facade has 27 passing unit tests covering malformed packets, theme escaping, stale generations, hidden/destroyed keys, notification coalescing and persistent worker errors. The smoke example requires an installed worker and checks actual returned red pixels. It is not a mock-engine test. Worker rendering/input tests are documented in `../cef-runtime/README.md`.

Linux debug desktop builds also prepare the worker automatically when a CEF distribution is installed, using its independent target directory. Debug launches discover `$HOME/.local/share/cef` without an override. Cold browser installation and release packaging remain explicit.
