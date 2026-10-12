# Live interactive HTML artifacts

Native agent chat renders completed `neoism-html` fences as live Chromium pages inline. Ordinary `html` fences remain source code. The CEF worker replaces the desktop's previous Servo worker; the Servo workspace is retained but is no longer built or selected by this feature. Markdown-file embeds and the web frontend are not wired yet.

## Build and enable

Normal desktop defaults include `cef-artifacts`, which links only the small browser-neutral IPC client, not Chromium. The worker is an independent workspace at `neoism-frontend/cef-runtime` and uses prebuilt CEF binaries. It has its own cached target directory; native debug desktop builds prepare it automatically, while release distributions package it separately.

For Linux debug builds on a machine with CEF installed, `cargo build -p neoism` also prepares the separate worker automatically, and `./target/debug/neoism` renders artifacts without feature or launch flags. The automatic worker build has its own target directory and reuses cached outputs. Cross-compiles and release packaging remain separate. A missing CEF distribution produces an actionable build warning rather than downloading a large browser silently.

On a new Linux development machine, install the matching prebuilt CEF distribution once:

```sh
cargo install export-cef-dir --version 154.5.0 --locked
export-cef-dir "$HOME/.local/share/cef"
```

This downloads Chromium binaries; it does not compile Chromium. The unpacked distribution on this machine is about 1.5GB. Do not use `--force` over an existing install unless replacing it intentionally.

For an explicitly selected CEF distribution:

```sh
NEOISM_CEF_DIR="$HOME/.local/share/cef" cargo build -p neoism
./target/debug/neoism
```

`bash misc/interactive-artifacts/build-cef.sh` can still build just the worker. The debug launcher automatically finds the standard `$HOME/.local/share/cef` install; custom locations use `NEOISM_CEF_DIR` or `CEF_PATH`.

The host locates `neoism-cef-runtime` beside the desktop executable, through `NEOISM_CEF_RUNTIME`, or, in debug builds, in `neoism-frontend/cef-runtime/target/debug`. `NEOISM_CEF_DIR` / `CEF_PATH` supplies separate CEF libraries/resources during development. See the worker README for binary installation and platform prerequisites.

Artifact instructions in the native server context require explicit `NEOISM_HTML_ARTIFACTS=1`; without it, ordinary releases do not advertise an uninstalled worker. `NEOISM_HTML_ARTIFACTS=0` disables instructions and desktop rendering. An already-running external server must be restarted after changing that override. Normal builds with custom `--no-default-features` must explicitly include `--features cef-artifacts`.

The native desktop automatically enables page execution only if its existing worker lookup resolves to a file. This presence check is cached until app restart and does not verify launchability or sandbox readiness. `NEOISM_HTML_ARTIFACTS=1` forces an attempt, retaining unavailable-worker diagnostics and fresh worker lookup on edit-to-retry; `=0` turns execution off. The default facade remains enabled, but the separately installed Linux-only worker is not packaged in ordinary platform releases; cross-platform runtime support is not complete. The worker requests Chromium sandboxing unless a development-only override disables it; failures must not silently downgrade security. Process separation, CSP and navigation restrictions are not an audited security boundary. Only enable content you trust until containment is verified.

## Chat syntax

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

Incomplete streaming fences remain inert until the closing fence arrives and display a skeleton instead of HTML source. Completed previews have no extra browser window, browser chrome or forced panel backdrop. They are actual browser documents, not screenshot attachments or CDP screencasts. The initial skeleton is 160 CSS pixels high; the host then fits the surface to its measured content, bounded to 32-640 CSS pixels (and the raster allocation limit). Taller documents scroll internally. Default body margins and backgrounds are zero/transparent; authored styling is preserved.

Outer corners use an antialiased transparency mask, revealing the surrounding chat rather than painting an extra border or card. Rounded hit testing uses the same full-surface geometry, never the temporary scroll slice.

For design alternatives, emit several labeled completed fences. Each becomes an independent live page. A dedicated variants picker and model-side screenshot/console preview tool are not implemented by the engine swap.

## Theme and layout

The host supplies 42 validated CSS variables based on the selected app theme, including custom chat style overrides. Colors are complete CSS colors: use `var(--foreground)`, not `hsl(var(--foreground))`. `--accent` now means the brand accent rather than a gray hover surface. `--font-sans` uses the selected chat family or a sans-serif fallback, not the terminal font by default.

Page colors: `--background`, `--foreground`. Surfaces: `--card`, `--card-foreground`, `--popover`, `--popover-foreground`, `--secondary`, `--secondary-foreground`. Muted/boundaries: `--muted`, `--muted-foreground`, `--border`, `--input`, `--ring`. Emphasis: `--primary`, `--primary-foreground`, `--accent`, `--accent-foreground`, `--accent-surface`, `--accent-surface-foreground`. Status: `--success`, `--success-foreground`, `--success-surface`, `--warning`, `--warning-foreground`, `--warning-surface`, `--destructive`, `--destructive-foreground`, `--destructive-surface`, `--info`, `--info-foreground`, `--info-surface`. Code: `--code-background`, `--code-foreground`. Charts: `--chart-1` through `--chart-6`. Typography/rounding: `--font-sans`, `--font-mono`, `--radius`.

Theme changes update the live document without reloading it. For canvas/chart-library rendering, re-read computed variables and redraw on `neoism-theme-changed`, preserving form values, selection, filters and zoom. The theme channel is presentation-only and does not expose filesystem, credentials, agent tools or the daemon.

Use a fluid layout. Avoid an outer card, border or banner title unless the user explicitly wants a full-screen mockup. Give inset panels padding and rounded corners. Keep charts at fixed pixel heights and avoid `100vh` / `height:100%` on the root. Exact remote asset restrictions are documented in the worker README.

## Architecture

Shared Rust code owns fence parsing, timeline layout, clipping and artifact requests. The desktop owns focus, input forwarding and a nonblocking IPC host. The separate CEF worker owns Chromium documents. CEF's off-screen paint callback supplies premultiplied BGRA; the worker converts it to RGBA for the existing Sugarloaf image-overlay path. No browser engine or conflicting native dependency graph is linked into the desktop.

Requests and frame delivery are bounded and generation-checked. Latest-frame mailboxes coalesce notifications, rejecting stale, hidden and destroyed views. Same-source geometry/theme changes preserve browser state. Static previews do not keep the desktop in a continuous redraw loop. CPU pixel transport is the initial path; accelerated texture delivery can be added without changing the chat block contract.

## Verification

The facade's 27 unit tests, 17 desktop bridge tests, 12 shared artifact tests and conditional model-guidance test pass. `cargo check -p neoism --features cef-artifacts` and the debug desktop build also pass. The prior standalone CEF feasibility spike exercised live 60fps animation, real mouse click handling and Chromium rendering of grid/SVG/gradients. These spike timings are not a whole-app benchmark. The integrated worker additionally passes six unit tests and the full sandbox-default live regression: native select dropdowns, dragging, keyboard shortcuts, stale-frame checks, theme/resize preservation, and clean shutdown. The host launches it without any CEF or artifact environment flags, and the opt-in online regression confirms pinned Chart.js loads and executes from jsDelivr. Actual whole-GUI latency and cross-platform behavior are not established by these tests.

The manual UI fixture is `smoke-test.html`. Check real button clicks, typing without composer leakage, autonomous animation, select menus, state-preserving resize/theme updates, clipping while scrolling the chat, focus returning to the composer, and session/window cleanup. Test non-ASCII input and IME separately: ordinary text input does not prove complete composition support.

Release bundles must retain the upstream CEF/Chromium license files and third-party notices with the distributed browser libraries.

## Remaining shipping work

Durable render/preview tools, a dedicated variants selection UI, download-on-first-use packaging, cross-platform validation, native IME candidate placement and verified sandbox containment remain separate work. The engine replacement does not claim those are finished.
