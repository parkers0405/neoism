# Neoism Rune frontend

Branch `rune-frontend` replaces the GPUiX frontend (`neoism-frontend/gpuix`: Solid/TypeScript over GPUI) and the
Sugarloaf-era painter it carries (`neoism-ui` display lists shipped through JSON into GPUiX `<canvas>`) with a
single native Rust app built on [Rune](https://github.com/anomalyco/rune). Everything is drawn with Rune elements;
nothing is painted by `neoism-ui`/Sugarloaf and no JavaScript runs.

The backend does not change. The app is a client of the workspace daemon (`neoism-workspace-daemon`) and speaks the
typed wire protocol in `neoism-protocol`. Daemon, agent server, PTYs, LSP, git and storage stay where they are.

**Goal: look and behave exactly like the current app.** `reference/` holds screenshots of the shipping GPUiX build
(v0.7.148, 1252x674 logical at scale 2, `pastel_dark`). Match structure, spacing, colors, fonts and motion; when in
doubt, open the current app next to the Rune build and compare.

## Layout

```
rune/
  README.md        this plan, the look and behavior spec
  reference/       screenshots of the current app (source of truth for looks)
  app/             the Rune app crate (`neoism-rune`), its own Cargo workspace
```

`rune/app` is deliberately outside the root Cargo workspace: Rune pins Rust 1.99.1 and its own dependency graph,
and the existing services must keep building unchanged until the cutover.

## Steps

1. **Reference capture** (done) - screenshots below, scroll/animation constants in [Motion](#motion).
2. **Skeleton** - `rune/app` window with the chrome: activity rail, workspace tab row, pane tab row, status line,
   `pastel_dark` tokens, fonts (Geist Mono, Cascadia Code NF icons, Press Start 2P headings). Headless PNG check
   against `reference/01-welcome.png`.
3. **Daemon client** - WebSocket to the published local daemon (`$XDG_RUNTIME_DIR/neoism/daemon-endpoint.json`,
   token file next to it), `Hello` handshake, request/reply correlation, event stream into app state. Typed with
   `neoism-protocol`. Parse on a background thread; the UI thread only applies typed events.
4. **Agent timeline** - conversation list (sidebar), history paging (byte-budgeted, see below), streaming deltas,
   markdown (headings, lists, inline colors, tables, code blocks with language header, Copy, line numbers, syntax
   colors), user bubbles with the pixel orb, tool and patch cards, footer metadata line, scroll physics.
5. **Composer** - floating card with starfield, multiline input with IME, chip row (agent, model, effort), attach,
   send/stop, path/context footer, `/` command menu, model/agent pickers.
6. **Home block terminal** - Warp-style blocks over daemon PTYs (`Pty` family), per-block cwd/command/output/
   duration/exit status, inline completion preview, follow-bottom, block selection.
7. **Editor, file tree, finder, palette, notes, agent panel, plain terminal** - in that order.
8. **Parity pass** - screenshot every reference surface in both apps, diff, fix. Frame trace both apps with the
   same scenarios (`scripts/analyze-gpuix-frames.py` method: presentation timestamps, not counters).
9. **Cutover** - switch the installed `neoism` binary to the Rune app, delete `neoism-frontend/gpuix`, the
   GPUiX pins/patches and the Sugarloaf painter paths, update packaging and release scripts.

Do not delete GPUiX or Sugarloaf before step 9: the current app is the visual and behavioral reference.

## Reference screenshots

| File | Surface |
| --- | --- |
| `01-welcome.png` | Welcome/Home: pixel wordmark, action grid with `[Alt + X]` hints, terminal composer at the bottom |
| `02-home-terminal-blocks.png` | Home block terminal: cwd header, command, output, `5 ms` / `exit 1` timing per block |
| `03-home-terminal-typing-preview.png` | Terminal composer with inline completion ghost text |
| `04-sidebar-conversations.png` | Agent sidebar: New chat, directory scope, Pinned/Recent (pixel headings), rows with age |
| `05-agent-timeline-markdown.png` | Assistant markdown: headings, paragraphs, inline colored terms, footer meta line |
| `06-agent-timeline-code-table.png` | Bordered table, code block (language header, Copy, line numbers, syntax colors), italic caption |
| `07-agent-timeline-full-width.png` | Timeline without sidebar |
| `08-agent-timeline-tool-cards.png` | User bubble + pixel orb, bullets, `Build · model · time · tok/s` footer |
| `09-composer-typing.png` | Composer with text |
| `10-composer-slash-commands.png` | `/` Commands menu: search, rows with command, description, kind (docs/modal/server) |
| `11-composer-slash-filtered.png` | Filtered `/mo` |
| `12-composer-model-picker.png` | Select model: recent models, providers with model counts |
| `13-composer-agent-picker.png` | Agents picker (build, plan) |
| `14-command-palette.png` | `Alt+P` palette with shortcut hints |
| `15-search.png`, `16-finder.png` | Fuzzy file finder overlay |
| `17-file-tree.png` | `Alt+E` file tree sidebar |
| `18-editor-code.png` | Editor: breadcrumbs, line numbers, wrap markers, syntax colors, LSP/position status chips |
| `19-notes.png` | `Alt+N` notes sidebar |
| `20-agent-panel.png` | `Alt+C` agent panel next to the editor |
| `21-new-terminal.png` | `Alt+T` plain terminal tab (block composer, `TERMINAL` mode) |
| `22-app-menu.png` | Hamburger menu: About, Settings, Database, Themes, Backgrounds, Extensions |
| `23-themes-modal.png`, `24-themes-modal-preview-moved.png` | Themes: search field, theme list on the left, preview card on the right whose code sample repaints in the highlighted theme as `Up/Down` moves (`↑↓ preview · Enter apply`) |
| `25-backgrounds-modal.png`, `26-backgrounds-modal-preview-moved.png` | Backgrounds: same list + preview layout; the preview card describes the highlighted background |
| `27-settings.png` | Settings: category sidebar (General, Appearance, Editor, Terminal, UI, Presence, Keybinds, Agent, Developer), toggles and selects, search field |

### Motion captures (`reference/motion/`)

Recorded from the live window at 120 fps (2504x1348) with `gpu-screen-recorder`; each `*-sheet.png` samples the
video so agents can read the motion without playing it. Extract any frame with
`ffmpeg -i reference/motion/<name>.mp4 -vf "select=eq(n\,N)" -vframes 1 frame.png`.

| File | What it shows |
| --- | --- |
| `wordmark-agent.mp4`, `wordmark-hover-sheet.png` | Pointer sweeping across the NEOISM wordmark on an `Alt+A` tab: each letter grows and lifts under the pointer and eases back after it leaves |
| `wordmark-click-1-sheet.png`, `wordmark-click-2-sheet.png` | Two clicks: the artwork clears and a pixel-cell effect rebuilds the letters (a random one of fifteen effects per click) |
| `focus-navigation/`, `focus-navigation-sheet.png`, `focus-navigation-tabs-zoom.png` | `Alt+Up/Left/Right/Down`, `Escape` from the composer through buffer tabs to workspace tabs and back |
| `timeline-wheel-scroll.mp4`, `-sheet.png` | Real mouse-wheel notches on the agent timeline: spring glide, retargeted notches, reverse direction |
| `editor-scroll-cursor.mp4`, `-sheet.png` | Editor wheel scroll, then `j j j j j w w w w e e b b G g g Ctrl+D Ctrl+U`: row-stepped smooth scroll and the Neovide cursor trail |
| `home-terminal-scroll.mp4`, `-sheet.png` | Home block terminal wheel scroll with sticky block headers |

## Look

- **Theme `pastel_dark`** (`neoism-frontend/gpuix/src/design/generated.ts`): bg `#000000`, fg `#e8e8e8`,
  surface `#1a1a1a`, hover `#1f1f1f`, border `#1c1c1c`, muted `#5a5a5a`, dim `#b0b0b0`, folder `#7ebae4`,
  red `#ef8891`, green `#9fe8c3`, yellow `#fbdf90`, blue `#99aee5`, magenta `#c2a2e3`, cyan `#b5c3ea`,
  white `#b5bcc9`. Syntax: comment `#7a7a7a`, string `#9fe8c3`, number `#eda685`, keyword/statement `#c2a2e3`,
  function/property `#99aee5`, type `#fbdf90`, constructor `#b5c3ea`, special `#ef8891`. Other themes live in the
  same file; port the whole table.
- **Fonts**: UI and code in the configured family (`~/.config/neoism/config.json`, default Geist Mono 15);
  icons from Cascadia Code NF (`neoism-paint/src/font/resources/CascadiaCode`); "Pinned", "Recent", "Crafting"
  and the wordmark style use Press Start 2P (`neoism-paint/src/font/resources/PressStart2P`).
- **Chrome**: 55 px activity rail (menu, files, notes, agent, search, servers with green dot); workspace tab row
  (folder icon + name, close, `+`); pane tab row (icon, title, close, `+`, active tab brighter); status line with
  mode pill (`NORMAL`/`AGENT`/`TERMINAL`), cwd pill, git branch chip, LSP chip, position, FPS pill.
- **Agent timeline**: transcript column with generous padding; user messages are right-aligned blue bubbles with a
  tail and the animated pixel orb avatar; assistant text is plain on black with inline colored terms; code blocks
  are rounded surface cards; the composer floats over the bottom of the transcript with a starfield inside the card
  and a path/context bar beneath it.

## Keyboard focus and the cursor

Window focus moves between zones: `sidebar`, `content`, `buffers` (pane tabs), `workspaces` (workspace tabs),
`details`, `device`. Source: `neoism-frontend/chrome-navigation.ts` and `gpuix/src/ui/App.tsx` (`handleChromeKey`).

- `Alt+Up`: content -> the active buffer tab (or workspace tabs when the pane has no tabs); buffers -> active
  workspace tab; workspaces stays. From content it first tries to focus the split pane above.
- `Alt+Down` (or plain `Down` on a tab row): workspaces -> active buffer tab (or content); buffers -> content; in
  content it focuses the split pane below.
- `Alt+Left/Right` in content focuses the neighbouring split; at the edge, Left goes to the sidebar (when open) and
  Right to the device/details panel. On a tab row, Left/Right (plain or Alt) steps tabs and wraps through the `+`
  slot (`keyboard.focus-step`); `Alt+Left` on the first tab enters the sidebar. Sidebar `Alt+Right` -> content.
- `Enter` on a focused tab activates it (or creates a Welcome tab / workspace on `+`); `Escape` steps back one row
  (workspaces -> buffers -> content).
- Focus indicator: a white bar caret drawn inside the focused tab/button, left of its icon; the `+` button also gets
  the hover surface. Content focus shows the normal text caret (block in Normal mode, beam in Insert).
- Each workspace remembers its last focus zone and restores it when re-activated.
- `Ctrl+Shift+Left/Right` selects the previous/next buffer tab; `Alt+1..9` selects workspaces.

Text cursors: editor and composer carets use the trail cursor below; carets blink with a hold while typing
(`DEFAULT_BLINK_PERIOD_MS`). The terminal composer shows inline completion ghost text after the caret.

## NEOISM wordmark

Shown on Welcome/Home (`splash` kind) and on new `Alt+A` agent tabs (`agent` kind). Source:
`shared/src/panels/agent_pane/view/wordmark.rs`, `shared/src/primitives/wordmark_animation.rs`,
`gpuix/native/src/wordmark.rs`; the same artwork and script appear in the browser callback pages.

- Artwork: the original NEOISM PNG (from the SVG; 1196x193, six letters at the SVG's serialized x origins), tinted
  per letter from the theme's wordmark colors (`look::wordmark_colors_or`).
- Hover is per letter: each letter has a hover value easing toward 1 under the pointer and 0 otherwise at rate
  `1 - exp(-14 dt)`. `letter_rect` scales the letter by `1 + hover * LETTER_HOVER_SCALE + sin(shimmer) * AMP`
  and lifts it by `hover * LETTER_HOVER_LIFT * height`; the idle shimmer phase is offset 0.16 cycles per letter.
- Glow: splash breathes on a 4.6 s cycle (`1 + 0.05 * (0.05 + 0.13 * breath + 0.4 * hover)`); agent uses
  `1.04 + 0.05 * hover`.
- Click: plays one of fifteen effects chosen at random (Scan, Scatter, Rain, Beams, Decrypt, Crumble, Slice, Wipe,
  Waves, Fireworks, Burn, Vhs, Rise, Checkerboard, Spiral) over 1.7-2.5 s, built from the letter bitmasks on a
  31.25-unit pixel grid (`MASKS`), then settles back to the artwork. A startup animation plays once; reduced motion
  disables hover motion and effects.

Port the masks, letter origins and effect algorithms directly; `wordmark_animation.rs` is already plain Rust with
no Sugarloaf dependency in its math.

## Motion

Exact constants from the shared Rust code; reproduce them with Rune springs or the same integrators.

- **Agent timeline, mouse wheel** (`shared/src/editor/scroll_model/misc.rs`, `agent_timeline_wheel`): a notch moves
  `3 rows x line height`, clamped to 3 notches per event, into a wheel *target*; the position follows a
  critically damped spring `omega = 16`, substeps of 1/240 s, `dt` capped at 50 ms, settles when error < 0.5 px and
  speed < 30 px/s (`timeline_scroll_policy.rs`, `step_timeline_spring`). Consecutive notches retarget the same
  spring, velocity carries over (one continuous glide).
- **Agent timeline, trackpad** (`state/timeline.rs`, `scroll_timeline_pixels`): pixels apply 1:1 immediately and
  inject `delta x 7` into a velocity clamped to +-2800 px/s; velocity decays exponentially with tau 0.28 s and stops
  below 50 px/s. Holding an edge does not build velocity. Touch flings clamp to +-6000 px/s.
- **Follow bottom**: streaming keeps the transcript pinned to the newest content through the same spring; scrolling
  up releases the latch; returning within 1 px of the bottom re-latches.
- **Ctrl+U/Ctrl+D**: half a viewport, through the same spring.
- **Editor scroll (Neovide feel)** (`shared/src/editor/code/viewport.rs`): critically damped spring `omega = 16`,
  velocity carried across retargets so held keys are one motion; jumps further than 1.25 viewports teleport the far
  part and only spring the last stretch. Wheel steps in whole rows.
- **Cursor trail (Neovide)** (`shared/src/panels/trail_cursor.rs`): four corner springs, critically damped,
  `omega = 4 / length`; length 0.10 s for jumps, 0.04 s for short moves (<= 2 cells); trail size 0.8 (leading edge
  leads, trailing edge lags); beam width 15% of a cell, underline 20%.
- **Block terminal**: the scrolled-away block's header (cwd + command) sticks to the top of the viewport while its
  output scrolls under it (`reference/motion/home-terminal-scroll-sheet.png`). Wheel pixels accumulate; whole rows commit to the scroll position and the sub-row residual
  offsets the content so scrolling is pixel smooth (`shared/src/terminal_scroll_state.rs`). New output follows the
  bottom unless the user scrolled away; layout changes keep the anchored block in place.
- **Overlays** (palette, pickers, finder, menus): a notch is `3 rows`, trackpad pixels 1:1.
- **Ambient**: composer starfield twinkles continuously; "Crafting" status word and scanner animate on a wall clock
  (not model time); user orb animates while visible; streaming text reveals word by word.

## Browser callback pages

MCP OAuth and provider "connect" flows finish in the user's browser on pages served by the agent server
(`neoism-agent/crates/neoism-agent-service-api/src/browser_pages.rs`: `mcp_connected`,
`provider_authorization_received/failed/cancelled`, with the animated wordmark). They are not part of the frontend
and stay unchanged. The Rune app must: open the URL from `ConnectOauthUrl` / `McpOauthUrl` in the system browser,
show the pending state in the provider/MCP modal, and react to `ConnectFinished`, `ConnectFailed`, `McpChanged`
and `McpFailed` events.

## Data and performance rules

Lessons from the GPUiX frame investigation (`docs/gpuix-live-rendering-baseline.md`):

- Never block the UI thread on daemon traffic. Parse WebSocket frames off the UI thread into typed events.
- Page older history by bytes (~160 KiB of message text per page, 8-64 server messages, start at 24) and request
  it ahead of the viewport.
- A head refresh of an already loaded transcript applies only the rows that changed.
- Animations and scroll run on Rune's render thread; app-side work must not be on the per-frame path.
- Measure missed vblanks from presentation timestamps (Hyprland reports `sequence=0`).
- A frame that re-runs the views (a commit) costs several times one that does not, so nothing
  that moves may need one. Ambient motion rides live values set from `poll`: the caret blink
  (`Cursor::blink`), the tool spinner (`tools::Spinner`), the activity line, the orbs (one
  `rune::Pixels` picture each) and the vsync heartbeat (`ui::chrome::Heartbeat`). `poll`
  returns true only when the views would draw something different.
- A layout handler on anything inside a scroll view fires on every frame of the scroll: it
  says `Tx::live_only` unless it changed something a view reads (the transcript's width).
- Rune keeps about five viewports of transcript rows mounted and builds each on every commit.
  Every row kind is a `rune::memo` keyed by what it draws (`Session::message_versions`,
  `ToolState::epoch`), and the row list itself is cached (`Session::row_items`): folding tool
  runs parses their diffs. A memo is only reused whole when nothing in it holds a live
  binding, which is why an orb is a picture and not a grid of live-coloured cells.
- A streaming reply's text is applied as it arrives and shown at most thirty times a second
  (`agent::StreamPace`), however fast the model writes.

## Running

```sh
rune/app/build.sh build --release
/tmp/neoism-rune-target/release/neoism-rune  # connects to the running local daemon
NEOISM_RUNE_OFFLINE=1 /tmp/neoism-rune-target/release/neoism-rune \
  --scene editor-long --snap /tmp/editor.png --size 1252x674 --scale 2 --at 2
```

The build helper selects Rune's pinned toolchain. Set `CARGO_TARGET_DIR` to override its default output directory.
Use `--list-scenes` for reproducible surfaces, `--tree` for element/accessibility inspection, and `--trace`
for per-frame geometry. Native windows do not replay `--script` in the pinned Rune version; the native checker
sends actual input to its own focused test window.

## Validation and remaining migration work

The editor now mounts visible wrapped rows and caches syntax/wrap geometry until text or width changes.
Scrolling and cursor movement reuse that cache. Editor and conversation scroll state belongs to each buffer
or session, so input over another split does not reuse the active pane's geometry.

LSP status, diagnostics, mouse/keyboard hover and completion are wired through the existing daemon protocol.
Live checks covered Rust, TypeScript, JavaScript and Python. Clipboard paste, mouse word/line selection,
dragging beyond the editor edge, `/usage`, Database browsing/editing and shortcut rebinding are implemented.
Markdown files intentionally use the code editor for now; Extensions remains removed.

Workspace creation is wired to Ctrl+Shift+W and the top `+`; Alt+1..9 restores each workspace's own panes,
drafts and focus. Ctrl+Shift+Left/Right steps tabs in the active pane. Agent and exact command-palette `q` close the current tab; `qall` closes
its workspace through the daemon, selects an adjacent workspace, and quits when the last workspace closes.
Tab close buttons appear only on hover, file icons share the tree's font and mapping, and agent tabs use
Neoism's image. Both strips reorder live while dragging, using GPUiX's slot-crossing rule and critically
damped neighbor slides. The dragged tab follows the pointer without an insertion line or border. Shift+Alt+Left/Right moves the focused tab;
workspace focus or Ctrl+Alt+Shift+Left/Right targets the workspace strip. Normal-mode Space+x closes the
editor buffer through the unsaved-edit guard. Home and agent wordmarks animate hover in every visible pane.
MCP-supplied artwork is shared by /mcp, its actions header, and timeline tool calls, with bundled marks and
the original plug as fallbacks. Dirty editor closes wait
for a successful save acknowledgement or an explicit discard choice. See VALIDATION.md for completed checks and the remaining parity/cutover work.

Run the checks from the repository root:

```sh
rune/app/build.sh test --release
python3 rune/scripts/check-headless.py
# Opens a bounded test window on workspace 5, then restores the prior focus.
python3 rune/scripts/check-native.py --scene editor-long --wheel --natural --size 624x674
python3 rune/scripts/check-trace.py /tmp/neoism-rune-verify/native-editor.jsonl --require-scroll --require-cursor
python3 rune/scripts/analyze-presentations.py /tmp/neoism-rune-verify/native-editor.wayland.log \
  --trim-seconds 2 --end-seconds 4.4
```

Frame pacing against the real daemon, without taking the keyboard or pointer: the app runs a
scripted workload on itself (`NEOISM_RUNE_DRIVE`, `app/src/drive.rs`) in a window on a headless
Hyprland output, and the script reports UI-thread work per frame, view re-runs and missed
refreshes for each marked span:

```sh
python3 rune/scripts/check-live-pacing.py --session <conversation id> \
  --drive "ready;wait:2;mark:wheel;wheel:30:3;wheel:-30:3;mark:pad;touch:700:2;mark:end;quit"
# A long agent run (every tool row live) with a fast reply streaming over it:
python3 rune/scripts/check-live-pacing.py --session <conversation id> \
  --drive "ready;wait:2;live;wait:1;mark:stream;stream:/path/to/reply.md:10;mark:end;quit"
```

`NEOISM_RUNE_FRAME_TRACE=1` prints each frame's `work_us` (poll start to encode end) and whether
it re-ran the views; `NEOISM_RUNE_TRACE_POLL=1` names what re-ran them; `RUNE_COMMIT_BUDGET_MS=N`
prints Rune's own breakdown of commits over `N` ms.

The native checker requires this machine's Hyprland Lua dispatch API, `wtype`, and `/dev/uinput` access for
wheel input. It stops sending input if focus changes. `--natural` exercises normal redraw scheduling;
without it, `--frames` forces continuous submissions and is insufficient to verify redraw scheduling.
Measure only intervals with active scrolling or animation: idle gaps are deliberate, not missed frames.

See [VALIDATION.md](VALIDATION.md) for measured results and their limits. Step 8 remains in progress:
the screenshot suite exercises surfaces and input, but does not establish complete visual/behavioral parity.
Step 9 has not run; the installed app and release packaging still use GPUiX, which remains the reference.
