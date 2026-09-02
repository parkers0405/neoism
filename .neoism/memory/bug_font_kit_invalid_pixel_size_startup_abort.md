---
name: "font-kit Invalid_Pixel_Size startup abort"
description: "Linux startup FreeType assertion removed by eliminating font-kit face loads and preserving collection indexes"
type: "bug"
scope: "project"
origin: "2026-07-09 session"
created: "2026-08-17"
updated: "2026-08-17"
---

## Root cause

Linux desktop startup called `font_kit::Handle::load()` while selecting configured regular/style/symbol fonts. font-kit 0.14.3's FreeType loader asserts that `FT_Set_Char_Size` succeeds; bitmap/color fonts such as Noto Color Emoji can return FreeType error 0x17 (`FT_Err_Invalid_Pixel_Size`). Release builds use `panic=abort`, so this killed Neoism before window creation and cannot be caught with `catch_unwind`.

The same unsafe load existed in Linux emoji fallback, and `MemSource::from_fonts` eagerly loaded every font in `additional-dirs`.

## Fix

In `sugarloaf/src/font/loader.rs`, use font-kit only for fontconfig family/path discovery. Parse family, weight, width, style, and collection faces with `ttf-parser`; custom font directories now parse and isolate each face without constructing a font-kit FreeType font. In `sugarloaf/src/font/linux.rs`, emoji glyph coverage uses `ttf-parser::Face::glyph_index`. There should be no `handle.load()` in Sugarloaf.

`FontData::from_data` now accepts and preserves the selected TTC/OTC face index through Swash and name parsing instead of always rendering face 0.

## Verification

`cargo check -p sugarloaf` passes. Standalone `cargo test -p sugarloaf` is blocked by existing `neoism-window` feature wiring (neither x11 nor wayland enabled for that isolated test invocation), unrelated to this patch. Two loader regression unit tests cover valid metadata and malformed bytes.
