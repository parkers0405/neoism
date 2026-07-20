//! Typed parser for nvim's `redraw` notification batches.
//!
//! Ported from `neovide/src/bridge/events.rs`, trimmed to the subset
//! that maps cleanly onto a `Crosswords` grid (no cmdline, no message
//! routing, no float windows). Everything that actually paints into a
//! cell goes through this module; external popupmenu events are parsed
//! so the frontend can render completion UI in Rust.
//!
//! Each `redraw` notification from nvim is a batch: an array whose
//! first element is the event name and whose remaining elements are
//! per-event argument arrays. `parse_redraw_batch` decodes one batch
//! into zero or more typed `RedrawEvent`s; unknown events are dropped
//! silently so a future nvim minor release that adds an event doesn't
//! break the editor pane.

use std::collections::HashMap;
use std::convert::TryInto;
use std::fmt;

use rmpv::Value;

use neoism_terminal_core::colors::{AnsiColor, ColorRgb};
use neoism_terminal_core::crosswords::pos::{Column, Line};
use neoism_terminal_core::crosswords::style::{
    Style as RioStyle, StyleFlags, StyleId, DEFAULT_STYLE_ID,
};
use neoism_terminal_core::crosswords::Crosswords;

#[derive(Clone, Debug)]
pub enum ParseError {
    Array(Value),
    Map(Value),
    String(Value),
    U64(Value),
    I64(Value),
    F64(Value),
    Bool(Value),
    Format(String),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Array(v) => write!(f, "expected array, got {v}"),
            ParseError::Map(v) => write!(f, "expected map, got {v}"),
            ParseError::String(v) => write!(f, "expected string, got {v}"),
            ParseError::U64(v) => write!(f, "expected u64, got {v}"),
            ParseError::I64(v) => write!(f, "expected i64, got {v}"),
            ParseError::F64(v) => write!(f, "expected f64, got {v}"),
            ParseError::Bool(v) => write!(f, "expected bool, got {v}"),
            ParseError::Format(s) => write!(f, "format error: {s}"),
        }
    }
}

impl std::error::Error for ParseError {}

type Result<T> = std::result::Result<T, ParseError>;

/// 0xRRGGBB packed into a u32 — matches nvim's wire format. Unpacking
/// to a struct is the renderer's job (sugarloaf wants `[f32; 4]`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct PackedColor(pub u32);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Colors {
    pub foreground: Option<PackedColor>,
    pub background: Option<PackedColor>,
    pub special: Option<PackedColor>,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum UnderlineStyle {
    #[default]
    None,
    Underline,
    UnderCurl,
    UnderDot,
    UnderDash,
    UnderDouble,
}

#[derive(Clone, Debug, Default)]
pub struct Style {
    pub colors: Colors,
    pub reverse: bool,
    pub italic: bool,
    pub bold: bool,
    pub strikethrough: bool,
    pub blend: u8,
    pub underline: UnderlineStyle,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum EditorMode {
    #[default]
    Normal,
    Insert,
    Visual,
    Replace,
    CmdLine,
    Unknown(String),
}

#[derive(Clone, Debug)]
pub struct GridLineCell {
    /// Empty for the right half of a double-width char.
    pub text: String,
    /// `None` → reuse the highlight id from the previous cell in the
    /// same `GridLine` event (always `Some` for the first cell).
    pub highlight_id: Option<u64>,
    /// Repeat-count; `None` → draw once. Double-width never repeats.
    pub repeat: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PopupMenuItem {
    pub word: String,
    pub kind: String,
    pub menu: String,
    pub info: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PopupMenu {
    pub items: Vec<PopupMenuItem>,
    pub selected: i64,
    pub row: u64,
    pub col: u64,
    pub grid: u64,
    pub max_word_chars: usize,
}

impl PopupMenu {
    pub fn selected_index(&self) -> Option<usize> {
        if self.selected < 0 {
            return None;
        }
        let selected = self.selected as usize;
        (selected < self.items.len()).then_some(selected)
    }
}

/// Subset of nvim's redraw events relevant to a single-grid editor
/// pane. Everything not represented is intentionally dropped.
#[derive(Clone, Debug)]
pub enum RedrawEvent {
    SetTitle {
        title: String,
    },
    ModeChange {
        mode: EditorMode,
        mode_index: u64,
    },
    BusyStart,
    BusyStop,
    Flush,
    Resize {
        grid: u64,
        width: u64,
        height: u64,
    },
    DefaultColorsSet {
        colors: Colors,
    },
    HighlightAttributesDefine {
        id: u64,
        style: Style,
    },
    GridLine {
        grid: u64,
        row: u64,
        column_start: u64,
        cells: Vec<GridLineCell>,
    },
    Clear {
        grid: u64,
    },
    Destroy {
        grid: u64,
    },
    CursorGoto {
        grid: u64,
        row: u64,
        column: u64,
    },
    Scroll {
        grid: u64,
        top: u64,
        bottom: u64,
        left: u64,
        right: u64,
        rows: i64,
        columns: i64,
    },
    /// `win_viewport`: nvim emits this whenever the viewport position
    /// in the buffer changes — j/k near scrolloff, Ctrl-D/Ctrl-U,
    /// page-up/down, jumping to a line. `scroll_delta` is the LINE
    /// COUNT the viewport moved (positive = scrolled DOWN through the
    /// buffer, negative = scrolled UP). This is the canonical signal
    /// for scroll animation — it always fires when the viewport
    /// moves, even when nvim sends grid_line redraws instead of
    /// grid_scroll (which is the case for big jumps like Ctrl-U).
    /// Neovide drives its `scroll_animation` exclusively from this
    /// event; we do the same for the editor_scroll spring.
    WinViewport {
        grid: u64,
        /// Top buffer line currently visible (0-based). Used to
        /// detect "at top of file" — when topline == 0 AND the user
        /// continues scrolling up, the elastic edge bounce kicks in.
        topline: u64,
        /// Bottom-exclusive buffer line. `botline >= line_count` means
        /// "at bottom of file"; combined with continued scroll-down
        /// input, that's the trigger for the bottom edge bounce.
        botline: u64,
        /// Total line count of the buffer. Cached so `botline >=
        /// line_count` is comparable without an extra rpc roundtrip.
        line_count: u64,
        scroll_delta: f64,
        /// Buffer-coordinate cursor line/col (0-based) — the presence
        /// plane publishes these so remote screens can draw this
        /// pane's caret at the true buffer position.
        curline: u64,
        curcol: u64,
        /// Gutter width in cells (0 when the producer doesn't know).
        textoff: u64,
    },
    PopupMenuShow {
        menu: PopupMenu,
    },
    PopupMenuSelect {
        selected: i64,
    },
    PopupMenuHide,
}

fn parse_array(value: Value) -> Result<Vec<Value>> {
    value.try_into().map_err(ParseError::Array)
}

fn parse_map(value: Value) -> Result<Vec<(Value, Value)>> {
    value.try_into().map_err(ParseError::Map)
}

fn parse_string(value: Value) -> Result<String> {
    match value {
        Value::String(s) => Ok(s.into_str().unwrap_or_else(|| "\u{FFFD}".into())),
        other => Err(ParseError::String(other)),
    }
}

fn parse_u64(value: Value) -> Result<u64> {
    value.try_into().map_err(ParseError::U64)
}

fn parse_i64(value: Value) -> Result<i64> {
    value.try_into().map_err(ParseError::I64)
}

#[allow(dead_code)]
fn parse_bool(value: Value) -> Result<bool> {
    value.try_into().map_err(ParseError::Bool)
}

fn extract_values<const N: usize>(values: Vec<Value>) -> Result<[Value; N]> {
    if values.len() < N {
        return Err(ParseError::Format(format!(
            "need {N} values, got {}",
            values.len()
        )));
    }
    let mut out: Vec<Value> = values.into_iter().take(N).collect();
    while out.len() < N {
        out.push(Value::Nil);
    }
    out.try_into()
        .map_err(|_| ParseError::Format("extract_values length mismatch".into()))
}

fn parse_set_title(args: Vec<Value>) -> Result<RedrawEvent> {
    let [title] = extract_values(args)?;
    Ok(RedrawEvent::SetTitle {
        title: parse_string(title)?,
    })
}

fn parse_mode_change(args: Vec<Value>) -> Result<RedrawEvent> {
    let [mode, idx] = extract_values(args)?;
    let mode_name = parse_string(mode)?;
    let mode = match mode_name.as_str() {
        "normal" => EditorMode::Normal,
        "insert" => EditorMode::Insert,
        "visual" => EditorMode::Visual,
        "replace" => EditorMode::Replace,
        "cmdline_normal" | "cmdline_insert" | "cmdline_replace" => EditorMode::CmdLine,
        _ => EditorMode::Unknown(mode_name),
    };
    Ok(RedrawEvent::ModeChange {
        mode,
        mode_index: parse_u64(idx)?,
    })
}

fn parse_grid_resize(args: Vec<Value>) -> Result<RedrawEvent> {
    let [grid, width, height] = extract_values(args)?;
    Ok(RedrawEvent::Resize {
        grid: parse_u64(grid)?,
        width: parse_u64(width)?,
        height: parse_u64(height)?,
    })
}

fn parse_default_colors(args: Vec<Value>) -> Result<RedrawEvent> {
    // nvim sends 5 args (fg, bg, sp, term_fg, term_bg) — we use the gui triplet.
    let [fg, bg, sp, _, _] = extract_values(args)?;
    Ok(RedrawEvent::DefaultColorsSet {
        colors: Colors {
            foreground: parse_optional_packed_color(fg),
            background: parse_optional_packed_color(bg),
            special: parse_optional_packed_color(sp),
        },
    })
}

fn parse_optional_packed_color(value: Value) -> Option<PackedColor> {
    match value {
        Value::Integer(i) => i.as_u64().map(|n| PackedColor(n as u32)),
        _ => None,
    }
}

fn parse_style(style_map: Value) -> Result<Style> {
    let attrs = parse_map(style_map)?;
    let mut style = Style::default();

    for (key, value) in attrs {
        let Value::String(name) = key else { continue };
        let Some(name) = name.as_str() else { continue };
        match (name, value) {
            ("foreground", Value::Integer(c)) => {
                style.colors.foreground = c.as_u64().map(|n| PackedColor(n as u32));
            }
            ("background", Value::Integer(c)) => {
                style.colors.background = c.as_u64().map(|n| PackedColor(n as u32));
            }
            ("special", Value::Integer(c)) => {
                style.colors.special = c.as_u64().map(|n| PackedColor(n as u32));
            }
            ("reverse", Value::Boolean(b)) => style.reverse = b,
            ("italic", Value::Boolean(b)) => style.italic = b,
            ("bold", Value::Boolean(b)) => style.bold = b,
            ("strikethrough", Value::Boolean(b)) => style.strikethrough = b,
            ("blend", Value::Integer(n)) => {
                style.blend = n.as_u64().unwrap_or(0) as u8;
            }
            ("underline", Value::Boolean(true)) => {
                style.underline = UnderlineStyle::Underline;
            }
            ("undercurl", Value::Boolean(true)) => {
                style.underline = UnderlineStyle::UnderCurl;
            }
            ("underdotted" | "underdot", Value::Boolean(true)) => {
                style.underline = UnderlineStyle::UnderDot;
            }
            ("underdashed" | "underdash", Value::Boolean(true)) => {
                style.underline = UnderlineStyle::UnderDash;
            }
            ("underdouble" | "underlineline", Value::Boolean(true)) => {
                style.underline = UnderlineStyle::UnderDouble;
            }
            _ => {}
        }
    }
    Ok(style)
}

fn parse_hl_attr_define(args: Vec<Value>) -> Result<RedrawEvent> {
    // (id, attrs, term_attrs, info)
    let [id, attrs, _, _] = extract_values(args)?;
    Ok(RedrawEvent::HighlightAttributesDefine {
        id: parse_u64(id)?,
        style: parse_style(attrs)?,
    })
}

fn parse_grid_line_cell(value: Value) -> Result<GridLineCell> {
    let mut parts = parse_array(value)?;
    if parts.is_empty() {
        return Err(ParseError::Format("grid_line cell is empty".into()));
    }
    let text = parse_string(parts.remove(0))?;
    let highlight_id = if !parts.is_empty() {
        Some(parse_u64(parts.remove(0))?)
    } else {
        None
    };
    let repeat = if !parts.is_empty() {
        Some(parse_u64(parts.remove(0))?)
    } else {
        None
    };
    Ok(GridLineCell {
        text,
        highlight_id,
        repeat,
    })
}

fn parse_grid_line(args: Vec<Value>) -> Result<RedrawEvent> {
    let [grid, row, col_start, cells] = extract_values(args)?;
    let cells = parse_array(cells)?
        .into_iter()
        .map(parse_grid_line_cell)
        .collect::<Result<Vec<_>>>()?;
    Ok(RedrawEvent::GridLine {
        grid: parse_u64(grid)?,
        row: parse_u64(row)?,
        column_start: parse_u64(col_start)?,
        cells,
    })
}

const POPUP_WORD_MAX_CHARS: usize = 96;
const POPUP_KIND_MAX_CHARS: usize = 32;
const POPUP_MENU_MAX_CHARS: usize = 96;

fn bounded_popup_string(value: Option<&Value>, max_chars: usize) -> String {
    let Some(text) = value.and_then(|value| value.as_str()) else {
        return String::new();
    };

    let mut out = String::new();
    for (idx, ch) in text.chars().enumerate() {
        if idx >= max_chars {
            break;
        }
        out.push(ch);
    }
    out
}

fn parse_popupmenu_item(value: Value) -> Result<PopupMenuItem> {
    let parts = parse_array(value)?;
    Ok(PopupMenuItem {
        word: bounded_popup_string(parts.get(0), POPUP_WORD_MAX_CHARS),
        kind: bounded_popup_string(parts.get(1), POPUP_KIND_MAX_CHARS),
        menu: bounded_popup_string(parts.get(2), POPUP_MENU_MAX_CHARS),
        // Nvim/LSP can attach large docs here. The Rust popup does not
        // render it, so keeping it only bloats redraw bursts.
        info: String::new(),
    })
}

fn parse_popupmenu_show(args: Vec<Value>) -> Result<RedrawEvent> {
    if args.len() < 4 {
        return Err(ParseError::Format(format!(
            "popupmenu_show needs at least 4 args, got {}",
            args.len()
        )));
    }
    let mut iter = args.into_iter();
    let items = parse_array(
        iter.next()
            .ok_or_else(|| ParseError::Format("popupmenu_show missing items".into()))?,
    )?
    .into_iter()
    .map(parse_popupmenu_item)
    .collect::<Result<Vec<_>>>()?;
    let max_word_chars = items
        .iter()
        .map(|item| item.word.chars().count())
        .max()
        .unwrap_or(0);
    let selected =
        parse_i64(iter.next().ok_or_else(|| {
            ParseError::Format("popupmenu_show missing selected".into())
        })?)?;
    let row = parse_u64(
        iter.next()
            .ok_or_else(|| ParseError::Format("popupmenu_show missing row".into()))?,
    )?;
    let col = parse_u64(
        iter.next()
            .ok_or_else(|| ParseError::Format("popupmenu_show missing col".into()))?,
    )?;
    let grid = iter.next().and_then(|v| parse_u64(v).ok()).unwrap_or(0);

    Ok(RedrawEvent::PopupMenuShow {
        menu: PopupMenu {
            items,
            selected,
            row,
            col,
            grid,
            max_word_chars,
        },
    })
}

fn parse_grid_clear(args: Vec<Value>) -> Result<RedrawEvent> {
    let [grid] = extract_values(args)?;
    Ok(RedrawEvent::Clear {
        grid: parse_u64(grid)?,
    })
}

fn parse_grid_destroy(args: Vec<Value>) -> Result<RedrawEvent> {
    let [grid] = extract_values(args)?;
    Ok(RedrawEvent::Destroy {
        grid: parse_u64(grid)?,
    })
}

fn parse_grid_cursor_goto(args: Vec<Value>) -> Result<RedrawEvent> {
    let [grid, row, col] = extract_values(args)?;
    // nvim has been observed to send -1 transiently during shutdown;
    // saturate to 0 instead of erroring out the whole batch.
    let row = parse_i64(row)?.max(0) as u64;
    let col = parse_i64(col)?.max(0) as u64;
    Ok(RedrawEvent::CursorGoto {
        grid: parse_u64(grid)?,
        row,
        column: col,
    })
}

/// `win_viewport` payload (Neovim ≥ 0.6):
///   [grid, win, topline, botline, curline, curcol, line_count, scroll_delta]
/// Older Neovim sent only the first 6/7 fields; if `scroll_delta` is
/// missing we treat it as zero and skip animation (matches what
/// neovide does in that branch).
fn parse_win_viewport(args: Vec<Value>) -> Result<Option<RedrawEvent>> {
    if args.len() < 7 {
        // Old protocol — no scroll_delta field. Nothing to animate.
        return Ok(None);
    }
    let mut iter = args.into_iter();
    let grid = parse_u64(
        iter.next()
            .ok_or_else(|| ParseError::Format("win_viewport missing grid".into()))?,
    )?;
    // win
    iter.next();
    let topline = iter.next().and_then(|v| parse_u64(v).ok()).unwrap_or(0);
    let botline = iter.next().and_then(|v| parse_u64(v).ok()).unwrap_or(0);
    let curline = iter.next().and_then(|v| parse_u64(v).ok()).unwrap_or(0);
    let curcol = iter.next().and_then(|v| parse_u64(v).ok()).unwrap_or(0);
    let line_count = iter.next().and_then(|v| parse_u64(v).ok()).unwrap_or(0);
    let scroll_delta = match iter.next() {
        Some(v) => match v {
            Value::F64(f) => f,
            Value::F32(f) => f as f64,
            Value::Integer(i) => i.as_i64().unwrap_or(0) as f64,
            _ => 0.0,
        },
        None => 0.0,
    };
    // Emit the event even when scroll_delta is zero — the renderer
    // uses topline/botline/line_count to detect at-edge for the
    // elastic bounce when wheel/key input keeps coming but nvim
    // can't scroll any further.
    Ok(Some(RedrawEvent::WinViewport {
        grid,
        topline,
        botline,
        line_count,
        scroll_delta,
        curline,
        curcol,
        textoff: 0,
    }))
}

fn parse_grid_scroll(args: Vec<Value>) -> Result<RedrawEvent> {
    let [grid, top, bottom, left, right, rows, columns] = extract_values(args)?;
    Ok(RedrawEvent::Scroll {
        grid: parse_u64(grid)?,
        top: parse_u64(top)?,
        bottom: parse_u64(bottom)?,
        left: parse_u64(left)?,
        right: parse_u64(right)?,
        rows: parse_i64(rows)?,
        columns: parse_i64(columns)?,
    })
}

/// Parse one element of nvim's `redraw` notify args.
///
/// Each element is `[event_name, args0, args1, ...]` where the same
/// event_name can repeat for many sets of args (e.g. one `grid_line`
/// batch for every row in a redraw). Unknown event names yield an
/// empty `Vec` rather than an error.
pub fn parse_redraw_batch(value: Value) -> Result<Vec<RedrawEvent>> {
    let mut iter = parse_array(value)?.into_iter();
    let event_name = iter
        .next()
        .ok_or_else(|| ParseError::Format("empty redraw batch".into()))
        .and_then(parse_string)?;

    let mut out = Vec::with_capacity(iter.len());
    for params in iter {
        let params = parse_array(params)?;
        let parsed = match event_name.as_str() {
            "set_title" => Some(parse_set_title(params)?),
            "mode_change" => Some(parse_mode_change(params)?),
            "busy_start" => Some(RedrawEvent::BusyStart),
            "busy_stop" => Some(RedrawEvent::BusyStop),
            "flush" => Some(RedrawEvent::Flush),
            "grid_resize" => Some(parse_grid_resize(params)?),
            "default_colors_set" => Some(parse_default_colors(params)?),
            "hl_attr_define" => Some(parse_hl_attr_define(params)?),
            "grid_line" => Some(parse_grid_line(params)?),
            "grid_clear" => Some(parse_grid_clear(params)?),
            "grid_destroy" => Some(parse_grid_destroy(params)?),
            "grid_cursor_goto" => Some(parse_grid_cursor_goto(params)?),
            "grid_scroll" => Some(parse_grid_scroll(params)?),
            "win_viewport" => parse_win_viewport(params)?,
            "popupmenu_show" => Some(parse_popupmenu_show(params)?),
            "popupmenu_select" => {
                let [selected] = extract_values(params)?;
                Some(RedrawEvent::PopupMenuSelect {
                    selected: parse_i64(selected)?,
                })
            }
            "popupmenu_hide" => Some(RedrawEvent::PopupMenuHide),
            _ => None,
        };
        if let Some(ev) = parsed {
            out.push(ev);
        }
    }
    Ok(out)
}

/// In-memory mirror of nvim's highlight-attribute table. Indexed by
/// the `id` from `hl_attr_define`. Owned by the editor pane so the
/// renderer can map `Square::style_id` cells back to colors when it
/// gains awareness of nvim grids in Phase 2f. For Phase 2e the table
/// is still populated but only the default colors actually paint.
pub type HighlightTable = HashMap<u64, Style>;

/// Single-grid-only resize adapter. `Crosswords::resize` requires a
/// `Dimensions` impl, but we only need width + height.
struct EditorDims {
    columns: usize,
    lines: usize,
}

impl neoism_terminal_core::crosswords::grid::Dimensions for EditorDims {
    fn total_lines(&self) -> usize {
        self.lines
    }
    fn screen_lines(&self) -> usize {
        self.lines
    }
    fn columns(&self) -> usize {
        self.columns
    }
}

/// Apply a batch of typed redraw events to a `Crosswords` grid.
///
/// Coverage today (Phase 2e MVP): the events that are required to put
/// glyphs on the screen at the right positions — `grid_resize`,
/// `grid_clear`, `grid_line`, `grid_scroll`, `grid_cursor_goto`,
/// `default_colors_set`, `hl_attr_define`, `flush`. `flush` is only a
/// batch boundary; preceding cell/color/scroll events carry the damage.
/// `set_title` is accumulated into `title_out`. Anything else is a no-op.
///
/// Returns the number of visible grid/cursor/color changes actually
/// applied. Batch boundaries and frontend-only overlay events are not
/// counted; the caller handles overlay dirtiness separately.
pub fn apply_redraw_events(
    crosswords: &mut Crosswords,
    hl_table: &mut HighlightTable,
    default_colors: &mut Colors,
    title_out: &mut Option<String>,
    events: &[RedrawEvent],
    target_grid: u64,
) -> usize {
    let mut applied = 0usize;
    let mut needs_full_damage = false;

    // Nvim is allowed to batch highlight definitions and grid lines in
    // either order. Rio stores resolved concrete StyleIds on cells, so a
    // grid_line processed before its hl_attr_define would permanently
    // render as default until the line is redrawn by scrolling. Preload
    // the highlight/default-color tables for the whole drained batch
    // before applying any cells.
    for ev in events {
        match ev {
            RedrawEvent::DefaultColorsSet { colors } => {
                if default_colors != colors {
                    *default_colors = colors.clone();
                    needs_full_damage = true;
                    applied += 1;
                }
            }
            RedrawEvent::HighlightAttributesDefine { id, style } => {
                hl_table.insert(*id, style.clone());
                applied += 1;
            }
            _ => {}
        }
    }

    for ev in events {
        match ev {
            RedrawEvent::SetTitle { title } => {
                *title_out = Some(title.clone());
                applied += 1;
            }
            RedrawEvent::DefaultColorsSet { colors } => {
                let _ = colors;
            }
            RedrawEvent::HighlightAttributesDefine { id, style } => {
                let _ = (id, style);
            }
            RedrawEvent::Resize {
                grid,
                width,
                height,
            } => {
                if *grid != target_grid {
                    continue;
                }
                let dims = EditorDims {
                    columns: (*width as usize).max(1),
                    lines: (*height as usize).max(1),
                };
                if crosswords.columns() != dims.columns
                    || crosswords.screen_lines() != dims.lines
                {
                    crosswords.resize(dims);
                    needs_full_damage = true;
                    applied += 1;
                }
            }
            RedrawEvent::Clear { grid } => {
                if *grid != target_grid {
                    continue;
                }
                let lines = crosswords.screen_lines() as i32;
                let cols = crosswords.columns();
                let default_style_id =
                    resolve_style_id(crosswords, hl_table, default_colors, 0);
                for row in 0..lines {
                    let line = Line(row);
                    for c in 0..cols {
                        let sq = &mut crosswords.grid[line][Column(c)];
                        sq.clear();
                        sq.set_style_id(default_style_id);
                    }
                }
                needs_full_damage = true;
                applied += 1;
            }
            RedrawEvent::GridLine {
                grid,
                row,
                column_start,
                cells,
            } => {
                if *grid != target_grid {
                    continue;
                }
                let cols = crosswords.columns();
                if *row as usize >= crosswords.screen_lines() {
                    tracing::trace!(
                        target: "neoism::nvim_pump",
                        row,
                        screen_lines = crosswords.screen_lines(),
                        "skipping stale grid_line outside viewport"
                    );
                    continue;
                }
                let line = Line(*row as i32);
                let mut col = *column_start as usize;
                let mut last_hl: Option<u64> = None;
                // Cache the most recently resolved StyleId — nvim
                // typically runs the same hl across many cells in a
                // line, so this collapses the per-cell intern lookup
                // into one per highlight transition.
                let mut last_style_id: StyleId = DEFAULT_STYLE_ID;
                let mut style_id_dirty = true;
                for cell in cells {
                    if let Some(hl) = cell.highlight_id {
                        if last_hl != Some(hl) {
                            style_id_dirty = true;
                        }
                        last_hl = Some(hl);
                    }
                    if style_id_dirty {
                        last_style_id = match last_hl {
                            Some(hl) => {
                                resolve_style_id(crosswords, hl_table, default_colors, hl)
                            }
                            None => DEFAULT_STYLE_ID,
                        };
                        style_id_dirty = false;
                    }
                    // nvim emits "" for the right half of a wide char —
                    // no glyph, but the trailing cell still belongs to
                    // the wide pair. Mark it as Spacer so the renderer
                    // doesn't try to paint a separate glyph there.
                    let ch = cell.text.chars().next();
                    let repeat = cell.repeat.unwrap_or(1) as usize;
                    for _ in 0..repeat {
                        if col >= cols {
                            break;
                        }
                        // Empty text → wide-char trailing half.
                        // Mark the previous cell Wide first (separate
                        // borrow scope), then drop into the trailing
                        // cell write below.
                        let is_wide_trailing =
                            matches!(ch, None) || matches!(ch, Some(c) if c.is_control());
                        if is_wide_trailing && col > 0 {
                            use neoism_terminal_core::crosswords::square::Wide;
                            let prev = &mut crosswords.grid[line][Column(col - 1)];
                            prev.set_wide(Wide::Wide);
                        }
                        let sq = &mut crosswords.grid[line][Column(col)];
                        sq.clear();
                        if is_wide_trailing {
                            use neoism_terminal_core::crosswords::square::Wide;
                            sq.set_wide(Wide::Spacer);
                        } else if let Some(c) = ch {
                            sq.set_c(c);
                        }
                        sq.set_style_id(last_style_id);
                        col += 1;
                    }
                }
                crosswords.damage_line(*row as usize);
                applied += 1;
            }
            RedrawEvent::CursorGoto { grid, row, column } => {
                if *grid != target_grid {
                    continue;
                }
                let cols = crosswords.columns();
                let lines = crosswords.screen_lines();
                let r = (*row as i32).min(lines.saturating_sub(1) as i32).max(0);
                let c = (*column as usize).min(cols.saturating_sub(1));
                let next_row = Line(r);
                let next_col = Column(c);
                if crosswords.grid.cursor.pos.row != next_row
                    || crosswords.grid.cursor.pos.col != next_col
                {
                    crosswords.grid.cursor.pos.row = next_row;
                    crosswords.grid.cursor.pos.col = next_col;
                    applied += 1;
                }
            }
            RedrawEvent::Flush => {}
            RedrawEvent::Scroll {
                grid,
                top,
                bottom,
                left,
                right,
                rows,
                ..
            } => {
                if *grid != target_grid {
                    continue;
                }
                // nvim grid_scroll: copy the rect [top, bottom) × [left, right)
                // by `rows` (positive = scroll up / contents move toward top,
                // negative = scroll down). nvim then sends grid_line events for
                // the freed rows, so we only need to shift existing cells and
                // blank the freed rows — if the matching grid_line slips into
                // a later batch, the freed rect must not still hold the rows
                // that just scrolled off, or the renderer will paint them as
                // ghost rows for one frame ("chunks of previous lines").
                let screen_lines = crosswords.screen_lines();
                let screen_cols = crosswords.columns();
                let top = (*top as usize).min(screen_lines);
                let bottom = (*bottom as usize).min(screen_lines);
                let left = (*left as usize).min(screen_cols);
                let right = (*right as usize).min(screen_cols);
                let rows = *rows;

                let mut shifted = false;
                if top < bottom && left < right && rows != 0 {
                    let span = bottom - top;
                    let default_style_id =
                        resolve_style_id(crosswords, hl_table, default_colors, 0);
                    let (freed_from, freed_to) = if rows > 0 {
                        let n = (rows as usize).min(span);
                        if n < span {
                            for i in top..(bottom - n) {
                                let src = (i + n) as i32;
                                let dst = i as i32;
                                for c in left..right {
                                    let cell = crosswords.grid[Line(src)][Column(c)];
                                    crosswords.grid[Line(dst)][Column(c)] = cell;
                                }
                            }
                            (bottom - n, bottom)
                        } else {
                            // n == span: the entire rect is being replaced.
                            (top, bottom)
                        }
                    } else {
                        let n = ((-rows) as usize).min(span);
                        if n < span {
                            let mut i = bottom;
                            while i > top + n {
                                i -= 1;
                                let src = (i - n) as i32;
                                let dst = i as i32;
                                for c in left..right {
                                    let cell = crosswords.grid[Line(src)][Column(c)];
                                    crosswords.grid[Line(dst)][Column(c)] = cell;
                                }
                            }
                            (top, top + n)
                        } else {
                            (top, bottom)
                        }
                    };
                    // Blank the freed rows so any delayed grid_line cannot
                    // leave stale off-screen rows visible as "ghost lines"
                    // for a frame, and so a full-rect scroll (rows == span)
                    // does not leave the entire pre-scroll content sitting
                    // under the upcoming grid_line writes.
                    for i in freed_from..freed_to {
                        let line = Line(i as i32);
                        for c in left..right {
                            let sq = &mut crosswords.grid[line][Column(c)];
                            sq.clear();
                            sq.set_style_id(default_style_id);
                        }
                    }
                    shifted = true;
                }
                if shifted {
                    // The terminal grid is the source of truth. Damage every row whose
                    // CPU cells moved; the renderer's retained-grid copy is only an
                    // optimization and is not guaranteed to run when editor scrollback
                    // and animation offsets cancel each other in the same frame.
                    for row in top..bottom {
                        crosswords.damage_line(row);
                    }
                    applied += 1;
                }
            }
            RedrawEvent::WinViewport { .. } => {}
            RedrawEvent::PopupMenuShow { .. }
            | RedrawEvent::PopupMenuSelect { .. }
            | RedrawEvent::PopupMenuHide => {}
            RedrawEvent::ModeChange { mode, .. } => {
                // Map nvim's mode → cursor glyph. Normal/Visual/Replace
                // get the chunky block (so the user can tell at a glance
                // they're not in insert), Insert gets the thin Beam,
                // CmdLine/Unknown fall back to Block. Underline isn't
                // used: Replace is rarely entered from a Warp-style
                // workflow and keeping the palette small avoids subtle
                // shape flicker through transient modes.
                use neoism_terminal_core::ansi::CursorShape;
                let shape = match mode {
                    EditorMode::Insert => CursorShape::Beam,
                    EditorMode::Normal
                    | EditorMode::Visual
                    | EditorMode::Replace
                    | EditorMode::CmdLine
                    | EditorMode::Unknown(_) => CursorShape::Block,
                };
                crosswords.cursor_shape = shape;
                crosswords.default_cursor_shape = shape;
                applied += 1;
            }
            RedrawEvent::Destroy { .. }
            | RedrawEvent::BusyStart
            | RedrawEvent::BusyStop => {
                // Phase 2e MVP: tracked in tests, no grid mutation yet.
                applied += 1;
            }
        }
    }

    if needs_full_damage {
        crosswords.mark_fully_damaged();
    }

    applied
}

/// Convert a packed `0xRRGGBB` u32 to Rio's `ColorRgb`.
fn packed_to_rgb(c: PackedColor) -> ColorRgb {
    let v = c.0;
    ColorRgb {
        r: ((v >> 16) & 0xff) as u8,
        g: ((v >> 8) & 0xff) as u8,
        b: (v & 0xff) as u8,
    }
}

/// Translate an nvim highlight (Style) into Rio's interned cell style.
/// Falls back to the editor's default colors for any channel nvim left
/// unset for this hl id (its convention: only emit deltas from default).
fn nvim_style_to_rio(nvim: &Style, default_colors: &Colors) -> RioStyle {
    let fg_color = nvim.colors.foreground.or(default_colors.foreground);
    let bg_color = nvim.colors.background.or(default_colors.background);

    let fg = match fg_color {
        Some(c) => AnsiColor::Spec(packed_to_rgb(c)),
        None => RioStyle::default().fg,
    };
    let bg = match bg_color {
        Some(c) => AnsiColor::Spec(packed_to_rgb(c)),
        None => RioStyle::default().bg,
    };
    let underline_color = nvim
        .colors
        .special
        .map(|c| AnsiColor::Spec(packed_to_rgb(c)));

    let mut flags = StyleFlags::empty();
    if nvim.bold {
        flags |= StyleFlags::BOLD;
    }
    if nvim.italic {
        flags |= StyleFlags::ITALIC;
    }
    if nvim.strikethrough {
        flags |= StyleFlags::STRIKEOUT;
    }
    if nvim.reverse {
        flags |= StyleFlags::INVERSE;
    }
    match nvim.underline {
        UnderlineStyle::None => {}
        UnderlineStyle::Underline => flags |= StyleFlags::UNDERLINE,
        UnderlineStyle::UnderDouble => flags |= StyleFlags::DOUBLE_UNDERLINE,
        UnderlineStyle::UnderCurl => flags |= StyleFlags::UNDERCURL,
        UnderlineStyle::UnderDot => flags |= StyleFlags::DOTTED_UNDERLINE,
        UnderlineStyle::UnderDash => flags |= StyleFlags::DASHED_UNDERLINE,
    }

    RioStyle {
        fg,
        bg,
        underline_color,
        flags,
    }
}

/// Resolve an nvim highlight id → interned `StyleId` on the grid. `0`
/// is nvim's default-style sentinel — short-circuits to Rio's default.
fn resolve_style_id(
    crosswords: &mut Crosswords,
    hl_table: &HighlightTable,
    default_colors: &Colors,
    hl_id: u64,
) -> StyleId {
    if hl_id == 0 {
        let rio_style = nvim_style_to_rio(&Style::default(), default_colors);
        return crosswords.grid.style_set.intern(rio_style);
    }
    let Some(nvim_style) = hl_table.get(&hl_id) else {
        return DEFAULT_STYLE_ID;
    };
    let rio_style = nvim_style_to_rio(nvim_style, default_colors);
    crosswords.grid.style_set.intern(rio_style)
}

/// 0xRRGGBB → `[r, g, b, a]` in 0..=1 floats. Convenience for the
/// renderer side which works in sugarloaf's f32 colorspace.
pub fn unpack_color_f32(c: PackedColor) -> [f32; 4] {
    let v = c.0;
    let r = ((v >> 16) & 0xff) as f32 / 255.0;
    let g = ((v >> 8) & 0xff) as f32 / 255.0;
    let b = (v & 0xff) as f32 / 255.0;
    [r, g, b, 1.0]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> Value {
        Value::from(v)
    }

    #[test]
    fn unpack_white() {
        let [r, g, b, a] = unpack_color_f32(PackedColor(0xffffff));
        assert!((r - 1.0).abs() < 1e-6);
        assert!((g - 1.0).abs() < 1e-6);
        assert!((b - 1.0).abs() < 1e-6);
        assert_eq!(a, 1.0);
    }

    #[test]
    fn parses_flush() {
        let batch = Value::Array(vec![s("flush"), Value::Array(vec![])]);
        let events = parse_redraw_batch(batch).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], RedrawEvent::Flush));
    }

    #[test]
    fn parses_grid_resize() {
        let batch = Value::Array(vec![
            s("grid_resize"),
            Value::Array(vec![
                Value::from(1u64),
                Value::from(80u64),
                Value::from(24u64),
            ]),
        ]);
        let events = parse_redraw_batch(batch).unwrap();
        match &events[0] {
            RedrawEvent::Resize {
                grid,
                width,
                height,
            } => {
                assert_eq!((*grid, *width, *height), (1, 80, 24));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_grid_line_with_repeat() {
        // Cell layout: [text, hl_id, repeat]. nvim sends only the
        // fields it needs to (1, 2, or 3 entries).
        let cell_text_only = Value::Array(vec![s("a")]);
        let cell_text_hl = Value::Array(vec![s("b"), Value::from(7u64)]);
        let cell_text_hl_rep =
            Value::Array(vec![s("c"), Value::from(7u64), Value::from(3u64)]);
        let batch = Value::Array(vec![
            s("grid_line"),
            Value::Array(vec![
                Value::from(1u64),
                Value::from(0u64),
                Value::from(0u64),
                Value::Array(vec![cell_text_only, cell_text_hl, cell_text_hl_rep]),
            ]),
        ]);
        let events = parse_redraw_batch(batch).unwrap();
        match &events[0] {
            RedrawEvent::GridLine {
                grid,
                row,
                column_start,
                cells,
            } => {
                assert_eq!((*grid, *row, *column_start), (1, 0, 0));
                assert_eq!(cells.len(), 3);
                assert_eq!(cells[0].text, "a");
                assert_eq!(cells[0].highlight_id, None);
                assert_eq!(cells[1].highlight_id, Some(7));
                assert_eq!(cells[2].repeat, Some(3));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_cursor_goto_clamps_negative() {
        // nvim has shipped occasional -1s during shutdown; clamp to 0.
        let batch = Value::Array(vec![
            s("grid_cursor_goto"),
            Value::Array(vec![
                Value::from(1u64),
                Value::from(-1i64),
                Value::from(5u64),
            ]),
        ]);
        let events = parse_redraw_batch(batch).unwrap();
        match &events[0] {
            RedrawEvent::CursorGoto { row, column, .. } => {
                assert_eq!((*row, *column), (0, 5));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn unknown_event_skipped() {
        let batch = Value::Array(vec![
            s("made_up_event"),
            Value::Array(vec![Value::from(1u64)]),
        ]);
        let events = parse_redraw_batch(batch).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn parses_popupmenu_show() {
        let item = Value::Array(vec![s("println"), s("Function"), s("std"), s("macro")]);
        let batch = Value::Array(vec![
            s("popupmenu_show"),
            Value::Array(vec![
                Value::Array(vec![item]),
                Value::from(0i64),
                Value::from(4u64),
                Value::from(12u64),
                Value::from(1u64),
            ]),
        ]);
        let events = parse_redraw_batch(batch).unwrap();
        match &events[0] {
            RedrawEvent::PopupMenuShow { menu } => {
                assert_eq!(menu.selected, 0);
                assert_eq!((menu.row, menu.col, menu.grid), (4, 12, 1));
                assert_eq!(menu.items[0].word, "println");
                assert_eq!(menu.items[0].kind, "Function");
                assert_eq!(menu.items[0].info, "");
                assert_eq!(menu.max_word_chars, "println".len());
                assert_eq!(menu.selected_index(), Some(0));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parse_default_colors_three_gui_two_term() {
        let batch = Value::Array(vec![
            s("default_colors_set"),
            Value::Array(vec![
                Value::from(0xffffffu64),
                Value::from(0u64),
                Value::from(0xff0000u64),
                Value::from(0u64),
                Value::from(0u64),
            ]),
        ]);
        let events = parse_redraw_batch(batch).unwrap();
        match &events[0] {
            RedrawEvent::DefaultColorsSet { colors } => {
                assert_eq!(colors.foreground.unwrap().0, 0xffffff);
                assert_eq!(colors.background.unwrap().0, 0);
                assert_eq!(colors.special.unwrap().0, 0xff0000);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_mode_change() {
        let batch = Value::Array(vec![
            s("mode_change"),
            Value::Array(vec![s("insert"), Value::from(2u64)]),
        ]);
        let events = parse_redraw_batch(batch).unwrap();
        match &events[0] {
            RedrawEvent::ModeChange { mode, mode_index } => {
                assert_eq!(*mode, EditorMode::Insert);
                assert_eq!(*mode_index, 2);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_hl_attr_define_with_styles() {
        let attr_map = Value::Map(vec![
            (s("foreground"), Value::from(0xffaa33u64)),
            (s("bold"), Value::Boolean(true)),
            (s("italic"), Value::Boolean(true)),
            (s("undercurl"), Value::Boolean(true)),
        ]);
        let batch = Value::Array(vec![
            s("hl_attr_define"),
            Value::Array(vec![
                Value::from(5u64),
                attr_map,
                Value::Map(vec![]),
                Value::Array(vec![]),
            ]),
        ]);
        let events = parse_redraw_batch(batch).unwrap();
        match &events[0] {
            RedrawEvent::HighlightAttributesDefine { id, style } => {
                assert_eq!(*id, 5);
                assert_eq!(style.colors.foreground.unwrap().0, 0xffaa33);
                assert!(style.bold);
                assert!(style.italic);
                assert_eq!(style.underline, UnderlineStyle::UnderCurl);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn apply_grid_line_writes_chars_into_grid() {
        use neoism_terminal_core::crosswords::pos::CursorState;
        use neoism_terminal_core::crosswords::Crosswords;
        // 5x2 grid, default cursor.
        let dims = (2usize, 5usize); // (lines, columns) per the test impl
        let mut term = Crosswords::new(
            dims,
            CursorState::default().content,
            neoism_terminal_core::TerminalId::new(0),
            0,
        );
        term.reset_damage();

        let events = vec![
            RedrawEvent::GridLine {
                grid: 1,
                row: 0,
                column_start: 1,
                cells: vec![
                    GridLineCell {
                        text: "x".into(),
                        highlight_id: Some(0),
                        repeat: None,
                    },
                    GridLineCell {
                        text: "y".into(),
                        highlight_id: None,
                        repeat: Some(2),
                    },
                ],
            },
            RedrawEvent::CursorGoto {
                grid: 1,
                row: 1,
                column: 3,
            },
            RedrawEvent::Flush,
        ];

        let mut hl = HighlightTable::new();
        let mut colors = Colors::default();
        let mut title = None;
        let n =
            apply_redraw_events(&mut term, &mut hl, &mut colors, &mut title, &events, 1);
        assert_eq!(n, 2, "grid and cursor events should apply");

        // Verify grid mutation: row 0 cols 1..=3 = 'x','y','y'.
        assert_eq!(term.grid[Line(0)][Column(1)].c(), 'x');
        assert_eq!(term.grid[Line(0)][Column(2)].c(), 'y');
        assert_eq!(term.grid[Line(0)][Column(3)].c(), 'y');
        // Cursor moved to (1, 3).
        assert_eq!(term.grid.cursor.pos.row, Line(1));
        assert_eq!(term.grid.cursor.pos.col, Column(3));
        // Flush is only a batch boundary; the grid_line carries row damage.
        match term.peek_damage_event() {
            Some(neoism_terminal_core::damage::TerminalDamage::Partial(lines)) => {
                assert!(lines.iter().any(|line| line.line == 0));
            }
            other => panic!("expected partial row damage, got {other:?}"),
        }
    }

    #[test]
    fn apply_grid_scroll_damages_scrolled_rect_rows() {
        use neoism_terminal_core::crosswords::pos::CursorState;
        use neoism_terminal_core::crosswords::Crosswords;
        let mut term = Crosswords::new(
            (4usize, 3usize),
            CursorState::default().content,
            neoism_terminal_core::TerminalId::new(0),
            0,
        );
        for row in 0..4 {
            term.grid[Line(row)][Column(0)].set_c(char::from(b'0' + row as u8));
        }
        term.reset_damage();

        let events = vec![RedrawEvent::Scroll {
            grid: 1,
            top: 0,
            bottom: 4,
            left: 0,
            right: 3,
            rows: 1,
            columns: 0,
        }];
        let mut hl = HighlightTable::new();
        let mut colors = Colors::default();
        let mut title = None;
        let n =
            apply_redraw_events(&mut term, &mut hl, &mut colors, &mut title, &events, 1);

        assert_eq!(n, 1);
        assert_eq!(term.grid[Line(0)][Column(0)].c(), '1');
        assert_eq!(term.grid[Line(1)][Column(0)].c(), '2');
        assert_eq!(term.grid[Line(2)][Column(0)].c(), '3');
        // Every shifted CPU-grid row is damaged. The retained-grid copy is an
        // optimization and can be skipped when animation offsets cancel.
        match term.peek_damage_event() {
            Some(neoism_terminal_core::damage::TerminalDamage::Partial(lines)) => {
                assert_eq!(lines.len(), 4);
                for row in 0..4 {
                    assert!(lines.iter().any(|line| line.line == row));
                }
            }
            other => panic!("expected shifted-row damage, got {other:?}"),
        }
    }

    #[test]
    fn apply_grid_scroll_full_span_clears_and_damages_rect() {
        use neoism_terminal_core::crosswords::pos::CursorState;
        use neoism_terminal_core::crosswords::Crosswords;
        let mut term = Crosswords::new(
            (4usize, 3usize),
            CursorState::default().content,
            neoism_terminal_core::TerminalId::new(0),
            0,
        );
        for row in 0..4 {
            term.grid[Line(row)][Column(0)].set_c(char::from(b'0' + row as u8));
        }
        term.reset_damage();

        // rows == span: full-rect scroll, every cell must be blanked so
        // pre-scroll glyphs cannot "ghost" through a delayed grid_line.
        let events = vec![RedrawEvent::Scroll {
            grid: 1,
            top: 0,
            bottom: 4,
            left: 0,
            right: 3,
            rows: 4,
            columns: 0,
        }];
        let mut hl = HighlightTable::new();
        let mut colors = Colors::default();
        let mut title = None;
        let n =
            apply_redraw_events(&mut term, &mut hl, &mut colors, &mut title, &events, 1);

        assert_eq!(n, 1, "full-span scroll must register as applied work");
        for row in 0..4 {
            assert_eq!(
                term.grid[Line(row)][Column(0)].c(),
                '\0',
                "row {row} should be blank after full-span scroll",
            );
        }
        match term.peek_damage_event() {
            Some(neoism_terminal_core::damage::TerminalDamage::Partial(lines)) => {
                assert_eq!(lines.len(), 4);
                for row in 0..4 {
                    assert!(lines.iter().any(|line| line.line == row));
                }
            }
            other => panic!("expected full rect row damage, got {other:?}"),
        }
    }

    #[test]
    fn apply_grid_scroll_blanks_freed_rows_on_partial_shift() {
        use neoism_terminal_core::crosswords::pos::CursorState;
        use neoism_terminal_core::crosswords::Crosswords;
        let mut term = Crosswords::new(
            (4usize, 3usize),
            CursorState::default().content,
            neoism_terminal_core::TerminalId::new(0),
            0,
        );
        for row in 0..4 {
            term.grid[Line(row)][Column(0)].set_c(char::from(b'0' + row as u8));
        }
        term.reset_damage();

        // rows=2, span=4: cells move up by 2; freed rows [2,4) hold the
        // PRE-scroll '2','3' until a follow-up grid_line repaints them.
        // We blank them so that if grid_line slips into a later batch the
        // user does not see stale '2','3' as ghost lines for a frame.
        let events = vec![RedrawEvent::Scroll {
            grid: 1,
            top: 0,
            bottom: 4,
            left: 0,
            right: 3,
            rows: 2,
            columns: 0,
        }];
        let mut hl = HighlightTable::new();
        let mut colors = Colors::default();
        let mut title = None;
        let n =
            apply_redraw_events(&mut term, &mut hl, &mut colors, &mut title, &events, 1);

        assert_eq!(n, 1);
        assert_eq!(term.grid[Line(0)][Column(0)].c(), '2');
        assert_eq!(term.grid[Line(1)][Column(0)].c(), '3');
        assert_eq!(term.grid[Line(2)][Column(0)].c(), '\0');
        assert_eq!(term.grid[Line(3)][Column(0)].c(), '\0');
    }

    #[test]
    fn apply_grid_line_skips_stale_row_after_resize() {
        use neoism_terminal_core::crosswords::pos::CursorState;
        use neoism_terminal_core::crosswords::Crosswords;
        let mut term = Crosswords::new(
            (2usize, 5usize),
            CursorState::default().content,
            neoism_terminal_core::TerminalId::new(0),
            0,
        );

        let events = vec![
            RedrawEvent::Resize {
                grid: 1,
                width: 5,
                height: 1,
            },
            RedrawEvent::GridLine {
                grid: 1,
                row: 1,
                column_start: 0,
                cells: vec![GridLineCell {
                    text: "x".into(),
                    highlight_id: None,
                    repeat: None,
                }],
            },
        ];

        let mut hl = HighlightTable::new();
        let mut colors = Colors::default();
        let mut title = None;
        let n =
            apply_redraw_events(&mut term, &mut hl, &mut colors, &mut title, &events, 1);

        assert_eq!(n, 1, "resize applies, stale row is ignored");
        assert_eq!(term.screen_lines(), 1);
        assert_eq!(term.grid[Line(0)][Column(0)].c(), '\0');
    }

    #[test]
    fn apply_grid_line_interns_style_from_hl_table() {
        use neoism_terminal_core::crosswords::pos::CursorState;
        use neoism_terminal_core::crosswords::Crosswords;
        let mut term = Crosswords::new(
            (1usize, 4usize),
            CursorState::default().content,
            neoism_terminal_core::TerminalId::new(0),
            0,
        );

        let mut hl = HighlightTable::new();
        // hl 9 = bold, custom fg.
        hl.insert(
            9,
            Style {
                colors: Colors {
                    foreground: Some(PackedColor(0xff0000)),
                    ..Default::default()
                },
                bold: true,
                ..Default::default()
            },
        );

        let events = vec![RedrawEvent::GridLine {
            grid: 1,
            row: 0,
            column_start: 0,
            cells: vec![GridLineCell {
                text: "Q".into(),
                highlight_id: Some(9),
                repeat: None,
            }],
        }];

        let mut colors = Colors::default();
        let mut title = None;
        apply_redraw_events(&mut term, &mut hl, &mut colors, &mut title, &events, 1);

        // The cell carries a non-default style id.
        let sid = term.grid[Line(0)][Column(0)].style_id();
        assert_ne!(sid, DEFAULT_STYLE_ID, "hl 9 should produce a fresh id");

        // And that id resolves back to a Style with bold + red fg.
        let resolved = term.grid.style_set.get(sid);
        assert!(resolved.flags.contains(StyleFlags::BOLD));
        match resolved.fg {
            AnsiColor::Spec(rgb) => assert_eq!((rgb.r, rgb.g, rgb.b), (0xff, 0, 0)),
            other => panic!("expected truecolor fg, got {other:?}"),
        }
    }

    #[test]
    fn rejects_completely_malformed_batch() {
        // First element must be a string event name.
        let batch = Value::Array(vec![Value::from(42u64)]);
        assert!(parse_redraw_batch(batch).is_err());
    }

    // Suppress unused-warning for parse_bool while we wait for events
    // that consume it (e.g. option_set in Phase 2e+).
    #[test]
    fn _bool_parser_compiles() {
        assert!(parse_bool(Value::Boolean(true)).unwrap());
        assert!(!parse_bool(Value::Boolean(false)).unwrap());
    }
}
