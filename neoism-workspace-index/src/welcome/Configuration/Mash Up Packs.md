# Mash Up Packs

A Mash Up Pack skins the **whole app as one look** — theme, shader,
fonts, scrollbars, markdown decorations, and icons applied together,
like a Minecraft mash-up pack. Open the picker from the command
palette: **Mash Up Packs**.

Two example packs are installed on first run and double as templates:

- **Phosphor** — green CRT phosphor with curved-glass shader.
- **Retro 95** — Windows-3.1 era light gray with the classic
  logo-color accents, chunky square scrollbars, pixel-style
  checkboxes, Windows-flag wordmark letters.

## Make your own pack, step by step

1. Create a folder under your config dir (`~/.config/neoism`):

   ```
   packs/my-look/
     pack.toml       # the manifest (required)
     theme.toml      # optional bundled IDE theme
     effect.glsl     # optional shader overlay
   ```

2. Write `pack.toml`. Every key is optional — set the slots you ship,
   the rest of the user's setup is left alone:

   ```toml
   [pack]
   name = "My Look"
   description = "One line for the picker"
   shader-overlay = "effect.glsl"     # or "builtin:ctv_round"
   wallpaper = "tile.png"             # image behind the whole window
   # wallpaper-opacity = 0.9          # alpha baked at load
   # theme = "tokyo_night"            # reference any theme by name
   # filters = ["preset.slangp"]      # RetroArch filter chains
   # font-family = "Px437 IBM VGA8"   # written into [fonts] on apply

   [scrollbar]
   width = 10
   square = true                      # chunky 90s bars (radius-factor = 0)
   min-thumb = 24
   thumb = "#8a8578"
   thumb-drag = "#5f5b52"
   track = "#cdc9bf"                  # unset = no track drawn

   [markdown]
   checkbox = "retro95"               # or "modern"
   # font-family = "Comic Neue"       # markdown surface only, terminal keeps its font

   [wordmark]
   # NEOISM wordmark letters (splash + agent home): one color = uniform
   # tint, several cycle per letter. Unset = the theme's fg.
   colors = ["#c23327", "#1e8e3e", "#2144c7", "#e8a90c"]

   [icons]
   folder = { color = "#000080" }
   "file.rs" = { glyph = "" }
   "status.branch" = { glyph = "" }
   ```

3. Optionally add `theme.toml`. Every color is optional — unset keys
   inherit from `extends`, so a minimal theme is just a few lines:

   ```toml
   name = "my_look"
   description = "Shown in the Theme Picker"
   extends = "pastel_dark"

   [colors]
   bg = "#030f06"
   fg = "#33ff66"
   accent = "#45ff7d"
   # full key list: surface, hover, border, muted, dim, folder,
   # red green yellow blue magenta cyan white black,
   # comment string number keyword statement func type
   # property constructor special
   ```

   The theme reaches *everything*: chrome, tabs, markdown render,
   agent pane, terminal ANSI colors, embedded nvim syntax — and a few
   derived surfaces follow automatically:

   - the **splash wordmark** is tinted to your theme's `fg`, and the
     splash menu rows use `fg`/`dim`/`green`;
   - floating panels (fuzzy finder, hover docs, modals, code blocks,
     the agent's Thinking card) use a derived *panel* color: on dark
     themes it's your `black` token, on **light themes** it flips to
     `white` so panels read as paper instead of black holes.

4. Shaders: any Shadertoy-style GLSL fragment (`mainImage`) works —
   see [[Shaders]]. `builtin:ctv_round` and `builtin:hypno_crt` ship
   in the box. Paths in `pack.toml` resolve relative to the pack dir.

5. Fonts: drop font files somewhere in `[fonts] additional-dirs` (or
   install them system-wide), then name the family in `font-family`.

6. Pick **Mash Up Packs** in the palette — your folder appears with
   its description and a summary of the slots it ships. Applying is
   live; no restart.

To share a pack, just share the folder — a pack is plain files, no
code. Drop a received pack into `packs/` and it shows up in the picker.

## Icon override keys

Each `[icons]` entry takes `glyph` (any string — e.g. a Nerd Font
char) and/or `color` (`"#RRGGBB"`); unset fields keep the built-in.

| Key | Where |
| --- | --- |
| `folder` | folder rows everywhere (tree, notes, breadcrumbs, finder) |
| `file` | default/unknown file-type icon |
| `file.<ext>` | per-extension, e.g. `file.rs`, `file.md` — wins over the built-in table |
| `workspace` | workspace root / Island tab icon |
| `tab.terminal` | terminal tab icon |
| `tab.new` | the "+" new-tab button (glyph only) |
| `status.mode`, `status.folder`, `status.branch`, `status.lines`, `status.lsp`, `status.split`, `status.error`, `status.warn`, `status.info`, `status.hint`, `status.file`, `status.terminal` | status bar pills; `error`/`warn`/`info`/`hint`/`lsp` also reach the diagnostics + LSP popups (glyph only) |
| `palette.neoism`, `palette.nvim`, `palette.markdown`, `palette.notebook`, `palette.draw`, `palette.neoism-agent`, `palette.lsp`, `palette.workspace` | command palette service icons — also the splash menu rows (glyph only) |
| `git.branch`, `git.close`, `git.check`, `git.chevron-down`, `git.chevron-right` | Git panel (Alt+G) chrome; its folder rows follow the global `folder` key (glyph only) |
| `note` | default markdown-note icon in the notes sidebar |

The terminal composer's completion menu follows `folder` and `file`
for its directory / plain-file rows too.

## Standalone themes

Drop a theme file (same format as `theme.toml`) into
`ide-themes/<name>.toml` and it appears in the Theme Picker — no pack
needed.

## Individual overrides

Applying a pack sets each slot; you can still change any slot
afterwards and your choice wins:

- **Theme** — Theme Picker (the pack never re-forces its theme on
  restart; your last pick is what persists).
- **Shader** — the Shaders picker.
- **Wallpaper** — set `[window] background-image` in config.toml and
  it beats the pack's wallpaper.
- **Scrollbars / markdown / icons** — `[look.*]` sections in
  `config.toml` override the pack field-by-field:

```toml
[look.scrollbar]
width = 8

[look.markdown]
checkbox = "modern"

[look.icons]
folder = { glyph = "", color = "#7ebae4" }
```

Deactivate a pack from the picker's **None** row — the theme stays,
the pack's shader and look slots clear.
