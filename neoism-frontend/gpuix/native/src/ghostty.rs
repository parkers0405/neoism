//! libghostty-vt adapter. Ghostty owns parser, screen, scrollback, modes,
//! Unicode, selection, search and keyboard/mouse protocol encoding. GPUIX paints.
#[allow(
    non_upper_case_globals,
    non_camel_case_types,
    non_snake_case,
    dead_code
)]
mod ffi {
    include!("ghostty_bindings.rs");
}
use ffi::*;
use neoism_ui::{
    panels::trail_cursor::TrailCursor, terminal_scroll_state::TerminalScroll,
};
use serde_json::{json, Value};
use std::{
    cell::RefCell, collections::HashMap, ffi::c_void, ptr, rc::Rc, slice, time::Instant,
};
use unicode_segmentation::UnicodeSegmentation;
fn n(v: &Value, k: &str, d: f32) -> f32 {
    v[k].as_f64().map(|v| v as f32).unwrap_or(d)
}
fn text<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}
fn flag(v: &Value, k: &str) -> bool {
    v[k].as_bool().unwrap_or(false)
}
fn check(result: GhosttyResult) -> Result<(), String> {
    if result == GHOSTTY_SUCCESS {
        Ok(())
    } else {
        Err(format!("libghostty-vt operation failed ({result})"))
    }
}
unsafe fn sized<T>() -> T {
    let mut value: T = std::mem::zeroed();
    *(&mut value as *mut T).cast::<usize>() = std::mem::size_of::<T>();
    value
}
fn rgb(value: GhosttyColorRgb) -> Value {
    json!({"Spec":{"r":value.r,"g":value.g,"b":value.b}})
}
unsafe extern "C" fn write_pty(
    _: GhosttyTerminal,
    userdata: *mut c_void,
    data: *const u8,
    len: usize,
) {
    if !userdata.is_null() && !data.is_null() {
        (*(userdata as *mut Effects))
            .writes
            .extend_from_slice(slice::from_raw_parts(data, len));
    }
}
struct Effects {
    api: Rc<GhosttyApi>,
    writes: Vec<u8>,
    dark: bool,
}
unsafe extern "C" fn device_attributes(
    _: GhosttyTerminal,
    _: *mut c_void,
    attributes: *mut GhosttyDeviceAttributes,
) -> bool {
    if attributes.is_null() {
        return false;
    }
    let mut value = GhosttyDeviceAttributes::default();
    // Ghostty identifies as VT220. Advertise color; this embedding does not
    // install clipboard-access effects and therefore does not advertise them.
    value.primary.conformance_level = 62;
    value.primary.features[0] = 22;
    value.primary.num_features = 1;
    value.secondary.device_type = 1;
    value.secondary.firmware_version = 10;
    *attributes = value;
    true
}
unsafe extern "C" fn report_size(
    terminal: GhosttyTerminal,
    userdata: *mut c_void,
    size: *mut GhosttySizeReportSize,
) -> bool {
    if userdata.is_null() || size.is_null() {
        return false;
    }
    let effects = &*(userdata as *const Effects);
    let mut rows = 0u16;
    let mut columns = 0u16;
    let mut width = 0u32;
    let mut height = 0u32;
    effects.api.ghostty_terminal_get(
        terminal,
        GHOSTTY_TERMINAL_DATA_ROWS,
        (&mut rows as *mut u16).cast(),
    );
    effects.api.ghostty_terminal_get(
        terminal,
        GHOSTTY_TERMINAL_DATA_COLS,
        (&mut columns as *mut u16).cast(),
    );
    effects.api.ghostty_terminal_get(
        terminal,
        GHOSTTY_TERMINAL_DATA_WIDTH_PX,
        (&mut width as *mut u32).cast(),
    );
    effects.api.ghostty_terminal_get(
        terminal,
        GHOSTTY_TERMINAL_DATA_HEIGHT_PX,
        (&mut height as *mut u32).cast(),
    );
    *size = GhosttySizeReportSize {
        rows,
        columns,
        cell_width: width / columns.max(1) as u32,
        cell_height: height / rows.max(1) as u32,
    };
    true
}
unsafe extern "C" fn color_scheme(
    _: GhosttyTerminal,
    userdata: *mut c_void,
    scheme: *mut GhosttyColorScheme,
) -> bool {
    if userdata.is_null() || scheme.is_null() {
        return false;
    }
    *scheme = if (*(userdata as *const Effects)).dark {
        GHOSTTY_COLOR_SCHEME_DARK
    } else {
        GHOSTTY_COLOR_SCHEME_LIGHT
    };
    true
}
#[derive(Default)]
struct Store {
    api: Option<Rc<GhosttyApi>>,
    next: u32,
    terminals: HashMap<u32, Terminal>,
}
type SearchMatch = ((u16, u32), (u16, u32));
thread_local! {static STORE:RefCell<Store>=RefCell::new(Store::default());}
struct Terminal {
    api: Rc<GhosttyApi>,
    inner: GhosttyTerminal,
    render: GhosttyRenderState,
    rows_iter: GhosttyRenderStateRowIterator,
    cells: GhosttyRenderStateRowCells,
    key_encoder: GhosttyKeyEncoder,
    key_event: GhosttyKeyEvent,
    mouse_encoder: GhosttyMouseEncoder,
    mouse_event: GhosttyMouseEvent,
    effects: Box<Effects>,
    scroll: TerminalScroll,
    trail: TrailCursor,
    last_frame: Instant,
    last_cursor: (usize, i32),
    vi: Option<(u16, u32)>,
    selection_anchor: GhosttyTrackedGridRef,
    selection_side: bool,
    selection_kind: u8,
    search: GhosttySearch,
    pattern: String,
    search_forward: bool,
    search_origin: Option<(u16, u32, u64)>,
    preview: Option<String>,
    preview_forward: bool,
    revision: u64,
    search_cache: RefCell<Option<(u64, String, Vec<SearchMatch>)>>,
    cell_cache: RefCell<Option<(u64, Rc<Vec<Option<char>>>)>>,
    compact: bool,
}
impl Drop for Terminal {
    fn drop(&mut self) {
        unsafe {
            let a = &self.api;
            a.ghostty_tracked_grid_ref_free(self.selection_anchor);
            a.ghostty_search_free(self.search);
            a.ghostty_mouse_event_free(self.mouse_event);
            a.ghostty_mouse_encoder_free(self.mouse_encoder);
            a.ghostty_key_event_free(self.key_event);
            a.ghostty_key_encoder_free(self.key_encoder);
            a.ghostty_render_state_row_cells_free(self.cells);
            a.ghostty_render_state_row_iterator_free(self.rows_iter);
            a.ghostty_render_state_free(self.render);
            a.ghostty_terminal_free(self.inner);
        }
    }
}
impl Terminal {
    unsafe fn new(api: Rc<GhosttyApi>, cols: u16, rows: u16) -> Result<Self, String> {
        let mut t = Self {
            api: api.clone(),
            inner: ptr::null_mut(),
            render: ptr::null_mut(),
            rows_iter: ptr::null_mut(),
            cells: ptr::null_mut(),
            key_encoder: ptr::null_mut(),
            key_event: ptr::null_mut(),
            mouse_encoder: ptr::null_mut(),
            mouse_event: ptr::null_mut(),
            effects: Box::new(Effects {
                api: api.clone(),
                writes: Vec::new(),
                dark: true,
            }),
            scroll: TerminalScroll::new(),
            trail: TrailCursor::new(),
            last_frame: Instant::now(),
            last_cursor: (0, 0),
            vi: None,
            selection_anchor: ptr::null_mut(),
            selection_side: false,
            selection_kind: 0,
            search: ptr::null_mut(),
            pattern: String::new(),
            search_forward: true,
            search_origin: None,
            preview: None,
            preview_forward: true,
            revision: 0,
            search_cache: RefCell::new(None),
            cell_cache: RefCell::new(None),
            compact: false,
        };
        let a = &t.api;
        check(a.ghostty_terminal_new(ptr::null(), &mut t.inner, cols, rows))?;
        check(a.ghostty_terminal_resize(t.inner, cols, rows, 10, 20))?;
        // Match Ghostty's application default (`grapheme-width-method=unicode`),
        // rather than the deliberately conservative bare VT-library default.
        let grapheme_mode = GhosttyTerminalModeConfig {
            mode: 2027,
            value: true,
        };
        check(a.ghostty_terminal_set(
            t.inner,
            GHOSTTY_TERMINAL_OPT_MODE_DEFAULT,
            (&grapheme_mode as *const GhosttyTerminalModeConfig).cast(),
        ))?;
        let max_lines = 2000usize;
        check(a.ghostty_terminal_set(
            t.inner,
            GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES,
            (&max_lines as *const usize).cast(),
        ))?;
        check(a.ghostty_render_state_new(ptr::null(), &mut t.render))?;
        check(a.ghostty_render_state_row_iterator_new(ptr::null(), &mut t.rows_iter))?;
        check(a.ghostty_render_state_row_cells_new(ptr::null(), &mut t.cells))?;
        check(a.ghostty_key_encoder_new(ptr::null(), &mut t.key_encoder))?;
        check(a.ghostty_key_event_new(ptr::null(), &mut t.key_event))?;
        check(a.ghostty_mouse_encoder_new(ptr::null(), &mut t.mouse_encoder))?;
        check(a.ghostty_mouse_event_new(ptr::null(), &mut t.mouse_event))?;
        check(a.ghostty_search_new(ptr::null(), &mut t.search, t.inner))?;
        check(a.ghostty_terminal_set(
            t.inner,
            GHOSTTY_TERMINAL_OPT_USERDATA,
            (&mut *t.effects as *mut Effects).cast(),
        ))?;
        check(a.ghostty_terminal_set(
            t.inner,
            GHOSTTY_TERMINAL_OPT_WRITE_PTY,
            write_pty as *const () as *const c_void,
        ))?;
        for (option, callback) in [
            (
                GHOSTTY_TERMINAL_OPT_DEVICE_ATTRIBUTES,
                device_attributes as *const (),
            ),
            (GHOSTTY_TERMINAL_OPT_SIZE, report_size as *const ()),
            (GHOSTTY_TERMINAL_OPT_COLOR_SCHEME, color_scheme as *const ()),
        ] {
            check(a.ghostty_terminal_set(t.inner, option, callback.cast()))?;
        }
        Ok(t)
    }
    unsafe fn configure_colors(&mut self, v: &Value) {
        let parse = |value: &str| -> Option<GhosttyColorRgb> {
            let value = value.strip_prefix('#')?;
            if value.len() < 6 {
                return None;
            }
            Some(GhosttyColorRgb {
                r: u8::from_str_radix(&value[0..2], 16).ok()?,
                g: u8::from_str_radix(&value[2..4], 16).ok()?,
                b: u8::from_str_radix(&value[4..6], 16).ok()?,
            })
        };
        if let Some(background) = parse(text(v, "background")) {
            self.effects.dark = (background.r as f32 * 0.2126
                + background.g as f32 * 0.7152
                + background.b as f32 * 0.0722)
                < 128.0;
        }
        let mut palette = [GhosttyColorRgb::default(); 256];
        self.api.ghostty_terminal_get(
            self.inner,
            GHOSTTY_TERMINAL_DATA_COLOR_PALETTE,
            palette.as_mut_ptr().cast(),
        );
        for (index, value) in v["palette"]
            .as_array()
            .into_iter()
            .flatten()
            .take(256)
            .enumerate()
        {
            if let Some(color) = value.as_str().and_then(parse) {
                palette[index] = color;
            }
        }
        self.api.ghostty_terminal_set(
            self.inner,
            GHOSTTY_TERMINAL_OPT_COLOR_PALETTE,
            palette.as_ptr().cast(),
        );
        for (name, option) in [
            ("foreground", GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND),
            ("background", GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND),
        ] {
            if let Some(color) = parse(text(v, name)) {
                self.api.ghostty_terminal_set(
                    self.inner,
                    option,
                    (&color as *const GhosttyColorRgb).cast(),
                );
            }
        }
    }
    unsafe fn get<T: Default>(&self, key: GhosttyTerminalData) -> T {
        let mut value = T::default();
        self.api
            .ghostty_terminal_get(self.inner, key, (&mut value as *mut T).cast());
        value
    }
    unsafe fn mode(&self, mode: u16) -> bool {
        let mut value = GhosttyTerminalModeConfig { mode, value: false };
        self.api.ghostty_terminal_get(
            self.inner,
            GHOSTTY_TERMINAL_DATA_MODE,
            (&mut value as *mut GhosttyTerminalModeConfig).cast(),
        );
        value.value
    }
    unsafe fn scrollbar(&self) -> GhosttyTerminalScrollbar {
        self.get(GHOSTTY_TERMINAL_DATA_SCROLLBAR)
    }
    unsafe fn point(
        &self,
        tag: GhosttyPointTag,
        x: u16,
        y: u32,
    ) -> Result<GhosttyGridRef, String> {
        let mut reference = sized::<GhosttyGridRef>();
        check(self.api.ghostty_terminal_grid_ref(
            self.inner,
            GhosttyPoint {
                tag,
                value: GhosttyPointValue {
                    coordinate: GhosttyPointCoordinate { x, y },
                },
            },
            &mut reference,
        ))?;
        Ok(reference)
    }
    unsafe fn viewport_point(
        &self,
        reference: &GhosttyGridRef,
    ) -> Option<GhosttyPointCoordinate> {
        let mut point = GhosttyPointCoordinate::default();
        (self.api.ghostty_terminal_point_from_grid_ref(
            self.inner,
            reference,
            GHOSTTY_POINT_TAG_VIEWPORT,
            &mut point,
        ) == GHOSTTY_SUCCESS)
            .then_some(point)
    }
    unsafe fn selection(&self) -> Option<GhosttySelection> {
        let mut value = sized::<GhosttySelection>();
        (self.api.ghostty_terminal_get(
            self.inner,
            GHOSTTY_TERMINAL_DATA_SELECTION,
            (&mut value as *mut GhosttySelection).cast(),
        ) == GHOSTTY_SUCCESS)
            .then_some(value)
    }
    unsafe fn set_selection(&self, selection: Option<&GhosttySelection>) {
        self.api.ghostty_terminal_set(
            self.inner,
            GHOSTTY_TERMINAL_OPT_SELECTION,
            selection
                .map(|s| (s as *const GhosttySelection).cast())
                .unwrap_or(ptr::null()),
        );
    }
    unsafe fn selected_text(&self) -> Option<String> {
        let mut options = sized::<GhosttyTerminalSelectionFormatOptions>();
        options.emit = GHOSTTY_FORMATTER_FORMAT_PLAIN;
        options.unwrap = true;
        options.trim = true;
        let mut len = 0;
        let result = self.api.ghostty_terminal_selection_format_buf(
            self.inner,
            options,
            ptr::null_mut(),
            0,
            &mut len,
        );
        if result == GHOSTTY_NO_VALUE {
            return None;
        }
        if len == 0 {
            return Some(String::new());
        }
        let mut bytes = vec![0; len];
        if self.api.ghostty_terminal_selection_format_buf(
            self.inner,
            options,
            bytes.as_mut_ptr(),
            bytes.len(),
            &mut len,
        ) != GHOSTTY_SUCCESS
        {
            return None;
        }
        bytes.truncate(len);
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }
    unsafe fn scroll_rows(&self, delta: isize) {
        self.api.ghostty_terminal_scroll_viewport(
            self.inner,
            GhosttyTerminalScrollViewport {
                tag: GHOSTTY_SCROLL_VIEWPORT_DELTA,
                value: GhosttyTerminalScrollViewportValue { delta },
            },
        );
    }
    unsafe fn bottom(&mut self) {
        self.api.ghostty_terminal_scroll_viewport(
            self.inner,
            GhosttyTerminalScrollViewport {
                tag: GHOSTTY_SCROLL_VIEWPORT_BOTTOM,
                value: GhosttyTerminalScrollViewportValue::default(),
            },
        );
        self.scroll.reset_wheel(0);
    }
    unsafe fn frame(&mut self) -> Result<Value, String> {
        if self.compact {
            return self.paint_frame();
        }
        let a = &self.api;
        check(a.ghostty_render_state_update(self.render, self.inner))?;
        check(a.ghostty_render_state_get(
            self.render,
            GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
            (&mut self.rows_iter as *mut GhosttyRenderStateRowIterator).cast(),
        ))?;
        let cols: u16 = self.get(GHOSTTY_TERMINAL_DATA_COLS);
        let rows: u16 = self.get(GHOSTTY_TERMINAL_DATA_ROWS);
        let effective_foreground: GhosttyColorRgb =
            self.get(GHOSTTY_TERMINAL_DATA_COLOR_FOREGROUND);
        let default_foreground: GhosttyColorRgb =
            self.get(GHOSTTY_TERMINAL_DATA_COLOR_FOREGROUND_DEFAULT);
        let effective_background: GhosttyColorRgb =
            self.get(GHOSTTY_TERMINAL_DATA_COLOR_BACKGROUND);
        let default_background: GhosttyColorRgb =
            self.get(GHOSTTY_TERMINAL_DATA_COLOR_BACKGROUND_DEFAULT);
        let changed =
            |a: GhosttyColorRgb, b: GhosttyColorRgb| (a.r, a.g, a.b) != (b.r, b.g, b.b);
        let plain_foreground = if changed(effective_foreground, default_foreground) {
            rgb(effective_foreground)
        } else {
            json!("Default")
        };
        let plain_background = if changed(effective_background, default_background) {
            rgb(effective_background)
        } else {
            json!("Default")
        };
        let mut viewport = Vec::new();
        let mut graphemes = serde_json::Map::new();
        let mut selection_rows = Vec::new();
        while a.ghostty_render_state_row_iterator_next(self.rows_iter) {
            let y = viewport.len();
            let mut selection = sized::<GhosttyRenderStateRowSelection>();
            if a.ghostty_render_state_row_get(
                self.rows_iter,
                GHOSTTY_RENDER_STATE_ROW_DATA_SELECTION,
                (&mut selection as *mut GhosttyRenderStateRowSelection).cast(),
            ) == GHOSTTY_SUCCESS
            {
                selection_rows.push(
                    json!({"row":y,"start":selection.start_x,"end":selection.end_x}),
                );
            }
            check(a.ghostty_render_state_row_get(
                self.rows_iter,
                GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
                (&mut self.cells as *mut GhosttyRenderStateRowCells).cast(),
            ))?;
            let mut line = Vec::new();
            while a.ghostty_render_state_row_cells_next(self.cells) {
                let x = line.len();
                let mut style = sized::<GhosttyStyle>();
                a.ghostty_render_state_row_cells_get(
                    self.cells,
                    GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE,
                    (&mut style as *mut GhosttyStyle).cast(),
                );
                let mut raw = 0;
                a.ghostty_render_state_row_cells_get(
                    self.cells,
                    GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW,
                    (&mut raw as *mut GhosttyCell).cast(),
                );
                let mut wide = GHOSTTY_CELL_WIDE_NARROW;
                a.ghostty_cell_get(
                    raw,
                    GHOSTTY_CELL_DATA_WIDE,
                    (&mut wide as *mut GhosttyCellWide).cast(),
                );
                let mut length = 0u32;
                a.ghostty_render_state_row_cells_get(
                    self.cells,
                    GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_LEN,
                    (&mut length as *mut u32).cast(),
                );
                let mut codepoints = vec![0u32; length as usize];
                if length > 0 {
                    a.ghostty_render_state_row_cells_get(
                        self.cells,
                        GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_BUF,
                        codepoints.as_mut_ptr().cast(),
                    );
                }
                let cluster: String =
                    codepoints.into_iter().filter_map(char::from_u32).collect();
                let ch = cluster.chars().next().unwrap_or(' ');
                if cluster.chars().count() > 1 {
                    graphemes.insert(format!("{y}:{x}"), json!(cluster));
                }
                let fg = match style.fg_color.tag {
                    GHOSTTY_STYLE_COLOR_RGB => rgb(style.fg_color.value.rgb),
                    GHOSTTY_STYLE_COLOR_PALETTE => {
                        let mut color = GhosttyColorRgb::default();
                        if a.ghostty_render_state_row_cells_get(
                            self.cells,
                            GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR,
                            (&mut color as *mut GhosttyColorRgb).cast(),
                        ) == GHOSTTY_SUCCESS
                        {
                            rgb(color)
                        } else {
                            json!({"Indexed":style.fg_color.value.palette})
                        }
                    }
                    _ => plain_foreground.clone(),
                };
                let mut bg_color = GhosttyColorRgb::default();
                let bg = if a.ghostty_render_state_row_cells_get(
                    self.cells,
                    GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR,
                    (&mut bg_color as *mut GhosttyColorRgb).cast(),
                ) == GHOSTTY_SUCCESS
                {
                    rgb(bg_color)
                } else {
                    plain_background.clone()
                };
                let mut flags = Vec::new();
                if style.bold {
                    flags.push("BOLD");
                }
                if style.italic {
                    flags.push("ITALIC");
                }
                if style.inverse {
                    flags.push("REVERSE");
                }
                if style.invisible {
                    flags.push("HIDDEN");
                }
                if style.underline != 0 {
                    flags.push("UNDERLINE");
                }
                if wide == GHOSTTY_CELL_WIDE_WIDE {
                    flags.push("WIDE_CHAR");
                }
                if wide == GHOSTTY_CELL_WIDE_SPACER_TAIL
                    || wide == GHOSTTY_CELL_WIDE_SPACER_HEAD
                {
                    flags.push("WIDE_CHAR_SPACER");
                }
                line.push(
                    json!({"c":ch.to_string(),"fg":fg,"bg":bg,"flags":flags.join(" | ")}),
                );
            }
            viewport.push(line);
        }
        let mut cursor = sized::<GhosttyRenderStateCursor>();
        check(a.ghostty_render_state_get(
            self.render,
            GHOSTTY_RENDER_STATE_DATA_CURSOR,
            (&mut cursor as *mut GhosttyRenderStateCursor).cast(),
        ))?;
        let scrollbar = self.scrollbar();
        let display_offset = scrollbar
            .total
            .saturating_sub(scrollbar.len)
            .saturating_sub(scrollbar.offset);
        let (x, y) = if let Some((x, y)) = self.vi {
            (x as i32, y as i64 - scrollbar.offset as i64)
        } else {
            (cursor.viewport_x as i32, cursor.viewport_y as i64)
        };
        let shape = if self.vi.is_some() {
            "Block"
        } else {
            match cursor.visual_style {
                GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BAR => "Beam",
                GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_UNDERLINE => "Underline",
                _ => "Block",
            }
        };
        let selection = self.selection();
        let range = selection.and_then(|s| {
            let start = self.viewport_point(&s.start)?;
            let end = self.viewport_point(&s.end)?;
            let (start, end) = if (start.y, start.x) > (end.y, end.x) {
                (end, start)
            } else {
                (start, end)
            };
            Some(json!({"start":[start.y,start.x],"end":[end.y,end.x]}))
        });
        let selection_text = self.selected_text();
        let writes = std::mem::take(&mut self.effects.writes);
        Ok(
            json!({"engine":"libghostty-vt","snapshot":{"cols":cols,"rows":rows,"viewport":viewport,"display_offset":display_offset,
      "cursor":{"col":x,"row":y,"visible":self.vi.is_some()||cursor.visible&&cursor.viewport_has_value,"shape":shape},"alt_screen":self.get::<GhosttyTerminalScreen>(GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN)==GHOSTTY_TERMINAL_SCREEN_ALTERNATE},
      "writes":writes,"offset":self.scroll.current_offset(0),"viMode":self.vi.is_some(),"selectionBlock":selection.map(|s|s.rectangle).unwrap_or(false),"appCursor":self.mode(1),"bracketedPaste":self.mode(2004),"graphemes":graphemes,"selection":selection_text,"selectionRange":range,"selectionRows":selection_rows,"searchMatches":self.matches(self.preview.as_deref().unwrap_or(&self.pattern)).into_iter().filter_map(|(start,end)|{if start.1 as u64>=scrollbar.offset&&(end.1 as u64)<scrollbar.offset+scrollbar.len{Some(json!({"start":[start.1 as u64-scrollbar.offset,start.0],"end":[end.1 as u64-scrollbar.offset,end.0]}))}else{None}}).collect::<Vec<_>>()}),
        )
    }
    /// Paint runs cross the host boundary once per style span, rather than once
    /// per cell. Ghostty remains authoritative for graphemes, colors and marks.
    /// One render-state row as packed style runs (iterator already positioned).
    unsafe fn paint_row(
        &mut self,
        y: usize,
        matches: &[((usize, usize), (usize, usize))],
        palette: &mut [Option<u32>; 256],
        fg: u32,
        bg: u32,
    ) -> Result<Vec<Value>, String> {
        let a = &self.api;
        let pack = paint_pack;
            let mut selection = sized::<GhosttyRenderStateRowSelection>();
            let selected = a.ghostty_render_state_row_get(
                self.rows_iter,
                GHOSTTY_RENDER_STATE_ROW_DATA_SELECTION,
                (&mut selection as *mut GhosttyRenderStateRowSelection).cast(),
            ) == GHOSTTY_SUCCESS;
            check(a.ghostty_render_state_row_get(
                self.rows_iter,
                GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
                (&mut self.cells as *mut GhosttyRenderStateRowCells).cast(),
            ))?;
            let mut line = Vec::new();
            let mut run: Option<PaintRun> = None;
            let mut col = 0;
            while a.ghostty_render_state_row_cells_next(self.cells) {
                let x = col;
                col += 1;
                let mut style = sized::<GhosttyStyle>();
                a.ghostty_render_state_row_cells_get(
                    self.cells,
                    GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE,
                    (&mut style as *mut GhosttyStyle).cast(),
                );
                let mut raw = 0;
                a.ghostty_render_state_row_cells_get(
                    self.cells,
                    GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW,
                    (&mut raw as *mut GhosttyCell).cast(),
                );
                let mut wide = GHOSTTY_CELL_WIDE_NARROW;
                a.ghostty_cell_get(
                    raw,
                    GHOSTTY_CELL_DATA_WIDE,
                    (&mut wide as *mut GhosttyCellWide).cast(),
                );
                if wide == GHOSTTY_CELL_WIDE_SPACER_TAIL
                    || wide == GHOSTTY_CELL_WIDE_SPACER_HEAD
                {
                    continue;
                }
                let color =
                    |value: GhosttyStyleColor,
                     fallback,
                     option,
                     palette: &mut [Option<u32>; 256]| {
                        match value.tag {
                            GHOSTTY_STYLE_COLOR_RGB => pack(value.value.rgb),
                            GHOSTTY_STYLE_COLOR_PALETTE => {
                                let index = value.value.palette as usize;
                                *palette[index].get_or_insert_with(|| {
                                    let mut c = GhosttyColorRgb::default();
                                    if a.ghostty_render_state_row_cells_get(
                                        self.cells,
                                        option,
                                        (&mut c as *mut GhosttyColorRgb).cast(),
                                    ) == GHOSTTY_SUCCESS
                                    {
                                        pack(c)
                                    } else {
                                        0x2000000 | index as u32
                                    }
                                })
                            }
                            _ => fallback,
                        }
                    };
                let foreground = color(
                    style.fg_color,
                    fg,
                    GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR,
                    &mut *palette,
                );
                // Blank cells can encode a colored background in their content
                // tag rather than the style (e.g. erase-to-end after SGR).
                let mut resolved_bg = GhosttyColorRgb::default();
                let background = if a.ghostty_render_state_row_cells_get(
                    self.cells,
                    GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR,
                    (&mut resolved_bg as *mut GhosttyColorRgb).cast(),
                ) == GHOSTTY_SUCCESS
                {
                    pack(resolved_bg)
                } else {
                    color(
                        style.bg_color,
                        bg,
                        GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR,
                        &mut *palette,
                    )
                };
                // The default color on either side of inverse is the other
                // theme color, so preserve inverse as a flag for the TS host.
                let flags = u32::from(style.bold)
                    | (u32::from(style.italic) << 1)
                    | (u32::from(style.underline != 0) << 2)
                    | (u32::from(style.inverse) << 3);
                let mut mark = 0;
                if matches.iter().any(|(s, e)| {
                    (y > s.1 || y == s.1 && x >= s.0) && (y < e.1 || y == e.1 && x <= e.0)
                }) {
                    mark = 1;
                }
                if selected
                    && x >= selection.start_x as usize
                    && x <= selection.end_x as usize
                {
                    mark = 2;
                }
                let key = [foreground, background, flags, mark];
                if run.as_ref().is_some_and(|r| r.style != key) {
                    line.push(run.take().unwrap().value());
                }
                let current = run.get_or_insert_with(|| PaintRun {
                    text: String::new(),
                    col: x,
                    width: 0,
                    style: key,
                });
                current.width += if wide == GHOSTTY_CELL_WIDE_WIDE { 2 } else { 1 };
                let mut length = 0u32;
                a.ghostty_render_state_row_cells_get(
                    self.cells,
                    GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_LEN,
                    (&mut length as *mut u32).cast(),
                );
                if style.invisible || length == 0 {
                    current.text.push(' ');
                    continue;
                }
                let mut stack = [0u32; 32];
                let mut spill = Vec::new();
                let points = if length as usize <= stack.len() {
                    &mut stack[..length as usize]
                } else {
                    spill.resize(length as usize, 0);
                    spill.as_mut_slice()
                };
                a.ghostty_render_state_row_cells_get(
                    self.cells,
                    GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_BUF,
                    points.as_mut_ptr().cast(),
                );
                for point in points {
                    if let Some(c) = char::from_u32(*point) {
                        current.text.push(if c == '\0' { ' ' } else { c });
                    }
                }
            }
            if let Some(run) = run {
                line.push(run.value());
            }
        Ok(line)
    }

    /// While the wheel residual shifts the grid by part of a row, the row just
    /// past the viewport on the uncovered side, so no blank strip shows.
    unsafe fn peek_row(&mut self, above: bool, fg: u32, bg: u32) -> Result<Option<Vec<Value>>, String> {
        let bar = self.scrollbar();
        let available = if above { bar.offset > 0 } else { bar.offset + bar.len < bar.total };
        if !available {
            return Ok(None);
        }
        let shift: isize = if above { -1 } else { 1 };
        self.scroll_rows(shift);
        let mut palette: [Option<u32>; 256] = [None; 256];
        let mut row = None;
        let result = (|| -> Result<(), String> {
            check(self.api.ghostty_render_state_update(self.render, self.inner))?;
            check(self.api.ghostty_render_state_get(
                self.render,
                GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
                (&mut self.rows_iter as *mut GhosttyRenderStateRowIterator).cast(),
            ))?;
            let mut index = 0;
            while self.api.ghostty_render_state_row_iterator_next(self.rows_iter) {
                let last = index + 1 == bar.len as usize;
                if (above && index == 0) || (!above && last) {
                    row = Some(self.paint_row(index, &[], &mut palette, fg, bg)?);
                    break;
                }
                index += 1;
            }
            Ok(())
        })();
        self.scroll_rows(-shift);
        check(self.api.ghostty_render_state_update(self.render, self.inner))?;
        result.map(|()| row)
    }

    unsafe fn paint_frame(&mut self) -> Result<Value, String> {
        let api = self.api.clone();
        let a = &*api;
        check(a.ghostty_render_state_update(self.render, self.inner))?;
        check(a.ghostty_render_state_get(
            self.render,
            GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
            (&mut self.rows_iter as *mut GhosttyRenderStateRowIterator).cast(),
        ))?;
        let pack = |c: GhosttyColorRgb| {
            0x1000000 | ((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32
        };
        let default = |value: GhosttyColorRgb, base: GhosttyColorRgb| {
            if (value.r, value.g, value.b) == (base.r, base.g, base.b) {
                0
            } else {
                pack(value)
            }
        };
        let fg = default(
            self.get(GHOSTTY_TERMINAL_DATA_COLOR_FOREGROUND),
            self.get(GHOSTTY_TERMINAL_DATA_COLOR_FOREGROUND_DEFAULT),
        );
        let bg = default(
            self.get(GHOSTTY_TERMINAL_DATA_COLOR_BACKGROUND),
            self.get(GHOSTTY_TERMINAL_DATA_COLOR_BACKGROUND_DEFAULT),
        );
        let bar = self.scrollbar();
        let matches: Vec<_> = self
            .matches(self.preview.as_deref().unwrap_or(&self.pattern))
            .into_iter()
            .filter(|(s, e)| {
                s.1 as u64 >= bar.offset && (e.1 as u64) < bar.offset + bar.len
            })
            .map(|(s, e)| {
                (
                    (s.0 as usize, (s.1 as u64 - bar.offset) as usize),
                    (e.0 as usize, (e.1 as u64 - bar.offset) as usize),
                )
            })
            .collect();
        let mut palette: [Option<u32>; 256] = [None; 256];
        let mut rows = Vec::new();
        while self.api.ghostty_render_state_row_iterator_next(self.rows_iter) {
            let y = rows.len();
            rows.push(self.paint_row(y, &matches, &mut palette, fg, bg)?);
        }
        let mut cursor = sized::<GhosttyRenderStateCursor>();
        check(a.ghostty_render_state_get(
            self.render,
            GHOSTTY_RENDER_STATE_DATA_CURSOR,
            (&mut cursor as *mut GhosttyRenderStateCursor).cast(),
        ))?;
        let (x, y) = self
            .vi
            .map(|(x, y)| (x as i32, y as i64 - bar.offset as i64))
            .unwrap_or((cursor.viewport_x as i32, cursor.viewport_y as i64));
        let shape = if self.vi.is_some() {
            "Block"
        } else {
            match cursor.visual_style {
                GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BAR => "Beam",
                GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_UNDERLINE => "Underline",
                _ => "Block",
            }
        };
        let selection = self.selection();
        let residual = self.scroll.current_offset(0);
        let extra = if residual != 0.0 {
            let above = residual > 0.0;
            self.peek_row(above, fg, bg)?.map(|runs| json!({"above": above, "runs": runs}))
        } else {
            None
        };
        Ok(json!({"engine":"libghostty-vt","paintRows":rows,"extraRow":extra,
            "snapshot":{"cols":self.get::<u16>(GHOSTTY_TERMINAL_DATA_COLS),"rows":self.get::<u16>(GHOSTTY_TERMINAL_DATA_ROWS),"viewport":[],
                "display_offset":bar.total.saturating_sub(bar.len).saturating_sub(bar.offset),
                "cursor":{"col":x,"row":y,"visible":self.vi.is_some()||cursor.visible&&cursor.viewport_has_value,"shape":shape},
                "alt_screen":self.get::<GhosttyTerminalScreen>(GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN)==GHOSTTY_TERMINAL_SCREEN_ALTERNATE},
            "writes":std::mem::take(&mut self.effects.writes),"offset":self.scroll.current_offset(0),"viMode":self.vi.is_some(),
            "selectionBlock":selection.map(|s|s.rectangle).unwrap_or(false),"appCursor":self.mode(1),"bracketedPaste":self.mode(2004),
            "graphemes":{},"selection":self.selected_text()}))
    }
    unsafe fn text_lines(&self) -> Result<Vec<String>, String> {
        let mut options = sized::<GhosttyFormatterTerminalOptions>();
        options.emit = GHOSTTY_FORMATTER_FORMAT_PLAIN;
        options.trim = false;
        options.unwrap = false;
        let mut formatter = ptr::null_mut();
        check(self.api.ghostty_formatter_terminal_new(
            ptr::null(),
            &mut formatter,
            self.inner,
            options,
        ))?;
        let mut len = 0;
        self.api
            .ghostty_formatter_format_buf(formatter, ptr::null_mut(), 0, &mut len);
        let mut bytes = vec![0u8; len];
        let result = self.api.ghostty_formatter_format_buf(
            formatter,
            bytes.as_mut_ptr(),
            bytes.len(),
            &mut len,
        );
        self.api.ghostty_formatter_free(formatter);
        check(result)?;
        bytes.truncate(len);
        Ok(String::from_utf8_lossy(&bytes)
            .split('\n')
            .map(str::to_owned)
            .collect())
    }
    unsafe fn matches(&self, pattern: &str) -> Vec<SearchMatch> {
        if pattern.is_empty() {
            return Vec::new();
        }
        if let Some((revision, query, matches)) = self.search_cache.borrow().as_ref() {
            if *revision == self.revision && query == pattern {
                return matches.clone();
            }
        }
        let found = self.compute_matches(pattern);
        *self.search_cache.borrow_mut() =
            Some((self.revision, pattern.to_owned(), found.clone()));
        found
    }
    unsafe fn compute_matches(&self, pattern: &str) -> Vec<SearchMatch> {
        if pattern.is_empty() {
            return Vec::new();
        }
        if regex::escape(pattern) == pattern {
            let needle = GhosttyString {
                ptr: pattern.as_ptr(),
                len: pattern.len(),
            };
            if self.api.ghostty_search_set(
                self.search,
                GHOSTTY_SEARCH_OPT_NEEDLE,
                (&needle as *const GhosttyString).cast(),
            ) == GHOSTTY_SUCCESS
                && self.api.ghostty_search_run(self.search) == GHOSTTY_SUCCESS
            {
                let mut buffer = GhosttySelectionBuffer::default();
                self.api.ghostty_search_get(
                    self.search,
                    GHOSTTY_SEARCH_DATA_MATCHES,
                    (&mut buffer as *mut GhosttySelectionBuffer).cast(),
                );
                if buffer.len == 0 {
                    return Vec::new();
                }
                let mut selections = vec![sized::<GhosttySelection>(); buffer.len];
                buffer.ptr = selections.as_mut_ptr();
                buffer.cap = selections.len();
                if self.api.ghostty_search_get(
                    self.search,
                    GHOSTTY_SEARCH_DATA_MATCHES,
                    (&mut buffer as *mut GhosttySelectionBuffer).cast(),
                ) == GHOSTTY_SUCCESS
                {
                    let mut found = Vec::new();
                    for selection in selections.iter().take(buffer.len) {
                        let mut start = GhosttyPointCoordinate::default();
                        let mut end = GhosttyPointCoordinate::default();
                        if self.api.ghostty_terminal_point_from_grid_ref(
                            self.inner,
                            &selection.start,
                            GHOSTTY_POINT_TAG_SCREEN,
                            &mut start,
                        ) != GHOSTTY_SUCCESS
                            || self.api.ghostty_terminal_point_from_grid_ref(
                                self.inner,
                                &selection.end,
                                GHOSTTY_POINT_TAG_SCREEN,
                                &mut end,
                            ) != GHOSTTY_SUCCESS
                        {
                            continue;
                        }
                        // Ghostty's find bar is ASCII-insensitive. Vim's default search is
                        // case-sensitive, so preserve that policy at the embedding boundary.
                        let mut options =
                            sized::<GhosttyTerminalSelectionFormatOptions>();
                        options.emit = GHOSTTY_FORMATTER_FORMAT_PLAIN;
                        options.selection = selection;
                        options.unwrap = true;
                        options.trim = false;
                        let mut actual = vec![0u8; pattern.len().saturating_mul(4) + 16];
                        let mut len = 0;
                        if self.api.ghostty_terminal_selection_format_buf(
                            self.inner,
                            options,
                            actual.as_mut_ptr(),
                            actual.len(),
                            &mut len,
                        ) == GHOSTTY_SUCCESS
                            && actual.get(..len) == Some(pattern.as_bytes())
                        {
                            found.push(((start.x, start.y), (end.x, end.y)));
                        }
                    }
                    found.sort_by_key(|(start, _)| (start.1, start.0));
                    return found;
                }
            }
        }
        let Ok(regex) = regex::Regex::new(pattern) else {
            return Vec::new();
        };
        let Ok(lines) = self.text_lines() else {
            return Vec::new();
        };
        let width = |text: &str| -> u16 {
            text.graphemes(true)
                .map(|g| {
                    let cps: Vec<u32> = g.chars().map(|c| c as u32).collect();
                    {
                        let mut width = 0;
                        self.api.ghostty_unicode_grapheme_width(
                            cps.as_ptr(),
                            cps.len(),
                            &mut width,
                        );
                        width as usize
                    }
                })
                .sum::<usize>()
                .min(u16::MAX as usize) as u16
        };
        let mut matches = Vec::new();
        for (row, line) in lines.iter().enumerate() {
            for found in regex.find_iter(line) {
                if found.is_empty() {
                    continue;
                }
                let start = width(&line[..found.start()]);
                let end = width(&line[..found.end()]).saturating_sub(1);
                matches.push(((start, row as u32), (end, row as u32)));
            }
        }
        matches
    }
    unsafe fn reveal_vi(&self) {
        if let Some((_, y)) = self.vi {
            let bar = self.scrollbar();
            let target = if (y as u64) < bar.offset {
                y as u64
            } else if y as u64 >= bar.offset + bar.len {
                y as u64 + 1 - bar.len
            } else {
                return;
            };
            self.api.ghostty_terminal_scroll_viewport(
                self.inner,
                GhosttyTerminalScrollViewport {
                    tag: GHOSTTY_SCROLL_VIEWPORT_ROW,
                    value: GhosttyTerminalScrollViewportValue {
                        row: target as usize,
                    },
                },
            );
        }
    }
    unsafe fn find(&mut self, pattern: &str, forward: bool, exclusive: bool) {
        let Some((x, y)) = self.vi else {
            return;
        };
        let candidates = self.matches(pattern);
        if candidates.is_empty() {
            return;
        }
        let found = if forward {
            candidates
                .iter()
                .find(|(p, _)| {
                    if exclusive {
                        (p.1, p.0) > (y, x)
                    } else {
                        (p.1, p.0) >= (y, x)
                    }
                })
                .or(candidates.first())
        } else {
            candidates
                .iter()
                .rev()
                .find(|(p, _)| {
                    if exclusive {
                        (p.1, p.0) < (y, x)
                    } else {
                        (p.1, p.0) <= (y, x)
                    }
                })
                .or(candidates.last())
        };
        if let Some((position, _)) = found {
            self.vi = Some(*position);
            self.reveal_vi();
        }
    }
    unsafe fn sync_vi_selection(&mut self) {
        if self.selection_kind == 0 {
            return;
        }
        let Some((x, y)) = self.vi else {
            return;
        };
        if self.selection_anchor.is_null() {
            return;
        }
        let mut anchor = sized::<GhosttyGridRef>();
        if self
            .api
            .ghostty_tracked_grid_ref_snapshot(self.selection_anchor, &mut anchor)
            != GHOSTTY_SUCCESS
        {
            return;
        }
        let Ok(end) = self.point(GHOSTTY_POINT_TAG_SCREEN, x, y) else {
            return;
        };
        let mut selection = sized::<GhosttySelection>();
        selection.start = anchor;
        selection.end = end;
        selection.rectangle = self.selection_kind == 4;
        if self.selection_kind == 3 {
            let cols: u16 = self.get(GHOSTTY_TERMINAL_DATA_COLS);
            let mut origin = GhosttyPointCoordinate::default();
            if self.api.ghostty_tracked_grid_ref_point(
                self.selection_anchor,
                GHOSTTY_POINT_TAG_SCREEN,
                &mut origin,
            ) == GHOSTTY_SUCCESS
            {
                let top = y.min(origin.y);
                let bottom = y.max(origin.y);
                selection.start = self
                    .point(GHOSTTY_POINT_TAG_SCREEN, 0, top)
                    .unwrap_or(anchor);
                selection.end = self
                    .point(GHOSTTY_POINT_TAG_SCREEN, cols.saturating_sub(1), bottom)
                    .unwrap_or(end);
            }
        }
        self.set_selection(Some(&selection));
    }
    unsafe fn row_wrap(&self, y: u32) -> bool {
        let Ok(reference) = self.point(GHOSTTY_POINT_TAG_SCREEN, 0, y) else {
            return false;
        };
        let mut row = 0;
        let mut wraps = false;
        if self.api.ghostty_grid_ref_row(&reference, &mut row) == GHOSTTY_SUCCESS {
            self.api.ghostty_row_get(
                row,
                GHOSTTY_ROW_DATA_WRAP,
                (&mut wraps as *mut bool).cast(),
            );
        }
        wraps
    }
    unsafe fn vi_cells(&self) -> Rc<Vec<Option<char>>> {
        if let Some((revision, cells)) = self.cell_cache.borrow().as_ref() {
            if *revision == self.revision {
                return cells.clone();
            }
        }
        let cols: u16 = self.get(GHOSTTY_TERMINAL_DATA_COLS);
        let bar = self.scrollbar();
        let mut cells = vec![Some(' '); bar.total as usize * cols as usize];
        if let Ok(lines) = self.text_lines() {
            for (row, line) in lines.iter().enumerate().take(bar.total as usize) {
                let mut col = 0;
                for grapheme in line.graphemes(true) {
                    let codepoints: Vec<u32> =
                        grapheme.chars().map(|c| c as u32).collect();
                    let mut width = 0;
                    self.api.ghostty_unicode_grapheme_width(
                        codepoints.as_ptr(),
                        codepoints.len(),
                        &mut width,
                    );
                    if col >= cols as usize {
                        break;
                    }
                    cells[row * cols as usize + col] = grapheme.chars().next();
                    for tail in 1..width as usize {
                        if col + tail < cols as usize {
                            cells[row * cols as usize + col + tail] = None;
                        }
                    }
                    col += (width as usize).max(1);
                }
            }
        }
        let cells = Rc::new(cells);
        *self.cell_cache.borrow_mut() = Some((self.revision, cells.clone()));
        cells
    }
    unsafe fn first_occupied(&self, row: u32) -> u16 {
        let cols: u16 = self.get(GHOSTTY_TERMINAL_DATA_COLS);
        let cells = self.vi_cells();
        let start = row as usize * cols as usize;
        (0..cols)
            .find(|x| {
                cells
                    .get(start + *x as usize)
                    .is_some_and(|c| !matches!(c, Some(' ' | '\t' | '\0')))
            })
            .unwrap_or(0)
    }
    unsafe fn vi_word_motion(&self, kind: &str, x: u16, y: u32) -> (u16, u32) {
        let cols: u16 = self.get(GHOSTTY_TERMINAL_DATA_COLS);
        let cells = self.vi_cells();
        if cells.is_empty() {
            return (x, y);
        }
        let last = cells.len() - 1;
        let mut position = (y as usize * cols as usize + x as usize).min(last);
        let left = matches!(kind, "b" | "B");
        let advance = |i: usize| {
            if left {
                i.saturating_sub(1)
            } else {
                (i + 1).min(last)
            }
        };
        let boundary = |i: usize| if left { i == 0 } else { i == last };
        let space = |i: usize| matches!(cells[i], Some(' ' | '\t' | '\0'));
        // Neoism's semantic separators, including punctuation as individual
        // stops; uppercase motions separate only at whitespace.
        let separator =
            |i: usize| cells[i].is_some_and(|c| ",│`|:\"' ()[]{}<>\t\0".contains(c));
        if left && cells[position].is_none() {
            position = position.saturating_sub(1);
        }
        if !left && position < last && cells[position + 1].is_none() {
            position += 1;
        }
        if matches!(kind, "b" | "w" | "e") {
            let expand = |mut i: usize| {
                if !separator(i) {
                    while !boundary(i) && !separator(advance(i)) {
                        i = advance(i);
                    }
                }
                i
            };
            let same_side = kind != "w";
            if !same_side && !boundary(position) {
                position = expand(position);
            }
            while !boundary(position) && space(advance(position)) {
                position = advance(position);
            }
            if !boundary(position) {
                position = advance(position);
            }
            if same_side && !boundary(position) {
                position = expand(position);
            }
        } else if matches!(kind, "B" | "E") {
            while !boundary(position) && space(advance(position)) {
                position = advance(position);
            }
            while !boundary(position) && !space(advance(position)) {
                position = advance(position);
            }
        } else {
            while !boundary(position) && !space(position) {
                position = advance(position);
            }
            while !boundary(position) && space(position) {
                position = advance(position);
            }
        }
        (
            (position % cols as usize) as u16,
            (position / cols as usize) as u32,
        )
    }
    unsafe fn vi_bracket(&self, x: u16, y: u32) -> (u16, u32) {
        let cols: u16 = self.get(GHOSTTY_TERMINAL_DATA_COLS);
        let cells = self.vi_cells();
        let at = y as usize * cols as usize + x as usize;
        let Some(Some(ch)) = cells.get(at) else {
            return (x, y);
        };
        let (other, forward) = match ch {
            '(' => (')', true),
            '[' => (']', true),
            '{' => ('}', true),
            ')' => ('(', false),
            ']' => ('[', false),
            '}' => ('{', false),
            _ => return (x, y),
        };
        let mut depth = 0usize;
        let mut i = at;
        loop {
            if forward {
                if i + 1 >= cells.len() {
                    break;
                }
                i += 1;
            } else {
                if i == 0 {
                    break;
                }
                i -= 1;
            }
            if cells[i] == Some(*ch) {
                depth += 1;
            } else if cells[i] == Some(other) {
                if depth == 0 {
                    return ((i % cols as usize) as u16, (i / cols as usize) as u32);
                }
                depth -= 1;
            }
        }
        (x, y)
    }
    unsafe fn vi_key(&mut self, v: &Value) -> Option<Value> {
        let key = text(v, "key");
        let ch = text(v, "char");
        let typed = if ch.is_empty() { key } else { ch };
        let ctrl = flag(v, "ctrl");
        if flag(v, "released") {
            return self.vi.map(|_| json!({"bytes":[],"handled":true}));
        }
        if flag(v, "alt") && flag(v, "shift") && (key == "space" || key == " ") {
            if self.vi.is_some() {
                self.vi = None;
                self.selection_kind = 0;
            } else {
                let bar = self.scrollbar();
                let x: u16 = self.get(GHOSTTY_TERMINAL_DATA_CURSOR_X);
                let y: u16 = self.get(GHOSTTY_TERMINAL_DATA_CURSOR_Y);
                let history: u64 = self.get(GHOSTTY_TERMINAL_DATA_SCROLLBACK_ROWS);
                self.vi = Some((
                    x,
                    (history + y as u64)
                        .clamp(bar.offset, bar.offset + bar.len.saturating_sub(1))
                        as u32,
                ));
            }
            self.scroll.reset_wheel(0);
            return Some(json!({"bytes":[],"handled":true}));
        }
        let Some((mut x, mut y)) = self.vi else {
            return None;
        };
        let mut clipboard = None;
        let cols: u16 = self.get(GHOSTTY_TERMINAL_DATA_COLS);
        let rows: u16 = self.get(GHOSTTY_TERMINAL_DATA_ROWS);
        let bar = self.scrollbar();
        if key == "i" && !ctrl || key == "c" && ctrl {
            self.vi = None;
            self.selection_kind = 0;
            self.set_selection(None);
            self.bottom();
        } else if key == "escape" {
            self.selection_kind = 0;
            self.set_selection(None);
        } else if key == "y" && !ctrl {
            clipboard = self.selected_text();
            self.selection_kind = 0;
            self.set_selection(None);
        } else if key == "v" || typed == "V" {
            let kind = if ctrl {
                4
            } else if flag(v, "shift") || typed == "V" {
                3
            } else {
                1
            };
            if self.selection_kind == kind {
                self.selection_kind = 0;
                self.set_selection(None);
            } else {
                self.selection_kind = kind;
                self.api
                    .ghostty_tracked_grid_ref_free(self.selection_anchor);
                self.selection_anchor = ptr::null_mut();
                let _ = self.api.ghostty_terminal_grid_ref_track(
                    self.inner,
                    GhosttyPoint {
                        tag: GHOSTTY_POINT_TAG_SCREEN,
                        value: GhosttyPointValue {
                            coordinate: GhosttyPointCoordinate { x, y },
                        },
                    },
                    &mut self.selection_anchor,
                );
                self.sync_vi_selection();
            }
        } else if ctrl && matches!(key, "u" | "d" | "b" | "f" | "y" | "e") {
            let count = match key {
                "b" | "f" => rows as i64,
                "y" | "e" => 1,
                _ => (rows as i64 / 2).max(1),
            };
            let delta = if matches!(key, "u" | "b" | "y") {
                -count
            } else {
                count
            };
            y = (y as i64 + delta).clamp(0, bar.total.saturating_sub(1) as i64) as u32;
            self.scroll_rows(delta as isize);
            x = self.first_occupied(y);
            self.vi = Some((x, y));
            self.sync_vi_selection();
        } else if !ctrl && matches!(typed, "n" | "N") {
            let pattern = self.pattern.clone();
            self.find(&pattern, self.search_forward != (typed == "N"), true);
            self.sync_vi_selection();
        } else if !ctrl {
            match typed {
                "h" | "left" => {
                    let cells = self.vi_cells();
                    let index = y as usize * cols as usize + x as usize;
                    if cells.get(index) == Some(&None) {
                        x = x.saturating_sub(1);
                    }
                    if x == 0 && y > 0 && self.row_wrap(y - 1) {
                        y -= 1;
                        x = cols.saturating_sub(1);
                    } else {
                        x = x.saturating_sub(1);
                    }
                }
                "l" | "right" => {
                    let cells = self.vi_cells();
                    let index = y as usize * cols as usize + x as usize;
                    if x + 1 < cols && cells.get(index + 1) == Some(&None) {
                        x += 1;
                    }
                    if x + 1 == cols && self.row_wrap(y) {
                        y = (y + 1).min(bar.total.saturating_sub(1) as u32);
                        x = 0;
                    } else {
                        x = (x + 1).min(cols.saturating_sub(1));
                    }
                }
                "k" | "up" => y = y.saturating_sub(1),
                "j" | "down" => y = (y + 1).min(bar.total.saturating_sub(1) as u32),
                "0" | "home" => {
                    while x == 0 && y > 0 && self.row_wrap(y - 1) {
                        y -= 1;
                    }
                    x = 0;
                }
                "$" | "end" => {
                    let cells = self.vi_cells();
                    let start = y as usize * cols as usize;
                    let last = (0..cols)
                        .rfind(|x| {
                            cells
                                .get(start + *x as usize)
                                .is_some_and(|c| !matches!(c, Some(' ' | '\t' | '\0')))
                        })
                        .unwrap_or(0);
                    x = if x < last {
                        last
                    } else if x + 1 == cols && self.row_wrap(y) {
                        while self.row_wrap(y) && y + 1 < bar.total as u32 {
                            y += 1;
                        }
                        let start = y as usize * cols as usize;
                        (0..cols)
                            .rfind(|x| {
                                cells.get(start + *x as usize).is_some_and(|c| {
                                    !matches!(c, Some(' ' | '\t' | '\0'))
                                })
                            })
                            .unwrap_or(x)
                    } else {
                        cols.saturating_sub(1)
                    };
                }
                "H" => {
                    y = bar.offset as u32;
                    x = self.first_occupied(y);
                }
                "M" => {
                    y = (bar.offset + (bar.len / 2).saturating_sub(1)) as u32;
                    x = self.first_occupied(y);
                }
                "L" => {
                    y = (bar.offset + bar.len.saturating_sub(1)) as u32;
                    x = self.first_occupied(y);
                }
                "g" => {
                    self.api.ghostty_terminal_scroll_viewport(
                        self.inner,
                        GhosttyTerminalScrollViewport {
                            tag: GHOSTTY_SCROLL_VIEWPORT_TOP,
                            value: GhosttyTerminalScrollViewportValue::default(),
                        },
                    );
                    let bar = self.scrollbar();
                    y = (y as u64)
                        .clamp(bar.offset, bar.offset + bar.len.saturating_sub(1))
                        as u32;
                }
                "G" => {
                    self.bottom();
                    let bar = self.scrollbar();
                    y = (y as u64)
                        .clamp(bar.offset, bar.offset + bar.len.saturating_sub(1))
                        as u32;
                }
                "z" => {
                    let row = y.saturating_sub((rows as u32 / 2).saturating_sub(1));
                    self.api.ghostty_terminal_scroll_viewport(
                        self.inner,
                        GhosttyTerminalScrollViewport {
                            tag: GHOSTTY_SCROLL_VIEWPORT_ROW,
                            value: GhosttyTerminalScrollViewportValue {
                                row: row as usize,
                            },
                        },
                    );
                }
                "^" => x = self.first_occupied(y),
                "w" | "b" | "e" | "W" | "B" | "E" => {
                    (x, y) = self.vi_word_motion(typed, x, y)
                }
                "%" => (x, y) = self.vi_bracket(x, y),
                _ => {}
            }
            self.vi = Some((x, y));
            self.reveal_vi();
            self.sync_vi_selection();
        }
        Some(json!({"bytes":[],"handled":true,"clipboard":clipboard}))
    }
    unsafe fn key(&mut self, v: &Value) -> Result<Value, String> {
        if let Some(reply) = self.vi_key(v) {
            return Ok(reply);
        }
        let a = &self.api;
        let key = text(v, "key");
        let typed = text(v, "char");
        a.ghostty_key_encoder_setopt_from_terminal(self.key_encoder, self.inner);
        a.ghostty_key_event_set_action(
            self.key_event,
            if flag(v, "released") {
                GHOSTTY_KEY_ACTION_RELEASE
            } else if flag(v, "repeat") {
                GHOSTTY_KEY_ACTION_REPEAT
            } else {
                GHOSTTY_KEY_ACTION_PRESS
            },
        );
        a.ghostty_key_event_set_key(self.key_event, key_code(key));
        let mut mods = 0;
        if flag(v, "shift") {
            mods |= GHOSTTY_MODS_SHIFT;
        }
        if flag(v, "ctrl") {
            mods |= GHOSTTY_MODS_CTRL;
        }
        if flag(v, "alt") {
            mods |= GHOSTTY_MODS_ALT;
        }
        if flag(v, "cmd") {
            mods |= GHOSTTY_MODS_SUPER;
        }
        a.ghostty_key_event_set_mods(self.key_event, mods as GhosttyMods);
        a.ghostty_key_event_set_consumed_mods(
            self.key_event,
            if !typed.is_empty() && flag(v, "shift") {
                GHOSTTY_MODS_SHIFT as GhosttyMods
            } else {
                0
            },
        );
        let typed = if typed.is_empty() && key == "space" {
            " "
        } else {
            typed
        };
        a.ghostty_key_event_set_utf8(self.key_event, typed.as_ptr().cast(), typed.len());
        a.ghostty_key_event_set_unshifted_codepoint(
            self.key_event,
            key.chars()
                .next()
                .filter(|_| key.chars().count() == 1)
                .map(|c| c as u32)
                .unwrap_or(0),
        );
        let mut output = vec![0u8; 256 + typed.len()];
        let mut len = 0;
        check(a.ghostty_key_encoder_encode(
            self.key_encoder,
            self.key_event,
            output.as_mut_ptr().cast(),
            output.len(),
            &mut len,
        ))?;
        output.truncate(len);
        Ok(json!({"bytes":output,"handled":false}))
    }
    unsafe fn mouse(&mut self, v: &Value) -> Result<Value, String> {
        if flag(v, "shift") || self.vi.is_some() {
            return Ok(json!({"handled":false,"bytes":[]}));
        }
        let tracking: self::GhosttyMouseTrackingMode =
            self.get(GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING);
        if tracking == GHOSTTY_MOUSE_TRACKING_NONE {
            return Ok(json!({"handled":false,"bytes":[]}));
        }
        let a = &self.api;
        a.ghostty_mouse_encoder_setopt_from_terminal(self.mouse_encoder, self.inner);
        let rows: u16 = self.get(GHOSTTY_TERMINAL_DATA_ROWS);
        let cols: u16 = self.get(GHOSTTY_TERMINAL_DATA_COLS);
        let row_height = n(v, "rowHeight", 20.0).max(1.0);
        let cell_width = n(v, "cellWidth", 10.0).max(1.0);
        let size = GhosttyMouseEncoderSize {
            size: std::mem::size_of::<GhosttyMouseEncoderSize>(),
            screen_width: (cols as f32 * cell_width) as u32,
            screen_height: (rows as f32 * row_height) as u32,
            cell_width: cell_width as u32,
            cell_height: row_height as u32,
            ..Default::default()
        };
        a.ghostty_mouse_encoder_setopt(
            self.mouse_encoder,
            GHOSTTY_MOUSE_ENCODER_OPT_SIZE,
            (&size as *const GhosttyMouseEncoderSize).cast(),
        );
        let kind = text(v, "kind");
        let wheel = kind == "wheel";
        a.ghostty_mouse_event_set_action(
            self.mouse_event,
            match kind {
                "up" => GHOSTTY_MOUSE_ACTION_RELEASE,
                "move" => GHOSTTY_MOUSE_ACTION_MOTION,
                _ => GHOSTTY_MOUSE_ACTION_PRESS,
            },
        );
        if wheel {
            a.ghostty_mouse_event_set_button(
                self.mouse_event,
                if n(v, "delta", 0.0) > 0.0 {
                    GHOSTTY_MOUSE_BUTTON_FOUR
                } else {
                    GHOSTTY_MOUSE_BUTTON_FIVE
                },
            );
        } else if let Some(button) = v["button"].as_u64() {
            a.ghostty_mouse_event_set_button(
                self.mouse_event,
                match button {
                    0 => GHOSTTY_MOUSE_BUTTON_LEFT,
                    1 => GHOSTTY_MOUSE_BUTTON_MIDDLE,
                    2 => GHOSTTY_MOUSE_BUTTON_RIGHT,
                    _ => GHOSTTY_MOUSE_BUTTON_UNKNOWN,
                },
            );
        } else {
            a.ghostty_mouse_event_clear_button(self.mouse_event);
        }
        let mods = if flag(v, "ctrl") {
            GHOSTTY_MODS_CTRL
        } else {
            0
        } | if flag(v, "alt") { GHOSTTY_MODS_ALT } else { 0 };
        a.ghostty_mouse_event_set_mods(self.mouse_event, mods as GhosttyMods);
        a.ghostty_mouse_event_set_position(
            self.mouse_event,
            GhosttyMousePosition {
                x: n(v, "col", 0.0) * cell_width + cell_width * 0.5,
                y: n(v, "row", 0.0) * row_height + row_height * 0.5,
            },
        );
        let mut bytes = vec![0u8; 128];
        let mut len = 0;
        check(a.ghostty_mouse_encoder_encode(
            self.mouse_encoder,
            self.mouse_event,
            bytes.as_mut_ptr().cast(),
            bytes.len(),
            &mut len,
        ))?;
        bytes.truncate(len);
        if wheel {
            let delta = n(v, "delta", 0.0)
                * if flag(v, "precise") {
                    1.0
                } else {
                    row_height * 3.0
                };
            let steps = self
                .scroll
                .add_wheel_delta(1, delta, row_height)
                .unsigned_abs()
                .min(200) as usize;
            bytes = bytes.repeat(steps);
        }
        Ok(json!({"handled":true,"bytes":bytes}))
    }
    unsafe fn select(&mut self, v: &Value) -> Result<(), String> {
        let rows: u16 = self.get(GHOSTTY_TERMINAL_DATA_ROWS);
        let cols: u16 = self.get(GHOSTTY_TERMINAL_DATA_COLS);
        let row = n(v, "row", 0.0).clamp(0.0, rows.saturating_sub(1) as f32) as u32;
        let col = n(v, "col", 0.0).clamp(0.0, cols.saturating_sub(1) as f32) as u16;
        let at = self.point(GHOSTTY_POINT_TAG_VIEWPORT, col, row)?;
        let side = n(v, "x", col as f32 * 10.0) / n(v, "cellWidth", 10.0).max(1.0)
            - col as f32
            >= 0.6;
        if flag(v, "start") {
            self.api
                .ghostty_tracked_grid_ref_free(self.selection_anchor);
            self.selection_anchor = ptr::null_mut();
            check(self.api.ghostty_terminal_grid_ref_track(
                self.inner,
                GhosttyPoint {
                    tag: GHOSTTY_POINT_TAG_VIEWPORT,
                    value: GhosttyPointValue {
                        coordinate: GhosttyPointCoordinate { x: col, y: row },
                    },
                },
                &mut self.selection_anchor,
            ))?;
            self.selection_kind = v["clicks"].as_u64().unwrap_or(1).min(3) as u8;
            self.selection_side = side;
            let mut selection = sized::<GhosttySelection>();
            if self.selection_kind == 2 {
                let mut options = sized::<GhosttyTerminalSelectWordOptions>();
                options.ref_ = at;
                check(self.api.ghostty_terminal_select_word(
                    self.inner,
                    &options,
                    &mut selection,
                ))?;
                self.set_selection(Some(&selection));
            } else if self.selection_kind == 3 {
                let mut options = sized::<GhosttyTerminalSelectLineOptions>();
                options.ref_ = at;
                check(self.api.ghostty_terminal_select_line(
                    self.inner,
                    &options,
                    &mut selection,
                ))?;
                self.set_selection(Some(&selection));
            } else {
                self.set_selection(None);
            }
        } else if !self.selection_anchor.is_null() {
            let mut anchor = GhosttyPointCoordinate::default();
            let mut target = GhosttyPointCoordinate::default();
            if self.api.ghostty_tracked_grid_ref_point(
                self.selection_anchor,
                GHOSTTY_POINT_TAG_SCREEN,
                &mut anchor,
            ) != GHOSTTY_SUCCESS
            {
                return Ok(());
            }
            check(self.api.ghostty_terminal_point_from_grid_ref(
                self.inner,
                &at,
                GHOSTTY_POINT_TAG_SCREEN,
                &mut target,
            ))?;
            let start = anchor.y as u64 * cols as u64
                + anchor.x as u64
                + u64::from(self.selection_side);
            let end = target.y as u64 * cols as u64 + target.x as u64 + u64::from(side);
            if start == end {
                self.set_selection(None);
                return Ok(());
            }
            let lo = start.min(end);
            let hi = start.max(end) - 1;
            let mut selection = sized::<GhosttySelection>();
            selection.start = self.point(
                GHOSTTY_POINT_TAG_SCREEN,
                (lo % cols as u64) as u16,
                (lo / cols as u64) as u32,
            )?;
            selection.end = self.point(
                GHOSTTY_POINT_TAG_SCREEN,
                (hi % cols as u64) as u16,
                (hi / cols as u64) as u32,
            )?;
            self.set_selection(Some(&selection));
        }
        Ok(())
    }
}
fn key_code(key: &str) -> GhosttyKey {
    match key {
        "enter" | "return" => GHOSTTY_KEY_ENTER,
        "tab" => GHOSTTY_KEY_TAB,
        "space" | " " => GHOSTTY_KEY_SPACE,
        "escape" => GHOSTTY_KEY_ESCAPE,
        "backspace" => GHOSTTY_KEY_BACKSPACE,
        "delete" => GHOSTTY_KEY_DELETE,
        "insert" => GHOSTTY_KEY_INSERT,
        "home" => GHOSTTY_KEY_HOME,
        "end" => GHOSTTY_KEY_END,
        "pageup" => GHOSTTY_KEY_PAGE_UP,
        "pagedown" => GHOSTTY_KEY_PAGE_DOWN,
        "up" => GHOSTTY_KEY_ARROW_UP,
        "down" => GHOSTTY_KEY_ARROW_DOWN,
        "left" => GHOSTTY_KEY_ARROW_LEFT,
        "right" => GHOSTTY_KEY_ARROW_RIGHT,
        "`" => GHOSTTY_KEY_BACKQUOTE,
        "-" => GHOSTTY_KEY_MINUS,
        "=" => GHOSTTY_KEY_EQUAL,
        "[" => GHOSTTY_KEY_BRACKET_LEFT,
        "]" => GHOSTTY_KEY_BRACKET_RIGHT,
        "\\" => GHOSTTY_KEY_BACKSLASH,
        ";" => GHOSTTY_KEY_SEMICOLON,
        "'" => GHOSTTY_KEY_QUOTE,
        "," => GHOSTTY_KEY_COMMA,
        "." => GHOSTTY_KEY_PERIOD,
        "/" => GHOSTTY_KEY_SLASH,
        _ if key.len() == 1 && key.as_bytes()[0].is_ascii_alphabetic() => {
            GHOSTTY_KEY_A + (key.as_bytes()[0].to_ascii_lowercase() - b'a') as i32
        }
        _ if key.len() == 1 && key.as_bytes()[0].is_ascii_digit() => {
            GHOSTTY_KEY_DIGIT_0 + (key.as_bytes()[0] - b'0') as i32
        }
        _ if key.starts_with('f') => key[1..]
            .parse::<i32>()
            .ok()
            .filter(|n| (1..=25).contains(n))
            .map(|n| GHOSTTY_KEY_F1 + n - 1)
            .unwrap_or(GHOSTTY_KEY_UNIDENTIFIED),
        _ => GHOSTTY_KEY_UNIDENTIFIED,
    }
}
struct PaintRun {
    text: String,
    col: usize,
    width: usize,
    style: [u32; 4],
}
impl PaintRun {
    fn value(self) -> Value {
        json!([self.text, self.col, self.width, self.style[0], self.style[1], self.style[2], self.style[3]])
    }
}
fn paint_pack(c: GhosttyColorRgb) -> u32 {
    0x1000000 | ((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32
}
pub fn dispatch(v: &Value) -> Option<Result<Value, String>> {
    let op = text(v, "op");
    let id = v["id"].as_u64().unwrap_or(0) as u32;
    if !op.starts_with("ghostty.")
        && !(op == "dispose" && (0x70000000..0x80000000).contains(&id))
    {
        return None;
    }
    Some(STORE.with(|store|{let mut store=store.borrow_mut();unsafe{
   if op=="ghostty.load"{if store.api.is_none(){store.api=Some(Rc::new(GhosttyApi::new(text(v,"path")).map_err(|_|"Could not load pinned libghostty-vt".to_owned())?));}return Ok(json!({"loaded":true}));}
   if op=="ghostty.new"{let api=store.api.clone().ok_or("libghostty-vt is not loaded")?;let mut t=Terminal::new(api,n(v,"cols",100.0).clamp(2.0,500.0)as u16,n(v,"rows",30.0).clamp(1.0,200.0)as u16)?;t.configure_colors(v);t.compact=flag(v,"compact");store.next+=1;let id=0x70000000+store.next;store.terminals.insert(id,t);return Ok(json!({"id":id}));}
   if op=="dispose"{store.terminals.remove(&id);return Ok(Value::Null);}
   let t=store.terminals.get_mut(&id).ok_or("Unknown Ghostty terminal")?;
   match op{
    "ghostty.feed"=>{let bytes:Vec<u8>=v["bytes"].as_array().into_iter().flatten().filter_map(|b|b.as_u64().map(|b|b as u8)).collect();let before=t.get::<GhosttyTerminalScreen>(GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN);t.api.ghostty_terminal_vt_write(t.inner,bytes.as_ptr(),bytes.len());t.revision=t.revision.wrapping_add(1);
      if before!=t.get::<GhosttyTerminalScreen>(GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN){t.api.ghostty_tracked_grid_ref_free(t.selection_anchor);t.selection_anchor=ptr::null_mut();t.selection_kind=0;t.vi=None;}},
    "ghostty.resize"=>{check(t.api.ghostty_terminal_resize(t.inner,n(v,"cols",100.0).clamp(2.0,500.0)as u16,n(v,"rows",30.0).clamp(1.0,200.0)as u16,n(v,"cellWidth",10.0).max(1.0).round()as u32,n(v,"rowHeight",20.0).max(1.0).round()as u32))?;t.revision=t.revision.wrapping_add(1);},
    "ghostty.reset"=>{let cols=t.get(GHOSTTY_TERMINAL_DATA_COLS);let rows=t.get(GHOSTTY_TERMINAL_DATA_ROWS);let compact=t.compact;*t=Terminal::new(t.api.clone(),cols,rows)?;t.compact=compact;t.configure_colors(v);},
    "ghostty.bottom"=>t.bottom(),
    "ghostty.scroll"=>{
      // Desktop `terminal_edge_rejected`: input into a hard edge never builds a
      // residual, and one left pointing past an edge is dropped, so the grid
      // never parks between rows or drifts past the top/bottom then snaps back.
      let edge=|t:&Terminal,toward_history:bool|{let bar=t.scrollbar();if toward_history{bar.offset==0}else{bar.offset+bar.len>=bar.total}};
      let d=n(v,"delta",0.0);
      if d!=0.0&&edge(t,d>0.0){t.scroll.reset_wheel(0);}
      else{let delta=t.scroll.add_wheel_delta(0,d,n(v,"rowHeight",20.0));t.scroll_rows(-(delta as isize));
        let r=t.scroll.current_offset(0);if r!=0.0&&edge(t,r>0.0){t.scroll.reset_wheel(0);}}},
    "ghostty.select"=>t.select(v)?,"ghostty.key"=>return t.key(v),"ghostty.mouse"=>return t.mouse(v),
    "ghostty.paste"=>{let mut input=text(v,"text").as_bytes().to_vec();let mut output=vec![0u8;input.len()+32];let mut len=0;check(t.api.ghostty_paste_encode(input.as_mut_ptr().cast(),input.len(),t.mode(2004),output.as_mut_ptr().cast(),output.len(),&mut len))?;output.truncate(len);return Ok(json!({"bytes":output}));},
    "ghostty.focus"=>{let mut bytes=vec![0u8;8];let mut len=0;if t.mode(1004){check(t.api.ghostty_focus_encode(if flag(v,"focused"){GHOSTTY_FOCUS_GAINED}else{GHOSTTY_FOCUS_LOST},bytes.as_mut_ptr().cast(),bytes.len(),&mut len))?;}bytes.truncate(len);return Ok(json!({"bytes":bytes}));},
    "ghostty.cursor"=>{
      let mut cursor=sized::<GhosttyRenderStateCursor>();check(t.api.ghostty_render_state_get(t.render,GHOSTTY_RENDER_STATE_DATA_CURSOR,(&mut cursor as *mut GhosttyRenderStateCursor).cast()))?;
      let bar=t.scrollbar();let (col,line)=t.vi.map(|(x,y)|(x,y as i64-bar.offset as i64)).unwrap_or((cursor.viewport_x,cursor.viewport_y as i64));
      let cell=n(v,"cellWidth",10.0);let row=n(v,"rowHeight",20.0);let x=col as f32*cell;let y=line as f32*row+t.scroll.current_offset(0);
      let shape=if t.vi.is_some(){neoism_terminal_core::ansi::CursorShape::Block}else{match cursor.visual_style{GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BAR=>neoism_terminal_core::ansi::CursorShape::Beam,GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_UNDERLINE=>neoism_terminal_core::ansi::CursorShape::Underline,_=>neoism_terminal_core::ansi::CursorShape::Block}};
      let was_animating=t.trail.is_animating();t.trail.set_cursor_shape(shape);let position=(col as usize,line as i32);if position!=t.last_cursor{t.trail.set_destination(x,y,cell,row);}else{t.trail.set_destination_no_jump(x,y,cell,row);}t.last_cursor=position;let now=Instant::now();let dt=now.duration_since(t.last_frame).as_secs_f32();t.last_frame=now;t.trail.animate(cell,row,if was_animating{dt}else{0.0});
      return Ok(json!({"quad":t.trail.quad_points(),"animating":t.trail.is_animating(),"hold":t.trail.blink_hold_visible(neoism_ui::editor::optimistic::DEFAULT_BLINK_PERIOD_MS)}));
    },
    "ghostty.search.start"=>{if let Some((x,y))=t.vi{t.search_origin=Some((x,y,t.scrollbar().offset));}t.preview=Some(String::new());t.preview_forward=v["forward"].as_bool().unwrap_or(true);},
    "ghostty.search.preview"=>{let pattern=text(v,"pattern");t.preview=Some(pattern.into());if let Some((x,y,_))=t.search_origin{t.vi=Some((x,y));}t.find(pattern,t.preview_forward,false);},
    "ghostty.search.commit"=>{t.pattern=t.preview.take().unwrap_or_default();t.search_forward=t.preview_forward;t.search_origin=None;},
    "ghostty.search.cancel"=>{if let Some((x,y,offset))=t.search_origin.take(){t.vi=Some((x,y));t.api.ghostty_terminal_scroll_viewport(t.inner,GhosttyTerminalScrollViewport{tag:GHOSTTY_SCROLL_VIEWPORT_ROW,value:GhosttyTerminalScrollViewportValue{row:offset as usize}});}t.preview=None;},
    "ghostty.frame"=>{},_=>return Err("Unsupported Ghostty operation".into()),
   }t.frame()
 }}))
}
