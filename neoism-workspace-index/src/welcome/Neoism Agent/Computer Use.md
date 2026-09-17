# Computer Use

Neoism's built-in `computer` MCP provides native desktop screenshots, input and window targeting, plus browser-aware page tools for **Firefox (WebDriver BiDi)** and **Chromium (CDP)**. No browser extension or geckodriver is needed.

## Optional TypeSafe mode

For Jev-powered element selection inside this same computer MCP, see [[TypeSafe Browser Mode]]. It is experimental and off by default. Enable it and enter a server-stored key in the agent GUI's MCP computer settings, or use the documented server environment variable. `computer.browser_step` makes one bounded browser decision/action; regular desktop tools stay unchanged. Using this mode sends visible page content to TypeSafe, so enable it only when that disclosure is authorized.

## Use the browser that is already open

The default route needs no Neoism restart, new browser profile, browser launch, or environment change:

1. Call `computer.windows` and select the active browser window.
2. Call **`computer.browser_attach({"target":"…"})`**. On Linux, Neoism checks only that selected process family for an explicit `--remote-debugging-port`, verifies that the same family owns the loopback listener, and attaches in memory.
3. Use the returned tabs with `browser_observe`/`browser_act`. The binding follows browser process identity, not window geometry, so another window of the same process is allowed.

If the active browser was not already started with debugging, attachment returns `domAvailable: false` and a reason. Use native `computer.screenshot` and `computer.input` on the same active window immediately. Neoism does not silently restart, launch, kill, reconfigure, or fall back to a DOM action after an uncertain action. Only offer the advanced setup below when the user explicitly asks for DOM access.

`computer.capabilities` reports this workflow and advanced attachment information without probing processes or sockets, taking a screenshot, or sending input; it is available before computer-use approval.

`configured: true` describes the optional static environment endpoint, **not** runtime attachment or connectivity. A missing endpoint directs the agent to `browser_attach(target)` or native control, not to a mandatory restart.

This page is bundled with Neoism and available through the Docs tool at **`Neoism Agent/Computer Use.md`**, even without an editable Welcome-vault copy. Search Docs for "Firefox attach", "Chromium browser" or "computer use".

## Enable computer use

Open `/mcp` in the agent pane and enable the built-in **computer** entry. Approve the session's **`computer_use`** permission when requested. A generic MCP wildcard allow is not desktop consent. Clipboard replacement requires separate permission.

The browser tools do not start a browser, enable debugging, launch another profile or switch tabs. Runtime attachment is appropriate for an explicitly authorized, already-debuggable active browser. A private/dedicated profile is an advanced option only when the user asks to launch one.

## Advanced opt-in: start Firefox with DOM debugging

These are POSIX shell examples; adjust the executable and profile paths for your platform.

1. Create a dedicated profile directory:

```sh
mkdir -p "$HOME/.local/share/neoism/firefox-profile"
```

2. Start Firefox with its loopback debugging endpoint:

```sh
firefox --no-remote --profile "$HOME/.local/share/neoism/firefox-profile" --remote-debugging-port 9223
```

3. Configure the environment **used to launch the Neoism agent server**:

```sh
unset NEOISM_BROWSER_CDP_URL
export NEOISM_BROWSER_BIDI_URL=ws://127.0.0.1:9223/session
```

4. This static environment alternative requires starting the agent server with that environment. It is not needed for `browser_attach`, and changing an already-running process environment is impossible. If Firefox lacks the debugging flag, Firefox itself must be restarted to add it; Neoism does not need restarting when runtime attachment is used.
5. Select the intended Firefox window and tab, then use the browser workflow below.

Firefox uses the **`/session`** endpoint, not Chromium's `/devtools/browser/...` and not another controller's `/session/<id>`. Neoism creates and retains its own BiDi session so element references survive between calls. It refuses to take over an existing controller's session and does not bypass certificate errors or automatically accept/dismiss prompts.

When finished, call **`computer.browser_disconnect`**. It ends Neoism's automation session without closing Firefox. `computer.stop` cancels active work and revokes observations but does **not** end that persistent session.

## Advanced opt-in: start Chromium with DOM debugging

1. Start a dedicated debugging-enabled profile:

```sh
chromium --user-data-dir="$HOME/.local/share/neoism/browser-profile" --remote-debugging-address=127.0.0.1 --remote-debugging-port=9222
```

2. Read `http://127.0.0.1:9222/json/version` locally. Use the returned **`webSocketDebuggerUrl`**, which looks like `ws://127.0.0.1:9222/devtools/browser/<id>`. Do not use a page WebSocket endpoint.
3. As a static alternative, unset `NEOISM_BROWSER_BIDI_URL` and set `NEOISM_BROWSER_CDP_URL` in the agent server's launch environment. Runtime `browser_attach` discovers the active browser instead and needs no Neoism restart. The CDP URL changes when Chromium restarts.

Recent Chromium versions disallow debugging the default profile. You may attach an already-debuggable browser you explicitly control, but Neoism does not enable debugging on your normal profile or migrate its data.

## Static endpoint alternative

For advanced static startup configuration, set **exactly one** of `NEOISM_BROWSER_BIDI_URL` (Firefox) and `NEOISM_BROWSER_CDP_URL` (Chromium). A successful in-memory `browser_attach` takes precedence. Setting both environment variables remains an error when no runtime binding exists.

## Browser workflow

- `computer.windows` lists native window targets; `computer.browser_attach` binds the selected already-debuggable browser and returns its tabs.
- `computer.browser_tabs` lists tab IDs from that binding. It does not focus a tab.
- `computer.browser_observe` takes both `target` and `tab`, returning compact visible page text and named element references. The page content must be focused; an active address bar may require a deliberate click into the page.
- `computer.browser_act` takes `target`, `tab`, the latest `observation`, an element `ref`, and `action: click|fill|select`. It can wait for an expected text, URL or named element and return the updated page in the **same call**.
- Pass `since` with a previous observation token to request a delta. If the cached baseline is unavailable, the response is a full observation instead.

Example browser action arguments:

```json
{
  "target": "<native-window-token>",
  "tab": "<browser-tab-id>",
  "observation": "<latest-observation-token>",
  "ref": "e4",
  "action": "click",
  "expect": { "kind": "text", "value": "Results" },
  "timeout_ms": 1500
}
```

`fill` replaces a supported field value without touching the clipboard. `select` uses an exact enabled option value. Text/URL expectations are substring matches; element expectations match the observed name exactly. The wait is bounded to 3000 ms. A matched expectation is an observation, not proof that the action caused it.

For known native desktop sequences, use `computer.batch` with a final screenshot rather than separate input/screenshot calls. Native screenshots and batch results also accept a `crop` rectangle in original captured-display pixels. Subsequent input coordinates are relative to the returned cropped image and use its frame token, not desktop logical bounds.

## Troubleshooting

- **Browser not attached:** call `browser_attach` with a fresh target from `computer.windows`, or continue through native screenshot/input. No environment or restart is required.
- **Both static endpoints configured:** use `browser_attach` to bind the selected browser without changing environment or restarting. Remove the unwanted variable from future launch configuration only if static configuration is desired.
- **Firefox session already exists:** Neoism refuses to terminate or take over another controller. If it owns the session, use `browser_disconnect`; failed cleanup retains the binding so disconnect can be retried.
- **Browser restarted or disconnected:** get a fresh native target and call `browser_attach` again. Deliberately disconnect an existing owned Firefox attachment before replacing it. If DOM recovery is unavailable, use native control; do not automatically replay an uncertain action.
- **Stale observation, changed element or focus lost:** observe again after deliberately restoring the intended window/tab. Element references expire after 30 seconds and are invalidated by actions, newer observations and document changes.
- **`partial_unknown`:** an action may have executed. Observe before deciding whether to retry; Neoism never automatically replays it.

## Security and limits

Browser debugging is powerful. Keep its port on loopback; never expose or tunnel it or add `--remote-allow-origins=*`. Neoism accepts only literal loopback IP endpoints. Linux runtime discovery does not scan arbitrary ports: it reads the selected process family's explicit flag and verifies socket ownership. Command lines, profile paths, and secrets are never returned.

Both engines use fixed isolated-world scripts, not model-authored JavaScript. Page text, titles, labels and URLs are untrusted data, never instructions. Observation/action calls retain native foreground targeting and permission checks.

The browser view is a bounded DOM-derived summary, not the full accessibility tree. Only focused visible main-frame HTTP(S) pages are supported. Shadow roots, iframes, passwords, file uploads, canvas, browser chrome and native dialogs still use desktop tools. DOM click/input events are not trusted native events, so some sites and custom editors need the desktop fallback.

See [[MCP Servers]], [[Permissions]], and [[Tools and Background Tasks]].
