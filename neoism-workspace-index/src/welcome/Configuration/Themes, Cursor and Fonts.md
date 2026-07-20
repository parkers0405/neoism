# Themes, Cursor & Fonts

All set under `[neoism]`, `[cursor]`, and `[fonts]` in [[Configuration|config.json]].

## Themes

One `theme` colors everything — chrome, terminal, and the editor's syntax palette — so it all matches.

```toml
[neoism]
theme = "pastel_dark"
```

Built-in themes: `pastel_dark`, `nvchad_one`, `tokyo_night`, `catppuccin_mocha`. You can also switch live from the command palette (search "theme"); your pick is saved back to `config.json`.

## Cursor

```toml
[neoism]
cursor-color = "#5c9cf5"   # #RRGGBB / RRGGBB / #RGB — overrides the theme accent
cursor-style = "solid"     # "solid" or "rainbow"
blinking-cursor = true     # friendly alias for [cursor] blinking

[cursor]
shape = "block"            # block | underline | beam | hidden
blinking = false
blinking-interval = 530    # ms
```

- **`cursor-color`** overrides the theme's cursor accent everywhere — including the caret collaborators see.
- **`cursor-style = "rainbow"`** animates a full hue sweep and ignores `cursor-color`. In multiplayer, everyone's rainbow caret sweeps in phase.

## Fonts

```toml
[fonts]
family = "cascadiacode"
size = 14.0
# optional per-style overrides:
# regular = "..."   bold = "..."   italic = "..."   bold-italic = "..."
```

The default is Cascadia Code at 14pt. Set `line-height` at the top level to loosen or tighten line spacing.
