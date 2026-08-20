# Neoism Fonts

Open this file in the Neoism **Markdown editor** to see the decorative fonts rendered.

## Decorative / medieval fonts

These are the "illuminated" fonts. Use them in Markdown for drop-caps and decorative
first-letters via:

```
::illuminate[X]{style=NAME size=N}
```

`X` is the letter, `size=N` is how many lines tall the drop-cap is, and `NAME` is one of:
`fraktur`, `maguntia`, `manuscript`, `cinzel`, `cinzelblack`, `pirata`, `medieval`.

### Fraktur — heavy blackletter (UnifrakturCook)
::illuminate[F]{style=fraktur size=3} raktur — the quick brown fox jumps over the lazy dog.

### Maguntia — blackletter (UnifrakturMaguntia)
::illuminate[M]{style=maguntia size=3} aguntia — the quick brown fox jumps over the lazy dog.

### Cinzel — elegant engraved caps (CinzelDecorative Bold)
::illuminate[C]{style=cinzel size=3} INZEL — THE QUICK BROWN FOX JUMPS OVER THE LAZY DOG.

### Cinzel Black — heavier engraved caps (CinzelDecorative Black)
::illuminate[C]{style=cinzelblack size=3} INZEL BLACK — THE QUICK BROWN FOX.

### Pirata One — gothic display
::illuminate[P]{style=pirata size=3} irata — the quick brown fox jumps over the lazy dog.

### MedievalSharp — readable medieval  *(this is the one on the sidebar drop-caps)*
::illuminate[M]{style=medieval size=3} edievalSharp — the quick brown fox jumps over the lazy dog.

## UI & editor fonts

These are set in `~/.config/neoism/config.toml` under `[fonts]` (`family` + `size`), not the
`::illuminate` syntax:

- **Cascadia Code** (`cascadiacode`) — the default UI + terminal + editor font.
- **Geist Mono** — bundled monospace (used on the web build).
- **Symbols Nerd Font** — provides the icon glyphs (folder/file/branch icons, etc.).
- **Noto Sans Symbols 2** (SIL Open Font License 1.1) — bundled on the web build for
  geometric shapes, box/block elements and task checkboxes (U+2610 / U+2611). The
  desktop reaches these through system fontconfig fallback; the browser has no such
  fallback, so they rendered as tofu until this was embedded.
- **Twemoji Mozilla** (CC-BY 4.0, Twitter/Mozilla) — bundled on the web build for
  colour emoji: Markdown page icons (`icon:` frontmatter), emoji in notes and agent
  text. COLR (vector colour) rather than a CBDT bitmap font, so it costs ~1.4 MB
  instead of ~10 MB and rasterises through swash's existing colour-glyph path.

> Tip: change the editor/terminal font with:
> ```toml
> [fonts]
> family = "cascadiacode"
> size = 14.0
> ```
