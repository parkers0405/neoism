//! Reusable, renderer-owned file browser modal.
//!
//! Paths are canonical host paths supplied by a trusted host/daemon location
//! discovery response. Display labels never become filesystem paths.

use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;
use unicode_segmentation::UnicodeSegmentation;
use web_time::Instant;

use crate::animation::ease_out_cubic;

use crate::event::{
    KeyDescriptor, KeyState, LogicalKey, Modifiers, NamedKey, PointerButton, UiEvent,
    WheelMode,
};
use crate::panels::file_tree::icons::folder_icon;
use crate::panels::file_tree::{
    icon_for_file, FONT_SIZE, FRAME_RADIUS, FRAME_STROKE, ICON_FONT_SIZE,
};
use crate::primitives::geom::intersect_rect;
use crate::primitives::IdeTheme;

const ORDER: u8 = 29;
const USEFUL_ROWS: f32 = 16.0;
const ROW_HEIGHT: f32 = 32.0;
const ROW_PADDING_X: f32 = 16.0;
const OPEN_SECONDS: f32 = 0.18;
const DIRECTORY_SECONDS: f32 = 0.16;
const CLICK_SECONDS: f32 = 0.16;

fn transition_remaining(start: Option<Instant>, seconds: f32) -> f32 {
    start.map_or(0.0, |start| {
        1.0 - ease_out_cubic((start.elapsed().as_secs_f32() / seconds).clamp(0.0, 1.0))
    })
}

fn fade_color(mut color: [u8; 4], opacity: f32) -> [u8; 4] {
    color[3] = (color[3] as f32 * opacity.clamp(0.0, 1.0)) as u8;
    color
}

#[derive(Clone, Debug, Default)]
struct TextCursor {
    byte: usize,
    anchor: Option<usize>,
    scroll_x: f32,
    stops: Vec<(usize, f32)>,
}

impl TextCursor {
    fn reset(&mut self, text: &str) {
        self.byte = text.len();
        self.anchor = None;
        self.scroll_x = 0.0;
        self.stops.clear();
    }

    fn snap_after_edit(&mut self, text: &str) {
        self.byte = text
            .grapheme_indices(true)
            .map(|(byte, _)| byte)
            .find(|byte| *byte >= self.byte)
            .unwrap_or(text.len());
        self.anchor = None;
        self.stops.clear();
    }

    fn selection(&self) -> Option<std::ops::Range<usize>> {
        self.anchor
            .filter(|anchor| *anchor != self.byte)
            .map(|anchor| anchor.min(self.byte)..anchor.max(self.byte))
    }

    fn move_to(&mut self, byte: usize, extend: bool) {
        if extend {
            self.anchor.get_or_insert(self.byte);
        } else {
            self.anchor = None;
        }
        self.byte = byte;
    }

    fn key(&mut self, text: &mut String, key: NamedKey, extend: bool) -> bool {
        let previous = || {
            text.grapheme_indices(true)
                .map(|(i, _)| i)
                .take_while(|i| *i < self.byte)
                .last()
                .unwrap_or(0)
        };
        let next = || {
            text.grapheme_indices(true)
                .map(|(i, _)| i)
                .find(|i| *i > self.byte)
                .unwrap_or(text.len())
        };
        match key {
            NamedKey::ArrowLeft => {
                let byte = if !extend {
                    self.selection().map(|range| range.start)
                } else {
                    None
                }
                .unwrap_or_else(previous);
                self.move_to(byte, extend);
            }
            NamedKey::ArrowRight => {
                let byte = if !extend {
                    self.selection().map(|range| range.end)
                } else {
                    None
                }
                .unwrap_or_else(next);
                self.move_to(byte, extend);
            }
            NamedKey::Home => self.move_to(0, extend),
            NamedKey::End => self.move_to(text.len(), extend),
            NamedKey::Backspace | NamedKey::Delete => {
                let range = self.selection().unwrap_or_else(|| {
                    if key == NamedKey::Backspace {
                        previous()..self.byte
                    } else {
                        self.byte..next()
                    }
                });
                let changed = !range.is_empty();
                self.byte = range.start;
                text.replace_range(range, "");
                self.snap_after_edit(text);
                return changed;
            }
            _ => {}
        }
        false
    }

    fn insert(&mut self, text: &mut String, value: &str) {
        let range = self.selection().unwrap_or(self.byte..self.byte);
        self.byte = range.start + value.len();
        text.replace_range(range, value);
        self.snap_after_edit(text);
    }

    fn click(&mut self, x: f32, text: &str) {
        self.byte = self
            .stops
            .iter()
            .min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs()))
            .map(|(byte, _)| *byte)
            .unwrap_or(text.len());
        self.anchor = None;
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FileBrowserDensity {
    pub font_size: f32,
    pub icon_size: f32,
    pub row_height: f32,
    pub row_padding_x: f32,
    pub frame_radius: f32,
    pub frame_stroke: f32,
    pub header_height: f32,
    pub toolbar_height: f32,
    pub footer_height: f32,
    pub sidebar_width: f32,
    pub visual_control_height: f32,
    pub touch_hit_height: f32,
}

pub const FILE_BROWSER_DENSITY: FileBrowserDensity = FileBrowserDensity {
    font_size: FONT_SIZE,
    icon_size: ICON_FONT_SIZE,
    row_height: ROW_HEIGHT,
    row_padding_x: ROW_PADDING_X,
    frame_radius: FRAME_RADIUS,
    frame_stroke: FRAME_STROKE,
    header_height: 48.0,
    toolbar_height: 48.0,
    footer_height: 56.0,
    sidebar_width: 184.0,
    visual_control_height: 32.0,
    touch_hit_height: 44.0,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileBrowserMode {
    AttachImage,
    OpenFile,
    ChooseDirectory,
}

impl FileBrowserMode {
    fn title(self) -> &'static str {
        match self {
            Self::AttachImage => "Attach a picture",
            Self::OpenFile => "Open file",
            Self::ChooseDirectory => "Choose a folder",
        }
    }
    fn action(self) -> &'static str {
        match self {
            Self::AttachImage => "Attach",
            Self::OpenFile => "Open",
            Self::ChooseDirectory => "Select",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileBrowserEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileBrowserLocation {
    pub kind: String,
    pub label: String,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileBrowserRequest {
    LoadLocations,
    ListDirectory { path: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileBrowserSelection {
    pub mode: FileBrowserMode,
    pub path: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FileBrowserGeometry {
    pub viewport: [f32; 4],
    pub card: [f32; 4],
    pub sidebar: Option<[f32; 4]>,
    pub list: [f32; 4],
    pub cancel: [f32; 4],
    pub accept: [f32; 4],
    pub cancel_hit: [f32; 4],
    pub accept_hit: [f32; 4],
    pub narrow: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileBrowserPalette {
    pub backdrop: u32,
    pub card: u32,
    pub sidebar: u32,
    pub body: u32,
    pub border: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FileBrowserHit {
    Scrim,
    Header,
    Sidebar(usize),
    Back,
    Forward,
    Up,
    Path,
    Search,
    Row { visible: usize, source: usize },
    Footer,
    Cancel,
    Accept,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct VisibleRowGeometry {
    full: [f32; 4],
    visible: [f32; 4],
}

impl FileBrowserPalette {
    pub fn from_theme(theme: &IdeTheme) -> Self {
        Self {
            backdrop: theme.bg,
            card: theme.panel_bg(),
            sidebar: theme.panel_bg(),
            body: theme.panel_bg(),
            border: theme.border,
        }
    }
}

impl FileBrowserDensity {
    fn with_font_scale(mut self, scale: f32) -> Self {
        let scale = scale.clamp(0.5, 3.0);
        self.font_size *= scale;
        self.icon_size *= scale;
        self
    }
}

#[derive(Debug)]
pub struct FileBrowserModal {
    active: bool,
    mode: FileBrowserMode,
    current_path: String,
    path_input: String,
    path_cursor: TextCursor,
    search: String,
    search_cursor: TextCursor,
    search_focused: bool,
    path_focused: bool,
    entries: Vec<FileBrowserEntry>,
    selected: usize,
    selection_explicit: bool,
    scroll_px: f32,
    back: Vec<String>,
    forward: Vec<String>,
    recents: Vec<String>,
    locations: Vec<FileBrowserLocation>,
    pending_requests: Vec<FileBrowserRequest>,
    pending_selection: Option<FileBrowserSelection>,
    last_geometry: Option<FileBrowserGeometry>,
    error: Option<String>,
    listing_ready: bool,
    safe_area: [f32; 4],
    font_scale: f32,
    opened_at: Option<Instant>,
    listing_at: Option<Instant>,
    clicked_at: Option<(FileBrowserHit, Instant)>,
    hovered: Option<FileBrowserHit>,
}

impl Default for FileBrowserModal {
    fn default() -> Self {
        Self::new()
    }
}

impl FileBrowserModal {
    pub fn new() -> Self {
        Self {
            active: false,
            mode: FileBrowserMode::OpenFile,
            current_path: String::new(),
            path_input: String::new(),
            path_cursor: TextCursor::default(),
            search: String::new(),
            search_cursor: TextCursor::default(),
            search_focused: false,
            path_focused: false,
            entries: Vec::new(),
            selected: 0,
            selection_explicit: false,
            scroll_px: 0.0,
            back: Vec::new(),
            forward: Vec::new(),
            recents: Vec::new(),
            locations: Vec::new(),
            pending_requests: Vec::new(),
            pending_selection: None,
            last_geometry: None,
            error: None,
            listing_ready: false,
            safe_area: [0.0; 4],
            font_scale: 1.0,
            opened_at: None,
            listing_at: None,
            clicked_at: None,
            hovered: None,
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn is_animating(&self) -> bool {
        self.active
            && (transition_remaining(self.opened_at, OPEN_SECONDS) > 0.0
                || transition_remaining(self.listing_at, DIRECTORY_SECONDS) > 0.0
                || transition_remaining(
                    self.clicked_at.map(|(_, time)| time),
                    CLICK_SECONDS,
                ) > 0.0)
    }

    fn animated_geometry(&self, viewport: [f32; 4]) -> FileBrowserGeometry {
        let safe = self.safe_viewport(viewport);
        let mut g = Self::geometry(safe);
        let room = (safe[1] + safe[3] - g.card[1] - g.card[3]).max(0.0);
        let dy = 12.0_f32.min(room) * transition_remaining(self.opened_at, OPEN_SECONDS);
        for rect in [
            &mut g.card,
            &mut g.list,
            &mut g.cancel,
            &mut g.accept,
            &mut g.cancel_hit,
            &mut g.accept_hit,
        ] {
            rect[1] += dy;
        }
        if let Some(side) = g.sidebar.as_mut() {
            side[1] += dy;
        }
        g
    }
    fn click_strength(&self, hit: FileBrowserHit) -> f32 {
        self.clicked_at
            .filter(|(clicked, _)| *clicked == hit)
            .map_or(0.0, |(_, time)| {
                transition_remaining(Some(time), CLICK_SECONDS)
            })
    }

    pub fn mode(&self) -> FileBrowserMode {
        self.mode
    }
    pub fn current_path(&self) -> &str {
        &self.current_path
    }
    pub fn recents(&self) -> &[String] {
        &self.recents
    }
    pub fn set_recents(&mut self, recents: Vec<String>) {
        self.recents = sanitize_recents(recents);
    }
    pub fn set_font_scale(&mut self, scale: f32) {
        self.font_scale = scale.clamp(0.5, 3.0);
    }
    /// Insets are `[top, right, bottom, left]` in logical canvas pixels.
    pub fn set_safe_area(&mut self, top: f32, right: f32, bottom: f32, left: f32) {
        self.safe_area = [top.max(0.0), right.max(0.0), bottom.max(0.0), left.max(0.0)];
        // Do not accept input against a card laid out with obsolete insets.
        self.last_geometry = None;
    }

    fn safe_viewport(&self, [x, y, w, h]: [f32; 4]) -> [f32; 4] {
        let [top, right, bottom, left] = self.safe_area;
        [
            x + left,
            y + top,
            (w - left - right).max(1.0),
            (h - top - bottom).max(1.0),
        ]
    }

    pub fn open(&mut self, mode: FileBrowserMode, start: &str, recents: Vec<String>) {
        self.active = true;
        self.mode = mode;
        self.recents = sanitize_recents(recents);
        self.back.clear();
        self.forward.clear();
        self.search.clear();
        self.search_focused = false;
        self.path_focused = true;
        self.search_cursor.reset("");
        self.opened_at = Some(Instant::now());
        self.listing_at = None;
        self.clicked_at = None;
        self.hovered = None;
        self.pending_selection = None;
        self.last_geometry = None;
        self.error = None;
        self.current_path = start.to_string();
        self.path_input = start.to_string();
        self.path_cursor.reset(&self.path_input);
        self.entries.clear();
        self.locations.clear();
        self.listing_ready = false;
        self.selected = 0;
        self.selection_explicit = false;
        self.scroll_px = 0.0;
        self.pending_requests.clear();
        self.pending_requests
            .push(FileBrowserRequest::LoadLocations);
    }

    pub fn close(&mut self) {
        self.active = false;
        self.opened_at = None;
        self.listing_at = None;
        self.clicked_at = None;
        self.hovered = None;
        self.pending_requests.clear();
        self.last_geometry = None;
    }

    pub fn geometry(viewport: [f32; 4]) -> FileBrowserGeometry {
        let density = FILE_BROWSER_DENSITY;
        let [vx, vy, vw, vh] = viewport;
        let narrow = vw < 520.0;
        let margin = if narrow { 8.0 } else { 18.0 };
        let toolbar_h = if narrow {
            density.touch_hit_height * 2.0
        } else {
            density.toolbar_height
        };
        let footer_h = if narrow {
            density.touch_hit_height
        } else {
            density.footer_height
        };
        let w = (if narrow {
            vw - margin * 2.0
        } else {
            960.0_f32.min(vw - margin * 2.0)
        })
        .max(1.0);
        let desired_h = density.header_height
            + toolbar_h
            + footer_h
            + density.row_height * USEFUL_ROWS;
        let h = desired_h.min(vh - margin * 2.0).max(1.0);
        let x = vx + (vw - w) * 0.5;
        let y = vy + (vh - h) * 0.5;
        let side = if narrow {
            None
        } else {
            Some([
                x,
                y + density.header_height,
                density.sidebar_width,
                h - density.header_height - footer_h,
            ])
        };
        let body_x = x + side.map(|_| density.sidebar_width).unwrap_or(0.0);
        let body_w = w - side.map(|_| density.sidebar_width).unwrap_or(0.0);
        let list = [
            body_x,
            y + density.header_height + toolbar_h,
            body_w,
            h - density.header_height - toolbar_h - footer_h,
        ];
        let available = (w - density.row_padding_x * 2.0 - 8.0).max(2.0);
        let button_w = 128.0_f32.min(available * 0.58);
        let cancel_w = 96.0_f32.min(available - button_w);
        let button_h = density.visual_control_height;
        let by = y + h - footer_h + (footer_h - button_h) * 0.5;
        let accept = [
            x + w - density.row_padding_x - button_w,
            by,
            button_w,
            button_h,
        ];
        let cancel = [accept[0] - cancel_w - 8.0, by, cancel_w, button_h];
        let hit_h = if narrow {
            density.touch_hit_height
        } else {
            button_h
        };
        let hit_pad = (hit_h - button_h) * 0.5;
        FileBrowserGeometry {
            viewport,
            card: [x, y, w, h],
            sidebar: side,
            list,
            cancel,
            accept,
            cancel_hit: [cancel[0], cancel[1] - hit_pad, cancel[2], hit_h],
            accept_hit: [accept[0], accept[1] - hit_pad, accept[2], hit_h],
            narrow,
        }
    }

    pub fn occlusion_rect(&self) -> Option<[f32; 4]> {
        self.active
            .then(|| self.last_geometry.map(|g| g.card))
            .flatten()
    }
    pub fn occlusion_rect_for(&self, viewport: [f32; 4]) -> Option<[f32; 4]> {
        self.active.then(|| {
            let base = Self::geometry(self.safe_viewport(viewport)).card;
            let painted = self.animated_geometry(viewport).card;
            [base[0], base[1], base[2], painted[1] + painted[3] - base[1]]
        })
    }

    pub fn drain_requests(&mut self) -> Vec<FileBrowserRequest> {
        std::mem::take(&mut self.pending_requests)
    }
    pub fn take_selection(&mut self) -> Option<FileBrowserSelection> {
        self.pending_selection.take()
    }

    pub fn pointer_kind_at(
        &self,
        x: f32,
        y: f32,
    ) -> crate::widgets::modal::ModalPointerKind {
        use crate::widgets::modal::ModalPointerKind;
        if !self.active {
            return ModalPointerKind::Default;
        }
        match self.hit_test(x, y) {
            Some(FileBrowserHit::Path | FileBrowserHit::Search) => ModalPointerKind::Text,
            Some(FileBrowserHit::Accept) if !self.can_accept() => {
                ModalPointerKind::Default
            }
            Some(FileBrowserHit::Back) if self.back.is_empty() => {
                ModalPointerKind::Default
            }
            Some(FileBrowserHit::Forward) if self.forward.is_empty() => {
                ModalPointerKind::Default
            }
            Some(
                FileBrowserHit::Sidebar(_)
                | FileBrowserHit::Row { .. }
                | FileBrowserHit::Back
                | FileBrowserHit::Forward
                | FileBrowserHit::Up
                | FileBrowserHit::Cancel
                | FileBrowserHit::Accept,
            ) => ModalPointerKind::Pointer,
            _ => ModalPointerKind::Default,
        }
    }

    /// Path and search are the only keyboard-bearing controls in the picker.
    /// Rows, navigation buttons, and footer actions must remain keyboard-free.
    pub fn text_entry_at(&self, x: f32, y: f32) -> bool {
        self.active
            && self.last_geometry.is_some_and(|g| {
                contains(path_control_rect(g, false), x, y)
                    || contains(search_control_rect(g, false), x, y)
            })
    }

    pub fn set_listing(&mut self, path: &str, entries: Vec<FileBrowserEntry>) -> bool {
        let Ok(path) = normalize_browser_path(path) else {
            return false;
        };
        if !self.active || path != self.current_path {
            return false;
        }
        self.entries = entries
            .into_iter()
            .filter(|e| !e.name.starts_with('.') && valid_child_name(&e.name))
            .filter(|e| {
                e.is_dir
                    || match self.mode {
                        FileBrowserMode::OpenFile => true,
                        FileBrowserMode::AttachImage => is_supported_image_name(&e.name),
                        FileBrowserMode::ChooseDirectory => false,
                    }
            })
            .collect();
        self.entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        self.selected = 0;
        self.selection_explicit = false;
        self.scroll_px = 0.0;
        self.error = None;
        self.listing_ready = true;
        self.listing_at = Some(Instant::now());
        true
    }

    pub fn set_locations(&mut self, locations: Vec<FileBrowserLocation>) -> bool {
        if !self.active {
            return false;
        }
        self.locations = locations
            .into_iter()
            .filter(|location| normalize_browser_path(&location.path).is_ok())
            .collect();
        let preferred = normalize_browser_path(&self.current_path)
            .ok()
            .filter(|path| {
                self.locations
                    .iter()
                    .any(|location| host_path_within(path, &location.path))
            })
            .or_else(|| {
                self.locations
                    .iter()
                    .find(|location| location.kind == "workspace")
                    .or_else(|| self.locations.first())
                    .map(|location| location.path.clone())
            });
        let Some(path) = preferred else {
            self.set_error("No available locations");
            return false;
        };
        let draft = (self.path_input != display_path(&self.current_path)
            || self.path_cursor.anchor.is_some()
            || self.path_cursor.byte != self.path_input.len())
        .then(|| (self.path_input.clone(), self.path_cursor.clone()));
        let search_draft = (self.search.clone(), self.search_cursor.clone());
        self.current_path = path;
        self.request_current();
        self.search = search_draft.0;
        self.search_cursor = search_draft.1;
        if let Some((text, cursor)) = draft {
            self.path_input = text;
            self.path_cursor = cursor;
        }
        true
    }

    pub fn set_error(&mut self, message: impl Into<String>) {
        self.error = Some(message.into());
        self.listing_ready = false;
    }

    /// Native twin of the daemon-backed host pump. The same relative-path
    /// validator runs before joining anything onto `root`.
    pub fn fulfill_requests_native(&mut self, _root: &Path) {
        for request in self.drain_requests() {
            let FileBrowserRequest::ListDirectory { path } = request else {
                continue;
            };
            let resolved = match resolve_native_browser_path(&self.locations, &path) {
                Ok(path) => path,
                Err(error) => {
                    self.set_error(error);
                    continue;
                }
            };
            match std::fs::read_dir(&resolved) {
                Ok(read) => {
                    let entries = read
                        .filter_map(Result::ok)
                        .filter_map(|entry| {
                            let name = entry.file_name().to_str()?.to_string();
                            let meta = entry.metadata().ok()?;
                            Some(FileBrowserEntry {
                                name,
                                is_dir: meta.is_dir(),
                                size: meta.is_file().then(|| meta.len()),
                            })
                        })
                        .collect();
                    self.set_listing(&path, entries);
                }
                Err(error) => self.set_error(format!("Could not open folder: {error}")),
            }
        }
    }

    pub fn press_named(&mut self, key: NamedKey) {
        let _ = self.press_named_with_modifiers(key, Modifiers::empty());
    }

    pub fn press_named_with_modifiers(
        &mut self,
        key: NamedKey,
        modifiers: Modifiers,
    ) -> bool {
        let descriptor = KeyDescriptor {
            physical: crate::event::PhysicalKey(0),
            logical: LogicalKey::Named(key),
            state: KeyState::Pressed,
            modifiers,
            repeat: false,
        };
        self.handle_key(&descriptor)
    }

    pub fn input_text(&mut self, text: &str) {
        self.insert_text(text);
    }
    pub fn resolve_native_selection(
        &self,
        path: &str,
    ) -> Result<std::path::PathBuf, String> {
        resolve_native_browser_path(&self.locations, path)
    }

    fn visible_entries(&self) -> Vec<(usize, &FileBrowserEntry)> {
        let q = self.search.to_lowercase();
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| q.is_empty() || e.name.to_lowercase().contains(&q))
            .collect()
    }

    fn request_current(&mut self) {
        self.entries.clear();
        self.listing_ready = false;
        self.selected = 0;
        self.selection_explicit = false;
        self.scroll_px = 0.0;
        self.error = None;
        self.path_input = display_path(&self.current_path);
        if self
            .clicked_at
            .is_some_and(|(hit, _)| matches!(hit, FileBrowserHit::Row { .. }))
        {
            self.clicked_at = None;
        }
        self.path_cursor.reset(&self.path_input);
        self.search.clear();
        self.search_cursor.reset("");
        self.listing_at = None;
        self.hovered = None;
        self.pending_requests
            .push(FileBrowserRequest::ListDirectory {
                path: self.current_path.clone(),
            });
    }

    pub fn navigate(&mut self, path: &str) -> bool {
        let Ok(path) = normalize_browser_path(path) else {
            self.set_error("Invalid location");
            return false;
        };
        if path == self.current_path {
            if !self.listing_ready {
                self.request_current();
            }
            return true;
        }
        self.back.push(self.current_path.clone());
        self.forward.clear();
        self.current_path = path;
        self.request_current();
        true
    }

    pub fn go_back(&mut self) -> bool {
        let Some(path) = self.back.pop() else {
            return false;
        };
        self.forward.push(self.current_path.clone());
        self.current_path = path;
        self.request_current();
        true
    }
    pub fn go_forward(&mut self) -> bool {
        let Some(path) = self.forward.pop() else {
            return false;
        };
        self.back.push(self.current_path.clone());
        self.current_path = path;
        self.request_current();
        true
    }
    pub fn go_up(&mut self) -> bool {
        let normalized = self.current_path.replace('\\', "/");
        let Some(index) = normalized.trim_end_matches('/').rfind('/') else {
            return false;
        };
        let parent = if index == 0 {
            "/".to_string()
        } else {
            normalized[..index].to_string()
        };
        if !self
            .locations
            .iter()
            .any(|location| host_path_within(&parent, &location.path))
        {
            return false;
        }
        self.navigate(&parent)
    }

    fn selected_entry(&self) -> Option<&FileBrowserEntry> {
        self.selection_explicit
            .then(|| self.visible_entries().get(self.selected).map(|(_, e)| *e))
            .flatten()
    }

    fn activate_selected(&mut self) -> bool {
        if self.mode == FileBrowserMode::ChooseDirectory
            && self.selected_entry().is_none()
        {
            return self.accept_path(self.current_path.clone());
        }
        let Some(entry) = self.selected_entry().cloned() else {
            return false;
        };
        let path = join_path(&self.current_path, &entry.name);
        if entry.is_dir {
            self.navigate(&path)
        } else {
            self.accept_path(path)
        }
    }

    fn can_accept(&self) -> bool {
        self.listing_ready
            && self.error.is_none()
            && (self.mode == FileBrowserMode::ChooseDirectory
                || self.selected_entry().is_some())
    }

    fn accept_button(&mut self) -> bool {
        if self.mode == FileBrowserMode::ChooseDirectory {
            let path = self
                .selected_entry()
                .filter(|entry| entry.is_dir)
                .map(|entry| join_path(&self.current_path, &entry.name))
                .unwrap_or_else(|| self.current_path.clone());
            self.accept_path(path)
        } else {
            self.activate_selected()
        }
    }

    fn accept_path(&mut self, path: String) -> bool {
        if !self.listing_ready || self.error.is_some() {
            return false;
        }
        if self.mode == FileBrowserMode::ChooseDirectory
            && path != self.current_path
            && !self.selected_entry().is_some_and(|entry| {
                entry.is_dir && join_path(&self.current_path, &entry.name) == path
            })
        {
            return false;
        }
        if self.mode == FileBrowserMode::AttachImage && !is_supported_image_name(&path) {
            return false;
        }
        self.remember_recent(self.current_path.clone());
        self.pending_selection = Some(FileBrowserSelection {
            mode: self.mode,
            path,
        });
        self.active = false;
        self.last_geometry = None;
        true
    }

    fn remember_recent(&mut self, path: String) {
        self.recents.retain(|p| p != &path);
        self.recents.insert(0, path);
        self.recents.truncate(8);
    }

    pub fn scroll_pixels(&mut self, delta: f32) {
        let rows = self.visible_entries().len() as f32;
        let list_h = self.last_geometry.map(|g| g.list[3]).unwrap_or(300.0);
        let max = (rows * FILE_BROWSER_DENSITY.row_height - list_h).max(0.0);
        self.scroll_px = (self.scroll_px + delta).clamp(0.0, max);
    }

    pub fn handle_event(&mut self, event: &UiEvent) -> bool {
        if !self.active {
            return false;
        }
        match event {
            UiEvent::Key(k) if k.state == KeyState::Pressed => self.handle_key(k),
            UiEvent::Text(text) => {
                self.insert_text(text);
                true
            }
            UiEvent::PointerDown {
                button: PointerButton::Left,
                x,
                y,
                click_count,
                ..
            } => {
                self.pointer_down(*x, *y, *click_count);
                true
            }
            // This modal deliberately commits on pointer-down. Native and web
            // Chrome both route that event directly to `pointer_down`; making
            // pointer-up a second activation owner would double-open rows and
            // could target a different row after a wheel/async listing update.
            UiEvent::PointerUp {
                button: PointerButton::Left,
                ..
            } => true,
            UiEvent::Wheel { dy, mode, .. } => {
                let px = match mode {
                    WheelMode::Pixel => *dy,
                    WheelMode::Line => *dy * FILE_BROWSER_DENSITY.row_height,
                    WheelMode::Page => *dy * 260.0,
                };
                self.scroll_pixels(px);
                true
            }
            // Resize units differ between native physical pixels and the web
            // logical canvas. Invalidate now; the next render is the single
            // authority that can publish correctly scaled hit geometry.
            UiEvent::Resize { .. } => {
                self.last_geometry = None;
                true
            }
            UiEvent::PointerMove { x, y, .. } => {
                self.hovered = self.hit_test(*x, *y);
                true
            }
            UiEvent::PointerLeave => {
                self.hovered = None;
                true
            }
            UiEvent::PointerDown { .. } | UiEvent::PointerUp { .. } => true,
            _ => false,
        }
    }

    fn handle_key(&mut self, key: &KeyDescriptor) -> bool {
        let shortcut = key.modifiers.intersects(Modifiers::CTRL | Modifiers::META);
        // Printable commits arrive separately so IME and key events never insert twice.
        if let LogicalKey::Character(text) = &key.logical {
            if shortcut && text.eq_ignore_ascii_case("a") {
                let (text, cursor) = if self.path_focused {
                    (&self.path_input, &mut self.path_cursor)
                } else {
                    self.search_focused = true;
                    (&self.search, &mut self.search_cursor)
                };
                cursor.anchor = Some(0);
                cursor.byte = text.len();
            }
            return true;
        }
        let LogicalKey::Named(named) = key.logical else {
            return true;
        };
        if key.modifiers.contains(Modifiers::ALT) {
            if named == NamedKey::ArrowLeft {
                self.go_back();
                return true;
            }
            if named == NamedKey::ArrowRight {
                self.go_forward();
                return true;
            }
        }
        match named {
            NamedKey::Escape => self.close(),
            NamedKey::Tab => {
                self.path_focused = !self.path_focused;
                self.search_focused = !self.path_focused;
            }
            NamedKey::ArrowUp | NamedKey::ArrowDown => {
                let count = self.visible_entries().len();
                self.selected = if !self.selection_explicit {
                    0
                } else if named == NamedKey::ArrowUp {
                    self.selected.saturating_sub(1)
                } else {
                    (self.selected + 1).min(count.saturating_sub(1))
                };
                self.selection_explicit = count > 0;
                self.path_focused = false;
                self.search_focused = false;
                self.reveal_selected();
            }
            NamedKey::Enter => {
                if self.path_focused {
                    let path = self.path_input.clone();
                    if self.navigate(&path) {
                        self.path_focused = false;
                    }
                } else {
                    self.activate_selected();
                }
            }
            NamedKey::ArrowLeft
            | NamedKey::ArrowRight
            | NamedKey::Home
            | NamedKey::End
            | NamedKey::Backspace
            | NamedKey::Delete => {
                let changed = if self.path_focused {
                    self.path_cursor.key(
                        &mut self.path_input,
                        named,
                        key.modifiers.contains(Modifiers::SHIFT),
                    )
                } else {
                    self.search_focused = true;
                    self.search_cursor.key(
                        &mut self.search,
                        named,
                        key.modifiers.contains(Modifiers::SHIFT),
                    )
                };
                if changed && self.search_focused {
                    self.selected = 0;
                    self.selection_explicit = false;
                    self.scroll_px = 0.0;
                }
            }
            _ => {}
        }
        true
    }

    fn insert_text(&mut self, text: &str) {
        let filtered: String = text.chars().filter(|c| !c.is_control()).collect();
        if filtered.is_empty() {
            return;
        }
        if self.path_focused {
            self.path_cursor.insert(&mut self.path_input, &filtered);
        } else {
            self.search_focused = true;
            self.search_cursor.insert(&mut self.search, &filtered);
            self.selected = 0;
            self.selection_explicit = false;
            self.scroll_px = 0.0;
        }
    }

    fn reveal_selected(&mut self) {
        let Some(g) = self.last_geometry else {
            return;
        };
        let top = self.selected as f32 * FILE_BROWSER_DENSITY.row_height;
        if top < self.scroll_px {
            self.scroll_px = top;
        }
        if top + FILE_BROWSER_DENSITY.row_height > self.scroll_px + g.list[3] {
            self.scroll_px = top + FILE_BROWSER_DENSITY.row_height - g.list[3];
        }
    }

    pub fn pointer_down(&mut self, x: f32, y: f32, click_count: u8) {
        if !self.active {
            return;
        }
        let Some(hit) = self.hit_test(x, y) else {
            return;
        };
        let same_click = self.clicked_at.is_some_and(|(previous, _)| previous == hit);
        self.clicked_at = Some((hit, Instant::now()));
        if !matches!(hit, FileBrowserHit::Path | FileBrowserHit::Search) {
            self.path_focused = false;
            self.search_focused = false;
        }
        match hit {
            FileBrowserHit::Scrim => self.close(),
            FileBrowserHit::Cancel => self.close(),
            FileBrowserHit::Accept => {
                let _ = self.accept_button();
            }
            FileBrowserHit::Back => {
                self.go_back();
            }
            FileBrowserHit::Forward => {
                self.go_forward();
            }
            FileBrowserHit::Up => {
                self.go_up();
            }
            FileBrowserHit::Search => {
                self.search_focused = true;
                self.path_focused = false;
                self.search_cursor.click(x, &self.search);
                if click_count >= 2 {
                    self.search_cursor.anchor = Some(0);
                    self.search_cursor.byte = self.search.len();
                }
            }
            FileBrowserHit::Path => {
                self.path_focused = true;
                self.search_focused = false;
                self.path_cursor.click(x, &self.path_input);
                if click_count >= 2 {
                    self.path_cursor.anchor = Some(0);
                    self.path_cursor.byte = self.path_input.len();
                }
            }
            FileBrowserHit::Sidebar(row) => {
                let places = place_paths(&self.locations, &self.recents);
                if let Some((_, path, _)) = places.get(row) {
                    let path = path.clone();
                    self.navigate(&path);
                }
            }
            FileBrowserHit::Row { visible, source } => {
                // Both indices are checked again so an async listing mutation
                // can never redirect a stale hit to another entry.
                if self
                    .visible_entries()
                    .get(visible)
                    .is_some_and(|(s, _)| *s == source)
                {
                    let already_selected =
                        self.selection_explicit && self.selected == visible;
                    self.selected = visible;
                    self.selection_explicit = true;
                    if click_count >= 2 && same_click && already_selected {
                        self.activate_selected();
                    }
                }
            }
            FileBrowserHit::Header | FileBrowserHit::Footer => {}
        }
    }

    fn hit_test(&self, x: f32, y: f32) -> Option<FileBrowserHit> {
        let g = self.last_geometry?;
        if !contains(g.card, x, y) {
            return Some(FileBrowserHit::Scrim);
        }

        // Highest visual controls own their inflated hit boxes first.
        if contains(g.cancel_hit, x, y) {
            return Some(FileBrowserHit::Cancel);
        }
        if contains(g.accept_hit, x, y) {
            return Some(FileBrowserHit::Accept);
        }
        for (index, hit) in [
            FileBrowserHit::Back,
            FileBrowserHit::Forward,
            FileBrowserHit::Up,
        ]
        .into_iter()
        .enumerate()
        {
            if contains(nav_control_rect(g, index, false), x, y) {
                return Some(hit);
            }
        }
        if contains(search_control_rect(g, false), x, y) {
            return Some(FileBrowserHit::Search);
        }
        if contains(path_control_rect(g, false), x, y) {
            return Some(FileBrowserHit::Path);
        }

        if let Some(side) = g.sidebar {
            if contains(side, x, y) {
                let row = ((y - side[1] - 4.0) / FILE_BROWSER_DENSITY.row_height).floor()
                    as isize;
                let count = place_paths(&self.locations, &self.recents).len();
                return if row >= 0
                    && (row as usize) < count
                    && side[1] + 4.0 + (row + 1) as f32 * FILE_BROWSER_DENSITY.row_height
                        <= side[1] + side[3]
                {
                    Some(FileBrowserHit::Sidebar(row as usize))
                } else {
                    Some(FileBrowserHit::Footer)
                };
            }
        }
        if contains(g.list, x, y) {
            let row = ((y - g.list[1] + self.scroll_px) / FILE_BROWSER_DENSITY.row_height)
                .floor() as usize;
            if let Some((source, _)) = self.visible_entries().get(row) {
                if visible_row_geometry(
                    g.list,
                    row,
                    self.scroll_px,
                    FILE_BROWSER_DENSITY.row_height,
                )
                .is_some_and(|geometry| contains(geometry.visible, x, y))
                {
                    return Some(FileBrowserHit::Row {
                        visible: row,
                        source: *source,
                    });
                }
            }
            return Some(FileBrowserHit::Footer);
        }
        if y < g.list[1] {
            Some(FileBrowserHit::Header)
        } else {
            Some(FileBrowserHit::Footer)
        }
    }

    pub fn render(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        viewport: [f32; 4],
        theme: &IdeTheme,
    ) {
        if !self.active {
            self.last_geometry = None;
            return;
        }
        let mut g = self.animated_geometry(viewport);
        let [vx, vy, vw, vh] = viewport;
        let density = FILE_BROWSER_DENSITY.with_font_scale(self.font_scale);
        let opts = DrawOpts {
            font_size: density.font_size,
            ..DrawOpts::default()
        };
        let available = (g.card[2] - density.row_padding_x * 2.0 - 8.0).max(2.0);
        let accept_w = (sugarloaf.text_mut().measure(self.mode.action(), &opts) + 32.0)
            .max(128.0)
            .min(available * 0.58);
        let cancel_w = (sugarloaf.text_mut().measure("Cancel", &opts) + 32.0)
            .max(96.0)
            .min(available - accept_w);
        for rect in [&mut g.accept, &mut g.accept_hit] {
            rect[0] = g.card[0] + g.card[2] - density.row_padding_x - accept_w;
            rect[2] = accept_w;
        }
        for rect in [&mut g.cancel, &mut g.cancel_hit] {
            rect[0] = g.accept[0] - cancel_w - 8.0;
            rect[2] = cancel_w;
        }
        self.last_geometry = Some(g);
        let opening = transition_remaining(self.opened_at, OPEN_SECONDS);
        let entering = transition_remaining(self.listing_at, DIRECTORY_SECONDS);
        let row_offset = entering * 12.0;
        let row_opacity = 1.0 - entering * 0.8;
        let palette = FileBrowserPalette::from_theme(theme);
        sugarloaf.rect(
            None,
            vx,
            vy,
            vw,
            vh,
            theme.f32_alpha(palette.backdrop, 0.72 - opening * 0.35),
            0.0,
            ORDER,
        );
        let [x, y, w, h] = g.card;
        sugarloaf.rounded_rect(
            None,
            x,
            y,
            w,
            h,
            theme.f32(palette.border),
            0.0,
            density.frame_radius,
            ORDER + 1,
        );
        sugarloaf.rounded_rect(
            None,
            x + density.frame_stroke,
            y + density.frame_stroke,
            w - density.frame_stroke * 2.0,
            h - density.frame_stroke * 2.0,
            theme.f32(palette.card),
            0.0,
            (density.frame_radius - density.frame_stroke).max(0.0),
            ORDER + 2,
        );
        sugarloaf.rect(
            None,
            x + density.frame_stroke,
            y + density.header_height,
            w - density.frame_stroke * 2.0,
            1.0,
            theme.f32(palette.border),
            0.0,
            ORDER + 3,
        );
        draw_text_in_rect_bold(
            sugarloaf,
            [
                x + density.row_padding_x,
                y,
                w - density.row_padding_x * 2.0,
                density.header_height,
            ],
            self.mode.title(),
            density.font_size + 1.0,
            theme.u8(theme.fg),
            false,
            true,
        );
        if let Some(side) = g.sidebar {
            sugarloaf.rect(
                None,
                side[0],
                side[1],
                side[2],
                side[3],
                theme.f32(palette.sidebar),
                0.0,
                ORDER + 2,
            );
            for (i, (label, path, icon)) in place_paths(&self.locations, &self.recents)
                .into_iter()
                .enumerate()
            {
                let ry = side[1] + 4.0 + i as f32 * density.row_height;
                if ry + density.row_height > side[1] + side[3] {
                    break;
                }
                let place_hit = FileBrowserHit::Sidebar(i);
                if path == self.current_path || self.hovered == Some(place_hit) {
                    sugarloaf.rounded_rect(
                        None,
                        side[0] + 4.0,
                        ry,
                        side[2] - 8.0,
                        density.row_height,
                        theme.f32_alpha(
                            theme.fg,
                            0.07 + self.click_strength(place_hit) * 0.12,
                        ),
                        0.0,
                        5.0,
                        ORDER + 3,
                    );
                }
                draw_text_in_rect(
                    sugarloaf,
                    [
                        side[0] + density.row_padding_x,
                        ry,
                        16.0,
                        density.row_height,
                    ],
                    icon,
                    density.icon_size,
                    folder_icon(path == self.current_path).1,
                    true,
                    true,
                );
                draw_text_in_rect(
                    sugarloaf,
                    [
                        side[0] + density.row_padding_x + 24.0,
                        ry,
                        side[2] - density.row_padding_x * 2.0 - 24.0,
                        density.row_height,
                    ],
                    &label,
                    density.font_size,
                    theme.u8(theme.fg),
                    false,
                    true,
                );
            }
        }
        for (i, glyph) in ["‹", "›", "↑"].into_iter().enumerate() {
            let rect = nav_control_rect(g, i, true);
            let hit = [
                FileBrowserHit::Back,
                FileBrowserHit::Forward,
                FileBrowserHit::Up,
            ][i];
            sugarloaf.rounded_rect(
                None,
                rect[0],
                rect[1],
                rect[2],
                rect[3],
                theme.f32_alpha(
                    theme.fg,
                    0.04 + self.click_strength(hit) * 0.14
                        + if self.hovered == Some(hit) { 0.03 } else { 0.0 },
                ),
                0.0,
                5.0,
                ORDER + 3,
            );
            draw_text_in_rect(
                sugarloaf,
                rect,
                glyph,
                density.icon_size,
                theme.u8(theme.muted),
                true,
                true,
            );
        }
        let path_rect = path_control_rect(g, true);
        input_box(
            sugarloaf,
            path_rect,
            &self.path_input,
            "",
            &mut self.path_cursor,
            self.path_focused,
            density.font_size,
            theme,
        );
        input_box(
            sugarloaf,
            search_control_rect(g, true),
            &self.search,
            "Filter",
            &mut self.search_cursor,
            self.search_focused,
            density.font_size,
            theme,
        );
        sugarloaf.rect(
            None,
            g.list[0],
            g.list[1],
            g.list[2],
            g.list[3],
            theme.f32(palette.body),
            0.0,
            ORDER + 2,
        );
        let rows = self.visible_entries();
        let first = (self.scroll_px / density.row_height).floor() as usize;
        for (visual, (source, entry)) in rows.iter().skip(first).enumerate() {
            let row = first + visual;
            let Some(row_geometry) =
                visible_row_geometry(g.list, row, self.scroll_px, density.row_height)
            else {
                if g.list[1] + row as f32 * density.row_height - self.scroll_px
                    >= g.list[1] + g.list[3]
                {
                    break;
                }
                continue;
            };
            let ry = row_geometry.full[1];
            let row_clip = row_geometry.visible;
            let row_hit = FileBrowserHit::Row {
                visible: row,
                source: *source,
            };
            if (self.selection_explicit && row == self.selected)
                || self.hovered == Some(row_hit)
            {
                if let Some(selection) = intersect_rect(
                    [g.list[0] + 4.0, ry, g.list[2] - 8.0, density.row_height],
                    g.list,
                ) {
                    sugarloaf.rounded_rect(
                        None,
                        selection[0],
                        selection[1],
                        selection[2],
                        selection[3],
                        theme.f32_alpha(
                            theme.fg,
                            (0.07 + self.click_strength(row_hit) * 0.14) * row_opacity,
                        ),
                        0.0,
                        6.0,
                        ORDER + 3,
                    );
                }
            }
            let (glyph, color) = if entry.is_dir {
                folder_icon(false)
            } else {
                icon_for_file(&entry.name)
            };
            draw_text_in_rect_clipped(
                sugarloaf,
                [
                    g.list[0] + density.row_padding_x + row_offset,
                    ry,
                    16.0,
                    density.row_height,
                ],
                row_clip,
                glyph,
                density.icon_size,
                fade_color(color, row_opacity),
                true,
                true,
            );
            draw_text_in_rect_clipped(
                sugarloaf,
                [
                    g.list[0] + density.row_padding_x + 24.0 + row_offset,
                    ry,
                    (g.list[2] - if entry.is_dir { 72.0 } else { 118.0 } - row_offset)
                        .max(1.0),
                    density.row_height,
                ],
                row_clip,
                &entry.name,
                density.font_size,
                fade_color(theme.u8(theme.fg), row_opacity),
                false,
                true,
            );
            if let Some(size) = entry.size.filter(|_| !entry.is_dir) {
                let label = format_size(size);
                draw_text_in_rect_clipped(
                    sugarloaf,
                    [g.list[0] + g.list[2] - 78.0, ry, 66.0, density.row_height],
                    row_clip,
                    &label,
                    11.0,
                    theme.u8(theme.muted),
                    true,
                    true,
                );
            }
            if entry.is_dir {
                draw_text_in_rect_clipped(
                    sugarloaf,
                    [g.list[0] + g.list[2] - 32.0, ry, 16.0, density.row_height],
                    row_clip,
                    "›",
                    density.icon_size,
                    fade_color(theme.u8(theme.muted), row_opacity),
                    true,
                    true,
                );
            }
            let _ = source;
        }
        if rows.is_empty() {
            let label = if self.error.is_some() {
                "Folder unavailable"
            } else if !self.listing_ready {
                "Loading..."
            } else if !self.search.is_empty() {
                "No matches"
            } else if self.mode == FileBrowserMode::ChooseDirectory {
                "No subfolders"
            } else {
                "No files here"
            };
            draw_text_in_rect(
                sugarloaf,
                g.list,
                label,
                density.font_size,
                theme.u8(theme.muted),
                true,
                true,
            );
        }
        let footer_h = if g.narrow {
            density.touch_hit_height
        } else {
            density.footer_height
        };
        sugarloaf.rect(
            None,
            x + density.frame_stroke,
            y + h - footer_h,
            w - density.frame_stroke * 2.0,
            1.0,
            theme.f32(palette.border),
            0.0,
            ORDER + 3,
        );
        if let Some(error) = &self.error {
            draw_text_in_rect(
                sugarloaf,
                [
                    x + density.row_padding_x,
                    y + h - footer_h,
                    (g.cancel[0] - x - density.row_padding_x * 2.0).max(1.0),
                    footer_h,
                ],
                error,
                11.0,
                theme.u8(theme.red),
                false,
                true,
            );
        }
        button(
            sugarloaf,
            g.cancel,
            "Cancel",
            false,
            true,
            self.click_strength(FileBrowserHit::Cancel),
            density.font_size,
            theme,
        );
        button(
            sugarloaf,
            g.accept,
            self.mode.action(),
            true,
            self.can_accept(),
            self.click_strength(FileBrowserHit::Accept),
            density.font_size,
            theme,
        );
    }
}

pub fn normalize_workspace_path(input: &str) -> Result<String, ()> {
    let replaced = input.trim().trim_start_matches("./").replace('\\', "/");
    if replaced == "/" || replaced.is_empty() || replaced == "." {
        return Ok(String::new());
    }
    let path = Path::new(&replaced);
    if path.is_absolute() {
        return Err(());
    }
    let mut parts = Vec::new();
    for c in path.components() {
        match c {
            Component::Normal(s) => {
                let s = s.to_str().ok_or(())?;
                if s.is_empty() {
                    return Err(());
                }
                parts.push(s);
            }
            Component::CurDir => {}
            _ => return Err(()),
        }
    }
    Ok(parts.join("/"))
}

pub fn normalize_browser_path(input: &str) -> Result<String, ()> {
    let input = input.trim();
    let absolute = input.starts_with('/')
        || input.starts_with("\\\\")
        || (input.len() >= 3
            && input.as_bytes()[1] == b':'
            && matches!(input.as_bytes()[2], b'/' | b'\\'));
    if !absolute || input.split(['/', '\\']).any(|component| component == "..") {
        return Err(());
    }
    Ok(input.to_string())
}

pub fn is_supported_image_name(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|x| x.to_str())
        .map(|x| {
            matches!(
                x.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "avif" | "svg"
            )
        })
        .unwrap_or(false)
}

pub fn image_mime_for_name(name: &str) -> Option<&'static str> {
    match Path::new(name)
        .extension()?
        .to_str()?
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        "avif" => Some("image/avif"),
        "svg" => Some("image/svg+xml"),
        _ => None,
    }
}

pub fn resolve_native_browser_path(
    locations: &[FileBrowserLocation],
    requested: &str,
) -> Result<std::path::PathBuf, String> {
    normalize_browser_path(requested).map_err(|_| "Invalid picker path".to_string())?;
    let candidate = Path::new(requested)
        .canonicalize()
        .map_err(|_| "That location is unavailable".to_string())?;
    if !locations
        .iter()
        .any(|location| candidate.starts_with(&location.path))
    {
        return Err("That location is outside the allowed picker roots".into());
    }
    Ok(candidate)
}

fn sanitize_recents(items: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for item in items {
        if let Ok(path) = normalize_browser_path(&item) {
            if !out.contains(&path) {
                out.push(path);
            }
        }
    }
    out.truncate(8);
    out
}
fn valid_child_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
}
fn join_path(parent: &str, child: &str) -> String {
    if parent.ends_with(['/', '\\']) {
        format!("{parent}{child}")
    } else if parent.contains('\\') && !parent.contains('/') {
        format!("{parent}\\{child}")
    } else {
        format!("{parent}/{child}")
    }
}
fn host_path_within(path: &str, root: &str) -> bool {
    let path = path.replace('\\', "/").trim_end_matches('/').to_string();
    let root = root.replace('\\', "/").trim_end_matches('/').to_string();
    path == root
        || path
            .strip_prefix(&root)
            .is_some_and(|suffix| suffix.starts_with('/'))
}
fn display_path(path: &str) -> String {
    path.into()
}
fn place_paths(
    locations: &[FileBrowserLocation],
    recents: &[String],
) -> Vec<(String, String, &'static str)> {
    let icon = |kind: &str| match kind {
        "home" => "󰋜",
        "documents" => "󰈙",
        "downloads" => "󰉍",
        "pictures" => "󰋩",
        _ => "󰉋",
    };
    let valid_recents = recents
        .iter()
        .filter(|path| {
            locations
                .iter()
                .any(|location| host_path_within(path, &location.path))
        })
        .collect::<Vec<_>>();
    let mut out = valid_recents
        .first()
        .map(|path| ("Recent".to_string(), (*path).clone(), "󰋚"))
        .into_iter()
        .collect::<Vec<_>>();
    out.extend(locations.iter().map(|location| {
        (
            location.label.clone(),
            location.path.clone(),
            icon(&location.kind),
        )
    }));
    for p in valid_recents.into_iter().take(4) {
        out.push((
            p.replace('\\', "/")
                .rsplit('/')
                .next()
                .unwrap_or(p)
                .to_string(),
            p.clone(),
            "󰋚",
        ));
    }
    out
}
/// Half-open containment gives shared edges exactly one owner. This matters on
/// touch layouts, where adjacent 44 px control slots intentionally have no gap.
fn contains(r: [f32; 4], x: f32, y: f32) -> bool {
    r[2] > 0.0
        && r[3] > 0.0
        && x >= r[0]
        && x < r[0] + r[2]
        && y >= r[1]
        && y < r[1] + r[3]
}
fn visible_row_geometry(
    list: [f32; 4],
    row: usize,
    scroll_px: f32,
    row_height: f32,
) -> Option<VisibleRowGeometry> {
    let full = [
        list[0],
        list[1] + row as f32 * row_height - scroll_px,
        list[2],
        row_height,
    ];
    intersect_rect(full, list).map(|visible| VisibleRowGeometry { full, visible })
}
fn toolbar_rect(g: FileBrowserGeometry, visual: bool) -> [f32; 4] {
    let density = FILE_BROWSER_DENSITY;
    let bar_h = if g.narrow {
        density.touch_hit_height
    } else {
        density.toolbar_height
    };
    let h = if visual || !g.narrow {
        density.visual_control_height
    } else {
        density.touch_hit_height
    };
    [
        g.list[0],
        g.card[1] + density.header_height + (bar_h - h) * 0.5,
        g.list[2],
        h,
    ]
}
fn nav_control_rect(g: FileBrowserGeometry, index: usize, visual: bool) -> [f32; 4] {
    let density = FILE_BROWSER_DENSITY;
    let bar = toolbar_rect(g, visual);
    let hit_side = if g.narrow {
        density.touch_hit_height
    } else {
        density.visual_control_height
    };
    let slot_x = g.list[0] + density.row_padding_x + index as f32 * hit_side;
    if visual {
        [
            slot_x + (hit_side - density.visual_control_height) * 0.5,
            bar[1],
            density.visual_control_height,
            density.visual_control_height,
        ]
    } else if g.narrow {
        [slot_x, bar[1], hit_side, hit_side]
    } else {
        [
            slot_x,
            bar[1],
            density.visual_control_height,
            density.visual_control_height,
        ]
    }
}
fn search_control_rect(g: FileBrowserGeometry, visual: bool) -> [f32; 4] {
    let density = FILE_BROWSER_DENSITY;
    let bar = toolbar_rect(g, visual);
    if g.narrow {
        return [
            g.card[0] + density.row_padding_x,
            bar[1] + density.touch_hit_height,
            (g.card[2] - density.row_padding_x * 2.0).max(1.0),
            bar[3],
        ];
    }
    let width = 150.0;
    [
        g.card[0] + g.card[2] - density.row_padding_x - width,
        bar[1],
        width,
        bar[3],
    ]
}
fn path_control_rect(g: FileBrowserGeometry, visual: bool) -> [f32; 4] {
    let density = FILE_BROWSER_DENSITY;
    let bar = toolbar_rect(g, visual);
    let nav_span = if g.narrow {
        density.touch_hit_height * 3.0
    } else {
        density.visual_control_height * 3.0
    };
    let x = g.list[0] + density.row_padding_x + nav_span + 4.0;
    let right = if g.narrow {
        g.card[0] + g.card[2] - density.row_padding_x
    } else {
        search_control_rect(g, visual)[0] - 6.0
    };
    [x, bar[1], (right - x).max(1.0), bar[3]]
}
fn format_size(n: u64) -> String {
    if n >= 1_048_576 {
        format!("{:.1} MB", n as f64 / 1_048_576.0)
    } else if n >= 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}
fn centered_text_origin(rect: [f32; 4], text_width: f32, font_size: f32) -> [f32; 2] {
    [
        rect[0] + (rect[2] - text_width) * 0.5,
        rect[1] + (rect[3] - font_size) * 0.5,
    ]
}
fn draw_text_in_rect(
    s: &mut Sugarloaf,
    rect: [f32; 4],
    value: &str,
    size: f32,
    color: [u8; 4],
    center_x: bool,
    center_y: bool,
) {
    draw_text_in_rect_with_weight(s, rect, value, size, color, center_x, center_y, false);
}
fn draw_text_in_rect_bold(
    s: &mut Sugarloaf,
    rect: [f32; 4],
    value: &str,
    size: f32,
    color: [u8; 4],
    center_x: bool,
    center_y: bool,
) {
    draw_text_in_rect_with_weight(s, rect, value, size, color, center_x, center_y, true);
}
fn draw_text_in_rect_with_weight(
    s: &mut Sugarloaf,
    rect: [f32; 4],
    value: &str,
    size: f32,
    color: [u8; 4],
    center_x: bool,
    center_y: bool,
    bold: bool,
) {
    draw_text_in_rect_with_weight_clipped(
        s, rect, rect, value, size, color, center_x, center_y, bold,
    );
}
fn draw_text_in_rect_clipped(
    s: &mut Sugarloaf,
    rect: [f32; 4],
    clip: [f32; 4],
    value: &str,
    size: f32,
    color: [u8; 4],
    center_x: bool,
    center_y: bool,
) {
    draw_text_in_rect_with_weight_clipped(
        s, rect, clip, value, size, color, center_x, center_y, false,
    );
}
fn draw_text_in_rect_with_weight_clipped(
    s: &mut Sugarloaf,
    rect: [f32; 4],
    clip: [f32; 4],
    value: &str,
    size: f32,
    color: [u8; 4],
    center_x: bool,
    center_y: bool,
    bold: bool,
) {
    // Keep positioning tied to the full row while clipping raster output to
    // the authoritative list viewport. Re-centering in the visible slice
    // would make first/last rows jump while scrolling.
    let clip = intersect_rect(rect, clip).unwrap_or([rect[0], rect[1], 0.0, 0.0]);
    let opts = DrawOpts {
        font_size: size,
        color,
        bold,
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let width = s.text_mut().measure(value, &opts);
    let [x, y] = centered_text_origin(rect, if center_x { width } else { rect[2] }, size);
    let first = s.text_mut().instance_count();
    s.text_mut().draw(x, y, value, &opts);
    let _ = s
        .text_mut()
        .center_instances_in_rect(first, rect, center_x, center_y);
}
fn input_box(
    s: &mut Sugarloaf,
    r: [f32; 4],
    value: &str,
    placeholder: &str,
    cursor: &mut TextCursor,
    focused: bool,
    font_size: f32,
    theme: &IdeTheme,
) {
    s.rounded_rect(
        None,
        r[0],
        r[1],
        r[2],
        r[3],
        theme.f32(if focused { theme.muted } else { theme.border }),
        0.0,
        5.0,
        ORDER + 3,
    );
    s.rounded_rect(
        None,
        r[0] + 1.0,
        r[1] + 1.0,
        r[2] - 2.0,
        r[3] - 2.0,
        theme.f32(theme.panel_bg()),
        0.0,
        4.0,
        ORDER + 4,
    );
    let inner = [
        r[0] + 10.0,
        r[1] + 2.0,
        (r[2] - 20.0).max(1.0),
        (r[3] - 4.0).max(1.0),
    ];
    let font_size = font_size.min((inner[3] - 4.0).max(1.0));
    let opts = DrawOpts {
        font_size,
        color: theme.u8(if value.is_empty() {
            theme.muted
        } else {
            theme.fg
        }),
        clip_rect: Some(inner),
        ..DrawOpts::default()
    };
    let caret_width = s.text_mut().measure(&value[..cursor.byte], &opts);
    let full_width = s.text_mut().measure(value, &opts);
    if focused {
        cursor.scroll_x = cursor
            .scroll_x
            .max(caret_width - inner[2] + 2.0)
            .min(caret_width)
            .max(0.0);
    } else {
        cursor.scroll_x = cursor.scroll_x.min((full_width - inner[2]).max(0.0));
    }
    let text_x = inner[0] - cursor.scroll_x;
    cursor.stops.clear();
    cursor.stops.push((0, text_x));
    for (start, grapheme) in value.grapheme_indices(true) {
        let end = start + grapheme.len();
        cursor
            .stops
            .push((end, text_x + s.text_mut().measure(&value[..end], &opts)));
    }
    if focused {
        if let Some(range) = cursor.selection() {
            let left = text_x + s.text_mut().measure(&value[..range.start], &opts);
            let right = text_x + s.text_mut().measure(&value[..range.end], &opts);
            if let Some(selection) = intersect_rect(
                [left, inner[1] + 2.0, right - left, inner[3] - 4.0],
                inner,
            ) {
                s.rect(
                    None,
                    selection[0],
                    selection[1],
                    selection[2],
                    selection[3],
                    theme.f32_alpha(theme.accent, 0.25),
                    0.0,
                    ORDER + 5,
                );
            }
        }
    }
    let label = if value.is_empty() { placeholder } else { value };
    let first = s.text_mut().instance_count();
    s.text_mut().draw(
        text_x,
        inner[1] + (inner[3] - font_size) * 0.5,
        label,
        &opts,
    );
    let _ = s
        .text_mut()
        .center_instances_in_rect(first, inner, false, true);
    if focused {
        let x = (text_x + caret_width).clamp(inner[0], inner[0] + inner[2] - 1.0);
        s.rect(
            None,
            x,
            inner[1] + 3.0,
            1.5,
            (inner[3] - 6.0).max(1.0),
            theme.f32(theme.fg),
            0.0,
            ORDER + 6,
        );
    }
}
fn fitted_label_size(requested: f32, measured_width: f32, inner: [f32; 4]) -> f32 {
    requested
        * (inner[2] / measured_width.max(1.0))
            .min(inner[3] / requested.max(1.0))
            .min(1.0)
}

fn button(
    s: &mut Sugarloaf,
    r: [f32; 4],
    label: &str,
    primary: bool,
    enabled: bool,
    pressed: f32,
    font_size: f32,
    theme: &IdeTheme,
) {
    let inset = pressed * 1.5;
    let r = [
        r[0] + inset,
        r[1] + inset,
        r[2] - inset * 2.0,
        r[3] - inset * 2.0,
    ];
    s.rounded_rect(
        None,
        r[0],
        r[1],
        r[2],
        r[3],
        theme.f32_alpha(
            if primary { theme.fg } else { theme.border },
            if enabled { 1.0 } else { 0.35 },
        ),
        0.0,
        6.0,
        ORDER + 3,
    );
    if !primary {
        s.rounded_rect(
            None,
            r[0] + 1.0,
            r[1] + 1.0,
            r[2] - 2.0,
            r[3] - 2.0,
            theme.f32(theme.panel_bg()),
            0.0,
            5.0,
            ORDER + 4,
        );
    }
    let inner = [
        r[0] + 12.0,
        r[1] + 4.0,
        (r[2] - 24.0).max(1.0),
        (r[3] - 8.0).max(1.0),
    ];
    let measured = s.text_mut().measure(
        label,
        &DrawOpts {
            font_size,
            ..DrawOpts::default()
        },
    );
    let size = fitted_label_size(font_size, measured, inner);
    draw_text_in_rect(
        s,
        inner,
        label,
        size,
        theme.u8(if primary { theme.bg } else { theme.fg }),
        true,
        true,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    fn location(kind: &str, label: &str, path: &str) -> FileBrowserLocation {
        FileBrowserLocation {
            kind: kind.into(),
            label: label.into(),
            path: path.into(),
        }
    }
    fn ready(mode: FileBrowserMode) -> FileBrowserModal {
        let mut modal = FileBrowserModal::new();
        modal.open(mode, "", vec![]);
        assert!(modal.set_locations(vec![location(
            "workspace",
            "Workspace",
            "/workspace"
        )]));
        modal
    }
    fn select_all(modal: &mut FileBrowserModal) {
        modal.handle_event(&UiEvent::Key(KeyDescriptor {
            physical: crate::event::PhysicalKey(0),
            logical: LogicalKey::Character("a".into()),
            state: KeyState::Pressed,
            modifiers: Modifiers::CTRL,
            repeat: false,
        }));
    }

    #[test]
    fn key_and_text_event_pairs_insert_each_commit_once() {
        let mut modal = ready(FileBrowserMode::ChooseDirectory);
        select_all(&mut modal);
        let key = KeyDescriptor {
            physical: crate::event::PhysicalKey(0),
            logical: LogicalKey::Character("x".into()),
            state: KeyState::Pressed,
            modifiers: Modifiers::empty(),
            repeat: false,
        };
        modal.handle_event(&UiEvent::Key(key));
        assert_eq!(modal.path_input, "/workspace");
        modal.handle_event(&UiEvent::Text("x".into()));
        assert_eq!(modal.path_input, "x");
        modal.press_named(NamedKey::ArrowLeft);
        modal.handle_event(&UiEvent::Text("y".into()));
        assert_eq!(modal.path_input, "yx");
    }

    #[test]
    fn double_click_cannot_activate_a_different_row_after_layout_motion() {
        let mut modal = ready(FileBrowserMode::ChooseDirectory);
        modal.set_listing(
            "/workspace",
            vec![
                FileBrowserEntry {
                    name: "a".into(),
                    is_dir: true,
                    size: None,
                },
                FileBrowserEntry {
                    name: "b".into(),
                    is_dir: true,
                    size: None,
                },
            ],
        );
        let g = FileBrowserModal::geometry([0.0, 0.0, 1200.0, 800.0]);
        modal.last_geometry = Some(g);
        let x = g.list[0] + 30.0;
        modal.pointer_down(x, g.list[1] + ROW_HEIGHT * 0.5, 1);
        modal.pointer_down(x, g.list[1] + ROW_HEIGHT * 1.5, 2);
        assert_eq!(modal.current_path(), "/workspace");
        modal.pointer_down(x, g.list[1] + ROW_HEIGHT * 1.5, 2);
        assert_eq!(modal.current_path(), "/workspace/b");
    }

    #[test]
    fn address_bar_inserts_at_caret_and_enter_navigates() {
        let mut modal = ready(FileBrowserMode::ChooseDirectory);
        assert!(modal.path_focused);
        select_all(&mut modal);
        modal.input_text("/workspace/ab");
        modal.press_named(NamedKey::ArrowLeft);
        modal.input_text("X");
        assert_eq!(modal.path_input, "/workspace/aXb");
        modal.press_named(NamedKey::Delete);
        assert_eq!(modal.path_input, "/workspace/aX");
        modal.press_named(NamedKey::Enter);
        assert_eq!(modal.current_path(), "/workspace/aX");
        assert!(!modal.path_focused);
        assert!(modal
            .drain_requests()
            .iter()
            .any(|request| matches!(request,
                FileBrowserRequest::ListDirectory { path } if path == "/workspace/aX"
            )));
    }

    #[test]
    fn address_bar_keeps_focus_after_invalid_enter_and_supports_selection() {
        let mut modal = ready(FileBrowserMode::ChooseDirectory);
        select_all(&mut modal);
        modal.input_text("relative");
        modal.press_named(NamedKey::Enter);
        assert!(modal.path_focused);
        assert!(modal.error.is_some());
        select_all(&mut modal);
        modal.input_text("/workspace/good");
        modal.press_named(NamedKey::End);
        modal.press_named_with_modifiers(NamedKey::ArrowLeft, Modifiers::SHIFT);
        modal.input_text("D");
        assert_eq!(modal.path_input, "/workspace/gooD");
        modal.press_named(NamedKey::Enter);
        assert_eq!(modal.current_path(), "/workspace/gooD");
    }

    #[test]
    fn text_cursor_moves_and_deletes_whole_graphemes() {
        let mut text = "aé👩‍💻e\u{301}".to_string();
        let mut cursor = TextCursor::default();
        cursor.reset(&text);
        cursor.key(&mut text, NamedKey::ArrowLeft, false);
        assert_eq!(cursor.byte, "aé👩‍💻".len());
        cursor.key(&mut text, NamedKey::Backspace, false);
        assert_eq!(text, "aée\u{301}");
        cursor.key(&mut text, NamedKey::Home, false);
        cursor.key(&mut text, NamedKey::Delete, false);
        assert_eq!(text, "ée\u{301}");
        cursor.key(&mut text, NamedKey::End, false);
        cursor.key(&mut text, NamedKey::ArrowLeft, true);
        cursor.insert(&mut text, "Z");
        assert_eq!(text, "éZ");
    }

    #[test]
    fn edits_that_join_graphemes_leave_a_valid_caret_stop() {
        let mut text = "\u{301}".to_string();
        let mut cursor = TextCursor::default();
        cursor.insert(&mut text, "e");
        assert_eq!(cursor.byte, text.len());
        cursor.key(&mut text, NamedKey::Backspace, false);
        assert_eq!(text, "");
        text = "🇺 x 🇸".into();
        cursor.byte = "🇺 x ".len();
        cursor.anchor = Some("🇺".len());
        cursor.key(&mut text, NamedKey::Delete, false);
        assert_eq!(text, "🇺🇸");
        assert_eq!(cursor.byte, text.len());
    }

    #[test]
    fn text_cursor_click_uses_measured_character_positions() {
        let mut cursor = TextCursor::default();
        cursor.stops = vec![(0, 10.0), (1, 16.0), (3, 32.0)];
        cursor.click(30.0, "aé");
        assert_eq!(cursor.byte, 3);
        cursor.click(17.0, "aé");
        let mut text = "aé".to_string();
        cursor.insert(&mut text, "!");
        assert_eq!(text, "a!é");
    }

    #[test]
    fn location_reply_preserves_selection_and_caret_before_typing() {
        let mut modal = FileBrowserModal::new();
        modal.open(FileBrowserMode::ChooseDirectory, "/workspace", vec![]);
        select_all(&mut modal);
        modal.set_locations(vec![location("workspace", "Workspace", "/workspace")]);
        assert_eq!(modal.path_cursor.selection(), Some(0.."/workspace".len()));
        modal.input_text("/workspace/next");
        assert_eq!(modal.path_input, "/workspace/next");
        modal.open(FileBrowserMode::ChooseDirectory, "/workspace", vec![]);
        modal.press_named(NamedKey::ArrowLeft);
        let byte = modal.path_cursor.byte;
        modal.set_locations(vec![location("workspace", "Workspace", "/workspace")]);
        assert_eq!(modal.path_cursor.byte, byte);
        modal.input_text("X");
        assert_eq!(modal.path_input, "/workspacXe");
    }

    #[test]
    fn location_replies_preserve_path_and_filter_drafts_after_focus_changes() {
        let mut modal = FileBrowserModal::new();
        modal.open(FileBrowserMode::ChooseDirectory, "/workspace", vec![]);
        select_all(&mut modal);
        modal.input_text("/workspace/typed");
        modal.press_named(NamedKey::Tab);
        modal.input_text("projects");
        modal.press_named(NamedKey::ArrowLeft);
        let search_byte = modal.search_cursor.byte;
        assert!(modal.set_locations(vec![location(
            "workspace",
            "Workspace",
            "/workspace"
        )]));
        assert_eq!(modal.path_input, "/workspace/typed");
        assert_eq!(modal.search, "projects");
        assert_eq!(modal.search_cursor.byte, search_byte);
        assert!(modal.search_focused);
    }

    #[test]
    fn folder_single_click_selects_for_confirmation_and_double_click_opens() {
        let entries = vec![FileBrowserEntry {
            name: "child".into(),
            is_dir: true,
            size: None,
        }];
        let mut modal = ready(FileBrowserMode::ChooseDirectory);
        modal.set_listing("/workspace", entries.clone());
        assert!(modal.selected_entry().is_none());
        let g = FileBrowserModal::geometry([0.0, 0.0, 1200.0, 800.0]);
        modal.last_geometry = Some(g);
        let (x, y) = (g.list[0] + 30.0, g.list[1] + ROW_HEIGHT * 0.5);
        modal.pointer_down(x, y, 1);
        assert_eq!(modal.current_path(), "/workspace");
        assert!(modal.accept_button());
        assert_eq!(modal.take_selection().unwrap().path, "/workspace/child");
        modal.open(FileBrowserMode::ChooseDirectory, "/workspace", vec![]);
        modal.set_listing("/workspace", entries);
        modal.last_geometry = Some(g);
        modal.pointer_down(x, y, 1);
        modal.pointer_down(x, y, 2);
        assert_eq!(modal.current_path(), "/workspace/child");
        assert!(modal.take_selection().is_none());
        assert!(!modal.listing_ready);
        assert!(!modal.selection_explicit);
    }

    #[test]
    fn pointer_cursor_classification_uses_only_actionable_hits() {
        use crate::widgets::modal::ModalPointerKind;
        let mut modal = ready(FileBrowserMode::AttachImage);
        let g = FileBrowserModal::geometry([0.0, 0.0, 1200.0, 800.0]);
        modal.last_geometry = Some(g);
        let (x, y) = point(path_control_rect(g, false));
        assert_eq!(modal.pointer_kind_at(x, y), ModalPointerKind::Text);
        let (x, y) = point(g.accept_hit);
        assert_eq!(modal.pointer_kind_at(x, y), ModalPointerKind::Default);
        modal.set_listing(
            "/workspace",
            vec![FileBrowserEntry {
                name: "photo.png".into(),
                is_dir: false,
                size: None,
            }],
        );
        let (x, y) = (g.list[0] + 30.0, g.list[1] + ROW_HEIGHT * 0.5);
        assert_eq!(modal.pointer_kind_at(x, y), ModalPointerKind::Pointer);
        modal.pointer_down(x, y, 1);
        let (x, y) = point(g.accept_hit);
        assert_eq!(modal.pointer_kind_at(x, y), ModalPointerKind::Pointer);
        modal.close();
        assert_eq!(modal.pointer_kind_at(x, y), ModalPointerKind::Default);
    }

    #[test]
    fn opening_click_and_directory_animations_are_bounded_and_safe() {
        let mut modal = ready(FileBrowserMode::ChooseDirectory);
        assert!(modal.is_animating());
        for viewport in [[0.0, 0.0, 390.0, 500.0], [0.0, 0.0, 1200.0, 800.0]] {
            let painted = modal.animated_geometry(viewport).card;
            let cover = modal.occlusion_rect_for(viewport).unwrap();
            assert!(painted[1] >= viewport[1]);
            assert!(painted[1] + painted[3] <= viewport[1] + viewport[3]);
            assert!(cover[1] <= painted[1]);
            assert!(cover[1] + cover[3] >= painted[1] + painted[3] - 0.01);
        }
        let old = Instant::now() - web_time::Duration::from_secs(2);
        modal.opened_at = Some(old);
        assert!(!modal.is_animating());
        modal.set_listing(
            "/workspace",
            vec![FileBrowserEntry {
                name: "child".into(),
                is_dir: true,
                size: None,
            }],
        );
        assert!(modal.is_animating());
        modal.listing_at = Some(old);
        let g = FileBrowserModal::geometry([0.0, 0.0, 1200.0, 800.0]);
        modal.last_geometry = Some(g);
        modal.pointer_down(g.list[0] + 30.0, g.list[1] + ROW_HEIGHT * 0.5, 1);
        assert!(modal.is_animating());
        modal.clicked_at = modal.clicked_at.map(|(hit, _)| (hit, old));
        assert!(!modal.is_animating());
        modal.close();
        assert!(!modal.is_animating());
    }

    #[test]
    fn button_labels_fit_with_padding_at_all_supported_scales() {
        assert_eq!(FileBrowserMode::ChooseDirectory.action(), "Select");
        for width in [280.0, 390.0, 1200.0] {
            let g = FileBrowserModal::geometry([0.0, 0.0, width, 700.0]);
            for scale in [0.5, 1.0, 1.5, 3.0] {
                let requested = FILE_BROWSER_DENSITY.font_size * scale;
                for (label, r) in [
                    ("Attach", g.accept),
                    ("Open", g.accept),
                    ("Select", g.accept),
                    ("Cancel", g.cancel),
                ] {
                    let measured = label.len() as f32 * requested * 0.8;
                    let inner = [r[0] + 12.0, r[1] + 4.0, r[2] - 24.0, r[3] - 8.0];
                    let size = fitted_label_size(requested, measured, inner);
                    assert!(size <= inner[3] + 0.001);
                    assert!(measured * size / requested <= inner[2] + 0.001);
                }
            }
        }
    }

    #[test]
    fn invisible_partial_sidebar_rows_cannot_be_clicked() {
        let mut modal = ready(FileBrowserMode::ChooseDirectory);
        let mut g = FileBrowserModal::geometry([0.0, 0.0, 1200.0, 800.0]);
        let side = g.sidebar.as_mut().unwrap();
        side[3] = ROW_HEIGHT * 1.5 + 4.0;
        let (x, y) = (side[0] + 20.0, side[1] + 4.0 + ROW_HEIGHT * 1.25);
        modal.last_geometry = Some(g);
        assert_eq!(modal.hit_test(x, y), Some(FileBrowserHit::Footer));
    }

    #[test]
    fn correcting_an_invalid_path_restores_confirmation() {
        let mut modal = ready(FileBrowserMode::ChooseDirectory);
        assert!(modal.set_listing("/workspace", vec![]));
        assert!(!modal.navigate("relative/path"));
        assert!(!modal.accept_button());
        assert!(modal.navigate("/workspace"));
        assert!(modal.set_listing("/workspace", vec![]));
        assert!(modal.accept_button());
    }

    #[test]
    fn narrow_toolbar_reflows_search_below_navigation_and_path() {
        for width in [280.0, 320.0, 390.0] {
            let g = FileBrowserModal::geometry([0.0, 0.0, width, 600.0]);
            let search = search_control_rect(g, false);
            let path = path_control_rect(g, false);
            assert!(search[1] >= path[1] + path[3]);
            assert!(path[0] + path[2] <= g.card[0] + g.card[2]);
            for control in [
                nav_control_rect(g, 0, false),
                nav_control_rect(g, 1, false),
                nav_control_rect(g, 2, false),
                path,
                search,
            ] {
                assert!(intersect_rect(control, g.list).is_none());
                assert!(control[0] >= g.card[0]);
                assert!(control[0] + control[2] <= g.card[0] + g.card[2]);
            }
        }
    }

    #[test]
    fn directory_mode_filters_files_and_confirms_the_viewed_folder() {
        let mut modal = ready(FileBrowserMode::ChooseDirectory);
        modal.set_listing(
            "/workspace",
            vec![
                FileBrowserEntry {
                    name: "child".into(),
                    is_dir: true,
                    size: None,
                },
                FileBrowserEntry {
                    name: "readme.md".into(),
                    is_dir: false,
                    size: Some(12),
                },
            ],
        );
        assert_eq!(modal.entries.len(), 1);
        assert!(!modal.accept_path("/workspace/readme.md".into()));
        assert!(modal.accept_button());
        assert_eq!(modal.take_selection().unwrap().path, "/workspace");
    }

    #[test]
    fn directory_confirmation_waits_for_a_successful_listing() {
        let mut modal = ready(FileBrowserMode::ChooseDirectory);
        assert!(!modal.accept_button());
        assert!(modal.set_listing("/workspace", vec![]));
        assert!(modal.navigate("/workspace/child"));
        assert!(!modal.accept_button());
        modal.set_error("Permission denied");
        assert!(!modal.accept_button());
        assert!(modal.navigate("/workspace/child"));
        assert!(modal.drain_requests().iter().any(|request| matches!(
            request, FileBrowserRequest::ListDirectory { path } if path == "/workspace/child"
        )));
        assert!(modal.set_listing("/workspace/child", vec![]));
        assert!(modal.accept_button());
        assert_eq!(modal.take_selection().unwrap().path, "/workspace/child");
    }

    #[test]
    fn location_discovery_preserves_nested_host_start_directory() {
        for start in ["/host/projects/app", "C:\\Users\\host\\project"] {
            let root = if start.starts_with('/') {
                "/host"
            } else {
                "C:\\Users\\host"
            };
            let mut modal = FileBrowserModal::new();
            modal.open(FileBrowserMode::ChooseDirectory, start, vec![]);
            modal.set_locations(vec![location("home", "Home", root)]);
            assert_eq!(modal.current_path(), start);
            assert_eq!(
                modal.drain_requests().last(),
                Some(&FileBrowserRequest::ListDirectory { path: start.into() })
            );
        }
    }

    #[test]
    fn reopening_clears_previous_host_locations_and_ignores_closed_replies() {
        let mut modal = ready(FileBrowserMode::ChooseDirectory);
        modal.close();
        assert!(!modal.set_listing("/workspace", vec![]));
        assert!(!modal.set_locations(vec![location("home", "Home", "/old-host")]));
        modal.open(FileBrowserMode::ChooseDirectory, "/guest/dir", vec![]);
        assert!(modal.locations.is_empty());
        modal.set_locations(vec![location("workspace", "Workspace", "/host/workspace")]);
        assert_eq!(modal.current_path(), "/host/workspace");
        assert!(!modal.set_listing("/guest/dir", vec![]));
        assert!(!modal.accept_button());
    }

    #[test]
    fn enter_navigates_without_confirming_an_unloaded_directory() {
        let mut modal = ready(FileBrowserMode::ChooseDirectory);
        modal.set_listing(
            "/workspace",
            vec![FileBrowserEntry {
                name: "folder".into(),
                is_dir: true,
                size: None,
            }],
        );
        modal.press_named(NamedKey::ArrowDown);
        modal.press_named(NamedKey::Enter);
        assert_eq!(modal.current_path(), "/workspace/folder");
        assert!(modal.take_selection().is_none());
        assert!(!modal.accept_button());
    }

    #[test]
    fn browser_paths_are_absolute_and_labels_are_never_paths() {
        assert!(normalize_browser_path("Documents").is_err());
        assert!(normalize_browser_path("C:\\Users\\test\\Pictures").is_ok());
        let mut modal = FileBrowserModal::new();
        modal.open(FileBrowserMode::OpenFile, "", vec![]);
        modal.set_locations(vec![location(
            "documents",
            "Documents",
            "/home/me/Documents",
        )]);
        assert_eq!(
            modal.drain_requests().last(),
            Some(&FileBrowserRequest::ListDirectory {
                path: "/home/me/Documents".into()
            })
        );
    }
    #[test]
    fn image_filter_and_success_clear_error() {
        let mut modal = ready(FileBrowserMode::AttachImage);
        modal.set_error("old");
        assert!(modal.set_listing(
            "/workspace",
            vec![
                FileBrowserEntry {
                    name: ".hidden.png".into(),
                    is_dir: false,
                    size: None
                },
                FileBrowserEntry {
                    name: "a.txt".into(),
                    is_dir: false,
                    size: None
                },
                FileBrowserEntry {
                    name: "a.PNG".into(),
                    is_dir: false,
                    size: None
                },
            ]
        ));
        assert_eq!(modal.entries.len(), 1);
        assert!(modal.error.is_none());
    }
    #[test]
    fn palette_is_identity_mapped_from_theme_not_gray_literals() {
        let mut theme = IdeTheme::default();
        theme.bg = 0x123456;
        theme.surface = 0x234567;
        theme.border = 0x345678;
        let palette = FileBrowserPalette::from_theme(&theme);
        assert_eq!(palette.backdrop, theme.bg);
        assert_eq!(palette.sidebar, theme.panel_bg());
        assert_eq!(palette.body, palette.card);
        assert_eq!(palette.border, theme.border);
        assert_eq!(palette.card, theme.panel_bg());
    }
    #[test]
    fn centered_button_origin_centers_both_axes() {
        let rect = [10.0, 20.0, 72.0, FILE_BROWSER_DENSITY.visual_control_height];
        let origin = centered_text_origin(rect, 40.0, FILE_BROWSER_DENSITY.font_size);
        assert_eq!(origin, [26.0, 29.5]);
        assert_eq!(origin[0] + 20.0, rect[0] + rect[2] * 0.5);
        assert_eq!(
            origin[1] + FILE_BROWSER_DENSITY.font_size * 0.5,
            rect[1] + rect[3] * 0.5
        );
    }
    #[test]
    fn modal_keeps_shared_type_and_radius_with_roomier_rows() {
        assert_eq!(FILE_BROWSER_DENSITY.font_size, FONT_SIZE);
        assert_eq!(FILE_BROWSER_DENSITY.icon_size, ICON_FONT_SIZE);
        assert_eq!(FILE_BROWSER_DENSITY.row_height, ROW_HEIGHT);
        assert_eq!(FILE_BROWSER_DENSITY.row_padding_x, ROW_PADDING_X);
        assert_eq!(FILE_BROWSER_DENSITY.frame_radius, FRAME_RADIUS);
        assert_eq!(FILE_BROWSER_DENSITY.frame_stroke, FRAME_STROKE);
    }
    #[test]
    fn picker_font_density_follows_the_active_chrome_scale() {
        let scaled = FILE_BROWSER_DENSITY.with_font_scale(1.5);
        assert_eq!(scaled.font_size, FONT_SIZE * 1.5);
        assert_eq!(scaled.icon_size, ICON_FONT_SIZE * 1.5);
        assert_eq!(scaled.row_height, ROW_HEIGHT);
    }
    #[test]
    fn mobile_text_entry_hit_excludes_rows_and_buttons() {
        let mut modal = ready(FileBrowserMode::OpenFile);
        let geometry = FileBrowserModal::geometry([0.0, 0.0, 390.0, 700.0]);
        modal.last_geometry = Some(geometry);
        for rect in [
            path_control_rect(geometry, false),
            search_control_rect(geometry, false),
        ] {
            assert!(modal.text_entry_at(rect[0] + rect[2] * 0.5, rect[1] + rect[3] * 0.5));
        }
        assert!(!modal.text_entry_at(
            geometry.list[0] + 8.0,
            geometry.list[1] + FILE_BROWSER_DENSITY.row_height * 0.5,
        ));
        assert!(!modal.text_entry_at(geometry.accept[0] + 4.0, geometry.accept[1] + 4.0,));
    }
    #[test]
    fn main_list_click_cannot_activate_a_sidebar_location() {
        let mut modal = ready(FileBrowserMode::OpenFile);
        let _ = modal.drain_requests();
        assert!(modal.set_listing(
            "/workspace",
            vec![
                FileBrowserEntry {
                    name: "one.txt".into(),
                    is_dir: false,
                    size: None
                },
                FileBrowserEntry {
                    name: "two.txt".into(),
                    is_dir: false,
                    size: None
                },
            ],
        ));
        let geometry = FileBrowserModal::geometry([0.0, 0.0, 1200.0, 800.0]);
        modal.last_geometry = Some(geometry);
        modal.pointer_down(
            geometry.list[0] + geometry.list[2] * 0.5,
            geometry.list[1] + FILE_BROWSER_DENSITY.row_height * 1.5,
            1,
        );

        assert_eq!(modal.selected, 1);
        assert_eq!(modal.current_path(), "/workspace");
        assert!(modal.drain_requests().is_empty());
    }
    #[test]
    fn card_is_restrained_and_mobile_hits_do_not_inflate_visuals() {
        let desktop = FileBrowserModal::geometry([0.0, 0.0, 1600.0, 1000.0]);
        assert_eq!(desktop.card[2], 960.0);
        assert!(desktop.card[3] >= 640.0 && desktop.card[3] <= 680.0);
        let mobile = FileBrowserModal::geometry([0.0, 0.0, 390.0, 700.0]);
        assert_eq!(mobile.accept[3], FILE_BROWSER_DENSITY.visual_control_height);
        assert_eq!(mobile.accept_hit[3], FILE_BROWSER_DENSITY.touch_hit_height);
        assert_ne!(mobile.accept, mobile.accept_hit);
        assert_eq!(
            nav_control_rect(mobile, 0, true)[3],
            FILE_BROWSER_DENSITY.visual_control_height
        );
        assert_eq!(
            nav_control_rect(mobile, 0, false)[3],
            FILE_BROWSER_DENSITY.touch_hit_height
        );
    }
    #[test]
    fn choosing_image_uses_absolute_selected_path() {
        let mut modal = ready(FileBrowserMode::AttachImage);
        modal.set_listing(
            "/workspace",
            vec![FileBrowserEntry {
                name: "a.png".into(),
                is_dir: false,
                size: Some(2),
            }],
        );
        assert!(modal.selected_entry().is_none());
        modal.press_named(NamedKey::ArrowDown);
        assert!(modal.activate_selected());
        assert_eq!(modal.take_selection().unwrap().path, "/workspace/a.png");
    }
    #[test]
    fn narrow_geometry_and_occlusion_stay_safe() {
        let geometry = FileBrowserModal::geometry([0.0, 0.0, 390.0, 700.0]);
        assert!(geometry.narrow && geometry.sidebar.is_none());
        let mut modal = ready(FileBrowserMode::OpenFile);
        modal.last_geometry = Some(geometry);
        assert_eq!(modal.occlusion_rect(), Some(geometry.card));
    }

    fn point(rect: [f32; 4]) -> (f32, f32) {
        (rect[0] + rect[2] * 0.5, rect[1] + rect[3] * 0.5)
    }

    #[test]
    fn partial_rows_share_one_visual_and_hit_clip() {
        let list = [100.0, 80.0, 320.0, 70.0];
        let row_h = 26.0;
        let top = visible_row_geometry(list, 0, 7.0, row_h).unwrap();
        assert_eq!(top.full, [100.0, 73.0, 320.0, 26.0]);
        assert_eq!(top.visible, [100.0, 80.0, 320.0, 19.0]);

        let bottom = visible_row_geometry(list, 2, 7.0, row_h).unwrap();
        assert_eq!(bottom.full, [100.0, 125.0, 320.0, 26.0]);
        assert_eq!(bottom.visible, [100.0, 125.0, 320.0, 25.0]);
        assert!(visible_row_geometry(list, 3, 7.0, row_h).is_none());

        for visible in [top.visible, bottom.visible] {
            assert!(visible[1] >= list[1]);
            assert!(visible[1] + visible[3] <= list[1] + list[3]);
            assert!(contains(visible, visible[0], visible[1]));
            assert!(!contains(visible, visible[0], visible[1] + visible[3]));
        }
    }

    #[test]
    fn hit_map_has_one_z_ordered_owner_for_every_modal_region() {
        let mut modal = ready(FileBrowserMode::OpenFile);
        let _ = modal.drain_requests();
        assert!(modal.set_listing(
            "/workspace",
            (0..20)
                .map(|i| FileBrowserEntry {
                    name: format!("{i:02}.txt"),
                    is_dir: false,
                    size: Some(i),
                })
                .collect()
        ));
        let g = FileBrowserModal::geometry([0.0, 0.0, 1200.0, 800.0]);
        modal.last_geometry = Some(g);
        modal.scroll_px = 7.0;

        let side = g.sidebar.unwrap();
        let cases = [
            (point(nav_control_rect(g, 0, false)), FileBrowserHit::Back),
            (
                point(nav_control_rect(g, 1, false)),
                FileBrowserHit::Forward,
            ),
            (point(nav_control_rect(g, 2, false)), FileBrowserHit::Up),
            (point(path_control_rect(g, false)), FileBrowserHit::Path),
            (point(search_control_rect(g, false)), FileBrowserHit::Search),
            (point(g.cancel_hit), FileBrowserHit::Cancel),
            (point(g.accept_hit), FileBrowserHit::Accept),
            (
                (side[0] + 2.0, side[1] + 4.0 + ROW_HEIGHT * 0.5),
                FileBrowserHit::Sidebar(0),
            ),
            (
                (g.list[0] + 20.0, g.list[1] + 1.0),
                FileBrowserHit::Row {
                    visible: 0,
                    source: 0,
                },
            ),
            (
                (g.list[0] + 20.0, g.list[1] + ROW_HEIGHT),
                FileBrowserHit::Row {
                    visible: 1,
                    source: 1,
                },
            ),
            ((g.card[0] + 4.0, g.card[1] + 4.0), FileBrowserHit::Header),
            (
                (g.card[0] + 4.0, g.card[1] + g.card[3] - 2.0),
                FileBrowserHit::Footer,
            ),
            ((g.card[0] - 1.0, g.card[1]), FileBrowserHit::Scrim),
        ];
        for ((x, y), expected) in cases {
            assert_eq!(modal.hit_test(x, y), Some(expected), "hit at ({x}, {y})");
        }

        // The old sidebar test only considered y, so this main-list point
        // incorrectly navigated to a place whenever its row numbers matched.
        assert!(matches!(
            modal.hit_test(g.list[0] + 1.0, g.list[1] + 1.0),
            Some(FileBrowserHit::Row { .. })
        ));

        // Half-open slots give their shared boundary to the control on the right.
        let back = nav_control_rect(g, 0, false);
        assert_eq!(
            modal.hit_test(back[0] + back[2], back[1] + 1.0),
            Some(FileBrowserHit::Forward)
        );
    }

    #[test]
    fn bottom_partial_row_is_clickable_only_in_its_visible_intersection() {
        let mut modal = ready(FileBrowserMode::OpenFile);
        let _ = modal.drain_requests();
        assert!(modal.set_listing(
            "/workspace",
            (0..20)
                .map(|i| FileBrowserEntry {
                    name: format!("{i:02}.txt"),
                    is_dir: false,
                    size: None,
                })
                .collect()
        ));
        let mut g = FileBrowserModal::geometry([0.0, 0.0, 1200.0, 800.0]);
        g.list[3] = ROW_HEIGHT * 2.0 + 5.0;
        modal.last_geometry = Some(g);
        modal.scroll_px = 7.0;

        let bottom =
            visible_row_geometry(g.list, 2, modal.scroll_px, ROW_HEIGHT).unwrap();
        assert_eq!(bottom.visible[3], 12.0);
        let x = g.list[0] + 10.0;
        assert_eq!(
            modal.hit_test(x, bottom.visible[1] + 1.0),
            Some(FileBrowserHit::Row {
                visible: 2,
                source: 2
            })
        );
        assert_eq!(
            modal.hit_test(x, g.list[1] + g.list[3]),
            Some(FileBrowserHit::Footer)
        );
    }

    #[test]
    fn pointer_up_never_retargets_after_scroll_or_selection_change() {
        let mut modal = ready(FileBrowserMode::OpenFile);
        let _ = modal.drain_requests();
        assert!(modal.set_listing(
            "/workspace",
            (0..30)
                .map(|i| FileBrowserEntry {
                    name: format!("{i:02}.txt"),
                    is_dir: false,
                    size: None,
                })
                .collect()
        ));
        let g = FileBrowserModal::geometry([0.0, 0.0, 1200.0, 800.0]);
        modal.last_geometry = Some(g);
        let x = g.list[0] + 20.0;
        let y = g.list[1] + ROW_HEIGHT * 1.5;
        modal.pointer_down(x, y, 1);
        assert_eq!(modal.selected, 1);
        modal.scroll_pixels(ROW_HEIGHT * 3.0);
        modal.handle_event(&UiEvent::PointerUp {
            button: PointerButton::Left,
            x,
            y,
            modifiers: crate::event::Modifiers::empty(),
        });
        assert_eq!(modal.selected, 1);
        assert!(modal.take_selection().is_none());
    }

    #[test]
    fn geometry_is_invalidated_or_refreshed_across_modal_lifecycle() {
        let mut modal = ready(FileBrowserMode::OpenFile);
        modal.last_geometry = Some(FileBrowserModal::geometry([0.0, 0.0, 800.0, 600.0]));
        modal.set_safe_area(10.0, 0.0, 0.0, 0.0);
        assert!(modal.last_geometry.is_none());
        modal.handle_event(&UiEvent::Resize {
            w: 900,
            h: 700,
            scale: 1.0,
        });
        assert!(modal.last_geometry.is_none());
        modal.close();
        assert!(modal.last_geometry.is_none());
    }

    #[test]
    fn touch_hit_inflation_keeps_adjacent_controls_disjoint() {
        let g = FileBrowserModal::geometry([0.0, 0.0, 390.0, 700.0]);
        let controls = [
            nav_control_rect(g, 0, false),
            nav_control_rect(g, 1, false),
            nav_control_rect(g, 2, false),
            path_control_rect(g, false),
            search_control_rect(g, false),
        ];
        for left in 0..controls.len() {
            for right in left + 1..controls.len() {
                assert!(
                    intersect_rect(controls[left], controls[right]).is_none(),
                    "controls {left} and {right} overlap"
                );
            }
        }
        assert!(intersect_rect(g.cancel_hit, g.accept_hit).is_none());
    }

    #[test]
    fn click_actions_preserve_select_open_confirm_cancel_and_scrim_semantics() {
        let g = FileBrowserModal::geometry([0.0, 0.0, 1200.0, 800.0]);
        let row_point = |row: usize| {
            (
                g.list[0] + 20.0,
                g.list[1] + ROW_HEIGHT * (row as f32 + 0.5),
            )
        };

        let mut files = ready(FileBrowserMode::OpenFile);
        let _ = files.drain_requests();
        files.set_listing(
            "/workspace",
            vec![
                FileBrowserEntry {
                    name: "folder".into(),
                    is_dir: true,
                    size: None,
                },
                FileBrowserEntry {
                    name: "photo.png".into(),
                    is_dir: false,
                    size: Some(4),
                },
            ],
        );
        files.last_geometry = Some(g);
        let (x, y) = row_point(1);
        files.pointer_down(x, y, 1);
        assert!(files.is_active());
        assert_eq!(files.selected, 1);
        files.pointer_down(x, y, 2);
        assert_eq!(files.take_selection().unwrap().path, "/workspace/photo.png");

        let mut folder = ready(FileBrowserMode::OpenFile);
        let _ = folder.drain_requests();
        folder.set_listing(
            "/workspace",
            vec![FileBrowserEntry {
                name: "folder".into(),
                is_dir: true,
                size: None,
            }],
        );
        folder.last_geometry = Some(g);
        let (x, y) = row_point(0);
        folder.pointer_down(x, y, 1);
        assert_eq!(folder.current_path(), "/workspace");
        assert!(folder.selection_explicit);
        folder.pointer_down(x, y, 2);
        assert_eq!(folder.current_path(), "/workspace/folder");

        let mut confirm = ready(FileBrowserMode::OpenFile);
        let _ = confirm.drain_requests();
        confirm.set_listing(
            "/workspace",
            vec![FileBrowserEntry {
                name: "a.txt".into(),
                is_dir: false,
                size: None,
            }],
        );
        confirm.last_geometry = Some(g);
        let (x, y) = row_point(0);
        confirm.pointer_down(x, y, 1);
        let (x, y) = point(g.accept_hit);
        confirm.pointer_down(x, y, 1);
        assert_eq!(confirm.take_selection().unwrap().path, "/workspace/a.txt");

        for dismiss_at in [point(g.cancel_hit), (g.card[0] - 1.0, g.card[1])] {
            let mut dismiss = ready(FileBrowserMode::OpenFile);
            dismiss.last_geometry = Some(g);
            dismiss.pointer_down(dismiss_at.0, dismiss_at.1, 1);
            assert!(!dismiss.is_active());
            assert!(dismiss.last_geometry.is_none());
        }
    }
}
