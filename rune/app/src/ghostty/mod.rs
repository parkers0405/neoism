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

pub struct Capture {
    pub rows: Vec<Row>,
    pub start: u64,
    pub truncated: bool,
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
}

impl Drop for Vt {
    fn drop(&mut self) {
        unsafe {
            for (_, (reference, _)) in self.marks.drain() { self.api.ghostty_tracked_grid_ref_free(reference); }
            self.api.ghostty_terminal_free(self.inner);
        }
    }
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
        let mut vt = Vt { api: api.clone(), inner: ptr::null_mut(), effects: Box::new(Effects { api, writes: Vec::new(), dark: true }), marks: HashMap::new(), next_mark: 1 };
        unsafe {
            let a = &vt.api;
            check(a.ghostty_terminal_new(ptr::null(), &mut vt.inner, cols, rows))?;
            check(a.ghostty_terminal_resize(vt.inner, cols, rows, 10, 20))?;
            // Ghostty's application default (`grapheme-width-method=unicode`).
            let grapheme_mode = GhosttyTerminalModeConfig { mode: 2027, value: true };
            check(a.ghostty_terminal_set(vt.inner, GHOSTTY_TERMINAL_OPT_MODE_DEFAULT, (&grapheme_mode as *const GhosttyTerminalModeConfig).cast()))?;
            let max_lines = 2000usize;
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

    pub fn resize(&mut self, cols: u16, rows: u16) {
        unsafe { self.api.ghostty_terminal_resize(self.inner, cols.clamp(2, 500), rows.clamp(1, 200), 10, 20) };
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

    pub fn unmark(&mut self, mark: u64) {
        if let Some((reference, _)) = self.marks.remove(&mark) {
            unsafe { self.api.ghostty_tracked_grid_ref_free(reference) };
        }
    }

    /// Rows from `mark` (or `start`) up to `end` (exclusive), read straight from the grid.
    /// An unresolvable mark (pruned or reset) captures nothing rather than unrelated output.
    pub fn capture(&self, start: u64, end: u64, mark: Option<u64>) -> Result<Capture, String> {
        unsafe {
            let scrollbar: GhosttyTerminalScrollbar = self.get(GHOSTTY_TERMINAL_DATA_SCROLLBAR);
            let total = scrollbar.total;
            let requested_start = match mark {
                Some(mark) => {
                    let mut point = GhosttyPointCoordinate::default();
                    let resolved = self.marks.get(&mark).is_some_and(|(reference, screen)| {
                        *screen == self.get::<GhosttyTerminalScreen>(GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN)
                            && self.api.ghostty_tracked_grid_ref_point(*reference, GHOSTTY_POINT_TAG_SCREEN, &mut point) == GHOSTTY_SUCCESS
                    });
                    if !resolved {
                        let end = end.min(total);
                        return Ok(Capture { rows: Vec::new(), start: end, truncated: true });
                    }
                    point.y as u64
                }
                None => start,
            };
            let hi = end.min(total);
            let lo = requested_start.min(hi).max(hi.saturating_sub(2000));
            let truncated = lo != requested_start || hi != end || end < requested_start;
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
            Ok(Capture { rows, start: lo, truncated })
        }
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
