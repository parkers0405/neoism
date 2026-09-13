# Built-in computer-use MCP

Neoism Agent includes a linked `computer` MCP service. It runs in the **agent server's host desktop session**, including the embedded desktop server; there is no helper executable, browser automation dependency, shell-command backend, or separate computer-use UI.

## Enable deliberately

1. Enable **computer** in the existing Agent MCP picker. It is bundled but disabled by default. This is sufficient activation; no environment setup or process restart is needed.
2. Approve screenshots/input through the existing tool permission prompt. The permission category is `computer_use`; the pattern is the runtime tool ID, such as `mcp__computer__input` or `mcp__computer__screenshot`.

The equivalent entry in the agent's MCP configuration is:

```json
{
  "mcp": {
    "computer": {
      "type": "local",
      "command": ["builtin", "computer"],
      "enabled": true
    }
  }
}
```

This is an **agent configuration fragment**. In the desktop's grouped `config.json`, agent settings belong inside the `agent` block. Prefer the existing picker rather than creating another configuration file.

MCP enablement is the activation control; tool approvals and OS permissions still apply. Broad MCP or agent wildcard `allow` rules are not computer-use consent. Explicit `computer_use` grants are supported, broad denies remain effective, and the existing **dangerously skip permissions** mode still converts an ask into a one-time grant. Do not enable that mode if you want per-tool consent.

Direct MCP tool-call HTTP routes and non-session service calls cannot capture screenshots or inject input, even when the service is enabled. Capabilities and stop are safe non-input operations. There is deliberately no separate stdio exposure that bypasses this session approval boundary.

## Tools

Use the ordinary Agent `execute` MCP gateway:

```json
{"action":"call","tool":"computer.capabilities","arguments":{}}
```

After MCP enablement, session capabilities calls probe the native desktop and return display IDs, geometry, backend requirements, and runtime protocol/permission information. Non-session capabilities calls return prerequisites without accessing the desktop. Enumeration and protocol presence are not evidence that an application will accept input; untested capture permission is explicitly reported as such.

```json
{"action":"call","tool":"computer.screenshot","arguments":{"display":"<ID from capabilities>","target":"<listed target token>"}}
```

A screenshot returns PNG MCP image content and text containing a random `frame` token, display geometry, image width/height, and coordinate instructions. Images are also normalized into the existing `metadata.attachments` path used for provider media; this fixes image delivery for other image-returning MCP services as well. Screenshots and their potentially private contents are persisted with normal tool results and may be sent to the selected model provider.

```json
{"action":"call","tool":"computer.input","arguments":{"target":"<listed target token>","action":"click","frame":"<frame token>","x":800,"y":450,"button":"left"}}
{"action":"call","tool":"computer.input","arguments":{"target":"<listed target token>","action":"text","text":"Hello"}}
{"action":"call","tool":"computer.input","arguments":{"target":"<listed target token>","action":"key","key":"a","modifiers":["control"]}}
{"action":"call","tool":"computer.input","arguments":{"target":"<listed target token>","action":"scroll","amount":3,"horizontal":false}}
{"action":"call","tool":"computer.input","arguments":{"target":"<listed target token>","action":"drag","frame":"<frame token>","x":100,"y":100,"to_x":400,"to_y":200}}
{"action":"call","tool":"computer.stop","arguments":{}}
```

- Coordinates are **pixels in the returned image**, with a top-left origin, not desktop-global or original-resolution pixels. The backend maps them to the native display's coordinate space.
- `move`, `click`, and `drag` require the latest screenshot token. Tokens expire after 30 seconds, are replaced by the next screenshot, and are invalidated by stop/disconnect. Changed display bounds and out-of-image coordinates are rejected.
- `click` accepts `left`, `right`, or `middle`; `drag` uses the left button with 20 bounded motion steps over approximately 320 ms.
- Canonical `type` (`text` alias) accepts up to 512 Unicode scalars and selects one whole-string method before any input. On Linux it uses existing-layout strokes, never a temporary character map; unrepresentable text requires explicitly permitted clipboard fallback. Windows/macOS use their native Unicode adapters. See [Unified computer typing](computer-typing.md) for exact method and consent rules. `auto`, `keyboard`, and `native` reject C0/C1 controls, including CR/LF/Tab/NUL; deliberately forced `method:"paste"` permits literal multiline content except NUL. No automatic method emits Enter/Tab for a control character.
- Linux `paste` is the compatibility spelling of forced paste. It requires a target, `clipboard_policy:"replace"`, and separately granted **`computer_clipboard`** permission. It replaces the desktop clipboard, may expose the payload to clipboard history/other clients, and **does not restore the old clipboard**. `paste_shortcut` is `control-v` by default or explicitly `control-shift-v` for applications that require it. It accepts up to 512 Unicode characters including emoji, LF and Tab; NUL is rejected. Publication and shortcut dispatch do not prove that the intended field accepted the paste. Terminal applications can execute pasted commands; paste is not a sandbox.
- Keys: `enter`, `tab`, `escape`, `backspace`, `delete`, `space`, `up`, `down`, `left`, `right`, `home`, `end`, `page_up`, `page_down`, or one ASCII letter/digit (case-insensitive; use the `shift` modifier for shifted keys). Linux also accepts individual ASCII punctuation present in the unmodified layout, such as `key:"/"`. For underscore on a US layout, use `key:"-", modifiers:["shift"]`; a shifted symbol without an unmodified mapping is rejected before input. Use `text` for other supported Unicode characters. Optional modifiers (case-insensitive): `control`/`ctrl`/`Control_L`, `alt`/`option`/`Alt_L`, `shift`/`Shift_L`, and `meta`/`super`/`Super_L`/`win`/`windows`/`logo`/`cmd`/`command` (Command on macOS). Modifier names can also be tapped as keys: `{"action":"key","key":"Super_L"}`. Key aliases include `Return`, `Esc`, `ArrowLeft`/`ArrowRight`/`ArrowUp`/`ArrowDown`, `PageUp`/`PgUp`, and `PageDown`/`PgDn`. Unknown modifiers and duplicates—including equivalent aliases such as `super` plus `meta`—are rejected before injection.
- `press_key` (`key` alias) is for explicit shortcuts/navigation. Some applications, including Minecraft 26.2, ignore temporary character maps; that remapping path is no longer used by `type`. Existing-layout typing sends the actual character keys and required Shift/level-three selectors. Do not retry uncertain text or silently switch methods after dispatch.
- Shortcut chords resolve all native keycodes **before** pressing modifiers, then press/release those exact codes. Wayland uses the compositor's unmodified XKB layout, macOS uses unmodified native layout translation, and Windows uses fixed virtual keys. This preserves Shift+A, Shift+1 and Ctrl+Shift combinations without Enigo remapping them into literal unshifted text. Missing base-layout keys fail before any modifier is pressed. Ambiguous Wayland multi-seat or multi-layout selection is rejected when the compositor does not provide enough information.
- Scroll amount is -20 through 20; positive means down/right. Text, keys and scroll target the current native focus/pointer, not a browser tab selected by the service.
- There are no persistent key/button-down tools. Each action releases what it presses. Mouse release runs on cancellation/errors and unwinding; Enigo also releases held keys when dropped.
- Input results report `status:"dispatched_unverified"` and `applicationVerified:false`: the native API accepted events, but the target application's effect is not verified. Observe before choosing another action. In particular, inspect the destination content before Enter or another committing action in a command/message field. Never automatically retry an uncertain insertion; it may duplicate content.

Linux `paste` is an explicit clipboard-changing operation, not a spelling of `text`:

```json
{"action":"call","tool":"computer.input","arguments":{"target":"<listed target token>","action":"paste","text":"Full Unicode: 🦀 👩‍💻","clipboard_policy":"replace","paste_shortcut":"control-v"}}
```

The clipboard source is in-memory and event-driven: no temporary payload files, subprocess typing, or periodic idle polling. Publication has a 500 ms acknowledgement deadline; serving an individual pipe/socket is capped at 50 ms. Neither acknowledgement proves application consumption. Cancellation cannot undo text already consumed, clipboard disclosure, or history retained by other clients.

## Comparison with established input backends

These are public implementations, not claims about proprietary production internals:

| Implementation | Actual text path | Relevant boundary |
| --- | --- | --- |
| [Anthropic computer-use demo](https://github.com/anthropics/anthropic-quickstarts/blob/main/computer-use-demo/computer_use_demo/tools/computer.py) | `xdotool type` with pacing/chunks, followed by screenshots | X11 demo, not a native-Wayland guarantee |
| [Playwright](https://github.com/microsoft/playwright/blob/main/packages/playwright-core/src/server/input.ts) and [OpenAI's public SDK example](https://github.com/openai/openai-agents-python/blob/main/examples/tools/computer_use.py) | Keyboard events for supported characters; Chromium CDP `Input.insertText` for text insertion | Browser content only; does not cover the omnibox or other desktop apps |
| [wtype](https://github.com/atx/wtype/blob/master/main.c) | Custom Unicode keymap using ordinary-range virtual keycodes | Supports the need for compatible native codes; still requires IME/client testing |
| [dotool](https://git.sr.ht/~geb/dotool/tree/master/item/dotool.go) | Existing-layout chords and dead-key combinations; warns on impossible characters | Layout-dependent, not universal literal Unicode |

Neoism keeps host-native control rather than silently switching to CDP or clipboard paste. Clipboard transfer can expose text to clipboard history/other clients and changes paste semantics, so `paste` is a separate action with an explicit replace policy, never a fallback from `text`. Its in-memory Wayland source stays available until another clipboard selection replaces it; it neither reads nor restores previous clipboard contents. A native key event and a literal text insertion are different contracts. Active input methods and application key handlers can transform or intercept native keys. Focus checks establish a foreground window, not an editable field, and screenshots are evidence rather than exact-text acknowledgements.

Chromium sources for the dropped-key mechanism: [WaylandKeyboard](https://github.com/chromium/chromium/blob/main/ui/ozone/platform/wayland/host/wayland_keyboard.cc), [KeycodeConverter](https://github.com/chromium/chromium/blob/main/ui/events/keycodes/dom/keycode_converter.cc), and [the DomCode table](https://github.com/chromium/chromium/blob/main/ui/events/keycodes/dom/dom_code_data.inc).

## Focus safety and keymap changes

Previously `focus` confirmed foreground but did not persist a binding, while untargeted `input`/`batch` calls were accepted. Their `Check.target` stayed `None`, skipping native foreground validation; batch screenshots likewise returned `targetWindow:null`. Mandatory per-call targets now close that admission hole rather than guessing a remembered window. A rejected untargeted batch sends zero events.

The Linux Unicode path publishes a temporary full-layout overlay and neutral modifiers, checks foreground again after the publication acknowledgement and before each character, and restores/removes the keyboard on success or failure. It does not request window activation. Layout/modifier changes do affect the compositor's seat state and injected shortcut keys can invoke window-manager bindings, but source inspection alone does **not** establish that Unicode map publication caused a particular focus switch. A target guard stops subsequent characters/actions if such a switch is observed. Windows/macOS similarly use global native events with character/chord-press checks, not background-window delivery. Release pairs and cleanup are deliberately not blocked by lost focus. All platforms retain a check-to-injection race; this is accident prevention, not atomic OS routing or application isolation.

## Batches: mechanical steps, then observe

```json
{"action":"call","tool":"computer.batch","arguments":{"target":"<listed target token>","actions":[{"action":"key","key":"l","modifiers":["ctrl"]},{"action":"text","text":"Already chosen search terms"}],"screenshot":"<display ID>"}}
```

`batch` accepts **1–16 input action objects**, a **required** top-level `target`, and an optional `screenshot` display ID. Nested batches and per-action targets are rejected. The complete argument list is validated before the first action; total text across the batch is limited to 512 Unicode characters. It uses the same session permission, cancellation, revocation generation, and ten-second budget as individual input, with **one serialization lock for the entire sequence and final capture**.

Results report `status:"dispatched_unverified"`, `applicationVerified:false`, `completed`, `total`, zero-based `failedIndex`, `error`, and `partialActionPossible`. Explicit paste also reports clipboard-change metadata. Execution stops at the first failure. Completed means native API acceptance, not successful application behavior; the failed action can be partially delivered. The requested final screenshot is ordinary MCP image content with its own frame token, so it reaches the model through the existing attachment path. Capture failures are reported separately without hiding input progress. A failed batch with a recovery screenshot remains an **error** in the persisted session and provider tool result; its image is still delivered through the normal provider-media path. Structured error result metadata is retained on the tool part rather than discarded as a plain exception. Cancellation/revocation prevents subsequent capture. A native-call timeout returns the known completed count and explicitly warns that a native action may still be in flight; no final image is promised after timeout.

Batch only mechanical steps whose meaning is already known. **Do not batch semantic choices blindly** (for example, opening an unknown dialog and guessing its confirmation button). Observe the returned image before deciding what to do next.

### Screenshot settling

`screenshot` and `batch` accept `settle_ms` (**0–3000**, default **0**). Immediate mode captures once, does no stability sampling, and inserts no settling wait. A positive value explicitly opts into visual stability: roughly 100 ms probes, 250 ms of quiet after at least 300 ms of observation, using a bounded whole-screen sample. Cheap cancellation/deadline checks run during waits; target validation remains immediately before and after capture. Neither mode proves that an application consumed input or finished loading. Batch input cleanup finishes before its result capture.

Linux captures reuse one operation-scoped connection, refreshing full output topology before and after each capture. Encoding never enlarges small images and uses a faster lossless PNG path with a bounded compression fallback. Returned `timings` separate worker scheduling, native queries, planning, keyboard waits, capture, resize, PNG and base64. These exclude model/permission wait and response transport; stages may overlap. No typed text, window titles or image contents are included in timing labels. Synthetic CPU-only benchmarks showed 1280×720 text-like resize/PNG/base64 falling from 45.02 ms to 3.60 ms in an optimized test build; that is not a live desktop or universal latency claim.

The returned image is the last sampled frame, with `settling.settled`, `timedOut`, `elapsedMs`, and `captures`. A continuously animating screen can exhaust the budget: the image is still returned, explicitly **not settled**. `settle_ms: 0` requests immediate capture. This is visual settling, not proof of page load or application readiness; the overall ten-second operation budget still applies, and a native capture itself cannot be preempted. Capture/settling never runs after cancellation or revocation.

## Native windows and explicit foreground targets

```json
{"action":"call","tool":"computer.windows","arguments":{}}
{"action":"call","tool":"computer.focus","arguments":{"target":"<listed target token>"}}
{"action":"call","tool":"computer.screenshot","arguments":{"display":"<display ID>","target":"<target token>"}}
{"action":"call","tool":"computer.input","arguments":{"action":"click","target":"<target token>","frame":"<target-bound frame>","x":800,"y":450}}
{"action":"call","tool":"computer.wait","arguments":{"target":"<target token>","condition":"foreground","timeout_ms":1000}}
```

- `windows` returns native window ID, PID, app, title, bounds, foreground status, and opaque handles. Unchanged identity/geometry keeps the same handle across listings; there is no arbitrary 30-second window-handle expiry. Observed absence, changed identity or geometry, stop, and bounded cache eviction invalidate it. Native process-start evidence guards PID reuse; Hyprland's stable ID and compositor instance add window-birth evidence where available. Same-process unobserved ID reuse is not universally provable on platforms without a birth identifier. Screenshot-coordinate tokens still expire after 30 seconds.
- `focus` requests one exact window and verifies foreground status. OS focus refusal is an error, not reported success. It invalidates existing screenshot coordinates even if focus fails. If focusing moves/resizes a window, list it again before targeting input.
- `target` is **required** on every `input` and `batch`, even immediately after successful `focus`. There is no remembered focus binding—neither session-local nor global—so another session cannot supply an implicit target. Each caller must explicitly supply the listed window handle it intends to control. `focus` returns `focused:true`, the confirmed `target`, and `binding:"explicit-per-call"`; it does not reserve the foreground. `screenshot` alone may omit target for desktop observation; pass the same target when preparing window-bound pointer input.
- **Deliberate desktop workflow:** individual `input` calls may instead use `scope:"desktop"` for a global key chord or pointer/scroll action. This scope is mutually exclusive with `target`, forbids literal text, and is unavailable in batches. For example, `{"action":"key","key":"tab","modifiers":["alt"],"scope":"desktop"}` intentionally switches windows. Observe/list afterward and explicitly target subsequent typing. Desktop pointer input requires an untargeted screenshot.
- Targeted input requires the same window to remain foreground with unchanged geometry at every key-press/text-character checkpoint. It does **not** inject into background windows. Focus loss stops further input; releases/restoration still run for cleanup. **Do not automatically steal focus back or retry typing after interference**; report partial progress and let the user resolve the conflict. Batch final screenshots retain the batch target; focus loss produces `screenshotError`, not an unbound image silently presented as the intended window. For targeted pointer input, the screenshot must have the same target identity and geometry.
- Window bounds are native desktop metadata, **not** screenshot coordinates. All pointer coordinates remain pixels in a **display screenshot**, never window-relative coordinates. There is no window-only capture/crop API. Targeting does not constrain clicks to the window rectangle; it is a foreground precondition, not a sandbox.
- Native focus/identity checks and event injection are not atomic. External apps or the user can change focus between them; native IDs can be reused. The snapshot bindings and per-effect checks reduce mistakes but do **not** provide application isolation or an allowlist/security guarantee. Avoid concurrent physical keyboard/mouse use during automation.
- Linux window operations use **Hyprland's native Unix-socket IPC**, bounded to 4 MiB replies and two-second read/write timeouts; no `hyprctl` or shell. Focus first uses a fixed, read-only `/eval return` capability probe to select the legacy `focuswindow address:...` or Lua `hl.dsp.focus({ window = "address:..." })` dispatcher grammar. Only a validated hexadecimal native ID is interpolated; arbitrary Lua is not exposed as a tool. Compositor rejection text is preserved rather than collapsed to a generic error. Other Wayland compositors return unsupported errors for window operations: observation and explicit desktop-scoped actions remain available, but targeted typing fails closed.
- Windows uses native XCap enumeration with a per-monitor DPI thread context, exact foreground HWND checks, and `SetForegroundWindow`; foreground restrictions still apply.
- macOS uses CoreGraphics/XCap enumeration and AX exact-window matching/raise. XCap's `is_focused` only compares app PIDs, so it is deliberately **not** used for macOS target validation. AX reads the system's focused window and bridges its native window ID using `_AXUIElementGetWindow` (an undocumented OS API; failure is closed, never an app-only fallback). Accessibility permission is required. No accessibility tree/content inspection is exposed.

`wait` only observes a listed window becoming `foreground` or `closed`. It polls at approximately 100 ms with an explicit **100–3000 ms timeout**, returning `matched`/`timedOut`; cancellation and revocation are checked between probes. A backend error is not mistaken for a closed window. `closed` uses a native lifetime/owner-identity probe, not absence from the visible window list: Windows checks HWND existence and owner PID, macOS queries the specific CoreGraphics window without the on-screen filter, and Hyprland checks the unfiltered client identity reply without querying focus. Hidden, minimized, and off-Space windows are not considered closed. On macOS, a legitimate AX `NoValue` for focused application/window means no window is focused (for example, Finder desktop); permission and other AX errors still propagate. Native API calls can overrun the polling timeout. It does not infer page loads, network idle, element visibility, or application task completion.

## Platforms

### Linux: Omarchy / Hyprland

Uses `libwayshot` directly for Wayland capture and owned native virtual-keyboard, virtual-pointer, and explicit data-control clipboard implementations. Pointer actions never instantiate a keyboard. The runtime probe reports advertised virtual keyboard, virtual pointer, screen-copy and clipboard protocols. There is **no** silent dependency on `grim`, `ydotool`, `dotool`, root/uinput, CDP injection, or an XWayland fallback.

Requirements include the local Wayland connection, xdg-output, screencopy/image-copy support, virtual-keyboard and wlr-virtual-pointer protocols. Build/runtime libraries include Wayland, xkbcommon, GBM and DRM as required by the native crates. The host must run inside the logged-in graphical session; headless/SSH-only servers report errors rather than success.

Current limitation: capture supports selecting among multiple outputs, but pointer actions require **one output and one seat**. The native pointer validates the observed output name/logical geometry, binds that output when manager v2 is available, and sends screenshot pixels with their original width/height as normalized protocol extents. It never derives pointer coordinates from Enigo's display metadata. Keyboard input remains available. X11-only desktops and compositors without the necessary protocols are not supported by this backend.

### macOS

Uses native CoreGraphics capture through XCap, Enigo for key/button events, and paired CoreGraphics Unicode events for text. Grant **Screen Recording** and **Accessibility** to the actual host process in System Settings. The capability probe checks both without opening an OS prompt. Capture refuses to proceed without Screen Recording permission. Retina image pixels map back to CoreGraphics display coordinates. No AppleScript, `screencapture` executable, or permission bypass is used.

### Windows

Uses XCap's native Windows capture, Enigo's SendInput keyboard/button/wheel events, and `SetCursorPos` for the full virtual desktop, including negative monitor origins. Capture/enumeration/pointer positioning temporarily establish a per-monitor DPI-aware thread context and restore it afterward.

The server must run on the interactive user desktop. UIPI blocks input into higher-integrity apps. UAC/secure desktop, protected capture content, and service/session-0 operation are not supported; no elevation or bypass is attempted.

### Keyboard troubleshooting and semantics

Use `input` with `{"action":"key","key":"l","modifiers":["ctrl"]}` for a browser location/search shortcut, then `{"action":"text","text":"Google search terms"}` for literal text, and `{"action":"key","key":"Return"}` to submit. These are tool argument examples, not actions run during tests. Text preserves case, punctuation and Unicode; it does not pass through the shortcut-name parser or depend on an active Wayland input-method client.

Linux literal text no longer passes through Enigo's mutable Unicode lookup. Inspection of Enigo 0.6.1 found that `key_to_keycode` searches `min_keycode..max_keycode` (excluding the final valid code), while missing Unicode symbols can be appended at that maximum. Subsequent lookups can therefore remap them again; separate press/release symbol lookup also depends on mutable keymap/state updates. This does not establish that every observed case/punctuation failure came from that edge case, and the former mock keyboard tests could not establish native fidelity.

Linux typing and shortcuts share one original-layout `KeyboardSession` for preparation and dispatch. Before creating any virtual keyboard, the connection validates the desktop keymap and active group. The planner finds actual printable positions with no modifier, Shift, the map's genuine level-three selector, or Shift plus that selector. XKB simulation verifies exact character output and neutral state after releases; unsafe Control/Meta/Alt actions, dead-key composition, group switching and consumer-key aliases are rejected. The whole string is planned before its first event. The injector publishes only the unchanged captured baseline, never a character overlay. `type` selects this keyboard plan or a separately authorized whole-string paste before dispatch. See [Unified computer typing](computer-typing.md).

The earlier append-above-maximum design passed native XKB/Wayland tests but failed in Helium. Chromium's `WaylandKeyboard::DispatchKey` converts evdev through a fixed `DomCode` table before decoding a character; unknown positions were silently dropped. A later printable-position overlay still failed in Minecraft. These are historical failures, not supported automatic methods. The current planner uses unchanged normal key positions and direct printable keysyms; it also rejects consumer aliases such as XF86NumericPound, which looked like `#` to XKB but was dropped by GTK. Do not concurrently use the physical keyboard during automation.

Explicit finalization releases the injector's held keys, neutralizes only its virtual modifiers, acknowledges cleanup, and destroys its device. It runs after success and primary failure, with cleanup failures retained separately. `Drop` is a best-effort fallback, not how successful dispatch is reported. Physical held/latched/locked state is not replayed or claimed as restored.

Historical overlay regression tests remain because they exposed a Hyprland/Fcitx identity-cache failure: updating a keymap on the same text device did not forward restoration to the IME. The production typing path no longer creates those overlays, so that workaround is not its method-selection strategy.

Runtime revalidation refreshes the observed map/group before clipboard publication and keyboard dispatch. Changed assumptions stop the prepared plan without replanning or switching methods after a prefix. Keyboard observation and cleanup use deadline-aware socket polling rather than unbounded roundtrips. This checks native state, not application consumption. Typed failure facts preserve completed native units, current-unit uncertainty and cleanup errors; no character count is advertised as verified insertion.

The seat's current map is not guaranteed to be physical: it may belong to another virtual keyboard. The previous implementation could save a leftover text-only map as its “original,” then faithfully restore a map without Return or Control. It also acknowledged restoration but only flushed removal, while the next shortcut's Enigo constructor could switch the active keyboard before a separate resolver queried it. Starting maps bearing our old text-only or new overlay markers are rejected **before input** if a short read-only observation window does not receive a clean layout; they are never accepted as a baseline. If an old/interrupted build has left such a map active, use the physical keyboard once to reselect its layout before retrying; the tool does not guess a US layout or inject a dummy key to repair it.

A live Google-URL batch exposed an additional dependency interoperability failure: unnamed generated sections compile in libxkbcommon, but Enigo 0.6.1's `Keymap2::new_from_fd → ParsedKeymap → Keycodes::parse` calls `parse_section(...).unwrap()` and requires quoted section names. The compositor's reserialized unnamed map therefore crashed a subsequent Enigo operation. Generated maps now have explicit section names and are natively compiled/canonicalized before publishing. `python3 scripts/check-computer-keymap.py` imports the **exact locked Enigo source** into a disposable test crate: it reproduces the old panic through the real FD loader and tests the fixed canonical generated/compositor-reserialized maps with Google URLs, uppercase, punctuation, Unicode, repeats and all 512 entries. It does not open a desktop connection. The same harness also runs the production transaction and real shortcut resolver through **URL → Enter → Ctrl+L → text → Enter** on US, German, and French native XKB maps, parsing every published/restored map with Enigo's exact FD loader. Its queued-wire fixture applies requests only at acknowledgements, models the IME's identity-only keymap cache, and verifies both restoration and destruction barriers. A regression explicitly demonstrates that restoring the same device leaves the forwarded overlay active and that replacing the device fixes it without a keypress. Cancellation after text or Control-down, partial native errors, panics, and missing restoration acknowledgements are covered. This is deterministic native keymap/transaction coverage, not a live compositor-delivery test.

The opt-in `hyprland_production_keyboard_roundtrip` test opens its own Wayland test surface and verifies production keyboard delivery there; it never types into an existing browser or editor. Run with `NEOISM_LIVE_KEYBOARD_TEST=1 cargo test --locked -p neoism-agent-server hyprland_production_keyboard_roundtrip -- --ignored --nocapture`. It requires its test surface to remain focused and stops on focus loss. Fcitx does not send an unsolicited text-input `Done` on activation; that event is an edit-batch boundary, not a readiness acknowledgement.

For reproducible testing without injecting into the current desktop, `scripts/check-computer-keyboard-live.py` can run a nested Hyprland/Fcitx session with private runtime, DBus and configuration. Pass `NEOISM_LIVE_KEYBOARD_TEST=1`, `--nested`, and `--render-node /dev/dri/renderD129` (select an available render node). The outer compositor presents an ordinary window; test clients are sandboxed away from the host display socket and physical input devices. The launcher additionally requires trace evidence of Fcitx-grab key presses/releases, modifiers and keymaps. Pure headless startup without a DRM allocator is not supported by Hyprland 0.56.2/Aquamarine 0.14.0. The initial custom-receiver smoke test passed on Hyprland 0.56.2 + Fcitx 5.1.21, but it missed Chromium's dropped-key and supplementary-character failures. It is not the browser acceptance gate; use the real-browser suite below.

The worker owns serialization outside the native unwind boundary. Recoverable native panics become errors, invalidate frames, and retain batch progress/recovery-image handling; old poisoned unit locks are repaired, while a genuinely running native call remains busy. Explicit key/button guards perform best-effort release; each key release is isolated so one release panic cannot skip remaining modifiers. Enigo's redundant automatic drop-release is disabled to avoid a second panic during unwinding. **Unwind recovery cannot catch process aborts** (the repository's release profile currently uses `panic=abort`), and it cannot forcibly terminate a stuck OS call. These limits are distinct from the fixed poisoned-lock failure.

Native libxkbcommon tests cover supported key positions, multi-layout and custom-modifier maps, full-string preflight, chunk-boundary cancellation, and restoration. Historical parser fixtures still exercise the old maximum-keycode edge cases; they are not evidence of browser delivery.

### Real browser acceptance

`scripts/check-computer-browser-live.py` launches real native-Wayland Helium with a fresh profile and Chromium's sandbox retained. The fixture observes DOM input/focus/pointer events and local HTTP navigation; it never uses DOM/CDP input injection. Clipboard activity is confined to the private compositor. The pointer target is selected from actual screenshot pixels, then checked against the browser's received coordinates and focused element.

Verified through the unified facade: native-Wayland Helium at output scales **1.0 and 1.25** and browser zoom **100% and 125%**, with input/textarea/contenteditable. Checks cover exact existing-layout ASCII/punctuation/Shift, rejection before an unsupported suffix, rejection without clipboard permission, authorized automatic Unicode fallback, explicitly forced multiline paste, and follow-up shortcuts. The final private-headless run additionally typed 512 uppercase characters under the public 10-second guard using the production window validator. Host-input devices are disabled in that private compositor before application startup; the test does not measure per-key `hyprctl` process spawning or accept user modifier interference as a typing pass.

The original omnibox path is also tested: **Ctrl+L → native text URL → Return** must produce exactly one matching local HTTP request, a new document generation, and the exact browser-reported URL/query. This passed at both output scales; a DOM input-field pass alone does not satisfy it.

Run serially on a supported local test host (choose an available render node):

```sh
NEOISM_LIVE_KEYBOARD_TEST=1 python3 scripts/check-computer-browser-live.py --nested --render-node /dev/dri/renderD129 --scale 1 --private-headless-output
NEOISM_LIVE_KEYBOARD_TEST=1 python3 scripts/check-computer-browser-live.py --nested --render-node /dev/dri/renderD129 --scale 1.25 --private-headless-output
```

The 1.25 test uses one headless output inside the allocator-backed nested compositor, with actual screenshot/logical sizes 1280×800/1024×640. It does not claim that the outer Wayland presentation backend supports fractional scaling. This suite does not establish universal app/IME compatibility, multi-output pointer support, live drag/scroll correctness, or full MCP-permission-pipeline delivery. Those boundaries remain explicit.

Wayland keyboard keymaps are FD-backed buffers, not streams positioned at zero. In particular, virtual-keyboard writers such as Enigo can leave their descriptor at EOF before it is forwarded to another client. The backend reads the advertised byte range at **offset zero without changing the shared FD cursor**, strips trailing NUL terminators, and then parses it using libxkbcommon. Reading from the inherited cursor previously turned a valid keymap into an empty buffer and an `Invalid XKB keymap` failure. Truncated, oversized, embedded-NUL, and genuinely malformed maps still return errors—there is no guessed keyboard-layout fallback. A subsequent valid compositor keymap replaces an earlier parsing error.

## Cancellation and limits

Actions and captures are serialized **per agent process**. A concurrent action receives a busy error instead of building an unbounded input queue. Each operation has a ten-second budget, checked before subsequent input events; the async result also has a ten-second timeout. Model/session cancellation and dropping the call future signal the blocking worker, and `computer.stop` does not wait for the desktop lock. Disabling/disconnecting the MCP service signals stop. New calls capture the revocation generation **before** loading current enablement and carry that exact generation into their blocking worker. A disable between the enablement read and worker creation therefore rejects the worker instead of admitting it under the new generation. Old turn configuration snapshots cannot bypass revocation.

A blocking native OS/Wayland call cannot be forcibly preempted safely in-process. It can outlive the response timeout and retain the serialization lock until it returns; further calls fail busy. Cleanup is attempted when control returns. Stop is not rollback: already-delivered clicks/text cannot be undone. Multiple independently running agent server processes do not share this in-memory lock.

Capture is bounded to 64 million native pixels; output is resized to at most 1600 by 1600 pixels and an 8 MiB PNG. No continuous screen recording, audio, previous-clipboard reads, file transfer, browser/DOM input integration, or accessibility content/tree API is exposed. Clipboard replacement is available only through the explicit policy and separate permission described above. Window management is limited to listing and requesting focus.

## Verification

Automated tests cover picker-only activation through a mocked native backend without restart, session boundaries, disabled-by-default registration, wildcard consent rejection, preserved denies, live disablement despite old snapshots, image attachment normalization, argument bounds, coordinate scaling, stale frames/topology changes, cancellation flags, and button release on failures/unwinding. Existing MCP and provider-media tests exercise the unchanged gateway/result infrastructure.

The service is compiled on Linux with the integrated server. Its exact native source can also be cross-checked for `aarch64-apple-darwin` and `x86_64-pc-windows-msvc`; cross-checking is **not** OS runtime validation. Interactive capture/input, macOS TCC, Windows UIPI/DPI, Hyprland scaling and physical cancellation still need native smoke testing. No automated test silently types or clicks on the developer's desktop.
