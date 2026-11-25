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
9. **Cutover** - the release ships this app as `neoism` on Linux, macOS and Windows
   (`scripts/build-rune-release.py`, `scripts/package-rune-release.py`, `release-neoism.yml`): it answers the
   command line the installers and deployed updaters call (`src/cli.rs`), starts the bundled workspace daemon
   when none is running (`src/startup.rs`), and keeps the bundle layout those updaters validate. Still to do:
   delete `neoism-frontend/gpuix`, the GPUiX pins/patches and the Sugarloaf painter paths.

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
  Chat prose, user bubbles and composer input deliberately use a proportional sans at regular weight, with bold Markdown headings/emphasis. Its default is the platform's own interface font, as t3code's is (`ui/fonts.rs` `CHAT`: San Francisco, named `System Font`, on macOS; Segoe UI on Windows), and every choice falls back to Geist Sans, which is also the default on Linux. The existing licensed variable font in `neoism-paint/src/font/resources/Geist` is embedded in the app, without a runtime download. Chat-only styles live in `app/src/agent/typography.rs`; inline/fenced code, editor, terminals, tool grids, tables and composer chips retain their monospaced styles and metrics.
  Settings > Appearance separates **Chat font family** (`appearance.chat-font-family`, default the platform interface font; the daemon's schema still says `Geist` and Rune overrides it) from **Monospace font family** (`appearance.fonts.family`, default `Geist Mono` in Rune). Both offer bundled and installed families, apply without restarting, and persist through the daemon. Chat changes leave code and fixed-width UI alone; monospace changes update those surfaces and their editor/terminal cell widths. Icon and pixel-art fonts stay fixed.
- **Chrome**: 55 px activity rail of drawn line icons (`ui/feather.rs`: files, notes, agent, servers with green
  dot, settings); workspace tab row (folder icon + name, close, `+`); pane tab row (icon, title, close, `+`,
  active tab brighter; an accent dot while that tab's agent works, a yellow one for unsaved files). Tab titles
  use the proportional interface font (`Type::tab`), not the monospace. Status line with mode pill
  (`NORMAL`/`AGENT`/`TERMINAL`), cwd pill, git branch chip, LSP chip, position, FPS pill.
- **Open on phone**: in a local workspace, the desktop QR button asks the published loopback daemon for a one-time device invite. Connect the phone to Tailscale and scan in Neoism: a `v=2` device invite claims immediately without a password. Older device invites still prompt for their password. On a Fast Server hosted by this app, the button reopens its existing QR and copy-link modal; joining someone else's server does not grant a share action. The offline `workspace-phone-share` scene uses a reserved `.invalid` host and a public test CA, not a live invite.
- **Agent sidebar** (deliberately departs from `04-sidebar-conversations.png`): a search field with the project
  filter and New chat as icon buttons beside it; conversations are rounded cards with the project and age on top
  and the title and its model's provider mark below. The filter is a rounded popover ("Search projects…" over an
  accent rule, the current project badged with a check). `/` focuses the search; Escape clears, then leaves it.
  There are no Pinned/Recent headings: a pinned card carries a pin beside its age and leads the list.
  The sidebars, Settings (breadcrumb header, search above the categories), the palette, finder and menus use
  the proportional chat font (`Type::sans`, titles at 600) and line icons (`glyph::*_LINE`); new panels should
  follow them. Key hints, paths in previews and code stay monospaced.
- **Git panel** (`app/src/git.rs`, `app/src/git/`; after t3code's source control, read from its code): the
  status bar's branch pill opens a right-hand panel with three tabs. **Changes** has a branch picker (search,
  switch, create), one button that names the next step for the branch (`actions.rs` `quick`: commit, push,
  pull, publish, open a pull request, chained as daemon steps that stop at the first failure) with a menu of
  the single actions and the reason a disabled one cannot run, a row that opens the diff, the file checklist
  and a message box; an empty message is written from the changed files. The **diff** tab (`review.rs`) is
  every changed file in one virtual scroll, a folding header per file, stacked or side by side, a file list,
  a scope switch (uncommitted, or the branch against the default branch) and line comments that go into the
  newest agent chat's draft with their file and line. **Pull requests** (`pulls.rs`) lists and filters them
  and opens one in the diff tab: checks, review threads (resolve), conversation, review (comment, request
  changes, approve), merge by method, check out, and its changes in the diff view. **Issues** (`issues.rs`)
  does the same for issues, with a reply box and a button that starts an agent chat on the issue. Hosted
  data comes from the daemon through `gh`, so it is GitHub only. Sync, StageAll, CreateBranch, DiffRange
  and issues need a daemon of this version or later; against an older one the panel still commits, pushes
  and lists pull requests. Scenes: `git-panel`, `git-review`, `git-review-split`, `git-pulls`, `git-pull`,
  `git-issues`, `git-issue`.
- **Focused agent view** (departs from `05..08`; `app/src/agent/focus.rs`): a turn has two states and its work
  row's arrow says which. Folded (the arrow points right), running or done, none of its calls show: it is the
  prompt, the work row (`Working · 9 steps · 1 file changed`, failures counted in red; `Worked` once done), the
  everything the agent wrote (its notes on the way and the reply, in order) and, while it runs, the thought
  being had, one at a time; the running row's `Working` shimmers and its dots count up on live values. Open (the arrow points down, after a press on the row), it is the whole trace: every
  call as its card with output and diffs, held in place under the pointer; another press folds it. A sub-agent
  still at work keeps its card until it finishes; checklists, the reader's own shell commands and notices always
  show. The running turn's activity line names the step it is on after its timer. Every loaded turn folds, so
  an older turn's calls can be opened too. Settings > Agent > **Focused agent view**
  (`appearance.agent-focused-view`, default on, the app's own key) turns it off: every call is a card again.
  Headless scenes and tests draw the full trace unless they ask (`--scene work-focused`,
  `work-focused-settled`, `work-focused-open`).
- **Settings** (departs from `27-settings.png`): the rail's top button is a gear that opens it; there is no app
  menu button. The sidebar holds the search, then General, Appearance, Themes, Keybindings and the grouped pages
  (Code: Editor, Markdown, Terminal; Agents: Agent, Presence; System: Interface, Platform, Developer), then
  More (Backgrounds, Database, About). `editor.markdown.*` settings live on Markdown, apart from the code
  editor's. A page is a title, a summary and rounded cards of rows: name and description left, control right.
  No window cursor sits on the page; the keyboard row is tinted. Themes is a gallery: the hovered (else current)
  theme's syntax sample on top, then every theme as a card in its own colours; a click applies it. Scenes:
  `settings`, `settings-themes`, `settings-backgrounds`, `keybinds`. Cmd+W no longer closes a tab by default
  (rebindable in Keybindings). Rows inside a page sit under section headings (`Settings::section`: Fonts,
  Effects, Window, Panels, …). Themes and Backgrounds open with a small mock Neoism window in the theme's
  colours; Backgrounds draws the picture behind its conversation, with live sliders for opacity, blur, pixelation (`ui.window.background-image.pixel-size`) and ordered dithering (`ui.window.background-image.dither`). Sliders save on release. Pixelation and dithering are off by default and can be combined. Cover/Contain and the gallery of installed pictures sit below the preview.
- **All-projects catalog** (`agent/catalog.rs`): on `~` or with the filter on "All projects" the sidebar asks the
  daemon for the project list (`ListSessionProjects`) and each project's chats five at a time
  (`ListProjectSessions`). The first twelve projects load at once and the next twelve as the list is scrolled
  toward its end; "Load more" under a project fetches its next five (the only manual step, a per-project limit).
  One project's own list pages the same way: thirty chats, then older pages as it nears its end. A refresh keeps what was loaded beyond the first
  page. `App::threads` stays the flat list (pins, then loaded pages). A single project still uses `ListThreads`.
  The filter's rows are two lines: the project's name over its path.
- **Conversation status mark**: four squares in a card's top right, in place of its age, while it is not idle
  (`sidebar::status_mark`). Still on a yellow diamond when waiting on you; moving while at work, each kind its own pattern on the shared
  `tools::Spinner` live squares: ink orbit for a running reply, red chase round the diamond for background work,
  green draw-in and out while compacting. A conversation that goes from working to idle while it is not on
  screen shows a still white diamond (finished, unread) until it is opened or works again; only changes seen
  while the app runs count, and the set is not saved (`App::finished`, `note_activity`). `poll` ticks the spinner while the open panel has a conversation at
  work (`sidebar_working`). `NEOISM_RUNE_STATUS_MARKS=1 --scene projects-menu` shows them all.
- **Add project** (`agent/project_add.rs`): the folder button between the filter and New chat opens a centred
  card: a search row with a back arrow, the sources (New project, Local folder, Git URL, GitHub repository) and a
  footer of key hints. Local folder opens the folder browser. The others ask for a name or address in the same
  card and send the daemon `Git::CreateRepository` / `Git::CloneRepository`; it answers `RepositoryStarted` at
  once, then `RepositoryReady` (the workspace moves into the project) or `Error` (shown in the card). A daemon
  that does not acknowledge within 2.5 s predates the request: the same work then runs as a shell line in the
  workspace's terminal. New projects go beside the current project, or in `~/projects` from the home directory,
  or wherever "Change" picks in the folder browser. The daemon refuses a non-empty target, relative paths, `..`
  and anything but https/http/ssh/git/file or `user@host:path` addresses (never a `name::` helper transport). Scenes: `project-add`,
  `project-add-github`.
- **Model picker**: typing on its first screen searches every model of every provider (grouped under the
  provider's name), not only the recents and providers listed (`Picker::search_all`).
- **Settings rows**: Vim mode sits on General; `appearance.mashup-pack` and `appearance.palette` are hidden
  (`settings::HIDDEN`). Backgrounds has a blur slider beside opacity; the window blurs the picture off the UI
  thread at a reduced size (`backgrounds::decode_blurred`) and shows it sharp until that is ready.
- **Wallpaper**: the file tree, notes and agent sidebars stay opaque over a background picture; only the
  conversation, editor and home panes let it through.
- **Question rail**: the ticks sit in the transcript's left gutter and the hover bubble opens to their right. Scenes: `projects-menu`, `conversations-dashboard`, `family`.
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
- `Ctrl+Shift+Left/Right` selects the previous/next buffer tab; `Alt+1..9` selects workspaces on every platform, including Option on macOS. The physical number row is used so Option-generated symbols do not interfere; macOS also accepts `Cmd+1..9`.
- `Alt+Shift+Left/Right` reorders the active pane tab from content, sidebar or tab-strip focus; it reorders workspace tabs only when the workspace strip has focus. On macOS, Ctrl+Cmd+Shift+Left/Right is an additional reorder alias. `Ctrl+Alt+Shift+Left/Right` explicitly reorders the active workspace without first focusing its strip.

Text cursors: editor and composer carets use the trail cursor below; carets blink with a hold while typing
(`DEFAULT_BLINK_PERIOD_MS`). The terminal composer shows inline completion ghost text after the caret.

Row and editor cursors stay clipped to their active viewport, including while focus changes. The file tree and settings headers, menu bounds and neighbouring chrome must never be crossed by either the ordinary caret or the native glass lens.

The opt-in `appearance.effects.liquid-glass-cursor` uses a live optical Metal lens on macOS with Rune's default native Metal backend. Swift encodes magnification, dispersive edges and velocity-responsive highlights into Rune's own presentation command buffer; there is no SwiftUI overlay or captured window image. Other platforms, headless rendering, explicitly selected wgpu, reduced transparency/increased contrast, and occluding popups keep the ordinary Rune cursor. Reduced motion snaps the native lens. See [`app/native/README.md`](app/native/README.md) for the GPU-copy distinction, resource contract and verification commands.

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

Wordmark clicks retain all fifteen original effects on both Home and agent wordmarks. Pixel flames are a persistent decoration, independent of the click rotation: `appearance.wordmark-fire` defaults to `true` and can be disabled using the "Wordmark flames" settings toggle. The first-party renderer builds seeded geometric plumes with tapered, moving colour cores; no third-party fire code or assets are used. Hover, shimmer, glow and original splash positioning remain unchanged. Reduced motion disables flames and click effects. Home shortcut hints use the active theme's accent colour. Render the flame layer without changing saved settings using `--scene welcome-fire --snap /tmp/neoism-splash-fire.png --at 1`.

The shared `ui::fire::PixelFire` effect accepts a top-edge outline (one optional row per pixel column), a stable seed, a five-colour palette and an animation rate. Construct it once, then call `paint` on any Rune `Shape` used by a `canvas`, with an origin, cell size, clock and opacity. A flat edge uses `[Some(0); N]`; shaped glyphs supply their top occupied rows. Callers own visibility, reduced motion, redraw scheduling and replay/fade duration, so idle or hidden effects do not keep the app rendering.

During click effects, wordmark flames attach to their source foreground pixels using the same sampled translation, rotation, scale and opacity as the original animation. The flame canvas keeps the flying-piece bounds through click, settle and idle, so it never starts a separate layout animation at the handoff. This self-animated subtree uses `instant_changes` to prevent a second implicit spring on the returning artwork; the original procedural effects and live hover/shimmer still drive its motion. The flame clock never restarts on a click.

Fire palettes: Neoism Dark and Neoism Light retain the classic red/orange/yellow/ivory colours. Other themes derive the five flame colours from their background, accent, magenta, cyan and foreground. Palette selection happens at paint time, so switching themes does not rebuild or restart the cached simulation. New uses should pass `ui::fire::colors(app.pal)` unless they intentionally need a custom palette.

The agent activity labels (Crafting, Pondering, Tinkering and the other running states) carry smaller versions of the same flames, attached to each moving letter's visible top outline from the bundled Press Start 2P font, including lowercase heights, side bearings and scrambling punctuation. The hot roots overlap the glyph edge instead of floating above its layout box. They share the wordmark flame toggle, reduced-motion policy and theme palette. Cached live pixel textures advance at 16 Hz (or when a scrambling glyph changes) without rebuilding the status view.

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
  Rune on macOS deliberately uses native precision scrolling instead: finger and OS momentum deltas apply 1:1, without the reading gain or an additional synthesized coast. Mouse-notch springs and other platforms retain their existing behavior. Edges remain clamped; elastic overscroll is not implemented.
- **Follow bottom**: streaming keeps the transcript pinned to the newest content through the same spring; scrolling
  up releases the latch; returning within 1 px of the bottom re-latches.
- **A sent message opens its turn at the top**, when Settings > Agent > **Send to top**
  (`appearance.agent-turn-at-top`, default off) asks for it (t3code's new-turn anchor; `app/src/agent/turn.rs`): the transcript
  slides until the message rests 16 pt under the viewport's top, and the reply streams into the room below it. That
  room is a floor under the rows' height, so nothing moves while the turn is shorter than the view. Past that a
  turn that runs tools or reasons is followed as before; a reply of text alone keeps its start in view and runs
  on under the composer (jump-to-latest follows it). Wheel or trackpad input takes over at once.
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
- **Wide chat tables**: precision gestures choose one axis after the first 3 points and keep it through momentum. Horizontal gestures consume vertical noise instead of moving the transcript; a new vertical gesture still scrolls the transcript normally. Trackpads without explicit gesture boundaries reset the lock after 180 ms without finger input.
- **Direct touchscreen input**: a finger swipe scrolls agent prose/code and editor/Markdown documents without selecting or copying text. Tree rows scroll on a normal swipe; holding still for 500 ms before dragging allows the existing file/folder move gesture. This is a deliberate hold, not a hardware-pressure requirement. Mouse text selection, mouse file dragging, and direct touch dragging on sliders/scrollbars are unchanged. Rune's shared input patch is tracked under `vendor/rune`; see `vendor/rune/NEOISM.md` for the upstream pin and local changes.
- **Ambient**: composer starfield twinkles continuously; "Crafting" status word and scanner animate on a wall clock
  (not model time); user orb animates while visible; streaming text reveals word by word.

## Browser callback pages

MCP OAuth and provider "connect" flows finish in the user's browser on pages served by the agent server
(`neoism-agent/crates/neoism-agent-service-api/src/browser_pages.rs`: `mcp_connected`,
`provider_authorization_received/failed/cancelled`, with the animated wordmark). They are not part of the frontend
and stay unchanged. The Rune app must: open the URL from `ConnectOauthUrl` / `McpOauthUrl` in the system browser,
show the pending state in the provider/MCP modal, and react to `ConnectFinished`, `ConnectFailed`, `McpChanged`
and `McpFailed` events.

Connection/MCP cards are deliberate workflows: clicking the background, moving/resizing the window, changing focus, or restoring the composer owner must not dismiss their input or pending authorization state. Escape/back, Cancel, and Done remain explicit exits. MCP catalog changes refresh a pending authorization in place; unrelated or late OAuth/error events cannot overwrite a different picker. Slash-picker rows and header contents update immediately on search edits, while keyboard/wheel list and cursor motion retain their springs.

Sending an agent message without an available model opens provider connection or model selection before consuming the draft, pastes, or attachments and before creating a conversation/turn. Authentication or model selection does not automatically send that draft. The first explicitly selected available model is offered to the daemon as the user's default using its write-if-absent configuration path; existing explicit defaults are preserved and subsequent selections remain conversation-specific. A loading/failed catalog is not evidence that an existing restored/custom model is unavailable.

## Data and performance rules

Lessons from the GPUiX frame investigation (`docs/gpuix-live-rendering-baseline.md`):

- Never block the UI thread on daemon traffic. Parse WebSocket frames off the UI thread into typed events.
- Apply inbox events with a shared 1 ms soft budget across the window, at most 256 events per poll and 16 per workspace turn. Resume round-robin on the next poll, retaining FIFO order and explicitly waking while any queue remains. A single handler may exceed the deadline; the budget cannot preempt it. `NEOISM_RUNE_TRACE_POLL=1` reports applied events, remaining queue depth and drain time as `[inbox]` records.
- Live message mutations use the session's ID-to-position index. Content-only changes reuse it; replacing, reordering or removing history invalidates it. Length changes and mismatched cached IDs are checked too, but any new same-length structural mutation must explicitly invalidate message positions.
- A file is a shared document the daemon owns (`collab/doc.rs`): edits go out as Yrs updates and come back
  merged, saving asks the daemon to write it, and undo is the replica's. Never write a second path that
  changes `Document::lines` behind the binding without bumping `revision`.
- Page older history by bytes (~160 KiB of message text per page, 8-64 server messages, start at 24) and request
  it ahead of the viewport.
- A head refresh of an already loaded transcript applies only the rows that changed.
- Animations and scroll run on Rune's render thread; app-side work must not be on the per-frame path.
- Measure missed vblanks from presentation timestamps (Hyprland reports `sequence=0`).
- The agent question rail indexes user-question rows when the transcript structure changes. Scrolling searches that index and publishes only the old/new tick colors through live fills; changing the active question or reaching the bottom must not rebuild the transcript just to move its highlight.
- The status `FPS` badge on macOS Metal uses this window's actual `MTLDrawable.presentedTime` feedback, not encode spacing, display-link targets or monitor refresh. It reports **completed-sample throughput**: completed presentation intervals divided by their elapsed time, over a trailing one-second window anchored to the newest reported presentation (250 ms completed-history minimum; bounded to 256 timestamps). The sample immediately before the window boundary is retained, so a real gap crossing that boundary counts in full even when the measured span exceeds one second. Modest asynchronous callback lag therefore does not turn uniform actual 120 Hz into 118. The latest completed rate is retained until the newest actual sample is at least one second old, then reads zero on the next natural poll. A genuine stall lowers the rate once the next actual presentation closes the gap; it is not silently discarded as idle. Resuming recovers as that gap leaves the completed window. This is throughput, not a claim that every interval is smooth: use native `--trace` `present_dt_ms`/missed/bunched statistics to diagnose cadence. Unsupported surfaces/platforms and headless runs apply the same completed-interval calculation to drawn submissions and label it `SUB`; unchanged/skipped frames never count. Feedback and badge updates introduce no FPS timer or wakeup; text refreshes at most twice a second during existing work, and a completely resting screen can retain its last painted reading until work resumes. No median, target-rate clamp or forced 120 Hz timer is used. Existing macOS CVDisplayLink pacing follows the display, coalesces late ticks, and respects the built-in display's Low Power cap; measurement does not change it.
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
- A reply is drawn a heading's worth at a time, each piece a memo keyed by its own text
  (`prose::chunks`): while it streams only the last piece is parsed, highlighted and built.
  As one Markdown document a 50 KB reply cost 2.5 ms of every streaming commit.
- The window cursor is drawn at the root, over everything: in a file it rides the scroll
  (`Cursor::carry`) and travels on a live offset (`Cursor::place`), so scrolling under it
  commits nothing. Code rows are memos keyed by their own content (`editor/view.rs`,
  `cached_row`).
- On macOS a frame starts on its display-link tick, which runs just after the system's cut-off for
  the refresh before, so the frame has nearly the whole period (about 6.5 ms of UI-thread work
  on a 120 Hz panel, the GPU's own 2 to 3 ms aside). A tick is skipped only when the frame before
  was submitted after that tick ran, that is, when it overran its own slot
  (`vendor/rune/.../platform/display_link.rs`, `permits`). The earlier rule also skipped any
  tick less than half a period after the last submission: every frame over about 3.7 ms cost
  the next refresh, and a transcript that needed 4 ms a frame drew at 64 frames a second.
- A reply is cut into pieces at paragraphs too, not only headings, once a piece holds 1.2 KB
  (`prose::chunks`), with the kit's own collapsed margins between them, and a fenced code
  block of 48 lines or more is drawn in slabs of 32 lines (`timeline::code_slabs`), each a
  memo keyed by its own text. A text is shaped whole whenever it changes: as one element a
  900 line file streaming in cost 20 ms of every update by its end (37 frames a second), and
  40 KB of prose with no heading 7 ms. Both now cost what their last piece costs.
- A transcript is scrolled by holding one of its rows (`Tx::scroll_key_to`, `agent/anchor.rs`),
  never by an absolute offset: Rune works a held row out again after an older page lands and
  its rows are measured, and applies an offset as it was. Older history is asked for three
  viewports before the reader reaches its start, while they head up; nothing stops, waits or
  moves when it lands.

## Markdown spelling

Local links in agent replies, tables and task/goal cards accept Markdown section fragments such as `docs/guide.md#installation` in addition to existing line references. Relative agent links resolve from the workspace; links inside notes resolve from the source document. Percent-encoded paths/headings, rendered heading text, Unicode and repeated-heading suffixes are supported. A pending section belongs to the destination buffer and is applied when its asynchronous load completes, without stealing focus back from a later tab selection or replacing unsaved text.

Markdown documents do not attach to language servers or show LSP status/diagnostic pills. Spelling is off by default: enable Settings > Editor > Markdown spell check (`editor.markdown.spellcheck: true`) to show red squiggles and right-click correction menus in prose and tables. Disabling it removes cached marks and spelling menus immediately and stops new checks. Corrections preserve source markup and undo, and reject actions captured before an intervening edit, tab change or disabling the setting. Code, link spans, frontmatter and diagram source are excluded. Checking and suggestions use one bounded background worker and cached results, with completion-driven redraws rather than an idle timer. macOS uses its native NSSpellChecker with automatic language detection instead of rejecting valid words missing from a static word list; shared ignored and learned words are still respected. Other platforms use the shared dictionary, conservatively recognizing known typos if no system dictionary exists. Individual lines over 16 KiB are skipped.

## Mermaid diagrams

Closed `mermaid` fences render natively in agent replies and Markdown notes. Replies provide Source, Copy and a larger image view; language-tagged Mermaid code parts also render when complete. In notes, Edit beside Copy reveals the original fence in Insert mode, and Preview returns to the chart without saving or discarding edits. Entering Insert anywhere in a diagram still reveals its source. Rendering never rewrites the document or its undo/presence coordinates. Incomplete streamed fences and code parts remain literal. Errors retain readable, editable/copyable source instead of drawing a misleading partial chart.

The pinned native Merman engine runs on a lazy, bounded background worker, not inside Rune's layout, paint or input callbacks. Source and theme identify cached, prepared images; scrolling, cursor motion and zoom reuse them. Notes admit render jobs only for mounted virtual tiles, not every offscreen fence in a long document. Completions wake the window once and invalidate diagram-bearing views, not unrelated documents. There is no browser, JavaScript runtime, network request, shell renderer or idle animation timer. Fonts are prepared on the worker, and unresolved glyph coverage is reported rather than silently replacing labels with missing-glyph boxes.

Preview limits are deliberate: 32 KiB source, 512 model items, constrained layout work, 2 MiB SVG, 4 megapixels and a 4096-pixel raster side. The operation has a cooperative two-second deadline; the final bounded rasterizer is not forcibly interrupted. The cache retains up to 32 entries and 64 MiB of prepared outputs, excluding bounded in-flight work and images retained by visible views or the larger preview. Unsupported syntax, missing fonts or resource-limit failures keep the source available. Native Mermaid compatibility is not a claim of pixel-identical browser Mermaid support for every extension or custom directive.

Focused verification uses `cd rune/app && ./build.sh test -j 4 --bin neoism-rune mermaid:: -- --test-threads=1`. The explicitly ignored `ui::mermaid::tests::cached_chart_frame_diagnostic` renders chat and notes fixtures and prints warm native-frame CPU measurements; use `NEOISM_RUNE_OFFLINE=1 NEOISM_RUNE_MERMAID_PNG=/tmp/neoism-mermaid` with `--ignored --nocapture`. These CPU diagnostics do not replace live presentation-timestamp checks for a 120 Hz pacing claim.

## Sleep, wake and lost connections

Closing the lid must cost nothing: on opening it, every conversation (and every sub-agent) is
where its agent is now, without replaying what was missed, and with all of it in the history.

- **Noticing** (`daemon.rs`, `Liveness`): a socket to another machine does not report that
  machine going away, or this one sleeping. A connection quiet for 10 s is asked
  (`Workspace::Ping`) and dialed again if it does not answer within 8 s. The wall clock jumping
  5 s past the connection thread is a wake: it asks at once, on a 4 s deadline. A connection
  that answers after a wake reports `Inbound::Woke`; a new socket reports
  `Inbound::Connected { again: true }`.
- **Coming back** (`agent/resync.rs`): the daemon keeps event streams per socket, so a new
  socket follows each open conversation's family again (`ResumeStream` only: `SwitchThread`
  would also reset its settings). A stream is a live tail: it opens with the reply being
  written, whole, and continues from there. Each open transcript takes its newest history page
  over what is loaded (`merge_head`, only changed rows re-render); sub-agents fetched ahead
  are refreshed a few at a time or when opened. A page that shares no row with what was
  loaded (hours away from a remote agent) replaces the transcript and older history pages in
  above it. A history page never shortens text that has streamed since it was read.
- **SSH** (`servers/flow.rs`, `tunnel_lost`): the tunnel's `ssh` dies with the link. The same
  recipe is dialed again and the connection re-pointed at it (`Daemon::retarget`).
- **Daemon**: when its own stream from the agent breaks and reopens it sends
  `SessionEvent { kind: "stream.resumed" }`, and the app refreshes that family's transcripts.
- **Agent server** (`session_loop.rs`): after the host wakes, a provider stream that stays
  silent for 30 s is retried instead of waiting out the 5 minute idle watchdog (whose clock
  does not run during suspend); only while no tool call of the step has run.

`python3 rune/scripts/check-sleep-resume.py sleep wake away subagent` does this to a real
window mid-reply (the app stopped, its link gone dark) and compares the transcript it ends
with, row for row, against the same conversation opened fresh.

## Running

Set `presence.avatar-palette` to `"obsidian-gold"` in `config.json` and relaunch Rune to give your own animated presence orb a black, bronze, gold and champagne palette. Omit the setting to use the name-generated colours. The override is local to this app and does not change collaborators' avatars.

Window recovery is independent from live daemon state. `neoism --new-window` opens fresh tabs; a normal launch restores the most recently saved closed local window, never one still open in another process. Explicit directory/file launches also start independently so recovery cannot override the requested project. Each window holds an OS lock on its own recovery snapshot in `$XDG_STATE_HOME/neoism/rune/session.windows/` (the old `session.json` remains readable). Closing a window saves its tabs, splits, drafts and unsaved text, then releases its daemon resources; reopening starts fresh shells and reloads conversation history. Explicitly closing the last workspace forgets only that window's recovery. Startup keeps at most 20 clean closed-window snapshots, plus active windows and any snapshots containing unsaved edits or drafts; protected recovery is never automatically evicted. These disk snapshots do not keep hidden tabs or shells running. Linked-window synchronization is separate and is not implied by sharing a daemon or directory.

```sh
rune/app/build.sh build --release
RUNE_TARGET="${CARGO_TARGET_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/neoism-rune-target}"
"$RUNE_TARGET/release/neoism-rune"  # connects to the running local daemon
NEOISM_RUNE_OFFLINE=1 "$RUNE_TARGET/release/neoism-rune" \
  --scene editor-long --snap /tmp/editor.png --size 1252x674 --scale 2 --at 2
```

The build helper selects Rune's pinned toolchain and keeps build output in `${XDG_CACHE_HOME:-$HOME/.cache}/neoism-rune-target`, not RAM-backed `/tmp`. Set `CARGO_TARGET_DIR` to override it; relative overrides resolve from `rune/app`.
Use `--list-scenes` for reproducible surfaces, `--tree` for element/accessibility inspection, and `--trace`
for per-frame geometry. Native windows do not replay `--script` in the pinned Rune version; the native checker
sends actual input to its own focused test window.

The agent composer uses native blue atomic pills for selected skills, image/file anchors and substantial pasted text (more than 1,000 characters or 10 lines). A compact `[pasted N chars]` expands to the original text when sent; paste payloads and skill pills survive draft recovery. Backspace/Delete remove a pill as one item, and Undo restores it. Type `$` to search the skills catalog at the cursor and use Enter or Tab to insert a skill. `/tasks` opens the current conversation's live task list above the composer with the timeline's Markdown styling. Headless scenes: `tasks`, `tasks-empty`, `tasks-long`, `composer-tokens`, and `skills-mention`.

`/goals` opens the current conversation's persistent goal in the same above-composer panel, with Markdown content, lifecycle status, a live elapsed timer and any completion/blocker summary. `/goal` and its set/pause/resume/clear commands are unchanged. Age is wall-clock time since the server's persisted `created` timestamp (paused time is included and labelled); complete/blocked goals use their last `updated` timestamp instead of continuing to tick. Headless scenes: `goals`, `goals-paused`, `goals-complete`, and `goals-empty`.

**Workflows** in the command palette (`Alt+P`) opens the selected workspace’s prompt workflow catalog on the local daemon (Unix). Filter by name or ID; click a row or use ↑/↓ to inspect it. **Refresh** reloads definitions and diagnostics. **History** lists recent Rune manual and scheduled runs. **Open source** opens the revalidated Markdown source in the regular workspace editor; normal editor save behavior applies. **Activate**, **Pause**, and **Run now** ask for confirmation and check the source revision again on the server. Activate pins the exact source and revision across restarts. Editing, moving, or replacing that source pauses the schedule when Agent detects the change; refresh, inspect, and explicitly activate again. Activate and Run now require a workspace-root prompt with no skill or retries and with forbid concurrency; event schedules cannot be activated here. The catalog reads direct Markdown files in `.agent/{workflow,workflows}` and `.neoism/{workflow,workflows}`; it omits installation sources, linked files, nested folders and direct operator workflows. Headless scene: `workflows` (fixture data).

Chat Markdown image previews (`![description](path)`) load local absolute paths, workspace-relative paths, `file://` URLs and inline image data URLs through Rune's image cache. File reads and decoding run off the UI thread; completed decodes invalidate memoized Markdown so placeholders become pictures. Click a picture to open the existing image viewer. This is native-local loading, not a filesystem-serving HTTP endpoint; remote-host paths and web image URLs are not fetched. Use `--scene timeline-inline-image` to check the surface headlessly.

Sent photos sit above and outside the text bubble. Multiple photos form a fanned stack; horizontal trackpad swipes (or Shift+wheel) cycle every photo without scrolling the transcript sideways. Click the stack to open the selected photo full-window, then use horizontal swipes, Left/Right, or the viewer arrows to browse the set. Escape, the close button, or the backdrop dismisses the viewer. `--scene agent-attach` checks a single photo; set `NEOISM_RUNE_PHOTO_STACK=1` for four photos.

Chat attachments have a 20 MiB combined raw limit per message and must also fit the WebSocket encoding budget. The composer reports rejected files and keeps the draft if it exceeds the send budget. The picture picker lists PNG, JPEG, GIF, WebP, and BMP; export HEIC and other unsupported pictures as PNG or JPEG. Model providers may set lower limits or accept fewer formats.

Clicking a local image file in the file tree opens a read-only tab that fits the image to its pane without cropping. Reopening the file selects the existing tab. Use `--scene image-viewer` to check the file preview headlessly.

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

On macOS the same check is `check-live-pacing-macos.py` (no compositor needed; the window must
stay visible, since macOS presents nothing for a covered one, and the report says so when it
was). The trace adds `[rune-present]`, each drawable's Metal `presentedTime`, and the report
gives missed refreshes and what the FPS badge would have read. The `hand:<seconds>` step
scrolls as a hand on the trackpad does (`app/native/HumanScroll.swift`): real precise-scroll
`NSEvent`s with finger and momentum phases, runs of flicks, slow reading drags, reversals
and pauses, made on their own jittered clock and handed to the window's view, so they take
winit's and Rune's real input path and land between frames wherever they land.

```sh
python3 rune/scripts/check-live-pacing-macos.py --session <conversation id> --size 1728x1050 \
  --drive "ready;wait:2;mark:hand;hand:15;mark:live;live;wait:1;mark:livehand;hand:12:fight;mark:stream;stream:/path/reply.md:12;mark:end;quit"
# A real turn in a scratch project, scrolled by hand while the model edits files:
python3 rune/scripts/check-live-pacing-macos.py --cwd /path/to/scratch --timeout 220 \
  --drive "ready;wait:3;prompt:<a task that edits files and writes a long summary>;wait:4;mark:run;hand:150;mark:end;quit"
```

`NEOISM_RUNE_BURN_US=<n>` makes every poll take at least that long, to find how much work a
frame can hold before refreshes are missed (120 held to 6 ms on an M5 Pro at 1728x1050).
`RUNE_TRACE_TICKS=1` prints what each display-link tick was taken as, and `RUNE_TRACE_GPU=1`
when each frame was committed, ran on the GPU and reached the screen.

`NEOISM_RUNE_FRAME_TRACE=1` prints each frame's `work_us` (elapsed wall time from poll start to encode end) and whether the poll requested a view rebuild; `NEOISM_RUNE_TRACE_POLL=1` names those requests. Rune can also commit independently of the poll, for example when scrolling changes the virtual row window. `RUNE_COMMIT_BUDGET_MS=N` prints Rune's own breakdown of commits over `N` ms.

Scripted `touch` motion integrates its requested points per second over the actual elapsed time between polls. It does not quantize movement onto an independent 120 Hz event clock, which could introduce zero/double displacement steps into an otherwise display-paced capture. This diagnostic input does not change real trackpad event handling.

For native CPU versus wall-time measurements, build with `rune/app/build.sh build --release --features frame-profile`, run the same pacing checker, then run `python3 rune/scripts/analyze-frame-phases.py /path/to/capture.log`. This opt-in build adds `[rune-phases]` records for commit, layout, text, paint and apply work. Commit includes several nested phases, so do not sum them. Total `wall_ms` and `cpu_ms` both end at callback entry, after present; `work_us` ends earlier, at encode completion. Wall time minus thread CPU includes blocking and scheduler preemption; it is not a GPU timing measurement. Surface acquisition can occur before the poll and is not necessarily inside this interval. Normal builds leave phase profiling disabled.

The native checker requires this machine's Hyprland Lua dispatch API, `wtype`, and `/dev/uinput` access for
wheel input. It stops sending input if focus changes. `--natural` exercises normal redraw scheduling;
without it, `--frames` forces continuous submissions and is insufficient to verify redraw scheduling.
Measure only intervals with active scrolling or animation: idle gaps are deliberate, not missed frames.

See [VALIDATION.md](VALIDATION.md) for measured results and their limits. Step 8 remains in progress:
the screenshot suite exercises surfaces and input, but does not establish complete visual/behavioral parity.
Step 9 has not run; the installed app and release packaging still use GPUiX, which remains the reference.
