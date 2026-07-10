
use super::*;
use neoism_terminal_core::colors::{AnsiColor, ColorRgb, NamedColor};
use neoism_terminal_core::crosswords::grid::row::Row;

use neoism_terminal_core::crosswords::square::{
    CellFlags as SquareCellFlags, ContentTag, Square, Wide,
};
use neoism_terminal_core::crosswords::style::StyleFlags;
use neoism_ui::primitives::IdeTheme;
use sugarloaf::font::{FontData, FontLibrary, FontLibraryData, SymbolMap};
use sugarloaf::layout::RootStyle;
use sugarloaf::text::DrawOpts;
use sugarloaf::{Color, Sugarloaf, SugarloafRenderer, SugarloafWindowSize};

// Bundled fonts. We ship the same Geist Mono faces the desktop
// frontend's HTML chrome uses plus Symbols Nerd Font Mono for PUA
// icon glyphs, so cell metrics on web match desktop pixel-for-pixel.
// Bytes live in `.rodata` via `include_bytes!`; `FontData::
// from_static_slice` registers them without copying.
const FONT_GEIST_MONO_REGULAR: &[u8] =
    include_bytes!("../../assets/fonts/GeistMono-Regular.otf");
const FONT_GEIST_MONO_BOLD: &[u8] =
    include_bytes!("../../assets/fonts/GeistMono-Bold.otf");
const FONT_GEIST_MONO_ITALIC: &[u8] =
    include_bytes!("../../assets/fonts/GeistMono-Italic.otf");
const FONT_GEIST_MONO_BOLD_ITALIC: &[u8] =
    include_bytes!("../../assets/fonts/GeistMono-BoldItalic.otf");
const FONT_SYMBOLS_NERD_FONT_MONO: &[u8] =
    include_bytes!("../../assets/fonts/SymbolsNerdFontMono-Regular.ttf");

/// Convert an 8-bit RGB triple from the snapshot's theme into the
/// `[f32; 4]` color Sugarloaf geometry APIs expect (alpha = 1.0). The
/// canonical native renderer normalises by 255.0; we do the same.
#[inline]
fn rgb_to_f32(c: RgbTriple) -> [f32; 4] {
    [
        c.r as f32 / 255.0,
        c.g as f32 / 255.0,
        c.b as f32 / 255.0,
        1.0,
    ]
}

#[inline]
fn rgb_to_u8(c: RgbTriple) -> [u8; 4] {
    [c.r, c.g, c.b, 255]
}

#[inline]
fn dim_rgb(c: RgbTriple) -> RgbTriple {
    RgbTriple {
        r: ((c.r as f32) * 0.62) as u8,
        g: ((c.g as f32) * 0.62) as u8,
        b: ((c.b as f32) * 0.62) as u8,
    }
}

#[inline]
fn resolve_snapshot_color(
    color: ColorIndex,
    theme: &ThemeSnapshot,
    default: RgbTriple,
) -> RgbTriple {
    match color {
        ColorIndex::Default => default,
        ColorIndex::Named(ix) | ColorIndex::Indexed(ix) => {
            theme.palette.get(ix as usize).copied().unwrap_or(default)
        }
        ColorIndex::Spec { r, g, b } => RgbTriple { r, g, b },
    }
}

#[inline]
fn cell_snapshot_colors(
    cell: &CellSnapshot,
    theme: &ThemeSnapshot,
) -> (RgbTriple, RgbTriple) {
    let mut fg = resolve_snapshot_color(cell.fg, theme, theme.default_fg);
    let mut bg = resolve_snapshot_color(cell.bg, theme, theme.default_bg);
    if cell.flags.contains(CellFlags::REVERSE) {
        std::mem::swap(&mut fg, &mut bg);
    }
    if cell.flags.contains(CellFlags::DIM) {
        fg = dim_rgb(fg);
    }
    if cell.flags.contains(CellFlags::HIDDEN) {
        fg = bg;
    }
    (fg, bg)
}

#[inline]
fn ansi_to_snapshot_color(color: AnsiColor) -> ColorIndex {
    match color {
        AnsiColor::Named(NamedColor::Foreground)
        | AnsiColor::Named(NamedColor::Background) => ColorIndex::Default,
        AnsiColor::Named(name) => ColorIndex::Named(name as u8),
        AnsiColor::Indexed(index) => ColorIndex::Indexed(index),
        AnsiColor::Spec(rgb) => ColorIndex::Spec {
            r: rgb.r,
            g: rgb.g,
            b: rgb.b,
        },
    }
}

#[inline]
fn square_to_cell_snapshot(terminal: &Crosswords, square: &Square) -> CellSnapshot {
    match square.content_tag() {
        ContentTag::BgPalette => CellSnapshot {
            c: ' ',
            fg: ColorIndex::Default,
            bg: ColorIndex::Indexed(square.bg_palette_index()),
            flags: CellFlags::empty(),
            underline_color: None,
            hyperlink_id: None,
        },
        ContentTag::BgRgb => {
            let (r, g, b) = square.bg_rgb();
            CellSnapshot {
                c: ' ',
                fg: ColorIndex::Default,
                bg: ColorIndex::Spec { r, g, b },
                flags: CellFlags::empty(),
                underline_color: None,
                hyperlink_id: None,
            }
        }
        ContentTag::Codepoint => {
            let style = terminal.grid.style_of(square);
            let mut flags = CellFlags::empty();
            if style.flags.contains(StyleFlags::BOLD) {
                flags.insert(CellFlags::BOLD);
            }
            if style.flags.contains(StyleFlags::ITALIC) {
                flags.insert(CellFlags::ITALIC);
            }
            if style.flags.contains(StyleFlags::DIM) {
                flags.insert(CellFlags::DIM);
            }
            if style.flags.contains(StyleFlags::HIDDEN) {
                flags.insert(CellFlags::HIDDEN);
            }
            if style.flags.contains(StyleFlags::STRIKEOUT) {
                flags.insert(CellFlags::STRIKEOUT);
            }
            if style.flags.contains(StyleFlags::INVERSE) {
                flags.insert(CellFlags::REVERSE);
            }
            if style.flags.contains(StyleFlags::UNDERLINE) {
                flags.insert(CellFlags::UNDERLINE);
            }
            if style.flags.contains(StyleFlags::DOUBLE_UNDERLINE) {
                flags.insert(CellFlags::DOUBLE_UNDERLINE);
            }
            if style.flags.contains(StyleFlags::UNDERCURL) {
                flags.insert(CellFlags::UNDERCURL);
            }
            if style.flags.contains(StyleFlags::DOTTED_UNDERLINE) {
                flags.insert(CellFlags::DOTTED_UNDERLINE);
            }
            if style.flags.contains(StyleFlags::DASHED_UNDERLINE) {
                flags.insert(CellFlags::DASHED_UNDERLINE);
            }

            let sq_flags = square.cell_flags();
            if sq_flags.contains(SquareCellFlags::WRAPLINE) {
                flags.insert(CellFlags::WRAPLINE);
            }
            match square.wide() {
                Wide::Wide => flags.insert(CellFlags::WIDE_CHAR),
                Wide::Spacer | Wide::LeadingSpacer => {
                    flags.insert(CellFlags::WIDE_CHAR_SPACER)
                }
                Wide::Narrow => {}
            }

            CellSnapshot {
                c: square.c(),
                fg: ansi_to_snapshot_color(style.fg),
                bg: ansi_to_snapshot_color(style.bg),
                flags,
                underline_color: style.underline_color.map(ansi_to_snapshot_color),
                hyperlink_id: None,
            }
        }
    }
}

/// Convert an 8-bit RGB triple into a `sugarloaf::Color` (f64, 0..1).
/// Used for the framebuffer clear color via `set_background_color`.
#[inline]
fn rgb_to_sugar_color(c: RgbTriple) -> Color {
    Color {
        r: c.r as f64 / 255.0,
        g: c.g as f64 / 255.0,
        b: c.b as f64 / 255.0,
        a: 1.0,
    }
}

#[inline]
fn rgb_from_u32(value: u32) -> ColorRgb {
    ColorRgb {
        r: ((value >> 16) & 0xff) as u8,
        g: ((value >> 8) & 0xff) as u8,
        b: (value & 0xff) as u8,
    }
}

fn seed_terminal_theme(terminal: &mut Terminal, theme: &IdeTheme) {
    let colors = &mut terminal.inner.colors;
    let mut set = |slot: NamedColor, value: u32| {
        colors[slot] = Some(rgb_from_u32(value).to_arr());
    };
    set(NamedColor::Black, theme.black);
    set(NamedColor::Red, theme.red);
    set(NamedColor::Green, theme.green);
    set(NamedColor::Yellow, theme.yellow);
    set(NamedColor::Blue, theme.blue);
    set(NamedColor::Magenta, theme.magenta);
    set(NamedColor::Cyan, theme.cyan);
    set(NamedColor::White, theme.white);
    set(NamedColor::LightBlack, theme.muted);
    set(NamedColor::LightRed, theme.red);
    set(NamedColor::LightGreen, theme.green);
    set(NamedColor::LightYellow, theme.yellow);
    set(NamedColor::LightBlue, theme.blue);
    set(NamedColor::LightMagenta, theme.magenta);
    set(NamedColor::LightCyan, theme.cyan);
    set(NamedColor::LightWhite, theme.fg);
    set(NamedColor::Foreground, theme.fg);
    set(NamedColor::Background, theme.bg);
    set(NamedColor::Cursor, theme.accent);
    set(NamedColor::LightForeground, theme.fg);
    set(NamedColor::DimForeground, theme.dim);

    let mut set_dim = |slot: NamedColor, value: u32| {
        colors[slot] = Some(rgb_from_u32(value).to_arr_with_dim());
    };
    set_dim(NamedColor::DimBlack, theme.black);
    set_dim(NamedColor::DimRed, theme.red);
    set_dim(NamedColor::DimGreen, theme.green);
    set_dim(NamedColor::DimYellow, theme.yellow);
    set_dim(NamedColor::DimBlue, theme.blue);
    set_dim(NamedColor::DimMagenta, theme.magenta);
    set_dim(NamedColor::DimCyan, theme.cyan);
    set_dim(NamedColor::DimWhite, theme.white);
}

/// A canvas-bound terminal: Crosswords + the sugarloaf instance
/// that paints its viewport.
#[wasm_bindgen]
pub struct RenderedTerminal {
    terminal: Terminal,
    sugarloaf: Option<Sugarloaf<'static>>,
    cell_w: f32,
    cell_h: f32,
    // `font_library` is owned by the sugarloaf state internally
    // (it's cloned via `Arc<RwLock<_>>`), but we keep our handle so
    // future passes can hand the same library to follow-up state
    // (e.g. measuring glyph advances for cursor sizing).
    font_library: FontLibrary,
    // Active IdeTheme. Drives both the terminal's seeded color
    // palette (via `seed_terminal_theme`) and sugarloaf's swapchain
    // clear color (via `set_background_color`). Mutated by
    // `set_ide_theme`; defaults to `pastel_dark`.
    ide_theme: IdeTheme,
}

#[wasm_bindgen]
impl RenderedTerminal {
    /// Async constructor. JS:
    /// `const t = await RenderedTerminal.new(canvas, cols, rows, scale);`
    ///
    /// Builds the data-side `Terminal`, then awaits
    /// `Sugarloaf::from_canvas`. On surface-creation failure
    /// returns `Err` with the sugarloaf error message — JS sees a
    /// rejected promise carrying the reason.
    #[wasm_bindgen(js_name = "new")]
    pub async fn new(
        canvas: web_sys::HtmlCanvasElement,
        cols: u32,
        rows: u32,
        scale: f32,
    ) -> Result<RenderedTerminal, JsValue> {
        let mut terminal = Terminal::new(cols, rows);
        let initial_ide_theme = IdeTheme::default();
        seed_terminal_theme(&mut terminal, &initial_ide_theme);

        let size = SugarloafWindowSize {
            width: canvas.width() as f32,
            height: canvas.height() as f32,
        };

        // Build the font library from bundled Geist Mono + Symbols
        // Nerd Font Mono bytes. `FontLibrary::default()` would auto-
        // load only the sugarloaf-bundled CascadiaCodeNF — which
        // doesn't match the desktop frontend's primary font, so cell
        // metrics (glyph advance, x-height, line-height) would drift
        // from native. Registering Geist Mono Regular/Bold/Italic/
        // BoldItalic as the first four entries makes font_id=0 the
        // primary face, the same way the non-wasm `load()` path does.
        //
        // We append the Symbols Nerd Font Mono fallback last and
        // wire up the PUA symbol_maps that the non-wasm load() also
        // installs (U+E000..U+F900, U+F0000..U+FFFFE,
        // U+100000..U+10FFFE) so Nerd Font icon glyphs in the
        // composer / tabs / status line route to the right face.
        //
        // SugarloafRenderer::default() picks WebGL on wasm32 (see
        // SugarloafRenderer impl); RootStyle::default() is 14px /
        // 1.0 line-height. Subsequent tuning lives in `resize`.
        let font_library = FontLibrary::default();
        {
            let mut lib = font_library.inner.write();
            // Replace the default CascadiaCodeNF entry sitting at
            // font_id=0 with Geist Mono Regular by resetting the
            // whole `FontLibraryData` first — its private fields
            // (postscript_to_id, primary_metrics_cache) need to be
            // reset alongside `inner` and we can't touch those from
            // outside the sugarloaf crate. `Default` rebuilds an
            // empty registry that we then populate face by face;
            // `FontLibraryData::insert` indexes by `inner.len()`,
            // so order = font_id.
            *lib = FontLibraryData::default();

            // Force-enable font hinting. `FontLibraryData::default()`
            // already sets `hinting: true`, but be explicit so a
            // future refactor of sugarloaf's default doesn't silently
            // turn it off — hinting is what snaps glyphs to the
            // pixel grid and keeps them crisp under high-DPR
            // rasterization. (Sugarloaf currently exposes the toggle
            // as a plain `bool` field; if a richer
            // `with_text_aa_hinting`-style builder API is added
            // later, route through that instead.)
            // TODO(font-aa): wire a subpixel-AA toggle through here
            // once sugarloaf grows one.
            lib.hinting = true;

            lib.insert(FontData::from_static_slice(FONT_GEIST_MONO_REGULAR).unwrap());
            lib.insert(FontData::from_static_slice(FONT_GEIST_MONO_ITALIC).unwrap());
            lib.insert(FontData::from_static_slice(FONT_GEIST_MONO_BOLD).unwrap());
            lib.insert(FontData::from_static_slice(FONT_GEIST_MONO_BOLD_ITALIC).unwrap());

            let nerd_symbols_id = lib.inner.len();
            lib.insert(FontData::from_static_slice(FONT_SYMBOLS_NERD_FONT_MONO).unwrap());
            lib.symbol_maps = Some(vec![
                SymbolMap {
                    font_index: nerd_symbols_id,
                    range: '\u{E000}'..'\u{F900}',
                },
                SymbolMap {
                    font_index: nerd_symbols_id,
                    range: '\u{F0000}'..'\u{FFFFE}',
                },
                SymbolMap {
                    font_index: nerd_symbols_id,
                    range: '\u{100000}'..'\u{10FFFE}',
                },
            ]);
        }
        let renderer = SugarloafRenderer::default();
        let layout = RootStyle::new(scale, 14.0, 1.0);

        let mut sugarloaf =
            Sugarloaf::from_canvas(canvas, size, scale, renderer, &font_library, layout)
                .await
                .map_err(|e| JsValue::from_str(&format!("{e:?}")))?;

        // Seed sugarloaf's clear color from the IdeTheme so the
        // swapchain background matches the chrome/terminal palette
        // on the very first paint — before any `draw_cells` call
        // would otherwise have set it.
        sugarloaf
            .set_background_color(Some(initial_ide_theme.sugar(initial_ide_theme.bg)));

        Ok(RenderedTerminal {
            terminal,
            sugarloaf: Some(sugarloaf),
            cell_w: 8.0,
            cell_h: 16.0,
            font_library,
            ide_theme: initial_ide_theme,
        })
    }

    /// Feed PTY-emitted bytes through the parser into Crosswords.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.terminal.feed(bytes);
    }

    /// Resize both the terminal grid and the sugarloaf surface.
    /// `scale` is the device pixel ratio (window.devicePixelRatio).
    pub fn resize(&mut self, cols: u32, rows: u32, scale: f32) {
        let width_px = (cols as f32 * 8.0).max(1.0) as u32;
        let height_px = (rows as f32 * 16.0).max(1.0) as u32;
        self.set_cell_metrics(8.0, 16.0);
        self.resize_grid_and_surface(cols, rows, scale, width_px, height_px);
    }

    /// Render one frame. Reads `Crosswords::snapshot()` and walks
    /// the viewport, emitting one styled run per row through
    /// `sugarloaf.content().sel(id)`. This is the simplified first
    /// pass the task brief calls out: per-cell color/attr runs are
    /// a follow-up; today we emit one default-fg/default-bg span
    /// per row, plus a cursor rectangle.
    ///
    /// Call sequence (mirrors `sugarloaf/examples/text.rs`):
    ///   set_background_color → text(Some(id)) → content().sel(id)
    ///   .clear().add_span(row0).new_line().add_span(row1).…
    ///   .build() → set_position → rect(cursor) → render().
    pub fn render(&mut self) {
        self.draw_cells();
        self.present();
    }

    /// Emit terminal cell draw commands into the owned `sugarloaf`
    /// **without** presenting. Use this when something else (e.g.
    /// `ChromeBridge`) wants to queue additional draws over the
    /// terminal before a single `present()` flips the swapchain.
    pub fn draw_cells(&mut self) {
        self.draw_cells_at(0.0, 0.0);
    }

    pub(crate) fn draw_cells_at(&mut self, origin_x: f32, origin_y: f32) {
        self.draw_cells_in(origin_x, origin_y, None, false, false);
    }

    pub(crate) fn draw_cells_in(
        &mut self,
        origin_x: f32,
        origin_y: f32,
        clip_rect: Option<[f32; 4]>,
        suppress_prompt_row: bool,
        suppress_cursor: bool,
    ) {
        let snap = self.terminal.inner.snapshot();
        let Some(s) = self.sugarloaf.as_mut() else {
            return;
        };

        // Framebuffer clear.
        s.set_background_color(Some(rgb_to_sugar_color(snap.theme.default_bg)));

        let font_size = (self.cell_h * 0.875).clamp(8.0, 32.0);
        let text_y_pad = ((self.cell_h - font_size) * 0.5).max(0.0);
        let prompt_row = suppress_prompt_row.then_some(snap.cursor.row as usize);
        for (row_idx, row) in snap.viewport.iter().enumerate() {
            if prompt_row == Some(row_idx) {
                continue;
            }

            let row_y = origin_y + row_idx as f32 * self.cell_h;
            let mut bg_start: Option<(usize, RgbTriple)> = None;
            for (col_idx, cell) in row.iter().enumerate() {
                let (_, bg) = cell_snapshot_colors(cell, &snap.theme);
                let paints_bg = bg != snap.theme.default_bg;
                match (bg_start, paints_bg) {
                    (None, true) => bg_start = Some((col_idx, bg)),
                    (Some((start, active_bg)), true) if active_bg != bg => {
                        s.rect(
                            None,
                            origin_x + start as f32 * self.cell_w,
                            row_y,
                            (col_idx - start) as f32 * self.cell_w,
                            self.cell_h,
                            rgb_to_f32(active_bg),
                            0.0,
                            0,
                        );
                        bg_start = Some((col_idx, bg));
                    }
                    (Some((start, active_bg)), false) => {
                        s.rect(
                            None,
                            origin_x + start as f32 * self.cell_w,
                            row_y,
                            (col_idx - start) as f32 * self.cell_w,
                            self.cell_h,
                            rgb_to_f32(active_bg),
                            0.0,
                            0,
                        );
                        bg_start = None;
                    }
                    _ => {}
                }
            }
            if let Some((start, active_bg)) = bg_start {
                s.rect(
                    None,
                    origin_x + start as f32 * self.cell_w,
                    row_y,
                    (row.len().saturating_sub(start)) as f32 * self.cell_w,
                    self.cell_h,
                    rgb_to_f32(active_bg),
                    0.0,
                    0,
                );
            }

            let mut text_run = String::new();
            let mut run_start = 0usize;
            let mut run_style: Option<(RgbTriple, bool, bool)> = None;
            let flush_text_run =
                |s: &mut Sugarloaf<'static>,
                 text_run: &mut String,
                 run_start: usize,
                 run_style: Option<(RgbTriple, bool, bool)>| {
                    if text_run.is_empty() {
                        return;
                    }
                    let Some((fg, bold, italic)) = run_style else {
                        text_run.clear();
                        return;
                    };
                    let opts = DrawOpts {
                        font_size,
                        color: rgb_to_u8(fg),
                        bold,
                        italic,
                        clip_rect,
                        ..DrawOpts::default()
                    };
                    s.text_mut().draw(
                        origin_x + run_start as f32 * self.cell_w,
                        row_y + text_y_pad,
                        text_run,
                        &opts,
                    );
                    text_run.clear();
                };

            for (col_idx, cell) in row.iter().enumerate() {
                if cell.flags.contains(CellFlags::WIDE_CHAR_SPACER) || cell.c == ' ' {
                    flush_text_run(s, &mut text_run, run_start, run_style);
                    run_style = None;
                    continue;
                }
                let (fg, _) = cell_snapshot_colors(cell, &snap.theme);
                let bold = cell.flags.contains(CellFlags::BOLD);
                let italic = cell.flags.contains(CellFlags::ITALIC);
                let style = (fg, bold, italic);
                if run_style != Some(style) {
                    flush_text_run(s, &mut text_run, run_start, run_style);
                    run_start = col_idx;
                    run_style = Some(style);
                }
                text_run.push(cell.c);
            }
            flush_text_run(s, &mut text_run, run_start, run_style);

            for (col_idx, cell) in row.iter().enumerate() {
                let underline = cell.flags.intersects(
                    CellFlags::UNDERLINE
                        | CellFlags::UNDERCURL
                        | CellFlags::DOUBLE_UNDERLINE
                        | CellFlags::DOTTED_UNDERLINE
                        | CellFlags::DASHED_UNDERLINE,
                );
                if !underline {
                    continue;
                }
                let (fg, _) = cell_snapshot_colors(cell, &snap.theme);
                let underline_color = cell
                    .underline_color
                    .map(|color| resolve_snapshot_color(color, &snap.theme, fg))
                    .unwrap_or(fg);
                s.rect(
                    None,
                    origin_x + col_idx as f32 * self.cell_w,
                    row_y + self.cell_h - 2.0,
                    self.cell_w,
                    1.0,
                    rgb_to_f32(underline_color),
                    0.02,
                    1,
                );
            }
        }

        if snap.cursor.visible && !suppress_cursor {
            let x = origin_x + snap.cursor.col as f32 * self.cell_w;
            let y = origin_y + snap.cursor.row as f32 * self.cell_h;
            let cur = rgb_to_f32(snap.theme.cursor);
            let (w, h, y) = match snap.cursor.shape {
                SnapshotCursorShape::Beam => {
                    (2.0_f32.max(self.cell_w * 0.14), self.cell_h, y)
                }
                SnapshotCursorShape::Underline => (
                    self.cell_w,
                    2.0_f32.max(self.cell_h * 0.12),
                    y + self.cell_h - 2.0,
                ),
                SnapshotCursorShape::Hidden => (0.0, 0.0, y),
                SnapshotCursorShape::Block => (self.cell_w, self.cell_h, y),
            };
            if w > 0.0 && h > 0.0 {
                s.rect(None, x, y, w, h, cur, 0.04, 2);
            }
        }
    }

    pub(crate) fn draw_composed_rows_in(
        &mut self,
        rows: &[Row<Square>],
        source_row_indices: &[Option<usize>],
        origin_x: f32,
        origin_y: f32,
        clip_rect: Option<[f32; 4]>,
        suppress_prompt_row: bool,
        suppress_cursor: bool,
    ) {
        let snap = self.terminal.inner.snapshot();
        let cursor_abs = self
            .terminal
            .inner
            .absolute_row_for_line(self.terminal.inner.grid.cursor.pos.row);
        let cursor_display_row = source_row_indices
            .iter()
            .position(|source| *source == Some(cursor_abs));
        let prompt_row = suppress_prompt_row.then_some(cursor_display_row).flatten();
        let cell_rows: Vec<Vec<CellSnapshot>> = rows
            .iter()
            .map(|row| {
                row.inner
                    .iter()
                    .map(|square| square_to_cell_snapshot(&self.terminal.inner, square))
                    .collect()
            })
            .collect();

        self.draw_cell_snapshot_rows_in(
            &cell_rows,
            &snap.theme,
            cursor_display_row.map(|row| CursorSnapshot {
                row: row as u16,
                ..snap.cursor
            }),
            origin_x,
            origin_y,
            clip_rect,
            prompt_row,
            suppress_cursor,
        );
    }

    fn draw_cell_snapshot_rows_in(
        &mut self,
        rows: &[Vec<CellSnapshot>],
        theme: &ThemeSnapshot,
        cursor: Option<CursorSnapshot>,
        origin_x: f32,
        origin_y: f32,
        clip_rect: Option<[f32; 4]>,
        suppress_row: Option<usize>,
        suppress_cursor: bool,
    ) {
        let Some(s) = self.sugarloaf.as_mut() else {
            return;
        };

        s.set_background_color(Some(rgb_to_sugar_color(theme.default_bg)));

        let font_size = (self.cell_h * 0.875).clamp(8.0, 32.0);
        let text_y_pad = ((self.cell_h - font_size) * 0.5).max(0.0);
        for (row_idx, row) in rows.iter().enumerate() {
            if suppress_row == Some(row_idx) {
                continue;
            }

            let row_y = origin_y + row_idx as f32 * self.cell_h;
            let mut bg_start: Option<(usize, RgbTriple)> = None;
            for (col_idx, cell) in row.iter().enumerate() {
                let (_, bg) = cell_snapshot_colors(cell, theme);
                let paints_bg = bg != theme.default_bg;
                match (bg_start, paints_bg) {
                    (None, true) => bg_start = Some((col_idx, bg)),
                    (Some((start, active_bg)), true) if active_bg != bg => {
                        s.rect(
                            None,
                            origin_x + start as f32 * self.cell_w,
                            row_y,
                            (col_idx - start) as f32 * self.cell_w,
                            self.cell_h,
                            rgb_to_f32(active_bg),
                            0.0,
                            0,
                        );
                        bg_start = Some((col_idx, bg));
                    }
                    (Some((start, active_bg)), false) => {
                        s.rect(
                            None,
                            origin_x + start as f32 * self.cell_w,
                            row_y,
                            (col_idx - start) as f32 * self.cell_w,
                            self.cell_h,
                            rgb_to_f32(active_bg),
                            0.0,
                            0,
                        );
                        bg_start = None;
                    }
                    _ => {}
                }
            }
            if let Some((start, active_bg)) = bg_start {
                s.rect(
                    None,
                    origin_x + start as f32 * self.cell_w,
                    row_y,
                    (row.len().saturating_sub(start)) as f32 * self.cell_w,
                    self.cell_h,
                    rgb_to_f32(active_bg),
                    0.0,
                    0,
                );
            }

            let mut text_run = String::new();
            let mut run_start = 0usize;
            let mut run_style: Option<(RgbTriple, bool, bool)> = None;
            let flush_text_run =
                |s: &mut Sugarloaf<'static>,
                 text_run: &mut String,
                 run_start: usize,
                 run_style: Option<(RgbTriple, bool, bool)>| {
                    if text_run.is_empty() {
                        return;
                    }
                    let Some((fg, bold, italic)) = run_style else {
                        text_run.clear();
                        return;
                    };
                    let opts = DrawOpts {
                        font_size,
                        color: rgb_to_u8(fg),
                        bold,
                        italic,
                        clip_rect,
                        ..DrawOpts::default()
                    };
                    s.text_mut().draw(
                        origin_x + run_start as f32 * self.cell_w,
                        row_y + text_y_pad,
                        text_run,
                        &opts,
                    );
                    text_run.clear();
                };

            for (col_idx, cell) in row.iter().enumerate() {
                if cell.flags.contains(CellFlags::WIDE_CHAR_SPACER) || cell.c == ' ' {
                    flush_text_run(s, &mut text_run, run_start, run_style);
                    run_style = None;
                    continue;
                }
                let (fg, _) = cell_snapshot_colors(cell, theme);
                let bold = cell.flags.contains(CellFlags::BOLD);
                let italic = cell.flags.contains(CellFlags::ITALIC);
                let style = (fg, bold, italic);
                if run_style != Some(style) {
                    flush_text_run(s, &mut text_run, run_start, run_style);
                    run_start = col_idx;
                    run_style = Some(style);
                }
                text_run.push(cell.c);
            }
            flush_text_run(s, &mut text_run, run_start, run_style);

            for (col_idx, cell) in row.iter().enumerate() {
                let underline = cell.flags.intersects(
                    CellFlags::UNDERLINE
                        | CellFlags::UNDERCURL
                        | CellFlags::DOUBLE_UNDERLINE
                        | CellFlags::DOTTED_UNDERLINE
                        | CellFlags::DASHED_UNDERLINE,
                );
                if !underline {
                    continue;
                }
                let (fg, _) = cell_snapshot_colors(cell, theme);
                let underline_color = cell
                    .underline_color
                    .map(|color| resolve_snapshot_color(color, theme, fg))
                    .unwrap_or(fg);
                s.rect(
                    None,
                    origin_x + col_idx as f32 * self.cell_w,
                    row_y + self.cell_h - 2.0,
                    self.cell_w,
                    1.0,
                    rgb_to_f32(underline_color),
                    0.02,
                    1,
                );
            }
        }

        if let Some(cursor) = cursor.filter(|cursor| cursor.visible) {
            if !suppress_cursor {
                let x = origin_x + cursor.col as f32 * self.cell_w;
                let y = origin_y + cursor.row as f32 * self.cell_h;
                let cur = rgb_to_f32(theme.cursor);
                let (w, h, y) = match cursor.shape {
                    SnapshotCursorShape::Beam => {
                        (2.0_f32.max(self.cell_w * 0.14), self.cell_h, y)
                    }
                    SnapshotCursorShape::Underline => (
                        self.cell_w,
                        2.0_f32.max(self.cell_h * 0.12),
                        y + self.cell_h - 2.0,
                    ),
                    SnapshotCursorShape::Hidden => (0.0, 0.0, y),
                    SnapshotCursorShape::Block => (self.cell_w, self.cell_h, y),
                };
                if w > 0.0 && h > 0.0 {
                    s.rect(None, x, y, w, h, cur, 0.04, 2);
                }
            }
        }
    }

    /// Present the queued draws (terminal cells + any chrome).
    /// Cheap if there is nothing to flush. Split out from
    /// `render()` so an orchestrator like `ChromeBridge` can
    /// coalesce multiple `draw_*()` passes into a single swapchain
    /// flip.
    pub fn present(&mut self) {
        if let Some(s) = self.sugarloaf.as_mut() {
            s.render();
        }
    }

    /// Drain PTY responses (DSR / OSC) so JS can write them back.
    /// Same semantics as `Terminal::take_pty_writes`.
    pub fn take_pty_writes(&mut self) -> Vec<u8> {
        self.terminal.take_pty_writes()
    }

    /// Drain non-PTY effects as JSON. Mirrors `Terminal::drain_effects_json`.
    pub fn drain_effects_json(&mut self) -> JsValue {
        self.terminal.drain_effects_json()
    }

    /// Same caveat as `Terminal::snapshot`: serialized through
    /// `serde-wasm-bindgen`, cost is proportional to grid size.
    /// Most JS hosts won't need this — the renderer reads it
    /// directly inside `render()` — but it's here so tools can
    /// introspect state.
    pub fn snapshot(&self) -> JsValue {
        self.terminal.snapshot()
    }
}

// Silence the dead-code lint on `font_library` until a follow-up
// pass uses it (the field is here on purpose — see comment above).
impl RenderedTerminal {
    #[allow(dead_code)]
    fn font_library(&self) -> &FontLibrary {
        &self.font_library
    }

    /// Mutable access to the owned `sugarloaf`. Used by
    /// `ChromeBridge` so the chrome panels paint into the *same*
    /// sugarloaf instance the terminal cells were emitted into —
    /// otherwise the two would race to set up the swapchain.
    pub(crate) fn sugarloaf_mut(&mut self) -> Option<&mut Sugarloaf<'static>> {
        self.sugarloaf.as_mut()
    }

    /// Read access to the inner `Terminal` so the bridge can feed
    /// PTY output and drain PTY writes without reaching through
    /// the `wasm_bindgen` exported surface (which would require
    /// going through `JsValue`).
    pub(crate) fn terminal_mut(&mut self) -> &mut Terminal {
        &mut self.terminal
    }

    pub(crate) fn terminal_ref(&self) -> &Terminal {
        &self.terminal
    }

    pub(crate) fn set_cell_metrics(&mut self, cell_w: f32, cell_h: f32) {
        self.cell_w = cell_w.max(1.0);
        self.cell_h = cell_h.max(1.0);
    }

    pub(crate) fn resize_grid_and_surface(
        &mut self,
        cols: u32,
        rows: u32,
        scale: f32,
        width_px: u32,
        height_px: u32,
    ) {
        // 1) Resize the wgpu swapchain to PHYSICAL pixels
        //    (CSS-pixel dims * scale). `width_px` / `height_px` are
        //    in CSS pixels per `ChromeBridge::resize`; multiplying
        //    here keeps chrome layout in CSS pixels (so
        //    `set_layout(viewport)` doesn't double-multiply) while
        //    the backing store matches the device. wgpu sets the
        //    canvas backing attributes on `Surface::configure`, so
        //    the browser composites backing -> CSS rect 1:1.
        // 2) Set sugarloaf's scale factor from what the swapchain
        //    ACTUALLY got — `WgpuContext::resize` clamps to the
        //    device texture limit, and rescaling to the requested
        //    DPR while the surface shrank is the blurry-overflow
        //    bug (chrome painting past the swapchain edge). See
        //    `crate::effective_render_scale`.
        // 3) Update the cell grid (Crosswords) with logical cols/rows.
        if let Some(s) = self.sugarloaf.as_mut() {
            let w = (width_px as f32 * scale).max(1.0) as u32;
            let h = (height_px as f32 * scale).max(1.0) as u32;
            s.resize(w, h);
            let actual = s.window_size();
            s.rescale(crate::effective_render_scale(
                width_px,
                height_px,
                scale,
                actual.width,
                actual.height,
            ));
        }
        self.terminal.resize(cols, rows);
    }

    /// Resize only the terminal grid (cols × rows) without touching the
    /// sugarloaf surface. Use this when the viewport hasn't changed (e.g.
    /// a TUI hides the command-composer footer, expanding the terminal
    /// rect) so that `surface.configure()` is NOT called mid-frame — it
    /// unconditionally reconfigures the WebGL swap chain and clears the
    /// backing texture to black.
    pub(crate) fn resize_grid(&mut self, cols: u32, rows: u32) {
        self.terminal.resize(cols, rows);
    }

    /// Reseed both the terminal color palette AND sugarloaf's
    /// swapchain clear color from the named IdeTheme. Resolution
    /// goes through `IdeTheme::by_name`, so unknown names fall
    /// back to `pastel_dark`.
    pub(crate) fn apply_ide_theme(&mut self, name: &str) {
        let theme = IdeTheme::by_name(name);
        self.ide_theme = theme;
        seed_terminal_theme(&mut self.terminal, &theme);
        if let Some(s) = self.sugarloaf.as_mut() {
            s.set_background_color(Some(theme.sugar(theme.bg)));
        }
    }
}

// ============================================================
// ChromeBridge: neoism-ui Chrome wired up to JS through callbacks.
// ============================================================
//
// Lives alongside RenderedTerminal because it shares the
// sugarloaf instance — chrome panels must paint into the same
// surface the terminal cells were emitted into so a single
// present() flips the swapchain once. Constructed from JS:
//
//   const b = await ChromeBridge.new(canvas, cols, rows, scale, "/workspace");
//   b.set_list_dir((reqId, path) => { … fire websocket … });
//   …
//   // RAF loop:
//   b.handle_event(JSON.stringify(uiEvent));
//   b.render(performance.now());
//
// Service traits are sync but JS replies are async; the bridge
// returns `IoError::Pending(req_id)` immediately and the host
// re-enters chrome with `UiEvent::ServiceReply { request_id, payload }`
// once the daemon answers. See `service_reply` below.

use neoism_protocol::workspace::EditorSurfaceSummary;

use neoism_ui::layout::Rect as ChromeRect;

use neoism_ui::services::RequestId;

use neoism_ui::terminal_blocks::TerminalInputBuffer;
use neoism_ui::widgets::island::{Island, IslandContexts, IslandTabTitle};
use neoism_ui::Chrome;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

#[derive(Clone, Debug, serde::Deserialize)]
struct WorkspaceIslandTabInput {
    id: String,
    title: String,
    #[serde(default)]
    host_kind: String,
    #[serde(default)]
    program: Option<String>,
}

#[derive(Clone, Debug, serde::Deserialize)]
struct WorkspaceIslandInput {
    tabs: Vec<WorkspaceIslandTabInput>,
    #[serde(default)]
    active_id: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum WorkspaceIslandIntentKind {
    Activate,
    ContextMenu,
    OpenWorkspaces,
}

#[derive(Clone, Debug, serde::Serialize)]
struct WorkspaceIslandIntent {
    kind: WorkspaceIslandIntentKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    workspace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    x: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    y: Option<f32>,
}

struct WorkspaceIslandContexts<'a> {
    tabs: &'a [WorkspaceIslandTabInput],
    active_index: usize,
}

impl IslandContexts for WorkspaceIslandContexts<'_> {
    fn len(&self) -> usize {
        self.tabs.len()
    }

    fn current_index(&self) -> usize {
        self.active_index.min(self.tabs.len().saturating_sub(1))
    }

    fn title(&self, index: usize) -> Option<IslandTabTitle> {
        self.tabs.get(index).map(|tab| IslandTabTitle {
            content: tab.title.clone(),
            program: tab.program.clone(),
            icon_kind: Some(tab.host_kind.clone()),
        })
    }
}

/// JS-callback bag the JS-backed service shims read on every call.
/// Each `Option<js_sys::Function>` is the callback the host
/// installed via `ChromeBridge::set_*_cb`; calling it fires the
/// outbound side of an async round-trip. When the reply lands the
/// host calls `ChromeBridge::service_reply`.
struct JsServiceState {
    next_request_id: u64,
    list_dir: Option<js_sys::Function>,
    read_file: Option<js_sys::Function>,
    write_file: Option<js_sys::Function>,
    stat: Option<js_sys::Function>,
    clipboard_read: Option<js_sys::Function>,
    clipboard_write: Option<js_sys::Function>,
    command_run: Option<js_sys::Function>,
    git_status: Option<js_sys::Function>,
    git_diff: Option<js_sys::Function>,
    /// Search-service callbacks. Each fires with
    /// `(req_id, envelope_json)` where `envelope_json` is a
    /// serialized `SearchClientMessage` the host forwards over
    /// the workspace daemon websocket. The reply is delivered
    /// back through `service_reply(req_id, payload_json)`.
    search_collect_files: Option<js_sys::Function>,
    search_files: Option<js_sys::Function>,
    search_grep: Option<js_sys::Function>,
    search_git_changes: Option<js_sys::Function>,
    search_git_repo_root: Option<js_sys::Function>,
    /// Cheap synchronous clipboard cache. JS pushes the latest
    /// clipboard contents through `set_clipboard_value` so the
    /// `read()` shim can return a value without going async. None
    /// until JS has populated it at least once.
    clipboard_cached: Option<String>,
    /// OS-notification outbox. Fired on every
    /// `NotificationService::notify` call with
    /// `(title, body, level)` where `level` is one of the strings
    /// `"info" | "warn" | "error"`. JS routes this through
    /// `navigator.permissions` / `Notification` and (when denied
    /// or unsupported) falls back to the in-app toast stack via
    /// `push_notification`.
    notification_outbox: Option<js_sys::Function>,
    /// `performance.now()`-style monotonic ms set by `render()`.
    /// `ClockService::now_monotonic` returns this as a `Duration`.
    now_ms: f64,
}

impl JsServiceState {
    fn new() -> Self {
        Self {
            next_request_id: 1,
            list_dir: None,
            read_file: None,
            write_file: None,
            stat: None,
            clipboard_read: None,
            clipboard_write: None,
            command_run: None,
            git_status: None,
            git_diff: None,
            search_collect_files: None,
            search_files: None,
            search_grep: None,
            search_git_changes: None,
            search_git_repo_root: None,
            clipboard_cached: None,
            notification_outbox: None,
            now_ms: 0.0,
        }
    }

    fn alloc_request_id(&mut self) -> RequestId {
        let id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        id
    }
}

/// `Send + Sync` newtype around the shared `Rc<RefCell<…>>` of
/// JS-side state. wasm32 has a single thread; the trait bounds
/// the chrome assembly requires (`Send + Sync`) are vacuous here.
#[derive(Clone)]
pub(crate) struct SharedState(Rc<RefCell<JsServiceState>>);

// SAFETY: wasm32-unknown-unknown is single-threaded; `js_sys::Function`
// and `Rc` cannot escape this thread. The `Send + Sync` bounds on
// `neoism-ui`'s service traits exist for the native multi-threaded
// host; they are unobservable here.
unsafe impl Send for SharedState {}
unsafe impl Sync for SharedState {}

#[derive(Clone, Debug)]
struct AgentPendingPermission {
    legacy_request_id: Option<u64>,
    tool_request_id: Option<String>,
    session_id: Option<String>,
    selection: i32,
}

/// All agent UI state owned by `ChromeBridge`. Populated by
/// `agent_event(json)` ingestion and read by the `agent_*`
/// accessors. Kept private to the module so it can evolve without
/// touching the wasm-bindgen surface.
#[derive(Default)]
struct AgentBridgeState {
    /// Composer input text. JS pushes via `agent_set_input` and
    /// reads via `agent_input`.
    input: String,
    /// Input history (submitted prompts, oldest-first). New entries
    /// are appended on `agent_send_message`; `agent_history_step`
    /// walks this with `history_cursor`.
    history: Vec<String>,
    /// Cursor into `history`. `None` = composer is at the live
    /// edit slot (after the newest entry); `Some(i)` = the user is
    /// viewing history entry `i`.
    history_cursor: Option<usize>,
    /// Saved live input while the cursor is walking history, so
    /// stepping past the newest entry restores the user's draft.
    history_pending_live: Option<String>,
    /// True while a daemon-side turn is in flight (between
    /// `MessageStart` and `MessageEnd`).
    streaming: bool,
    /// Currently-active session id (set by `ThreadCreated` /
    /// `ThreadSwitched`). Stamped into outbound prompts so the
    /// daemon routes them through the matching session.
    session_id: Option<String>,
    /// Session the user explicitly selected and is waiting to
    /// hydrate. While this is set, stale events/history from a
    /// previously-running session must not overwrite the visible
    /// agent pane.
    requested_session_id: Option<String>,
    /// Directory used when creating a new agent-server session
    /// after the first prompt. Set by `agent_attach`, which mirrors
    /// desktop's open-pane boot path without creating a session.
    default_directory: Option<String>,
    default_agent: Option<String>,
    default_model: Option<String>,
    default_thinking: Option<String>,
    /// Prompt submitted before an agent-server session id exists.
    /// Desktop queues `EnsureSession` followed by `SendPrompt`; web
    /// mirrors that by sending `CreateThread`, then flushing this as
    /// `SubmitPrompt` when `ThreadCreated` arrives.
    pending_prompt: Option<PendingAgentPrompt>,
    /// True between sending a `CreateThread` and its `ThreadCreated`
    /// reply. A single Enter with no session drains BOTH
    /// `EnsureSession` and the pending-prompt arm, and each used to
    /// fire its own `CreateThread` — forking the turn across two
    /// sessions (the prompt ran on one while the pane bound the
    /// other, so the stream filter dropped every timeline event).
    thread_create_inflight: bool,
    /// Set when the user reset to a fresh chat (`/new`, agent
    /// re-invoke) while the previous session may still be
    /// streaming on the daemon. While true, session-scoped events
    /// are dropped (except the ones that introduce a new session)
    /// so the old conversation can't repaint the pane we just
    /// cleared. Cleared when a ThreadCreated/Switched/HistoryChunk
    /// binds a new session.
    suppress_stale_session_events: bool,
    /// Pending permission, if any.
    pending_permission: Option<AgentPendingPermission>,
    /// JS callback the bridge fires when it builds an outbound
    /// `AgentClientMessage`. Shape:
    /// `(request_id: number, envelope_json: string) => void`.
    send_cb: Option<js_sys::Function>,
    /// Monotonic counter feeding `request_id` on the outbound
    /// callback so JS can correlate streaming replies the same
    /// way file / git replies do.
    next_request_id: u64,
}

struct PendingAgentPrompt {
    text: String,
    attachments: Vec<neoism_protocol::agent::Attachment>,
    mode: Option<String>,
    model: Option<String>,
    thinking: Option<String>,
}

/// One queued "open this finder hit" intent. Produced by
/// `pick_finder_selection` (Enter / click on the finder); drained
/// by JS via `drain_finder_open_intents` and turned into an
/// `Editor::OpenBuffer` envelope plus a buffer-tab append. `line`
/// is `Some` for grep / git-changes hits, `None` for plain files.
/// `mode` is `"files" | "grep" | "git_changes"` and `query` is the
/// finder's last query string — JS uses these to seed the editor's
/// search highlight when opening a grep hit.
#[derive(Clone, Debug, serde::Serialize)]
struct FinderOpenIntent {
    path: String,
    line: Option<u32>,
    mode: &'static str,
    query: String,
}

/// One queued "execute this palette pick" intent. The bridge can
/// resolve one-shot `PaletteAction`s itself and hands host-owned
/// choices (font rows, buffer rows, ex/search commands) to JS with
/// enough payload for the web frontend to dispatch them concretely.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum PaletteIntent {
    /// Named `PaletteAction` variant. `action` is the stable name
    /// of the variant (e.g. `"ToggleGitDiffPanel"`) and is mapped
    /// 1:1 to a TS-side dispatcher.
    Action { action: &'static str },
    /// Run an ex command (`:Foo` typed in `:` mode or picked from
    /// the suggestion list). `command` is the trimmed command text
    /// without the leading colon.
    ExCommand { command: String },
    /// `/`-search commit. `query` is the search term;
    /// `match_location` is `Some((lnum, col))` when the user picked
    /// a live buffer-match row, else `None` for plain history /
    /// freeform commit.
    Search {
        query: String,
        match_location: Option<(u64, u64)>,
    },
    /// A selected font-family row. JS owns whether a family can be
    /// applied in the web bundle; the bridge preserves the exact
    /// family name so the host can act or notify accurately.
    Font { family: String },
    /// A selected IDE theme name. JS applies it through the same
    /// `set_ide_theme` bridge path used by settings/theme cycling.
    Theme { name: String },
    /// A selected shader/filter row. JS applies a browser-side
    /// approximation because the native GPU shader stack is not
    /// present in the web renderer.
    Shader {
        title: String,
        filter: Option<String>,
    },
    /// A selected buffer row backed by the web host's tab list.
    /// Workspace targets come from shared/native picker state;
    /// pane targets come from JS via `enter_palette_buffers_mode`.
    Buffer { target: PaletteBufferIntent },
    /// A selected workspace row from the grouped host→workspace
    /// tree (the desktop's Ctrl+Shift+W Workspaces modal). JS
    /// switches the daemon workspace, mirroring desktop's
    /// `switch_daemon_host_workspace`.
    Workspace { workspace_id: String },
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "target", rename_all = "snake_case")]
enum PaletteBufferIntent {
    Workspace { tab_index: usize },
    Pane { route_id: usize, tab_index: usize },
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StatusLineClickIntent {
    ToggleSplit,
    ToggleGitDiff,
    DiagnosticsOpened,
    DiagnosticJump { line: u64 },
    Consumed,
}

/// JS-facing wasm-bindgen surface that drives `neoism_ui::Chrome`
/// over `sugarloaf`. Holds the terminal renderer, the chrome
/// assembly, and the JS-backed service shims.
#[wasm_bindgen]
pub struct ChromeBridge {
    rendered: RenderedTerminal,
    chrome: Chrome<()>,
    services_state: SharedState,
    // Concrete trait objects, owned alongside the chrome. We
    // build a fresh `Services<'_>` referencing these per call so
    // we don't store the borrowed bundle (which would need a
    // self-referencing struct).
    files: Box<JsFilesService>,
    clipboard: Box<JsClipboardService>,
    commands: Box<JsCommandService>,
    git: Box<JsGitService>,
    clock: Box<JsClockService>,
    search: Box<JsSearchService>,
    notifications: Box<JsNotificationService>,
    workspace_root: PathBuf,
    /// Armed from boot until the shell shows its first prompt.
    /// While armed, an already-running command (reattaching to a
    /// session where codex/claude was live before the page loaded)
    /// dismisses the splash so it can't paint over the TUI. Once a
    /// real prompt has been seen, submit-time logic owns dismissal
    /// — without disarming, the few ms `clear` spends running
    /// re-dismissed the splash that `clear` is supposed to bring
    /// back.
    splash_tui_guard: bool,
    // Cached viewport so `set_layout` can re-flow when the
    // composer / modal visibility flips between `handle_event`
    // calls without the host having to re-pass the size.
    viewport: ChromeRect,
    /// Index of the buffer tab the user is viewing. `0` is the
    /// always-present Terminal tab; any other index shows the
    /// matching file-viewer overlay over the terminal rect.
    active_tab_index: usize,
    /// Tab index → cached file contents. The JS host fetches a
    /// file via FilesService when it opens a new tab and pushes
    /// the text here; the render path paints it over the
    /// terminal rect when that tab is active.
    tab_contents: std::collections::HashMap<usize, String>,
    /// Last CSS-px viewport height seen by markdown scroll/resize —
    /// feeds Ctrl+U/D half-page math in `markdown_key`.
    last_markdown_viewport_h: f32,
    /// Wave 8D web outbound co-editing: the Yrs origin id every
    /// local markdown edit from this browser is stamped with.
    /// Random, non-zero, below 2^53 — same constraints as the
    /// desktop's `MarkdownCrdtState::client_id`.
    markdown_crdt_client_id: u64,
    /// Gutter width (cells) of the active editor surface — added
    /// to buffer-column carets at conversion time.
    editor_viewport_textoff: u64,
    /// CRDT binding for the ACTIVE markdown pane's shared document
    /// (the chrome renders one markdown pane at a time, so one
    /// binding suffices; switching tabs re-binds via `crdt_pump`).
    markdown_crdt_binding:
        Option<neoism_ui::editor::markdown::doc_sync::MarkdownDocBinding>,
    /// Outbound CRDT client messages queued for the JS host to
    /// drain (`crdt_pump`) and ship over the websocket envelope.
    crdt_outbound: Vec<neoism_protocol::crdt::CrdtClientMessage>,
    /// Tab index → source file path (used to derive language for
    /// syntax highlighting).
    tab_paths: std::collections::HashMap<usize, String>,
    /// Tab index → JS-owned tab kind (`terminal`, `file`,
    /// `neoism-agent`). The buffer-tab panel can represent
    /// multiple closeable terminal tabs; `active_surface` consults
    /// this instead of assuming only index 0 is a shell.
    tab_kinds: std::collections::HashMap<usize, String>,
    /// User-facing font scale, folded into cell metrics on
    /// `set_font_scale`. `1.0` is the default cell size; the bridge
    /// clamps to `[0.5, 3.0]` to keep the chrome layout sane. Ctrl+=
    /// / Ctrl+- on the JS side fold against this value so repeated
    /// presses ramp up/down geometrically.
    active_font_scale: f32,
    /// Latest editor grid snapshot received from the daemon's
    /// embedded nvim proxy. Kept as raw JSON for diagnostics and
    /// compatibility; the active structured snapshot is stored in
    /// `editor_grid_snapshots` and painted by shared Chrome.
    editor_grid_snapshot: Option<String>,
    /// Running nvim grid snapshots keyed by editor surface id.
    /// Surface-less redraws keep using the legacy single snapshot;
    /// surface-scoped redraws update this store so web panes can
    /// render independent editor grids in a follow-up UI pass.
    editor_grid_snapshots: neoism_ui::editor_snapshot::EditorGridSnapshotStore,
    /// Surface id carried by the latest editor redraw frame. The
    /// current bridge still paints one visible grid, but preserving
    /// this lets JS/rust follow-up work route independent grids by
    /// pane route without changing the wire path again.
    editor_grid_surface_id: Option<String>,
    editor_default_fg: u32,
    editor_default_bg: u32,
    editor_viewport_topline: u64,
    editor_viewport_botline: u64,
    editor_viewport_line_count: u64,
    /// Row deltas already animated from `GridScroll` and waiting
    /// for the matching `WinViewport.scroll_delta`. The daemon can
    /// deliver both for one nvim movement; animating both makes
    /// held-arrow scroll feel doubled and choppy.
    pending_grid_scroll_animation_rows: i32,
    /// JS callback shipping nvim input bytes back to the daemon
    /// over the WebSocket. Installed by `set_nvim_send` shortly
    /// after `ChromeBridge::new`. `None` until installed; calls to
    /// `nvim_send_keys` are dropped silently in that case (matches
    /// the existing service-callback pattern).
    nvim_send: Option<js_sys::Function>,
    /// JS callback shipping PTY response bytes (DSR / OSC / cursor
    /// pos / clipboard reply) back to the daemon. When installed,
    /// `feed_pty_output` automatically drains
    /// `Terminal::take_pty_writes` and pushes the bytes through
    /// this callback so JS hosts don't need to poll. Mirrors the
    /// existing `nvim_send` pattern. `None` until installed; the
    /// `take_pty_writes` polling path still works (JS can keep
    /// using it for back-compat).
    pty_outbox: Option<js_sys::Function>,
    /// Device pixel ratio from the last `resize()` call. Cached so
    /// `render()` can re-derive and apply grid dimensions after the
    /// chrome layout changes internally (e.g. composer show/hide)
    /// without waiting for the next JS-driven resize.
    last_dpr_scale: f32,
    /// Web-side mirror of desktop terminal command-block state.
    /// Chrome still uses `SimpleInputBuffer` for visible composer
    /// text, while this buffer tracks submits, prompt transitions,
    /// durations, and output anchors for block-section rendering.
    terminal_blocks: TerminalInputBuffer,
    /// Requests raised by Rust-owned chrome (splash menu,
    /// command palette) for the JS host to open/focus the real
    /// Neoism Agent buffer tab in its canonical tab list.
    pending_agent_tab_opens: u32,
    /// Finder "open this hit" intents queued by `pick_finder_selection`
    /// when the user activates a row (Enter or click). Drained by JS
    /// via `drain_finder_open_intents` and turned into open-buffer
    /// dispatches the same way file-tree opens already are.
    pending_finder_open_intents: Vec<FinderOpenIntent>,
    /// Command-palette "execute this pick" intents queued by
    /// `pick_palette_action`. Drained by JS via
    /// `drain_palette_intents`; the JS side maps the intent kind
    /// onto a host-side handler (toggle panel, run ex command,
    /// etc.).
    pending_palette_intents: Vec<PaletteIntent>,
    /// Agent pane state — composer input, history, timeline,
    /// streaming flag, pending permission, JS send callback.
    /// Lives on the bridge so the web frontend can drive an
    /// `AgentPane`-equivalent without spawning a parallel store.
    agent_state: AgentBridgeState,
    /// Last `set_diagnostics(...)` payload, decoded into the
    /// panel's `PopupItem` shape. `show_diagnostics_at(...)` reads
    /// this when opening the popover so the visible list reflects
    /// the most recent push (the `DiagnosticsPush` wire message)
    /// without requiring callers to thread the items through every
    /// open call.
    cached_diagnostics: Vec<neoism_ui::panels::diagnostics_popup::PopupItem>,
    /// Latest daemon-known editor surface bindings. JS owns the
    /// pane routing and sends Bind/List/Close through the workspace
    /// envelope; this cache lets the bridge consume the matching
    /// replies without dropping them on older chrome paths.
    editor_surfaces: Vec<EditorSurfaceSummary>,
    workspace_island: Island,
    workspace_island_tabs: Vec<WorkspaceIslandTabInput>,
    workspace_island_active_id: Option<String>,
    pending_workspace_island_intents: Vec<WorkspaceIslandIntent>,
}

// ------------------------------------------------------------
// Submodule wiring. The JS-visible `#[wasm_bindgen] impl ChromeBridge`
// surface is split across the child modules below (moved verbatim from
// the former single impl); `RenderedTerminal` and all shared type
// definitions stay in this file so every child reaches them via
// `use super::*`.
// ------------------------------------------------------------
mod agent;
mod buffer_tabs_layout;
mod catalog;
mod chrome_bridge_core;
mod construct;
mod file_tree_workspace;
mod frame;
mod nvim_editor_grid;
mod overlays;
mod palettes_finder;
mod panels;
mod service_installers;
mod services;
mod status_line;
mod support;
mod terminal_input;

pub(crate) use catalog::*;
pub(crate) use services::*;
pub(crate) use support::*;
