//! libghostty-vt, the same terminal engine the GPUiX frontend used: Ghostty owns the parser,
//! screen, scrollback, Unicode and replies; Neoism reads cell runs back out of it. This is the
//! subset of the GPUiX adapter the block terminal needs (create, write, position, tracked marks,
//! capture), as a plain Rust type.

#[allow(non_upper_case_globals, non_camel_case_types, non_snake_case, dead_code, unsafe_op_in_unsafe_fn, clippy::all)]
mod ffi {
    include!("bindings.rs");

    impl GhosttyApi {
        // Exported by the pinned library, but not in the generated subset.
        pub unsafe fn grid_style_reader(&self) -> Result<unsafe extern "C" fn(*const GhosttyGridRef, *mut GhosttyStyle) -> GhosttyResult, String> {
            unsafe { self.__library.get(b"ghostty_grid_ref_style\0").map(|symbol| *symbol).map_err(|error| error.to_string()) }
        }
    }
}

use ffi::*;
use std::collections::HashMap;
use std::ffi::c_void;
use std::path::PathBuf;
use std::rc::Rc;
use std::{ptr, slice};

/// One styled run of a captured row: `text` starts at cell `col` and spans `width` cells.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub text: String,
    pub col: usize,
    pub width: usize,
    /// `0xRRGGBB`, or `None` for the theme default.
    pub fg: Option<u32>,
    pub bg: Option<u32>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

pub type Row = Vec<Run>;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Position {
    /// Absolute screen row (scrollback included).
    pub row: u64,
    pub col: u16,
    pub alt: bool,
}

fn check(result: GhosttyResult) -> Result<(), String> {
    if result == GHOSTTY_SUCCESS { Ok(()) } else { Err(format!("libghostty-vt operation failed ({result})")) }
}

unsafe fn sized<T>() -> T {
    unsafe {
        let mut value: T = std::mem::zeroed();
        *(&mut value as *mut T).cast::<usize>() = std::mem::size_of::<T>();
        value
    }
}

struct Effects {
    api: Rc<GhosttyApi>,
    writes: Vec<u8>,
    dark: bool,
}

unsafe extern "C" fn write_pty(_: GhosttyTerminal, userdata: *mut c_void, data: *const u8, len: usize) {
    if !userdata.is_null() && !data.is_null() {
        unsafe { (*(userdata as *mut Effects)).writes.extend_from_slice(slice::from_raw_parts(data, len)) };
    }
}

unsafe extern "C" fn device_attributes(_: GhosttyTerminal, _: *mut c_void, attributes: *mut GhosttyDeviceAttributes) -> bool {
    if attributes.is_null() { return false; }
    let mut value = GhosttyDeviceAttributes::default();
    // VT220 with colour, exactly as the GPUiX embedding advertises.
    value.primary.conformance_level = 62;
    value.primary.features[0] = 22;
    value.primary.num_features = 1;
    value.secondary.device_type = 1;
    value.secondary.firmware_version = 10;
    unsafe { *attributes = value };
    true
}

unsafe extern "C" fn report_size(terminal: GhosttyTerminal, userdata: *mut c_void, size: *mut GhosttySizeReportSize) -> bool {
    if userdata.is_null() || size.is_null() { return false; }
    unsafe {
        let effects = &*(userdata as *const Effects);
        let (mut rows, mut columns, mut width, mut height) = (0u16, 0u16, 0u32, 0u32);
        effects.api.ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_ROWS, (&mut rows as *mut u16).cast());
        effects.api.ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_COLS, (&mut columns as *mut u16).cast());
        effects.api.ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_WIDTH_PX, (&mut width as *mut u32).cast());
        effects.api.ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_HEIGHT_PX, (&mut height as *mut u32).cast());
        *size = GhosttySizeReportSize { rows, columns, cell_width: width / columns.max(1) as u32, cell_height: height / rows.max(1) as u32 };
    }
    true
}

unsafe extern "C" fn color_scheme(_: GhosttyTerminal, userdata: *mut c_void, scheme: *mut GhosttyColorScheme) -> bool {
    if userdata.is_null() || scheme.is_null() { return false; }
    unsafe { *scheme = if (*(userdata as *const Effects)).dark { GHOSTTY_COLOR_SCHEME_DARK } else { GHOSTTY_COLOR_SCHEME_LIGHT } };
    true
}

thread_local! {
    static API: std::cell::RefCell<Option<Result<Rc<GhosttyApi>, String>>> = const { std::cell::RefCell::new(None) };
}

/// Same lookup order as `gpuix/src/native-libraries.ts`, plus the repo's built artifact.
fn library_path() -> Option<PathBuf> {
    let name = if cfg!(windows) { "ghostty-vt.dll" } else if cfg!(target_os = "macos") { "libghostty-vt.dylib" } else { "libghostty-vt.so" };
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from));
    let candidates = [
        std::env::var_os("NEOISM_GHOSTTY_VT").map(PathBuf::from),
        std::env::var_os("NEOISM_GPUIX_GHOSTTY").map(PathBuf::from),
        exe_dir.as_ref().map(|d| d.join(name)),
        exe_dir.as_ref().map(|d| d.join("web/native").join(name)),
        Some(PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../neoism-frontend/gpuix/artifacts")).join(name)),
    ];
    candidates.into_iter().flatten().find(|p| p.exists())
}

fn api() -> Result<Rc<GhosttyApi>, String> {
    API.with(|slot| {
        slot.borrow_mut()
            .get_or_insert_with(|| {
                let path = library_path().ok_or("Ghostty's terminal engine (libghostty-vt) is not built")?;
                unsafe { GhosttyApi::new(&path) }.map(Rc::new).map_err(|e| format!("Could not load {}: {e}", path.display()))
            })
            .clone()
    })
}

pub struct Vt {
    api: Rc<GhosttyApi>,
    inner: GhosttyTerminal,
    effects: Box<Effects>,
    marks: HashMap<u64, (GhosttyTrackedGridRef, GhosttyTerminalScreen)>,
    next_mark: u64,
    /// Viewport reads (`viewport`) go through Ghostty's render state.
    render: GhosttyRenderState,
    rows_iter: GhosttyRenderStateRowIterator,
    cells: GhosttyRenderStateRowCells,
    key_encoder: GhosttyKeyEncoder,
    key_event: GhosttyKeyEvent,
    mouse_encoder: GhosttyMouseEncoder,
    mouse_event: GhosttyMouseEvent,
    colors: Colors,
    /// Cell size in pixels, for size reports and mouse encoding.
    cell: (u32, u32),
}

impl Drop for Vt {
    fn drop(&mut self) {
        unsafe {
            let a = &self.api;
            for (_, (reference, _)) in self.marks.drain() { a.ghostty_tracked_grid_ref_free(reference); }
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

/// What a viewport read returns: the visible rows and the cursor (when shown).
pub struct Screen {
    pub rows: Vec<Row>,
    pub cursor: Option<Cursor>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cursor {
    pub col: u16,
    pub row: u16,
    pub shape: CursorShape,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Beam,
    Underline,
}

/// A key for the encoder, named as the GPUiX host named them ("enter", "up", "a", "f5", ...).
pub struct KeyInput<'a> {
    pub key: &'a str,
    /// The text the key types, if any (already shifted).
    pub text: &'a str,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub logo: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseAction {
    Press,
    Release,
    Motion,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    WheelUp,
    WheelDown,
}

pub struct MouseInput {
    pub action: MouseAction,
    pub button: Option<MouseButton>,
    pub col: u16,
    pub row: u16,
    pub ctrl: bool,
    pub alt: bool,
}

#[derive(Clone)]
/// Theme colours for the 16 ANSI slots plus default foreground/background, as `0xRRGGBB`.
pub struct Colors {
    pub palette: [u32; 16],
    pub fg: u32,
    pub bg: u32,
}

fn rgb(c: u32) -> GhosttyColorRgb {
    GhosttyColorRgb { r: (c >> 16) as u8, g: (c >> 8) as u8, b: c as u8 }
}

fn pack(c: GhosttyColorRgb) -> u32 {
    ((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32
}

impl Vt {
    pub fn new(cols: u16, rows: u16, colors: &Colors) -> Result<Vt, String> {
        let api = api()?;
        let mut vt = Vt {
            api: api.clone(), inner: ptr::null_mut(), effects: Box::new(Effects { api, writes: Vec::new(), dark: true }),
            marks: HashMap::new(), next_mark: 1, render: ptr::null_mut(), rows_iter: ptr::null_mut(), cells: ptr::null_mut(),
            key_encoder: ptr::null_mut(), key_event: ptr::null_mut(), mouse_encoder: ptr::null_mut(), mouse_event: ptr::null_mut(),
            colors: colors.clone(), cell: (10, 20),
        };
        unsafe {
            let a = &vt.api;
            check(a.ghostty_terminal_new(ptr::null(), &mut vt.inner, cols, rows))?;
            check(a.ghostty_terminal_resize(vt.inner, cols, rows, 10, 20))?;
            check(a.ghostty_render_state_new(ptr::null(), &mut vt.render))?;
            check(a.ghostty_render_state_row_iterator_new(ptr::null(), &mut vt.rows_iter))?;
            check(a.ghostty_render_state_row_cells_new(ptr::null(), &mut vt.cells))?;
            check(a.ghostty_key_encoder_new(ptr::null(), &mut vt.key_encoder))?;
            check(a.ghostty_key_event_new(ptr::null(), &mut vt.key_event))?;
            check(a.ghostty_mouse_encoder_new(ptr::null(), &mut vt.mouse_encoder))?;
            check(a.ghostty_mouse_event_new(ptr::null(), &mut vt.mouse_event))?;
            // Ghostty's application default (`grapheme-width-method=unicode`).
            let grapheme_mode = GhosttyTerminalModeConfig { mode: 2027, value: true };
            check(a.ghostty_terminal_set(vt.inner, GHOSTTY_TERMINAL_OPT_MODE_DEFAULT, (&grapheme_mode as *const GhosttyTerminalModeConfig).cast()))?;
            // Headroom over the 2000 rows a block keeps, so its anchor mark is never pruned
            // (Ghostty prunes whole pages, well before the nominal limit).
            let max_lines = 10_000usize;
            check(a.ghostty_terminal_set(vt.inner, GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES, (&max_lines as *const usize).cast()))?;
            check(a.ghostty_terminal_set(vt.inner, GHOSTTY_TERMINAL_OPT_USERDATA, (&mut *vt.effects as *mut Effects).cast()))?;
            check(a.ghostty_terminal_set(vt.inner, GHOSTTY_TERMINAL_OPT_WRITE_PTY, write_pty as *const () as *const c_void))?;
            for (option, callback) in [
                (GHOSTTY_TERMINAL_OPT_DEVICE_ATTRIBUTES, device_attributes as *const ()),
                (GHOSTTY_TERMINAL_OPT_SIZE, report_size as *const ()),
                (GHOSTTY_TERMINAL_OPT_COLOR_SCHEME, color_scheme as *const ()),
            ] {
                check(a.ghostty_terminal_set(vt.inner, option, callback.cast()))?;
            }
        }
        vt.set_colors(colors);
        Ok(vt)
    }

    pub fn set_colors(&mut self, colors: &Colors) {
        self.colors = colors.clone();
        unsafe {
            let bg = rgb(colors.bg);
            self.effects.dark = (bg.r as f32 * 0.2126 + bg.g as f32 * 0.7152 + bg.b as f32 * 0.0722) < 128.0;
            let mut palette = [GhosttyColorRgb::default(); 256];
            self.api.ghostty_terminal_get(self.inner, GHOSTTY_TERMINAL_DATA_COLOR_PALETTE, palette.as_mut_ptr().cast());
            for (slot, &c) in palette.iter_mut().zip(colors.palette.iter()) { *slot = rgb(c); }
            self.api.ghostty_terminal_set(self.inner, GHOSTTY_TERMINAL_OPT_COLOR_PALETTE, palette.as_ptr().cast());
            for (c, option) in [(colors.fg, GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND), (colors.bg, GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND)] {
                let c = rgb(c);
                self.api.ghostty_terminal_set(self.inner, option, (&c as *const GhosttyColorRgb).cast());
            }
        }
    }

    unsafe fn get<T: Default>(&self, key: GhosttyTerminalData) -> T {
        let mut value = T::default();
        unsafe { self.api.ghostty_terminal_get(self.inner, key, (&mut value as *mut T).cast()) };
        value
    }

    /// Feed PTY output. Returns any bytes Ghostty wants written back (DA, size reports, ...).
    pub fn write(&mut self, bytes: &[u8]) -> Vec<u8> {
        unsafe { self.api.ghostty_terminal_vt_write(self.inner, bytes.as_ptr(), bytes.len()) };
        std::mem::take(&mut self.effects.writes)
    }

    /// Resize the grid; `cell` is the cell size in pixels (size reports, mouse positions).
    /// Returns whether the grid size changed.
    pub fn resize(&mut self, cols: u16, rows: u16, cell: (u32, u32)) -> bool {
        let (cols, rows) = (cols.clamp(2, 500), rows.clamp(1, 200));
        let cell = (cell.0.max(1), cell.1.max(1));
        let changed = self.size() != (cols, rows);
        if changed || cell != self.cell {
            self.cell = cell;
            unsafe { self.api.ghostty_terminal_resize(self.inner, cols, rows, cell.0, cell.1) };
        }
        changed
    }

    pub fn size(&self) -> (u16, u16) {
        unsafe { (self.get(GHOSTTY_TERMINAL_DATA_COLS), self.get(GHOSTTY_TERMINAL_DATA_ROWS)) }
    }

    /// A DEC private mode: 1 app cursor keys, 1007 alternate scroll, 2004 bracketed paste, ...
    pub fn mode(&self, mode: u16) -> bool {
        let mut value = GhosttyTerminalModeConfig { mode, value: false };
        unsafe { self.api.ghostty_terminal_get(self.inner, GHOSTTY_TERMINAL_DATA_MODE, (&mut value as *mut GhosttyTerminalModeConfig).cast()) };
        value.value
    }

    /// Whether the running program asked for mouse reports.
    pub fn mouse_tracking(&self) -> bool {
        unsafe { self.get::<GhosttyMouseTrackingMode>(GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING) != GHOSTTY_MOUSE_TRACKING_NONE }
    }

    /// The visible grid with resolved colours, read through Ghostty's render state (the GPUiX
    /// `paint_frame` path). Default colours stay `None` so the theme draws them.
    pub fn viewport(&mut self) -> Result<Screen, String> {
        let api = self.api.clone();
        let a = &*api;
        unsafe {
            check(a.ghostty_render_state_update(self.render, self.inner))?;
            check(a.ghostty_render_state_get(self.render, GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR, (&mut self.rows_iter as *mut GhosttyRenderStateRowIterator).cast()))?;
            let (theme_fg, theme_bg) = (self.colors.fg, self.colors.bg);
            let mut rows = Vec::new();
            while a.ghostty_render_state_row_iterator_next(self.rows_iter) {
                check(a.ghostty_render_state_row_get(self.rows_iter, GHOSTTY_RENDER_STATE_ROW_DATA_CELLS, (&mut self.cells as *mut GhosttyRenderStateRowCells).cast()))?;
                let mut line: Row = Vec::new();
                let mut run: Option<Run> = None;
                let mut col = 0usize;
                while a.ghostty_render_state_row_cells_next(self.cells) {
                    let x = col;
                    col += 1;
                    let mut style = sized::<GhosttyStyle>();
                    a.ghostty_render_state_row_cells_get(self.cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE, (&mut style as *mut GhosttyStyle).cast());
                    let mut raw = 0;
                    a.ghostty_render_state_row_cells_get(self.cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW, (&mut raw as *mut GhosttyCell).cast());
                    let mut wide = GHOSTTY_CELL_WIDE_NARROW;
                    a.ghostty_cell_get(raw, GHOSTTY_CELL_DATA_WIDE, (&mut wide as *mut GhosttyCellWide).cast());
                    if wide == GHOSTTY_CELL_WIDE_SPACER_TAIL || wide == GHOSTTY_CELL_WIDE_SPACER_HEAD { continue; }
                    let resolved = |value: GhosttyStyleColor, option| match value.tag {
                        GHOSTTY_STYLE_COLOR_RGB => Some(pack(value.value.rgb)),
                        GHOSTTY_STYLE_COLOR_PALETTE => {
                            let mut c = GhosttyColorRgb::default();
                            (a.ghostty_render_state_row_cells_get(self.cells, option, (&mut c as *mut GhosttyColorRgb).cast()) == GHOSTTY_SUCCESS).then(|| pack(c))
                        }
                        _ => None,
                    };
                    let mut fg = resolved(style.fg_color, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR);
                    // Blank cells can carry their background in the content tag (erase after SGR).
                    let mut c = GhosttyColorRgb::default();
                    let mut bg = if a.ghostty_render_state_row_cells_get(self.cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR, (&mut c as *mut GhosttyColorRgb).cast()) == GHOSTTY_SUCCESS {
                        Some(pack(c))
                    } else {
                        resolved(style.bg_color, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR)
                    };
                    if bg == Some(theme_bg) { bg = None; }
                    if style.inverse {
                        let (f, b) = (fg.unwrap_or(theme_fg), bg.unwrap_or(theme_bg));
                        fg = Some(b);
                        bg = Some(f);
                    }
                    let next = Run { text: String::new(), col: x, width: 0, fg, bg, bold: style.bold, italic: style.italic, underline: style.underline != 0 };
                    let same = run.as_ref().is_some_and(|r| (r.fg, r.bg, r.bold, r.italic, r.underline) == (next.fg, next.bg, next.bold, next.italic, next.underline));
                    if !same { if let Some(done) = run.take() { line.push(done); } }
                    let current = run.get_or_insert(next);
                    current.width += if wide == GHOSTTY_CELL_WIDE_WIDE { 2 } else { 1 };
                    let mut length = 0u32;
                    a.ghostty_render_state_row_cells_get(self.cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_LEN, (&mut length as *mut u32).cast());
                    if style.invisible || length == 0 {
                        current.text.push(' ');
                        continue;
                    }
                    let mut points = vec![0u32; length as usize];
                    a.ghostty_render_state_row_cells_get(self.cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_BUF, points.as_mut_ptr().cast());
                    for point in points {
                        if let Some(c) = char::from_u32(point) { current.text.push(if c == '\0' { ' ' } else { c }); }
                    }
                }
                if let Some(run) = run { line.push(run); }
                rows.push(line);
            }
            let mut cursor = sized::<GhosttyRenderStateCursor>();
            check(a.ghostty_render_state_get(self.render, GHOSTTY_RENDER_STATE_DATA_CURSOR, (&mut cursor as *mut GhosttyRenderStateCursor).cast()))?;
            let cursor = (cursor.visible && cursor.viewport_has_value).then(|| Cursor {
                col: cursor.viewport_x,
                row: cursor.viewport_y,
                shape: match cursor.visual_style {
                    GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BAR => CursorShape::Beam,
                    GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_UNDERLINE => CursorShape::Underline,
                    _ => CursorShape::Block,
                },
            });
            Ok(Screen { rows, cursor })
        }
    }

    /// Encode a key press for the running program, honouring its modes (app cursor, kitty
    /// keyboard, ...), as the GPUiX host's `key()` did.
    pub fn encode_key(&mut self, input: &KeyInput) -> Vec<u8> {
        let a = &*self.api;
        unsafe {
            a.ghostty_key_encoder_setopt_from_terminal(self.key_encoder, self.inner);
            // Mode sync resets macOS Option-as-Alt; Alt intent arrives explicitly, so keep Meta.
            let option_as_alt = GHOSTTY_OPTION_AS_ALT_TRUE;
            (a.ghostty_key_encoder_setopt)(self.key_encoder, GHOSTTY_KEY_ENCODER_OPT_MACOS_OPTION_AS_ALT, (&option_as_alt as *const GhosttyOptionAsAlt).cast());
            a.ghostty_key_event_set_action(self.key_event, GHOSTTY_KEY_ACTION_PRESS);
            a.ghostty_key_event_set_key(self.key_event, key_code(input.key));
            let mut mods = 0;
            if input.shift { mods |= GHOSTTY_MODS_SHIFT; }
            if input.ctrl { mods |= GHOSTTY_MODS_CTRL; }
            if input.alt { mods |= GHOSTTY_MODS_ALT; }
            if input.logo { mods |= GHOSTTY_MODS_SUPER; }
            a.ghostty_key_event_set_mods(self.key_event, mods as GhosttyMods);
            a.ghostty_key_event_set_consumed_mods(self.key_event, if !input.text.is_empty() && input.shift { GHOSTTY_MODS_SHIFT as GhosttyMods } else { 0 });
            let text = if input.text.is_empty() && input.key == "space" { " " } else { input.text };
            a.ghostty_key_event_set_utf8(self.key_event, text.as_ptr().cast(), text.len());
            let unshifted = input.key.chars().next().filter(|_| input.key.chars().count() == 1).map_or(0, |c| c as u32);
            a.ghostty_key_event_set_unshifted_codepoint(self.key_event, unshifted);
            let mut output = vec![0u8; 256 + text.len()];
            let mut len = 0;
            if a.ghostty_key_encoder_encode(self.key_encoder, self.key_event, output.as_mut_ptr().cast(), output.len(), &mut len) != GHOSTTY_SUCCESS {
                return Vec::new();
            }
            output.truncate(len);
            output
        }
    }

    /// Text as the program should receive a paste (bracketed when it asked for that).
    pub fn encode_paste(&self, text: &str) -> Vec<u8> {
        let mut input = text.as_bytes().to_vec();
        let mut output = vec![0u8; input.len() + 32];
        let mut len = 0;
        let ok = unsafe {
            self.api.ghostty_paste_encode(input.as_mut_ptr().cast(), input.len(), self.mode(2004), output.as_mut_ptr().cast(), output.len(), &mut len)
        } == GHOSTTY_SUCCESS;
        if !ok { return text.as_bytes().to_vec(); }
        output.truncate(len);
        output
    }

    /// A mouse report for the running program, or `None` when it isn't tracking the mouse
    /// (the GPUiX host's `mouse()`).
    pub fn encode_mouse(&mut self, input: &MouseInput) -> Option<Vec<u8>> {
        if !self.mouse_tracking() { return None; }
        let a = &*self.api;
        let (cols, rows) = self.size();
        let (cell_w, cell_h) = self.cell;
        unsafe {
            a.ghostty_mouse_encoder_setopt_from_terminal(self.mouse_encoder, self.inner);
            let size = GhosttyMouseEncoderSize {
                size: std::mem::size_of::<GhosttyMouseEncoderSize>(),
                screen_width: cols as u32 * cell_w,
                screen_height: rows as u32 * cell_h,
                cell_width: cell_w,
                cell_height: cell_h,
                ..Default::default()
            };
            a.ghostty_mouse_encoder_setopt(self.mouse_encoder, GHOSTTY_MOUSE_ENCODER_OPT_SIZE, (&size as *const GhosttyMouseEncoderSize).cast());
            a.ghostty_mouse_event_set_action(self.mouse_event, match input.action {
                MouseAction::Press => GHOSTTY_MOUSE_ACTION_PRESS,
                MouseAction::Release => GHOSTTY_MOUSE_ACTION_RELEASE,
                MouseAction::Motion => GHOSTTY_MOUSE_ACTION_MOTION,
            });
            match input.button {
                Some(button) => a.ghostty_mouse_event_set_button(self.mouse_event, match button {
                    MouseButton::Left => GHOSTTY_MOUSE_BUTTON_LEFT,
                    MouseButton::WheelUp => GHOSTTY_MOUSE_BUTTON_FOUR,
                    MouseButton::WheelDown => GHOSTTY_MOUSE_BUTTON_FIVE,
                }),
                None => a.ghostty_mouse_event_clear_button(self.mouse_event),
            }
            let mods = if input.ctrl { GHOSTTY_MODS_CTRL } else { 0 } | if input.alt { GHOSTTY_MODS_ALT } else { 0 };
            a.ghostty_mouse_event_set_mods(self.mouse_event, mods as GhosttyMods);
            a.ghostty_mouse_event_set_position(self.mouse_event, GhosttyMousePosition {
                x: (input.col as f32 + 0.5) * cell_w as f32,
                y: (input.row as f32 + 0.5) * cell_h as f32,
            });
            let mut bytes = vec![0u8; 128];
            let mut len = 0;
            if a.ghostty_mouse_encoder_encode(self.mouse_encoder, self.mouse_event, bytes.as_mut_ptr().cast(), bytes.len(), &mut len) != GHOSTTY_SUCCESS {
                return None;
            }
            bytes.truncate(len);
            Some(bytes)
        }
    }

    pub fn position(&self) -> Position {
        unsafe {
            let history: u64 = self.get(GHOSTTY_TERMINAL_DATA_SCROLLBACK_ROWS);
            Position {
                row: history + self.get::<u16>(GHOSTTY_TERMINAL_DATA_CURSOR_Y) as u64,
                col: self.get(GHOSTTY_TERMINAL_DATA_CURSOR_X),
                alt: self.get::<GhosttyTerminalScreen>(GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN) == GHOSTTY_TERMINAL_SCREEN_ALTERNATE,
            }
        }
    }

    /// Track the cursor cell so a block's start survives reflow and scrollback pruning.
    pub fn mark(&mut self) -> Result<u64, String> {
        let mark = self.next_mark;
        self.next_mark += 1;
        unsafe {
            let mut reference = ptr::null_mut();
            check(self.api.ghostty_terminal_grid_ref_track(self.inner, GhosttyPoint {
                tag: GHOSTTY_POINT_TAG_ACTIVE,
                value: GhosttyPointValue { coordinate: GhosttyPointCoordinate {
                    x: self.get(GHOSTTY_TERMINAL_DATA_CURSOR_X),
                    y: self.get::<u16>(GHOSTTY_TERMINAL_DATA_CURSOR_Y) as u32,
                } },
            }, &mut reference))?;
            self.marks.insert(mark, (reference, self.get(GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN)));
        }
        Ok(mark)
    }

    /// Track the start of screen row `row` (for a block whose own start scrolled out of range).
    pub fn mark_at(&mut self, row: u64) -> Option<u64> {
        let mark = self.next_mark;
        unsafe {
            let mut reference = ptr::null_mut();
            let point = GhosttyPoint { tag: GHOSTTY_POINT_TAG_SCREEN, value: GhosttyPointValue { coordinate: GhosttyPointCoordinate { x: 0, y: row as u32 } } };
            if self.api.ghostty_terminal_grid_ref_track(self.inner, point, &mut reference) != GHOSTTY_SUCCESS { return None; }
            self.marks.insert(mark, (reference, self.get(GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN)));
        }
        self.next_mark += 1;
        Some(mark)
    }

    /// The screen row a mark points at now, if it still resolves on the active screen.
    pub fn mark_row(&self, mark: u64) -> Option<u64> {
        let (reference, screen) = self.marks.get(&mark)?;
        unsafe {
            if *screen != self.get::<GhosttyTerminalScreen>(GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN) { return None; }
            let mut point = GhosttyPointCoordinate::default();
            (self.api.ghostty_tracked_grid_ref_point(*reference, GHOSTTY_POINT_TAG_SCREEN, &mut point) == GHOSTTY_SUCCESS).then_some(point.y as u64)
        }
    }

    pub fn unmark(&mut self, mark: u64) {
        if let Some((reference, _)) = self.marks.remove(&mark) {
            unsafe { self.api.ghostty_tracked_grid_ref_free(reference) };
        }
    }

    /// Screen rows `start..end`, read straight from the grid (at most the last 2000). Resolve a
    /// tracked start with `mark_row` first; an unresolvable mark should capture nothing.
    pub fn capture(&self, start: u64, end: u64) -> Result<Vec<Row>, String> {
        unsafe {
            let scrollbar: GhosttyTerminalScrollbar = self.get(GHOSTTY_TERMINAL_DATA_SCROLLBAR);
            let hi = end.min(scrollbar.total);
            let lo = start.min(hi).max(hi.saturating_sub(2000));
            let cols: u16 = self.get(GHOSTTY_TERMINAL_DATA_COLS);
            let style_reader = self.api.grid_style_reader()?;
            let mut palette = [GhosttyColorRgb::default(); 256];
            check(self.api.ghostty_terminal_get(self.inner, GHOSTTY_TERMINAL_DATA_COLOR_PALETTE, palette.as_mut_ptr().cast()))?;
            let color = |c: GhosttyStyleColor| match c.tag {
                GHOSTTY_STYLE_COLOR_RGB => Some(pack(c.value.rgb)),
                GHOSTTY_STYLE_COLOR_PALETTE => Some(pack(palette[c.value.palette as usize])),
                _ => None,
            };
            let mut rows = Vec::with_capacity((hi - lo) as usize);
            for y in lo..hi {
                let mut line: Row = Vec::new();
                let mut run: Option<Run> = None;
                for x in 0..cols {
                    let mut reference = sized::<GhosttyGridRef>();
                    check(self.api.ghostty_terminal_grid_ref(self.inner, GhosttyPoint {
                        tag: GHOSTTY_POINT_TAG_SCREEN,
                        value: GhosttyPointValue { coordinate: GhosttyPointCoordinate { x, y: y as u32 } },
                    }, &mut reference))?;
                    let mut raw = 0;
                    check(self.api.ghostty_grid_ref_cell(&reference, &mut raw))?;
                    let mut wide = GHOSTTY_CELL_WIDE_NARROW;
                    check(self.api.ghostty_cell_get(raw, GHOSTTY_CELL_DATA_WIDE, (&mut wide as *mut GhosttyCellWide).cast()))?;
                    if wide == GHOSTTY_CELL_WIDE_SPACER_HEAD || wide == GHOSTTY_CELL_WIDE_SPACER_TAIL { continue; }
                    let mut style = sized::<GhosttyStyle>();
                    check(style_reader(&reference, &mut style))?;
                    let mut background = color(style.bg_color);
                    let mut tag = GHOSTTY_CELL_CONTENT_CODEPOINT;
                    check(self.api.ghostty_cell_get(raw, GHOSTTY_CELL_DATA_CONTENT_TAG, (&mut tag as *mut GhosttyCellContentTag).cast()))?;
                    if tag == GHOSTTY_CELL_CONTENT_BG_COLOR_RGB {
                        let mut c = GhosttyColorRgb::default();
                        check(self.api.ghostty_cell_get(raw, GHOSTTY_CELL_DATA_COLOR_RGB, (&mut c as *mut GhosttyColorRgb).cast()))?;
                        background = Some(pack(c));
                    } else if tag == GHOSTTY_CELL_CONTENT_BG_COLOR_PALETTE {
                        let mut index = 0u8;
                        check(self.api.ghostty_cell_get(raw, GHOSTTY_CELL_DATA_COLOR_PALETTE, (&mut index as *mut u8).cast()))?;
                        background = Some(pack(palette[index as usize]));
                    }
                    let mut fg = color(style.fg_color);
                    if style.inverse { std::mem::swap(&mut fg, &mut background); }
                    let next = Run { text: String::new(), col: x as usize, width: 0, fg, bg: background, bold: style.bold, italic: style.italic, underline: style.underline != 0 };
                    let same = run.as_ref().is_some_and(|r| (r.fg, r.bg, r.bold, r.italic, r.underline) == (next.fg, next.bg, next.bold, next.italic, next.underline));
                    if !same { if let Some(done) = run.take() { line.push(done); } }
                    let current = run.get_or_insert(next);
                    current.width += if wide == GHOSTTY_CELL_WIDE_WIDE { 2 } else { 1 };
                    let mut stack = [0u32; 32];
                    let mut len = 0;
                    let result = self.api.ghostty_grid_ref_graphemes(&reference, stack.as_mut_ptr(), stack.len(), &mut len);
                    let mut spill = Vec::new();
                    let points = if result == GHOSTTY_OUT_OF_SPACE {
                        spill.resize(len, 0);
                        check(self.api.ghostty_grid_ref_graphemes(&reference, spill.as_mut_ptr(), spill.len(), &mut len))?;
                        &spill[..len]
                    } else {
                        check(result)?;
                        &stack[..len]
                    };
                    if style.invisible || points.is_empty() { current.text.push(' '); }
                    else { for p in points { if let Some(c) = char::from_u32(*p) { current.text.push(if c == '\0' { ' ' } else { c }); } } }
                }
                if let Some(run) = run { line.push(run); }
                rows.push(line);
            }
            Ok(rows)
        }
    }
}

/// Ghostty's key for a GPUiX-style key name.
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
        _ if key.len() == 1 && key.as_bytes()[0].is_ascii_alphabetic() => GHOSTTY_KEY_A + (key.as_bytes()[0].to_ascii_lowercase() - b'a') as i32,
        _ if key.len() == 1 && key.as_bytes()[0].is_ascii_digit() => GHOSTTY_KEY_DIGIT_0 + (key.as_bytes()[0] - b'0') as i32,
        _ if key.starts_with('f') => key[1..].parse::<i32>().ok().filter(|n| (1..=25).contains(n)).map_or(GHOSTTY_KEY_UNIDENTIFIED, |n| GHOSTTY_KEY_F1 + n - 1),
        _ => GHOSTTY_KEY_UNIDENTIFIED,
    }
}

/// Text of a captured row with trailing blanks trimmed.
pub fn row_text(row: &[Run]) -> String {
    let mut text = String::new();
    let mut col = 0;
    for run in row {
        text.extend(std::iter::repeat_n(' ', run.col.saturating_sub(col)));
        text.push_str(&run.text);
        col = run.col + run.width;
    }
    text.trim_end().to_owned()
}

/// Whether a row shows anything (ink or a coloured background).
pub fn row_visible(row: &[Run]) -> bool {
    row.iter().any(|r| !r.text.trim().is_empty() || r.bg.is_some())
}
