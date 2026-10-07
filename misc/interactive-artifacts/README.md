# Experimental interactive HTML artifacts

The first integration targets the native agent chat. A completed fence with the exact `neoism-html` info string reserves a 320-pixel browser viewport (before UI scaling). Ordinary `html` fences remain source code. Markdown-file embedding is not wired yet.

## Opt-in

The experimental integration is parked and **off by default**. Normal main-app/debug builds do not include the Servo host, and both browser packages are excluded from normal workspace builds. All implementation and cached worker artifacts remain available for future work.

To resume explicitly, build the desktop with `--features servo-artifacts`, build the independent worker, and launch with `NEOISM_SERVO_ARTIFACTS=1`. The runtime locates `neoism-servo-runtime` beside the desktop executable, through `NEOISM_SERVO_RUNTIME`, or (debug only) in this checkout's `neoism-frontend/servo-runtime/target/debug`. The environment flag alone cannot enable a desktop compiled without the feature.

The agent server must inherit `NEOISM_SERVO_ARTIFACTS=1` to include the built-in artifact/CSS-variable instructions in model context. An already-running external server does not acquire a new environment variable from the desktop. The guidance follows the existing system-prompt plugin lifecycle and does not enable browser execution by itself.

The worker is an independent workspace at `neoism-frontend/servo-runtime`. This keeps Servo's native FreeType dependency separate from Sugarloaf's font stack. Its compilation check is `cargo check --manifest-path neoism-frontend/servo-runtime/Cargo.toml`; the desktop workspace retains its existing toolchain.

This is an experimental, policy-restricted browser host, not an established security boundary. The initial Servo embedding runs in a separate worker process without a verified OS sandbox. Only enable it for HTML you trust. Process separation and network/resource restrictions do not protect against browser-engine vulnerabilities or hostile JavaScript exhausting CPU or memory.

## Chat syntax

A completed assistant-message block can contain a self-contained HTML document:

````markdown
```neoism-html
<!doctype html>
<html>
<body>
<button id="counter">Clicks: 0</button>
<script>
let clicks = 0;
const button = document.getElementById('counter');
button.addEventListener('click', () => {
  button.textContent = 'Clicks: ' + (++clicks);
});
</script>
</body>
</html>
```
````

Incomplete streaming fences remain inert code until their matching closing fence arrives, show at most 14 source lines, and reserve the same fixed-height slot as the final preview. The source is retained. Rendered previews have no extra header; the copy-source action is available inside the unavailable fallback only.

## Theme compatibility

Artifacts should use the selected app theme's CSS variables instead of hardcoded colors. The host supplies the actual palette, including custom theme and chat-style overrides, and updates it without reloading the document. Color variables contain complete CSS colors, so use `var(--background)` directly rather than wrapping it in `hsl(...)`. CSS-based content follows the variables automatically; JavaScript-rendered charts should re-read computed values when `neoism-theme-changed` fires on `window`.

Use `--background` and `--foreground` for the page, `--card` and `--card-foreground` for surfaces, `--muted` and `--muted-foreground` for secondary content, `--border` and `--input` for boundaries, and `--primary`/`--primary-foreground` or `--accent`/`--accent-foreground` for emphasis. Status colors are `--success`, `--warning`, `--destructive`, `--destructive-foreground` and `--info`; chart series use `--chart-1` through `--chart-6`. Typography and rounding use `--font-sans`, `--font-mono` and `--radius`.

```css
body {
  background: var(--background);
  color: var(--foreground);
  font-family: var(--font-sans);
}
.chart-series { fill: var(--chart-1); }
```

```javascript
function redrawChart() {
  const style = getComputedStyle(document.documentElement);
  const seriesColor = style.getPropertyValue('--chart-1').trim();
  // Redraw canvas/chart-library content with seriesColor.
}
window.addEventListener('neoism-theme-changed', redrawChart);
```

The theme channel changes presentation only. It does not grant access to filesystem, credentials, agent tools or the daemon. Theme updates must preserve form values, chart zoom and other JavaScript state. Font variables carry selected family names, not font binaries: the family must be available to the worker, and bundled/custom-font parity still needs runtime validation.

## Smoke test

`smoke-test.html` is self-contained and uses no external resources. Its contents are intended for a completed `neoism-html` assistant-message fence. This fixture does not itself publish an artifact or enable arbitrary HTML-file previews.

Check that button clicks increment the count, typing stays out of the composer, animation runs without pointer movement, and the select popup behaves correctly. Resize the pane and confirm the click count survives. Scroll the chat and verify clipping and hit targets agree. Click outside the artifact and confirm normal chat input resumes. Test non-ASCII input and IME separately; basic text input is not proof of complete composition support. Switch sessions and close the pane to check lifecycle cleanup.

## Architecture

Shared Rust code owns block parsing, fixed layout, clipping and artifact requests. The desktop host owns Servo instances and input. Servo paints into owned RGBA frames; Sugarloaf's existing image-overlay path composites those frames into the native interface. The initial CPU frame transport avoids requiring a new wgpu version or forcing the desktop onto the wgpu backend. Accelerated transport can replace frame delivery later without changing the chat block contract.

The initial resource policy uses synthetic document origins, denies external resource requests and privileged application access, and does not expose a general tool/daemon bridge. These restrictions must remain separate from any future OS sandbox work.

## Performance

Continuously animated artifacts publish at most 30 frames per second. Static input changes remain event-driven rather than waiting for an animation interval. Logically hidden documents are throttled inside Servo while their dedicated compositor scenes remain intact, preserving state without recreating the white-frame scroll regression.

The real-worker llvmpipe benchmark reduced publication from 122.66 to 26.67 FPS, visible CPU time from 3.09 to 1.79 seconds over a 1.5-second sample, and hidden CPU time from 0.23 to 0.02 seconds over a 1.2-second sample. Static input stayed within roughly 16-27 milliseconds. These are controlled worker measurements, not a whole-app benchmark.

Desktop presentation is frame-driven: accepted frames share a pending notification, unchanged status and successful control writes do not wake the GUI, and static previews do not own continuous window animation. Opaque rows copy without channel division, and externally revisioned stream handles do not hash full pixel buffers. Static assets retain their content-derived identity.

Native Vulkan updates reuse same-sized image, descriptor and staging resources. Transfers are still synchronous; fully frame-integrated asynchronous uploads remain further work. Ordinary static images do not retain a staging allocation after their first upload.

## Verification

The debug desktop and actual worker are built. The real-worker harness at `neoism-frontend/servo-runtime/tests/live_worker.py` passes on the default Mesa adapter and forced llvmpipe: actual RGBA pixels, button clicks, text input, autonomous animation, theme/resize state retention, waking from static idle, zero local-network requests, protocol-safe stdout and clean shutdown. The user also confirmed that the preview renders in Neoism. This does not replace validation of the surrounding timeline, scrolling, focus and platform-specific input.

The default shared/desktop build and `cargo check -p neoism --features servo-artifacts` pass. Focused tests pass: twelve artifact-filtered shared tests, thirty-four shared timeline tests, twenty IPC/theme/host contract tests, ten desktop geometry/frame/lifecycle/theme tests, and one conditional agent-guidance test. The fixture's click, input, animation and state-preserving theme-update JavaScript handlers also pass a lightweight Node check.

The broader shared test command currently encounters an unrelated `PanelContext` initializer missing `plugins` in `shared/tests/file_tree.rs`; the artifact tests were run with `--lib` to avoid compiling that integration test.

## Not yet release-ready

The independent engine worker passes `cargo check --locked --all-targets` on Rust 1.92, three isolated theme-script contract tests, and the actual rendering/input harness on Mesa and llvmpipe. Cross-platform behavior, native IME candidate/selection feedback, native controls/popups, bundled-font parity and an effective OS sandbox remain unverified or unsupported. Automatic height, durable publication/preview tools, Markdown-file embeds and accelerated texture transport are subsequent integration work. The experimental opt-in remains required.
