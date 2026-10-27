# Neoism Browser Control

Neoism's own browser runtime, MCP server and Chrome/Firefox extensions. The relay,
Chrome debugger adapter, executor, sessions, snapshots, extension UI and build
scripts are implemented here. The runtime uses Playwright, the MCP TypeScript SDK,
WebSocket and Zod libraries; it does not run or bundle OpenCode's browser package.

This replaces an earlier forked prototype. The prototype is excluded from the
shipping tree; its test counts and feature claims do not apply to this runtime.

## Build and enable

Requires Node.js 22.19+ and npm. From this directory:

```sh
npm ci --ignore-scripts
npm run build
npm run package:plugins
```

Neoism discovers `dist/mcp.js` and `dist/firefox.js` and offers `browser_chrome`
and `browser_firefox` in the MCP picker, disabled initially. Enable the engine
you want. Existing explicit MCP configuration is preserved. A packaged runtime
may be placed beside the agent executable under `browser-control`, under
`../lib/neoism/browser-control`, or selected with `NEOISM_BROWSER_CONTROL_DIR`.

Use `npm run stage:runtime` to assemble production dependencies and the built
extensions under `artifacts/runtime`. The extension packages are
`artifacts/neoism-browser-chrome-0.2.0.zip` and
`artifacts/neoism-browser-firefox-0.2.0.xpi`.

## Chrome

1. Open `chrome://extensions`, enable Developer mode, choose **Load unpacked**,
   and select `extension/chrome/dist` (or unzip the package first).
2. Run `node dist/cli.js pair chrome`, or call the MCP `connect` tool. Enter the
   returned local pairing token in the Neoism extension popup and connect.
3. Select a page and click **Attach current tab**. Chrome shows its debugger
   banner. Use **Detach current tab** to revoke the attachment.
4. Call `status`, then `session_adopt` with the selected `tabId`.

The Chrome extension uses the standard `chrome.debugger` API and needs no remote
debugging flag. Requires Chrome/Chromium 125+ and a distribution permitting
unpacked extensions. Other debugging clients may conflict with attachment.

## Firefox

1. Start a Firefox instance you intend to automate, for example
   `firefox --no-remote --profile /path/to/dedicated-profile --remote-debugging-port 9223`.
2. Open `about:debugging#/runtime/this-firefox`, choose **Load Temporary Add-on**,
   and select `extension/firefox/dist/manifest.json`.
3. Run `node dist/cli.js pair firefox` (or use `browser_firefox`'s `connect`).
   Enter the pairing token and `ws://127.0.0.1:9223/session` in the popup.
4. Connect, attach the desired page, then use `status` and `session_adopt`.

The XPI is **unsigned**; permanent installation in standard release Firefox
requires Mozilla signing. Temporary add-ons disappear when Firefox exits.
Firefox has no Chrome-style debugger extension API. Neoism's own adapter uses
Firefox's loopback WebDriver BiDi server and a version-pinned Playwright internal
integration. Keep Playwright at 1.64.1 until that adapter is revalidated. An
existing foreign automation session is never terminated or taken over. The BiDi
server itself provides profile-wide access: use an appropriate profile and keep
its debugging port on loopback. Extension tab approval is not process isolation.

## Tools and behavior

`guide` describes the browser workflow. `execute` supplies a real Playwright
`page`, persistent session `state`, `snapshot()`, `ref(id)` and `console.log`.

```js
await page.getByRole('textbox', {name: 'Name'}).fill('Neoism');
await page.getByRole('button', {name: 'Submit', exact: true}).click();
return await snapshot();
```

Snapshots include up to 150 visible controls across 16 frames, including open
shadow roots, with stable references to unchanged elements. Navigation expires
references. Results are bounded; these are compact DOM views, not complete
accessibility trees. Password field values are redacted. Page text remains
untrusted data, not agent instructions.

Sessions serialize operations and keep their own state, page and network
capture. Reset/delete closes only driver-created tabs and preserves adopted
tabs. `page_new` opens driver-owned tabs in the paired profile. `network_*` tools
capture at most 100 summaries without query strings, headers or bodies.
`screenshot` returns PNG images. `download` starts a browser-managed download and
returns its ID without claiming completion. `handoff` waits for the selected
tab's user to press **Continue in Neoism**. Only Neoism's own status overlay is
modified; other extensions and password-manager UI are preserved.

**Execution is trusted local Node JavaScript, with the same trust as a shell
tool. It is not a sandbox.** MCP permissions still apply. HTTP control requests
require the private local token; web-page origins are rejected. The extension
authenticates over a loopback WebSocket. Tokens are stored in user-only files
under `~/.neoism-browser-control`; `NEOISM_BROWSER_STATE_DIR` overrides that path.
Failed or uncertain operations are never replayed automatically. A timed-out
script revokes its tab to stop ordinary Playwright waits and poisons its session
until reset; attach again to continue. Arbitrary Node work cannot be forcibly
cancelled in-process.

`recording_*` and `flight_recorder_*` provide Neoism's bounded screenshot-sampled
WebM capture, requiring `ffmpeg` on PATH (or `NEOISM_FFMPEG`). They report actual
frame counts and errors, use a 64MB/1000-frame memory bound and never capture
audio. The rolling buffer retains at most 120 seconds and can save its latest
segment without stopping. Files are written under the private local state
directory. This is screenshot sampling, not compositor or tab-media capture.

This version does not expose isolated browser contexts, closed
shadow roots, browser chrome UI or other extensions' popups. Use Neoism's native
computer tools for desktop/browser UI. Chrome's extension CDP transport does not
provide ordinary Playwright download artifacts; use `download` instead. Check
`capabilities` for the actual engine and limits. No comparative superiority or
complete feature parity is claimed.

## Verification

```sh
npm run typecheck
npm test
npm run test:live
```

Live tests launch throwaway profiles and local HTTP fixtures, exercise the real
extensions and Playwright adapters, and never use accounts or real credentials.
The current live run used Chromium 146.0.7680.164 and Firefox 157.0; older Firefox
BiDi implementations have not been validated against this pinned adapter.
Set `NEOISM_TEST_CHROMIUM` or `NEOISM_TEST_FIREFOX` to select browser binaries.
Firefox's test-only add-on bootstrap is made in a temporary directory and never
included in either extension package. The production build and package scripts
contain no automatic tab-approval bootstrap.
