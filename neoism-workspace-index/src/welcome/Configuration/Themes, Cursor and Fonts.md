# Themes, Cursor & Fonts

Theming and typography live in the `appearance` block of [[Configuration|config.json]]; the caret you show collaborators lives in `presence`; the terminal's own cursor lives in `terminal`.

## Themes

One `theme` colors everything — chrome, terminal, and the editor's syntax palette — so it all matches.

```json
{ "appearance": { "theme": "pastel_dark" } }
```

Built-in themes: `pastel_dark`, `nvchad_one`, `tokyo_night`, `catppuccin_mocha`. You can also switch live from the command palette (search "theme"); your pick is saved back to `config.json`.

> `appearance.theme` is the IDE theme. A separate, optional `appearance.palette` key loads a terminal-only color file from `themes/<name>.json` — most people never need it.

## Cursor

Two different cursors: the caret **you** show collaborators (`presence`), and the **terminal's** block/beam cursor (`terminal.cursor`).

```json
{
  "presence": {
    "cursor-color": "#5c9cf5",   // #RRGGBB / RRGGBB / #RGB — overrides the theme accent
    "cursor-style": "solid"      // "solid" or "rainbow"
  },
  "terminal": {
    "cursor": {
      "shape": "block",          // block | underline | beam | hidden
      "blinking": false,
      "blinking-interval": 530   // ms
    }
  }
}
```

- **`presence.cursor-color`** overrides the theme's cursor accent everywhere — including the caret collaborators see.
- **`presence.cursor-style: "rainbow"`** animates a full hue sweep and ignores `cursor-color`. In multiplayer, everyone's rainbow caret sweeps in phase.

## Fonts

```json
{
  "appearance": {
    "fonts": {
      "family": "CascadiaCode",
      "size": 14.0,
      "weight": 400
      // optional per-style overrides:
      // "regular": {...}, "bold": {...}, "italic": {...}, "bold-italic": {...}
    },
    "line-height": 1.2
  }
}
```

The default is Cascadia Code at 14pt. `appearance.line-height` loosens or tightens line spacing; `weight` sets the base thickness (400 = normal, 700 = bold).
