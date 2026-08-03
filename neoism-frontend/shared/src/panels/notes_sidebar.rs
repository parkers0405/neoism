use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;
use web_time::Instant;

use sugarloaf::Sugarloaf;
use sugarloaf::text::DrawOpts;

use crate::animation::CriticallyDampedSpring;
use crate::panels::file_tree::icons::{
    FOLDER_CLOSED_ICON, FOLDER_OPEN_ICON, icon_for_file,
};
use crate::panels::file_tree::{
    FILE_TREE_MAX_WIDTH, FILE_TREE_MIN_WIDTH, FILE_TREE_WIDTH, FONT_SIZE, FRAME_RADIUS,
    FRAME_STROKE, ICON_FONT_SIZE, ICON_GAP, INDENT_PX, ROW_HEIGHT, ROW_PADDING_X,
    truncate_label,
};
use crate::primitives::ide_theme::IdeTheme;
use crate::primitives::{
    draw_icon_centered_with_occlusion, draw_text_with_occlusion, edge_row_radii,
    snap_to_device_px,
};

const DEPTH: f32 = 0.0;
const ORDER: u8 = 7;

// Spring tuning lifted verbatim from `file_tree` so the notes tree
// scrolls / moves its cursor with the exact same lag-offset feel as the
// chrome file tree (same omega, same closed-form math).
const SCROLL_ANIMATION_LENGTH: f32 = 0.30;
const CURSOR_ANIMATION_LENGTH: f32 = 0.12;

// Mac Finder-style spring-loaded drag-and-drop, lifted verbatim from
// `file_tree::drag` so a page/folder row drags with the exact same
// activation threshold and dwell-to-spring-open feel as the chrome
// file tree. Kept as local copies (not `pub use` of the file tree's
// `pub(super)` consts) so the notes panel stays self-contained.
//
/// Pixels the cursor must travel from the press point before an armed
/// press becomes a live drag — mirrors `file_tree::DRAG_ACTIVATION_PX`.
const NOTES_DRAG_ACTIVATION_PX: f32 = 5.0;
/// How long the cursor must dwell over a closed folder before it springs
/// open under the drag — mirrors `file_tree::SPRING_OPEN_DWELL`.
const NOTES_SPRING_OPEN_DWELL: Duration = Duration::from_millis(450);

#[derive(Clone, Debug)]
pub struct NotesSidebar {
    visible: bool,
    focused: bool,
    scale: f32,
    width: f32,
    workspace_name: String,
    workspace_path: Option<PathBuf>,
    all_entries: Vec<NoteSidebarEntry>,
    rows: Vec<NoteSidebarRow>,
    open_dirs: HashSet<PathBuf>,
    /// "+ New note" button of the empty-vault state, when drawn.
    empty_create_rect: Option<[f32; 4]>,
    /// When set, the empty state swaps its single "+ New note" button for
    /// the Notion-style "no linked vault" pair — "+ Create workspace
    /// vault" and "Select vault". Set by the host only for a served/joined
    /// workspace that has no linked vault; local workspaces never see it
    /// (they always resolve a vault), keeping local byte-identical.
    show_vault_actions: bool,
    /// "+ Create workspace vault" button rect (no-linked-vault state).
    empty_link_vault_rect: Option<[f32; 4]>,
    /// "Select vault" button rect (no-linked-vault state).
    empty_select_vault_rect: Option<[f32; 4]>,
    /// Live `icon:` values from OPEN buffers (value-picker accepts) —
    /// they beat the disk walk until the daemon flushes the file, else
    /// any refresh between accept and flush reverts the row's emoji.
    icon_overrides: HashMap<PathBuf, Option<String>>,
    selected_index: usize,
    selector_selected: bool,
    scroll_top: usize,
    // Scroll/cursor springs + wheel accumulator mirror `file_tree`'s
    // proven model so trackpad pixel scrolling, Ctrl+D/U half-page jumps
    // and Down/Up line moves feel identical to the chrome tree. See
    // `panels::file_tree::state::FileTree`.
    scroll: CriticallyDampedSpring,
    cursor_spring: CriticallyDampedSpring,
    wheel_accumulator: f32,
    last_scroll_frame: Instant,
    last_cursor_frame: Instant,
    last_panel_height_rows: usize,
    /// One-shot "the vault changed on disk, re-list me" flag. Set when an
    /// agent (or any external mutation) touches the vault while the panel
    /// is open; the host drains it via [`take_refresh`](Self::take_refresh)
    /// and answers with a fresh listing — same refresh-flag contract the
    /// chrome uses on first open. Without this the panel only refreshed on
    /// a manual close/open.
    pending_refresh: bool,
    note_rects: Vec<([f32; 4], usize)>,
    icon_rects: Vec<([f32; 4], usize)>,
    selected_cursor_rect: Option<[f32; 4]>,
    workspace_rect: Option<[f32; 4]>,
    /// The footer settings gear (right of the vault selector) — opens
    /// the Notes settings menu. Reachable with ArrowRight from the vault
    /// selector, clickable, focus tracked by `settings_selected`.
    settings_rect: Option<[f32; 4]>,
    settings_selected: bool,
    /// Compact create actions directly below the Notes wordmark.
    new_note_rect: Option<[f32; 4]>,
    new_folder_rect: Option<[f32; 4]>,
    /// Short accent pulse when the vault selector is activated.
    vault_press_started_at: Option<Instant>,
    /// Per-letter NEOISM wordmark header — same hover/shimmer animation
    /// as the splash and the agent home.
    wordmark: crate::panels::agent_pane::state::NeoismWordmarkState,
    /// Pending vim-style numeric count (e.g. `5` then `j` moves 5 rows).
    /// Accumulated by [`push_count_digit`](Self::push_count_digit) and
    /// consumed by the next motion via [`take_count`](Self::take_count).
    pending_count: Option<usize>,
    /// True after a lone `g`, so the next `g` completes `gg` (go-to-top).
    pending_g: bool,
    /// In-flight Finder-style drag of a page/folder row onto a folder or
    /// the vault root (spring-loaded move). `None` when nothing is being
    /// dragged. Mirrors `file_tree`'s `file_drag`.
    notes_drag: Option<NotesDragState>,
}

/// Live drag state: what page/folder is being dragged, where the ghost
/// is, and which folder (if any) is the current drop target. Mirrors
/// `file_tree::drag::FileDragState`.
#[derive(Clone, Debug)]
pub struct NotesDragState {
    /// Arm-time row of the dragged item (informational; the live drag
    /// re-resolves source + target by PATH so a spring-open re-indexing
    /// the rows mid-drag never loses them).
    #[allow(dead_code)]
    source_row: usize,
    /// Absolute path of the dragged page/folder.
    source_path: PathBuf,
    /// Label painted on the cursor-following ghost.
    source_label: String,
    /// Whether the dragged row is a folder (drives the ghost glyph).
    source_is_dir: bool,
    /// True once the cursor has moved past the activation threshold. The
    /// ghost only paints, and a release only moves, when this is set.
    live: bool,
    start_x: f32,
    start_y: f32,
    current_x: f32,
    current_y: f32,
    /// Path of the folder (or vault root) currently under the cursor that
    /// is a legal drop target, if any (drives the highlight + wiggle).
    hovered_dir: Option<PathBuf>,
    /// When the current `hovered_dir` was first entered — the dwell clock
    /// for spring-open and the phase origin for the wiggle.
    hovered_since: Option<Instant>,
    /// Folders already auto-sprung this drag, so the dwell fires once per
    /// folder instead of every frame past the threshold.
    sprang: HashSet<PathBuf>,
}

/// What a notes-sidebar drag release resolved to. Mirrors
/// `file_tree::drag::FileDropOutcome`.
pub enum NotesDropOutcome {
    /// The press never became a drag — the caller treats it as a click
    /// (open the note / toggle the folder).
    Click,
    /// Move `source` into directory `dest_dir`.
    Move { source: PathBuf, dest_dir: PathBuf },
    /// A live drag released over no valid target — do nothing.
    Cancel,
}

#[derive(Clone, Debug)]
pub struct NoteSidebarEntry {
    pub path: PathBuf,
    pub label: String,
    pub is_dir: bool,
    /// User-assigned icon (emoji or any glyph) overriding the default
    /// folder/file icon — Notion-style, persisted in the vault's
    /// `.neoism-icons.json` keyed by path relative to the vault root.
    pub icon: Option<String>,
    depth: usize,
    parent: PathBuf,
}

/// File name of the per-vault icon map (relative path → glyph).
pub const NOTES_ICONS_FILE: &str = ".neoism-icons.json";

/// Default Markdown icon shared with the file tree and buffer tabs.
pub const NOTE_DEFAULT_ICON: &str = "\u{f15c}";
/// Older builds wrote this former default glyph into the override map
/// when "Note" was selected. Treat it as a reset so frontmatter icons
/// on root-level notes are not masked forever.
const LEGACY_NOTE_DEFAULT_ICON: &str = "\u{f48a}";

#[derive(Clone, Debug)]
struct NoteSidebarRow {
    entry_index: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotesSidebarHit {
    WorkspacePicker,
    /// The footer settings gear.
    Settings,
    /// Pencil-note action directly below the Notes title.
    NewNote,
    /// Folder-plus action directly below the Notes title.
    NewFolder,
    Note(usize),
    /// The icon glyph of a row — opens the icon/emoji picker for it.
    NoteIcon(usize),
    /// The "+ New note" button shown by the empty vault state.
    CreateFirstNote,
    /// "+ Create workspace vault" — the no-linked-vault empty state's
    /// primary button (create + link a vault to this workspace/dir).
    CreateWorkspaceVault,
    /// "Select vault" — the no-linked-vault empty state's secondary
    /// button (open the vault selector).
    SelectVault,
}

impl Default for NotesSidebar {
    fn default() -> Self {
        Self {
            visible: false,
            focused: false,
            scale: 1.0,
            width: FILE_TREE_WIDTH,
            workspace_name: "Default".to_string(),
            workspace_path: None,
            all_entries: Vec::new(),
            rows: Vec::new(),
            open_dirs: HashSet::new(),
            empty_create_rect: None,
            show_vault_actions: false,
            empty_link_vault_rect: None,
            empty_select_vault_rect: None,
            icon_overrides: HashMap::new(),
            selected_index: 0,
            selector_selected: false,
            scroll_top: 0,
            scroll: CriticallyDampedSpring::new(),
            cursor_spring: CriticallyDampedSpring::new(),
            wheel_accumulator: 0.0,
            last_scroll_frame: Instant::now(),
            last_cursor_frame: Instant::now(),
            last_panel_height_rows: 1,
            pending_refresh: false,
            note_rects: Vec::new(),
            icon_rects: Vec::new(),
            selected_cursor_rect: None,
            workspace_rect: None,
            settings_rect: None,
            settings_selected: false,
            new_note_rect: None,
            new_folder_rect: None,
            vault_press_started_at: None,
            wordmark: crate::panels::agent_pane::state::NeoismWordmarkState {
                hover: [0.0; 6],
                last_frame_at: None,
                rect: None,
                click_started: None,
                click_pos: None,
            },
            pending_count: None,
            pending_g: false,
            notes_drag: None,
        }
    }
}

impl NotesSidebar {
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
        if !visible {
            // Settle the springs but KEEP `open_dirs` — the user's
            // expanded folders persist across a close/reopen, mirroring
            // the file tree. Resetting the springs only avoids a stale
            // lag-offset on the next open.
            self.scroll.reset();
            self.cursor_spring.reset();
            self.wheel_accumulator = 0.0;
        }
    }

    pub fn toggle_visible(&mut self) {
        self.set_visible(!self.visible);
    }

    pub fn toggle_focus_or_visibility(&mut self) -> bool {
        let was_visible = self.visible;
        if !self.visible {
            self.visible = true;
            self.focused = true;
        } else if self.focused {
            self.visible = false;
            self.focused = false;
        } else {
            self.focused = true;
        }
        was_visible != self.visible
    }

    pub fn is_focused(&self) -> bool {
        self.focused
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        if !focused {
            self.settings_selected = false;
            self.clear_pending();
        }
    }

    pub fn selected_cursor_rect(&self) -> Option<[f32; 4]> {
        self.selected_cursor_rect
    }

    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale.clamp(0.5, 3.0);
        // Row height changed under the springs — reset them so the next
        // motion measures against the new geometry (matches file_tree).
        self.scroll.reset();
        self.cursor_spring.reset();
    }

    pub fn width(&self) -> f32 {
        self.width
    }

    /// Window-chrome width setter used when a workspace swap installs a
    /// different panel instance — width belongs to the window, not the
    /// workspace.
    pub fn set_width(&mut self, width: f32) {
        self.width = width.clamp(FILE_TREE_MIN_WIDTH, FILE_TREE_MAX_WIDTH);
    }

    pub fn resize(&mut self, delta: f32) {
        self.width = (self.width + delta).clamp(FILE_TREE_MIN_WIDTH, FILE_TREE_MAX_WIDTH);
    }

    pub fn set_workspace(&mut self, name: impl Into<String>, path: Option<PathBuf>) {
        // Only wipe the expanded-folder set when the vault actually
        // changes. The Alt+N toggle re-calls `set_workspace` with the
        // SAME path on every open; clearing unconditionally was what
        // collapsed every open folder on a close/reopen.
        let vault_changed = self.workspace_path != path;
        self.workspace_name = name.into();
        self.workspace_path = path;
        if vault_changed {
            self.open_dirs.clear();
            self.icon_overrides.clear();
        }
        if let Some(root) = self.workspace_path.clone() {
            self.open_dirs.insert(root);
        }
        self.refresh_notes();
    }

    /// Toggle the Notion-style "no linked vault" empty state. When on,
    /// an empty panel offers "+ Create workspace vault" and "Select
    /// vault" instead of the single "+ New note". The host sets this only
    /// for a served/joined workspace with no linked vault; local
    /// workspaces leave it off so their empty state stays byte-identical.
    pub fn set_vault_actions(&mut self, show: bool) {
        self.show_vault_actions = show;
    }

    pub fn shows_vault_actions(&self) -> bool {
        self.show_vault_actions
    }

    /// Expand `dir` in the tree (no note opened, selection untouched) —
    /// used by the first-run welcome reveal. Mirrors how `set_workspace`
    /// / `refresh_notes` insert the root into `open_dirs`, then rebuilds
    /// the visible rows so the newly-expanded folder's children show.
    pub fn reveal_dir(&mut self, dir: &std::path::Path) {
        self.open_dirs.insert(dir.to_path_buf());
        self.rebuild_rows();
    }

    /// Mark the panel as wanting a fresh listing — set when something
    /// mutates the vault on disk (agent edits, file ops) while the panel
    /// is open. Native hosts can also just call [`refresh_notes`] which
    /// re-walks the filesystem directly; the flag exists so wasm hosts
    /// (no local fs) re-fetch through the daemon on the next frame. No-op
    /// while hidden — nobody is looking.
    pub fn mark_dirty(&mut self) {
        if self.visible {
            self.pending_refresh = true;
        }
    }

    /// Drain the one-shot "needs a listing" flag. The web host pumps this
    /// each frame and answers with `set_entries_from_host`; the native
    /// host can ignore it since it refreshes via the filesystem directly.
    pub fn take_refresh(&mut self) -> bool {
        std::mem::take(&mut self.pending_refresh)
    }

    pub fn refresh_notes(&mut self) {
        let selected_path = self.selected_note_path();
        self.all_entries.clear();
        let root = self.workspace_path.clone();
        if let Some(root) = &root {
            collect_note_entries(root, root, 0, &mut self.all_entries);
            self.open_dirs.insert(root.clone());
        }
        // Live in-buffer frontmatter edits first...
        self.apply_icon_overrides();
        // ...then the explicit `.neoism-icons.json` map LAST, so a picked
        // icon has the highest priority. It used to be applied BEFORE
        // `apply_icon_overrides`, so a stale `None` frontmatter override from
        // an open note (see `set_note_icon`) clobbered a just-picked icon —
        // the "root/open note icon doesn't stick" bug. A MISSING map entry
        // still must not wipe an existing icon.
        if let Some(root) = &root {
            let icons = load_notes_icons(root);
            if !icons.is_empty() {
                for entry in &mut self.all_entries {
                    if let Some(icon) =
                        entry.path.strip_prefix(root).ok().and_then(|rel| {
                            icons.get(&rel.to_string_lossy().into_owned())
                        })
                    {
                        entry.icon = Some(icon.clone());
                    }
                }
            }
        }
        self.log_resolved_note_icons("refresh_notes");
        self.all_entries.sort_by(|a, b| {
            a.parent
                .cmp(&b.parent)
                .then_with(|| b.is_dir.cmp(&a.is_dir))
                .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
        });
        self.rebuild_rows();
        if let Some(path) = selected_path {
            if let Some(row) = self.row_index_for_path(&path) {
                self.selected_index = row;
            }
        }
        self.clamp_selection_and_scroll();
    }

    /// Host push (web): replace the entry list with daemon-listed
    /// `(path, is_dir)` pairs. `refresh_notes` walks the local
    /// filesystem, which is a no-op on wasm — the web host lists the
    /// notes tree through the daemon's Files service and stores the
    /// result back here. Depth/parent derive from `workspace_path`.
    pub fn set_entries_from_host(&mut self, entries: Vec<(PathBuf, bool)>) {
        let Some(root) = self.workspace_path.clone() else {
            return;
        };
        let selected_path = self.selected_note_path();
        self.all_entries.clear();
        self.open_dirs.insert(root.clone());
        for (path, is_dir) in entries {
            if should_skip_note_entry(&root, &path) || path == root {
                continue;
            }
            let fallback = if is_dir { "folder" } else { "file" };
            let label = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(fallback)
                .to_string();
            let parent = path.parent().unwrap_or(&root).to_path_buf();
            let depth = path
                .strip_prefix(&root)
                .map(|rel| rel.components().count().saturating_sub(1))
                .unwrap_or(0);
            // Mirror the note's frontmatter `icon:` onto the row, exactly
            // like `refresh_notes`/`collect_note_entries` do. This host path
            // (daemon-listed notes — the path the DESKTOP sidebar actually
            // uses) previously left every icon `None`, so a note's page icon
            // never showed unless it also had a same-session in-memory
            // override — the "frontmatter emoji shows in the doc but the tree
            // row keeps the default md icon" bug. On wasm the fs read is a
            // graceful no-op (None); a remote host path that isn't local also
            // reads None (the daemon would have to supply it).
            let icon = if is_dir {
                None
            } else {
                note_frontmatter_icon(&path)
            };
            self.all_entries.push(NoteSidebarEntry {
                path,
                label,
                is_dir,
                icon,
                depth,
                parent,
            });
        }
        // Live buffer overrides first, then the explicit `.neoism-icons.json`
        // map LAST (highest priority) — same ordering as `refresh_notes`.
        self.apply_icon_overrides();
        let icons = load_notes_icons(&root);
        if !icons.is_empty() {
            for entry in &mut self.all_entries {
                if let Some(icon) = entry
                    .path
                    .strip_prefix(&root)
                    .ok()
                    .and_then(|rel| icons.get(&rel.to_string_lossy().into_owned()))
                {
                    entry.icon = Some(icon.clone());
                }
            }
        }
        self.log_resolved_note_icons("set_entries_from_host");
        self.all_entries.sort_by(|a, b| {
            a.parent
                .cmp(&b.parent)
                .then_with(|| b.is_dir.cmp(&a.is_dir))
                .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
        });
        self.rebuild_rows();
        if let Some(path) = selected_path {
            if let Some(row) = self.row_index_for_path(&path) {
                self.selected_index = row;
            }
        }
        self.clamp_selection_and_scroll();
    }

    /// In-place icon update for one note — the frontmatter `icon:` was
    /// just edited in the open buffer, so the disk-walk value the entry
    /// was built from is stale until the daemon flushes the file. The
    /// override is remembered so refreshes between the edit and the
    /// flush re-apply it instead of reverting to the disk value.
    pub fn set_note_icon(&mut self, path: &Path, icon: Option<String>) {
        // Normalise an empty/whitespace-only frontmatter value to `None`:
        // an empty `icon:` is "no icon", not a real glyph, and storing
        // `Some("")` would both render blank and (as a live override) clobber
        // a real icon read from the map or a later frontmatter edit.
        let icon = icon.filter(|glyph| !glyph.trim().is_empty());
        self.icon_overrides.insert(path.to_path_buf(), icon.clone());
        for entry in &mut self.all_entries {
            if entry.path == path {
                entry.icon = icon;
                return;
            }
        }
    }

    fn apply_icon_overrides(&mut self) {
        if self.icon_overrides.is_empty() {
            return;
        }
        for entry in &mut self.all_entries {
            // Only a LIVE `Some` icon from an open buffer overrides the
            // disk-read value. A `None` override (the buffer simply has no
            // frontmatter icon — e.g. a note was opened without one) must NOT
            // wipe the icon we just read from the note's frontmatter / the
            // `.neoism-icons.json` map. That stale-`None` wipe was the root of
            // "the page's frontmatter emoji shows in the doc but the sidebar
            // row keeps the default md icon". A real reset clears the JSON map
            // entry (or the frontmatter on disk), which the next refresh reads.
            if let Some(Some(icon)) = self.icon_overrides.get(&entry.path) {
                entry.icon = Some(icon.clone());
            }
        }
    }

    /// Diagnostic: dump every note row's resolved icon so we can tell whether
    /// a missing page icon is a LOAD problem (icon `None` here) or a RENDER
    /// problem (icon `Some` here but the row still draws the default). Enable
    /// with `RUST_LOG=neoism::notes=info`.
    fn log_resolved_note_icons(&self, source: &str) {
        for entry in &self.all_entries {
            if !entry.is_dir {
                tracing::info!(
                    target: "neoism::notes",
                    source,
                    note = %entry.label,
                    depth = entry.depth,
                    icon = ?entry.icon,
                    "resolved note row icon"
                );
            }
        }
    }

    pub fn selected_note_path(&self) -> Option<PathBuf> {
        if self.selector_selected {
            return None;
        }
        self.row_entry(self.selected_index)
            .map(|entry| entry.path.clone())
    }

    pub fn selected_index(&self) -> usize {
        self.selected_index
    }

    pub fn is_selector_selected(&self) -> bool {
        self.selector_selected
    }

    pub fn select_selector(&mut self) {
        self.selector_selected = true;
        self.settings_selected = false;
    }

    pub fn is_settings_selected(&self) -> bool {
        self.selector_selected && self.settings_selected
    }

    /// The footer settings gear rect — menus anchor to it.
    pub fn settings_button_rect(&self) -> Option<[f32; 4]> {
        self.settings_rect
    }

    /// ArrowLeft/Right on the footer walks vault selector <-> settings
    /// gear. Elsewhere horizontal keys keep their normal meaning.
    pub fn move_horizontal_focus(&mut self, right: bool) -> bool {
        if !self.selector_selected {
            return false;
        }
        if right && !self.settings_selected {
            self.settings_selected = true;
            true
        } else if !right && self.settings_selected {
            self.settings_selected = false;
            true
        } else {
            false
        }
    }

    pub fn workspace_path(&self) -> Option<PathBuf> {
        self.workspace_path.clone()
    }

    pub fn contains_path(&self, path: &Path) -> bool {
        self.all_entries.iter().any(|entry| entry.path == path)
    }

    pub fn note_icon_for_path(&self, path: &Path) -> Option<String> {
        let saved = || {
            let root = self.workspace_path.as_ref()?;
            let relative = path.strip_prefix(root).ok()?.to_string_lossy();
            load_notes_icons(root).get(relative.as_ref()).cloned()
        };
        saved()
            .or_else(|| self.icon_overrides.get(path).and_then(|icon| icon.clone()))
            .or_else(|| {
                self.all_entries
                    .iter()
                    .find(|entry| entry.path == path)
                    .and_then(|entry| entry.icon.clone())
            })
            .or_else(|| note_frontmatter_icon(path))
    }

    pub fn animate_workspace_selector_press(&mut self) {
        self.vault_press_started_at = Some(Instant::now());
    }

    pub fn workspace_selector_rect(&self) -> Option<[f32; 4]> {
        self.workspace_rect
    }

    pub fn note_path(&self, index: usize) -> Option<PathBuf> {
        self.row_entry(index).map(|entry| entry.path.clone())
    }

    pub fn note_is_dir(&self, index: usize) -> bool {
        self.row_entry(index).is_some_and(|entry| entry.is_dir)
    }

    pub fn set_selected(&mut self, index: usize) {
        self.selector_selected = false;
        self.settings_selected = false;
        if !self.rows.is_empty() {
            self.move_selection_to(index.min(self.rows.len().saturating_sub(1)));
        }
    }

    /// Move the keyboard selection to `new_selected`, nudging the cursor
    /// spring so the caret glides between rows — same lag-offset math as
    /// `file_tree::move_selection_to`.
    fn move_selection_to(&mut self, new_selected: usize) {
        if self.rows.is_empty() {
            return;
        }
        let new_selected = new_selected.min(self.rows.len().saturating_sub(1));
        if new_selected == self.selected_index {
            return;
        }
        let was_idle = self.cursor_spring.position == 0.0;
        let rows = self.selected_index as i32 - new_selected as i32;
        self.cursor_spring.position += rows as f32 * self.row_height();
        if was_idle {
            self.last_cursor_frame = Instant::now();
        }
        self.selected_index = new_selected;
        self.clamp_scroll(self.last_panel_height_rows);
    }

    pub fn select_next(&mut self) {
        if self.selector_selected {
            return;
        }
        if self.rows.is_empty() || self.selected_index + 1 >= self.rows.len() {
            self.selector_selected = true;
            self.settings_selected = false;
        } else {
            self.set_selected(
                (self.selected_index + 1).min(self.rows.len().saturating_sub(1)),
            );
        }
    }

    pub fn select_prev(&mut self) {
        if self.selector_selected {
            self.selector_selected = false;
            self.settings_selected = false;
            if !self.rows.is_empty() {
                self.selected_index = self.rows.len().saturating_sub(1);
                self.clamp_scroll(self.last_panel_height_rows);
            }
        } else if self.selected_index == 0 || self.rows.is_empty() {
            // Already at the top — the wordmark header is decorative.
        } else {
            self.set_selected(self.selected_index.saturating_sub(1));
        }
    }

    /// Half-page jump down (Ctrl+D / PageDown), clamped to the last row.
    /// Mirrors `file_tree::select_next_by`; lands on a real note row so
    /// the cursor spring animates the same way as single-step moves.
    pub fn select_next_by(&mut self, n: usize) {
        if self.rows.is_empty() {
            self.selector_selected = true;
            return;
        }
        self.set_selected(
            self.selected_index
                .saturating_add(n)
                .min(self.rows.len().saturating_sub(1)),
        );
    }

    /// Half-page jump up (Ctrl+U / PageUp), clamped to the first row.
    pub fn select_prev_by(&mut self, n: usize) {
        if self.rows.is_empty() {
            return;
        }
        self.set_selected(self.selected_index.saturating_sub(n));
    }

    /// Half a visible page, used by Ctrl+D / Ctrl+U. Falls back to a
    /// single row on a viewport too small to have measured yet.
    fn half_page(&self) -> usize {
        (self.last_panel_height_rows / 2).max(1)
    }

    /// Ctrl+D — jump the selection down half a page. Consuming the key
    /// here (instead of letting it fall through) is also what stops it
    /// leaking to the terminal behind the panel as an EOF that would
    /// close the shell.
    pub fn select_half_page_down(&mut self) {
        self.select_next_by(self.half_page());
    }

    /// Ctrl+U — jump the selection up half a page.
    pub fn select_half_page_up(&mut self) {
        self.select_prev_by(self.half_page());
    }

    /// Jump to the first note row (vim `gg` / `1`).
    pub fn select_first(&mut self) {
        self.clear_pending();
        if !self.rows.is_empty() {
            self.set_selected(0);
        }
    }

    /// Jump to the last note row (vim `$` / `G`).
    pub fn select_last(&mut self) {
        self.clear_pending();
        if !self.rows.is_empty() {
            self.set_selected(self.rows.len().saturating_sub(1));
        }
    }

    /// Jump to a 1-based row (vim `<count>G`). Out-of-range counts clamp
    /// to the last row; a zero count is treated as the first row.
    pub fn goto_row(&mut self, one_based: usize) {
        self.clear_pending();
        if !self.rows.is_empty() {
            self.set_selected(one_based.saturating_sub(1));
        }
    }

    /// Feed a typed digit into the pending vim count. A leading `0` with
    /// no count in progress is ignored (matches vim, where `0` is a
    /// motion). Returns true when the digit was absorbed as a count.
    pub fn push_count_digit(&mut self, digit: u32) -> bool {
        self.pending_g = false;
        if self.pending_count.is_none() && digit == 0 {
            return false;
        }
        let acc = self.pending_count.unwrap_or(0);
        // Saturate rather than overflow on absurdly long digit runs.
        self.pending_count = Some(acc.saturating_mul(10).saturating_add(digit as usize));
        true
    }

    /// Consume the pending count, defaulting to 1 when none was typed.
    /// Also clears any half-typed `gg`.
    pub fn take_count(&mut self) -> usize {
        self.pending_g = false;
        self.pending_count.take().unwrap_or(1).max(1)
    }

    /// Peek at the pending count without consuming it.
    pub fn pending_count(&self) -> Option<usize> {
        self.pending_count
    }

    /// Register a `g` keypress. Returns true when this completes a `gg`
    /// (the caller should jump to the top); false when it merely arms the
    /// first `g`.
    pub fn note_g(&mut self) -> bool {
        self.pending_count = None;
        if self.pending_g {
            self.pending_g = false;
            true
        } else {
            self.pending_g = true;
            false
        }
    }

    /// Drop any half-entered count / `gg`. Called on blur and after any
    /// non-count key so a stale prefix never applies to a later motion.
    pub fn clear_pending(&mut self) {
        self.pending_count = None;
        self.pending_g = false;
    }

    pub fn toggle_selected_dir(&mut self) -> bool {
        let Some(path) = self.selected_note_path() else {
            return false;
        };
        if !self.note_is_dir(self.selected_index) {
            return false;
        }
        if self.open_dirs.contains(&path) {
            self.open_dirs.remove(&path);
        } else {
            self.open_dirs.insert(path.clone());
        }
        self.rebuild_rows();
        if let Some(row) = self.row_index_for_path(&path) {
            self.selected_index = row;
        }
        self.clamp_selection_and_scroll();
        true
    }

    /// Effective row height in logical pixels (base * scale). Matches
    /// `file_tree::row_height` so both panels scroll in lockstep.
    pub fn row_height(&self) -> f32 {
        ROW_HEIGHT * self.scale
    }

    /// Number of note rows that fit in `panel_height` logical pixels
    /// (the inner content height, frame stroke already removed). Mirrors
    /// `file_tree::visible_rows_for_panel_height`.
    pub fn visible_rows_for_panel_height(&self, panel_height: f32) -> usize {
        let frame_stroke = (FRAME_STROKE * self.scale).max(2.0);
        let content_h = (panel_height - frame_stroke * 2.0).max(0.0);
        // The list does not own the whole content rect: the single-row
        // title/actions header and footer vault selector consume about
        // three rows including their gaps. Subtract them
        // so wheel/keyboard paging matches what the user actually sees.
        let row_h = self.row_height();
        if row_h <= 0.0 {
            return 1;
        }
        let chrome_rows = 3.0;
        ((content_h / row_h) - chrome_rows).floor().max(1.0) as usize
    }

    /// Bump `scroll_top` by `delta` rows in either direction, clamped to
    /// the panel height, and feed the lag spring so the motion eases.
    /// Mirrors `file_tree::scroll_by`.
    pub fn scroll_by(&mut self, delta: i32, panel_height_rows: usize) {
        let old = self.scroll_top;
        let max_top = self.max_scroll_top_for(panel_height_rows);
        if delta < 0 {
            self.scroll_top = self
                .scroll_top
                .saturating_sub(delta.unsigned_abs() as usize);
        } else {
            self.scroll_top = self.scroll_top.saturating_add(delta as usize).min(max_top);
        }
        if old != self.scroll_top {
            self.push_scroll_lag(old, self.scroll_top);
        }
    }

    /// Trackpad PIXEL scrolling. Accumulates sub-row pixel deltas and
    /// only steps `scroll_top` once a full row's worth has built up, so a
    /// slow two-finger drag moves smoothly rather than jumping a row per
    /// event. Overscroll at the edges is discarded. Lifted from
    /// `file_tree::scroll_pixels`.
    pub fn scroll_pixels(&mut self, delta_pixels: f32, panel_height_rows: usize) {
        let row_h = self.row_height();
        if row_h <= 0.0 || delta_pixels == 0.0 {
            return;
        }
        self.wheel_accumulator += delta_pixels;
        let mut rows = 0i32;
        while self.wheel_accumulator.abs() >= row_h {
            let sign = self.wheel_accumulator.signum();
            self.wheel_accumulator -= sign * row_h;
            rows += if sign > 0.0 { -1 } else { 1 };
        }
        if rows != 0 {
            self.scroll_by(rows, panel_height_rows);
        }
        let max_top = self.max_scroll_top_for(panel_height_rows);
        if (self.scroll_top == 0 && self.wheel_accumulator > 0.0)
            || (self.scroll_top == max_top && self.wheel_accumulator < 0.0)
        {
            self.wheel_accumulator = 0.0;
        }
    }

    fn push_scroll_lag(&mut self, old_top: usize, new_top: usize) {
        if old_top == new_top {
            return;
        }
        let was_idle = self.scroll.position == 0.0;
        let rows = new_top as i32 - old_top as i32;
        self.scroll.position += rows as f32 * self.row_height();
        if was_idle {
            self.last_scroll_frame = Instant::now();
        }
    }

    fn set_scroll_top(&mut self, new_top: usize) {
        let old = self.scroll_top;
        self.scroll_top = new_top;
        self.push_scroll_lag(old, self.scroll_top);
    }

    /// Step the scroll lag spring forward and return its current offset
    /// in logical pixels (snapped to the device grid by the render path).
    fn tick_scroll(&mut self) -> f32 {
        if self.scroll.position == 0.0 {
            self.last_scroll_frame = Instant::now();
            return 0.0;
        }
        let now = Instant::now();
        let dt = now
            .saturating_duration_since(self.last_scroll_frame)
            .as_secs_f32()
            .min(0.05);
        self.last_scroll_frame = now;
        self.scroll.update(dt, SCROLL_ANIMATION_LENGTH);
        self.scroll.position
    }

    /// Step the cursor lag spring forward and return its offset.
    fn tick_cursor(&mut self) -> f32 {
        if self.cursor_spring.position == 0.0 {
            self.last_cursor_frame = Instant::now();
            return 0.0;
        }
        let now = Instant::now();
        let dt = now
            .saturating_duration_since(self.last_cursor_frame)
            .as_secs_f32()
            .min(0.05);
        self.last_cursor_frame = now;
        self.cursor_spring.update(dt, CURSOR_ANIMATION_LENGTH);
        self.cursor_spring.position
    }

    /// True while a scroll or cursor spring is still settling — hosts use
    /// this to keep requesting redraws so the eased motion plays out
    /// instead of snapping on the next unrelated frame.
    pub fn is_animating(&self) -> bool {
        let wordmark_settling = self
            .wordmark
            .hover
            .iter()
            .any(|hover| *hover > 0.005 && *hover < 0.995);
        let vault_press_animating = self
            .vault_press_started_at
            .is_some_and(|started| started.elapsed() < Duration::from_millis(360));
        self.visible
            && (self.scroll.position != 0.0
                || self.cursor_spring.position != 0.0
                || wordmark_settling
                || vault_press_animating)
    }

    pub fn hit_test(&self, x: f32, y: f32) -> Option<NotesSidebarHit> {
        for (rect, index) in &self.icon_rects {
            if rect_contains(*rect, x, y) {
                return Some(NotesSidebarHit::NoteIcon(*index));
            }
        }
        for (rect, index) in &self.note_rects {
            if rect_contains(*rect, x, y) {
                return Some(NotesSidebarHit::Note(*index));
            }
        }
        if let Some(r) = self.settings_rect {
            if rect_contains(r, x, y) {
                return Some(NotesSidebarHit::Settings);
            }
        }
        if let Some(r) = self.new_note_rect {
            if rect_contains(r, x, y) {
                return Some(NotesSidebarHit::NewNote);
            }
        }
        if let Some(r) = self.new_folder_rect {
            if rect_contains(r, x, y) {
                return Some(NotesSidebarHit::NewFolder);
            }
        }
        if let Some(r) = self.empty_create_rect {
            if rect_contains(r, x, y) {
                return Some(NotesSidebarHit::CreateFirstNote);
            }
        }
        if let Some(r) = self.empty_link_vault_rect {
            if rect_contains(r, x, y) {
                return Some(NotesSidebarHit::CreateWorkspaceVault);
            }
        }
        if let Some(r) = self.empty_select_vault_rect {
            if rect_contains(r, x, y) {
                return Some(NotesSidebarHit::SelectVault);
            }
        }
        if rect_contains(self.workspace_rect?, x, y) {
            return Some(NotesSidebarHit::WorkspacePicker);
        }
        None
    }

    pub fn render(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        x_left: f32,
        y_top: f32,
        panel_width: f32,
        panel_height: f32,
        theme: &IdeTheme,
        occlusion: &[[f32; 4]],
        mouse: Option<(f32, f32)>,
        now_seconds: f32,
    ) {
        if !self.visible || panel_width <= 0.0 || panel_height <= 0.0 {
            return;
        }
        self.workspace_rect = None;
        self.settings_rect = None;
        self.new_note_rect = None;
        self.new_folder_rect = None;
        self.empty_create_rect = None;
        self.empty_link_vault_rect = None;
        self.empty_select_vault_rect = None;
        self.note_rects.clear();
        self.icon_rects.clear();
        self.selected_cursor_rect = None;

        let row_h = ROW_HEIGHT * self.scale;
        let font_size = FONT_SIZE * self.scale;
        let icon_size = ICON_FONT_SIZE * self.scale;
        let row_pad_x = ROW_PADDING_X * self.scale;
        let indent_px = INDENT_PX * self.scale;
        let icon_gap = ICON_GAP * self.scale;
        let frame_stroke = (FRAME_STROKE * self.scale).max(2.0);
        let frame_radius = FRAME_RADIUS * self.scale;
        let content_x = x_left + frame_stroke;
        let content_y = y_top + frame_stroke;
        let content_w = (panel_width - frame_stroke * 2.0).max(0.0);
        let content_h = (panel_height - frame_stroke * 2.0).max(0.0);
        let content_radius = (frame_radius - frame_stroke).max(0.0);
        let panel_bottom = content_y + content_h;
        let panel_clip = [content_x, content_y, content_w, content_h];

        draw_frame_top(
            sugarloaf,
            [x_left, y_top, panel_width, panel_height],
            theme.f32(theme.surface),
            theme.f32(theme.bg),
            frame_radius,
            frame_stroke,
        );

        let muted_opts = DrawOpts {
            font_size: font_size * 0.86,
            color: theme.u8(theme.muted),
            clip_rect: Some(panel_clip),
            ..DrawOpts::default()
        };
        let action_opts = DrawOpts {
            font_size: icon_size,
            color: theme.u8_alpha(theme.fg, 0.72),
            clip_rect: Some(panel_clip),
            ..DrawOpts::default()
        };

        let header_y = content_y + 8.0 * self.scale;
        // "Notes" splash header — the bundled Press Start 2P arcade
        // face (same as the agent side-panel headings) with the
        // wordmark's per-letter hover lift + shimmer, sized to span
        // the full panel top.
        let wordmark_h;
        {
            use crate::panels::agent_pane::view::wordmark::WordmarkState;
            const SPLASH: &str = "Notes";
            const SHIMMER_PERIOD: f32 = 3.4;
            const SHIMMER_AMP: f32 = 0.03;
            const HOVER_RATE: f32 = 10.0;
            const HOVER_SCALE: f32 = 0.18;
            const HOVER_LIFT: f32 = 0.16;
            let pixel_font = crate::primitives::pixel_font_id(sugarloaf);
            let target_w = (content_w - row_pad_x * 2.0).max(1.0);
            let probe_opts = DrawOpts {
                font_size: 10.0,
                font_id: pixel_font,
                ..DrawOpts::default()
            };
            let probe_w = sugarloaf.text_mut().measure(SPLASH, &probe_opts).max(1.0);
            // Match the agent side-panel heading size (~15.5px drawn);
            // the width fit only kicks in on very narrow panels.
            let splash_size =
                (16.0 * self.scale).min((10.0 * target_w / probe_w).max(10.0));
            wordmark_h = splash_size * 1.15;
            let rect = [content_x + row_pad_x, header_y, target_w, wordmark_h];
            self.wordmark.set_rect(rect);
            let dt = self.wordmark.frame_delta_seconds();
            let smoothing = 1.0 - (-HOVER_RATE * dt).exp();
            let letter_opts = DrawOpts {
                font_size: splash_size,
                color: theme.u8(theme.fg),
                font_id: pixel_font,
                clip_rect: Some(panel_clip),
                ..DrawOpts::default()
            };
            let mut lx = rect[0];
            for (i, ch) in SPLASH.chars().enumerate() {
                let letter = ch.to_string();
                let lw = sugarloaf.text_mut().measure(&letter, &letter_opts).max(1.0);
                let target = mouse
                    .map(|(mx, my)| {
                        mx >= lx
                            && mx <= lx + lw
                            && my >= rect[1]
                            && my <= rect[1] + wordmark_h
                    })
                    .unwrap_or(false) as u8 as f32;
                let hover = &mut self.wordmark.hover[i];
                *hover += (target - *hover) * smoothing;
                let hover = hover.clamp(0.0, 1.0);
                let shimmer = ((now_seconds / SHIMMER_PERIOD + i as f32 * 0.16)
                    * std::f32::consts::TAU)
                    .sin()
                    * SHIMMER_AMP;
                let extra = 1.0 + hover * HOVER_SCALE + shimmer;
                let opts = DrawOpts {
                    font_size: splash_size * extra,
                    ..letter_opts
                };
                let lift = hover * HOVER_LIFT * wordmark_h;
                draw_text_with_occlusion(
                    sugarloaf,
                    lx + lw * (1.0 - extra) * 0.5,
                    header_y + (wordmark_h - splash_size * extra) * 0.5 - lift,
                    &letter,
                    &opts,
                    occlusion,
                );
                lx += lw;
            }
        }

        // Quick-create actions share the title row: Notes on the left,
        // new note + folder hugging the right edge.
        let create_size = (row_h * 0.86).max(22.0 * self.scale);
        // Sugarloaf normalizes the Nerd Font run to the same centered
        // primary-font line box as the title, so the button and hit target
        // can use the row's geometric center without a family-specific lift.
        let create_y = header_y + (wordmark_h - create_size) * 0.5;
        let create_gap = 4.0 * self.scale;
        let create_right = content_x + content_w - row_pad_x;
        let new_folder_rect = [
            create_right - create_size,
            create_y,
            create_size,
            create_size,
        ];
        let new_note_rect = [
            new_folder_rect[0] - create_gap - create_size,
            create_y,
            create_size,
            create_size,
        ];
        self.new_note_rect = Some(new_note_rect);
        self.new_folder_rect = Some(new_folder_rect);
        let create_opts = DrawOpts {
            font_size: icon_size * 1.08,
            color: theme.u8_alpha(theme.fg, 0.72),
            clip_rect: Some(panel_clip),
            ..DrawOpts::default()
        };
        for (rect, glyph) in [(new_note_rect, "\u{f044}"), (new_folder_rect, "\u{f07b}")]
        {
            if mouse.is_some_and(|(mx, my)| rect_contains(rect, mx, my)) {
                sugarloaf.quad(
                    None,
                    rect[0],
                    rect[1],
                    rect[2],
                    rect[3],
                    theme.f32_alpha(theme.hover, 0.5),
                    [5.0 * self.scale; 4],
                    DEPTH,
                    ORDER + 2,
                );
            }
            draw_icon_centered_with_occlusion(
                sugarloaf,
                rect[0],
                rect,
                glyph,
                &create_opts,
                occlusion,
                true,
            );
        }

        let footer_y = content_y + content_h - row_h - 6.0 * self.scale;
        // Footer: vault selector row + the settings gear on its right
        // (vault and graph/settings controls stay out of the create row).
        let footer_divider_y = footer_y - 6.0 * self.scale;
        sugarloaf.rect(
            None,
            content_x + 6.0 * self.scale,
            footer_divider_y,
            (content_w - 12.0 * self.scale).max(0.0),
            (2.5 * self.scale).max(2.0),
            theme.f32_alpha(theme.border, 0.78),
            DEPTH,
            ORDER + 1,
        );
        let settings_w = row_h;
        let workspace_rect = [
            content_x + 6.0 * self.scale,
            footer_y,
            (content_w - settings_w - 16.0 * self.scale).max(0.0),
            row_h,
        ];
        self.workspace_rect = Some(workspace_rect);
        let settings_rect = [
            content_x + content_w - settings_w - 6.0 * self.scale,
            footer_y,
            settings_w,
            row_h,
        ];
        self.settings_rect = Some(settings_rect);
        if self.focused && self.selector_selected && self.settings_selected {
            sugarloaf.quad(
                None,
                settings_rect[0],
                settings_rect[1],
                settings_rect[2],
                settings_rect[3],
                theme.f32_alpha(theme.hover, 0.5),
                [6.0 * self.scale; 4],
                DEPTH,
                ORDER + 2,
            );
            // The block cursor stays visible while the gear is focused
            // — it parks just left of the settings button instead of
            // vanishing when focus walks off the vault selector.
            let cursor_w = (font_size * 0.6).max(2.0);
            let cursor_h = (row_h - 6.0 * self.scale).max(font_size).min(row_h);
            self.selected_cursor_rect = Some([
                (settings_rect[0] - cursor_w - 4.0 * self.scale).max(content_x),
                footer_y + (row_h - cursor_h) / 2.0,
                cursor_w,
                cursor_h,
            ]);
        }
        draw_icon_centered_with_occlusion(
            sugarloaf,
            settings_rect[0],
            settings_rect,
            "\u{f013}",
            &action_opts,
            occlusion,
            true,
        );
        let header_bottom = (header_y + wordmark_h).max(create_y + create_size);
        let list_y = header_bottom + 8.0 * self.scale;
        let list_h = (footer_y - list_y - 8.0 * self.scale).max(row_h);
        let rows_visible = (list_h / row_h).floor().max(1.0) as usize;
        // Re-clamp before painting — a terminal resize can shrink the
        // panel between input and frame. Use the bounds-only clamp (not
        // the selection-following clamp) so a wheel scroll that parks the
        // viewport away from the selection isn't snapped back. Mirrors
        // file_tree's render path.
        self.last_panel_height_rows = rows_visible;
        self.clamp_scroll_bounds(rows_visible);
        let scroll_offset =
            snap_to_device_px(self.tick_scroll(), sugarloaf.scale_factor());
        let cursor_offset = self.tick_cursor();

        if !self.selector_selected
            && !self.rows.is_empty()
            && self.selected_index < self.rows.len()
        {
            let row_ix = self.selected_index as isize - self.scroll_top as isize;
            let row_y = list_y + row_ix as f32 * row_h + scroll_offset + cursor_offset;
            let row_bottom = row_y + row_h;
            let visible_row_y = row_y.max(list_y);
            let visible_row_h = row_bottom.min(list_y + list_h) - visible_row_y;
            if visible_row_h > 0.0 {
                sugarloaf.quad(
                    None,
                    content_x,
                    visible_row_y,
                    content_w,
                    visible_row_h,
                    theme.f32(theme.surface),
                    edge_row_radii(
                        visible_row_y,
                        visible_row_h,
                        content_y,
                        panel_bottom,
                        content_radius,
                    ),
                    DEPTH,
                    ORDER + 2,
                );
                if self.focused {
                    let cursor_w = (font_size * 0.6).max(2.0);
                    let cursor_x = content_x + (row_pad_x - cursor_w).max(0.0);
                    let cursor_h = (row_h - 6.0 * self.scale)
                        .max(font_size)
                        .min(row_h)
                        .min(content_h.max(2.0));
                    let cursor_y = (row_y + (row_h - cursor_h) / 2.0)
                        .clamp(content_y, (panel_bottom - cursor_h).max(content_y));
                    self.selected_cursor_rect =
                        Some([cursor_x, cursor_y, cursor_w, cursor_h]);
                }
            }
        }

        // Spring-loaded drag: resolve the lifted source row, the hovered
        // drop-target folder, and the wiggle phase — all by PATH so a
        // spring-open re-indexing the rows mid-drag can't lose them.
        // Mirrors `file_tree::render`.
        let drag_source_row = self.notes_drag_source_row();
        let drag_hovered_row = self.notes_drag_hovered_row();
        let drag_wiggle_dx = self
            .notes_drag
            .as_ref()
            .filter(|drag| drag.live)
            .map(|drag| {
                let t = drag
                    .hovered_since
                    .map(|since| since.elapsed().as_secs_f32())
                    .unwrap_or(0.0);
                // Amplitude ramps in over ~120ms so the folder shivers to
                // life instead of snapping; ~4Hz feels alive, not jittery.
                let ramp = (t / 0.12).clamp(0.0, 1.0);
                (t * 26.0).sin() * 2.4 * self.scale * ramp
            })
            .unwrap_or(0.0);
        // The dragged row is LIFTED out of the list: its glyph + label
        // follow the cursor on a softly raised sheet (its slot in the
        // list dims to a placeholder). Laid out up front so the sheet's
        // rect can join the text-occlusion set — row labels paint in a
        // layer above plain quads and would bleed through it otherwise.
        // The sheet itself is painted after the rows.
        struct LiftedNoteRow {
            rect: [f32; 4],
            chevron: Option<&'static str>,
            icon: String,
            icon_color: [u8; 4],
            label: String,
            label_color: [u8; 4],
            radius: f32,
        }
        let lifted = if self.notes_drag.as_ref().is_some_and(|drag| drag.live) {
            let (source_label, source_is_dir, current_x, current_y) = {
                let drag = self.notes_drag.as_ref().unwrap();
                (
                    drag.source_label.clone(),
                    drag.source_is_dir,
                    drag.current_x,
                    drag.current_y,
                )
            };
            let entry = drag_source_row.and_then(|ix| self.row_entry(ix)).cloned();
            let is_open = entry
                .as_ref()
                .map(|e| self.open_dirs.contains(&e.path))
                .unwrap_or(false);
            let custom_icon = entry.as_ref().and_then(|e| e.icon.clone());
            let is_markdown_note = !source_is_dir
                && Path::new(&source_label)
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| {
                        ext.eq_ignore_ascii_case("md")
                            || ext.eq_ignore_ascii_case("markdown")
                            || ext.eq_ignore_ascii_case("mdx")
                    });
            let chevron =
                source_is_dir.then(|| if is_open { "\u{f078}" } else { "\u{f054}" });
            let icon: String = if let Some(custom) = custom_icon {
                custom
            } else if source_is_dir {
                if is_open {
                    FOLDER_OPEN_ICON
                } else {
                    FOLDER_CLOSED_ICON
                }
                .to_string()
            } else if is_markdown_note {
                crate::primitives::look::icon_override("note")
                    .and_then(|over| over.glyph)
                    .unwrap_or(NOTE_DEFAULT_ICON)
                    .to_string()
            } else {
                icon_for_file(&source_label).0.to_string()
            };
            let icon_color = if source_is_dir {
                theme.u8(theme.folder)
            } else if is_markdown_note {
                theme.u8_alpha(theme.fg, 0.72)
            } else {
                icon_for_file(&source_label).1
            };
            let pad = 10.0 * self.scale;
            let text_opts = DrawOpts {
                font_size,
                ..DrawOpts::default()
            };
            let glyph_opts = DrawOpts {
                font_size: icon_size,
                ..DrawOpts::default()
            };
            let label =
                truncate_label(&source_label, 240.0 * self.scale, sugarloaf, &text_opts);
            let label_w = sugarloaf.text_mut().measure(&label, &text_opts);
            let icon_w = sugarloaf.text_mut().measure(&icon, &glyph_opts);
            let chevron_w = chevron
                .map(|chev| sugarloaf.text_mut().measure(chev, &glyph_opts) + icon_gap)
                .unwrap_or(0.0);
            let sheet_w = pad + chevron_w + icon_w + icon_gap + label_w + pad;
            let sheet_h = row_h;
            // Sit the row just off the cursor's lower-right, like Finder.
            let sheet_x = current_x + 12.0 * self.scale;
            let sheet_y = current_y - sheet_h * 0.5;
            Some(LiftedNoteRow {
                rect: [sheet_x, sheet_y, sheet_w, sheet_h],
                chevron,
                icon,
                icon_color,
                label,
                label_color: theme.u8(theme.fg),
                radius: 6.0 * self.scale,
            })
        } else {
            None
        };
        // Fold the lifted sheet rect into the occlusion set the row text
        // honors, so the rows underneath don't bleed through it.
        let mut occlusion_owned: Vec<[f32; 4]>;
        let occlusion: &[[f32; 4]] = match lifted.as_ref() {
            Some(l) => {
                occlusion_owned = occlusion.to_vec();
                occlusion_owned.push(l.rect);
                &occlusion_owned
            }
            None => occlusion,
        };

        if self.rows.is_empty() {
            // Centered empty state: "No notes yet" plus action button(s)
            // underneath. Local / linked vaults get the single "+ New
            // note"; a served workspace with no linked vault gets the
            // Notion-style pair "+ Create workspace vault" / "Select
            // vault" (`show_vault_actions`).
            let empty_text = if self.show_vault_actions {
                "No notes vault linked"
            } else {
                "No notes yet"
            };
            let empty_w = sugarloaf.text_mut().measure(empty_text, &muted_opts);
            draw_text_with_occlusion(
                sugarloaf,
                content_x + ((content_w - empty_w) * 0.5).max(0.0),
                list_y + 5.0 * self.scale,
                empty_text,
                &muted_opts,
                occlusion,
            );
            let btn_font = font_size * 0.92;
            let scale = self.scale;
            let blue = theme.u8(theme.blue);
            let hover = theme.f32_alpha(theme.hover, 0.5);
            // Pill button: centered, hover-tinted, returns its rect for
            // hit-testing. Shared by both empty-state variants.
            let draw_btn =
                |sl: &mut Sugarloaf, icon: &str, label: &str, top: f32| -> [f32; 4] {
                    let opts = DrawOpts {
                        font_size: btn_font,
                        color: blue,
                        clip_rect: Some(panel_clip),
                        ..DrawOpts::default()
                    };
                    let label_w = sl.text_mut().measure(label, &opts);
                    let icon_slot = btn_font;
                    let icon_gap = 8.0 * scale;
                    let pad_h = 10.0 * scale;
                    let btn_w = icon_slot + icon_gap + label_w + pad_h * 2.0;
                    let btn = [
                        content_x + ((content_w - btn_w) * 0.5).max(0.0),
                        top,
                        btn_w,
                        row_h * 0.95,
                    ];
                    sl.quad(
                        None,
                        btn[0],
                        btn[1],
                        btn[2],
                        btn[3],
                        hover,
                        [8.0 * scale; 4],
                        DEPTH,
                        ORDER + 2,
                    );
                    let icon_x = btn[0] + pad_h;
                    draw_icon_centered_with_occlusion(
                        sl,
                        icon_x,
                        [icon_x, btn[1], icon_slot, btn[3]],
                        icon,
                        &opts,
                        occlusion,
                        true,
                    );
                    draw_text_with_occlusion(
                        sl,
                        icon_x + icon_slot + icon_gap,
                        btn[1] + (btn[3] - btn_font) / 2.0,
                        label,
                        &opts,
                        occlusion,
                    );
                    btn
                };
            let first_top = list_y + 5.0 * self.scale + row_h;
            if self.show_vault_actions {
                let link_btn =
                    draw_btn(sugarloaf, "\u{f067}", "Create workspace vault", first_top);
                let select_btn = draw_btn(
                    sugarloaf,
                    "\u{f07b}",
                    "Select vault",
                    first_top + row_h * 1.15,
                );
                self.empty_link_vault_rect = Some(link_btn);
                self.empty_select_vault_rect = Some(select_btn);
            } else {
                self.empty_create_rect =
                    Some(draw_btn(sugarloaf, "\u{f067}", "New note", first_top));
            }
        } else {
            // Overscan: while the lag spring is mid-flight the viewport
            // sits between two rows, so paint a row above/below the window
            // to fill the gap. Rows that fall fully outside the list band
            // are skipped per-row below. Mirrors file_tree's render loop.
            let overscan =
                ((scroll_offset.abs() / row_h).ceil() as usize).saturating_add(1);
            let start = self.scroll_top.saturating_sub(overscan);
            let end = (self.scroll_top + rows_visible + overscan).min(self.rows.len());
            for absolute_ix in start..end {
                let Some(entry) = self.row_entry(absolute_ix).cloned() else {
                    continue;
                };
                let row_ix = absolute_ix as isize - self.scroll_top as isize;
                let row_y = list_y + row_ix as f32 * row_h + scroll_offset;
                let row_bottom = row_y + row_h;
                let visible_row_y = row_y.max(list_y);
                let visible_row_h = row_bottom.min(list_y + list_h) - visible_row_y;
                if visible_row_h <= 0.0 {
                    continue;
                }
                self.note_rects
                    .push(([content_x, row_y, content_w, row_h], absolute_ix));

                let is_selected = absolute_ix == self.selected_index;
                // Spring-loaded drop target: accent-tinted band so it
                // reads as "release here". The source row dims to a
                // placeholder while it rides the cursor. Mirrors file_tree.
                let is_drop_target = drag_hovered_row == Some(absolute_ix);
                if is_drop_target {
                    sugarloaf.quad(
                        None,
                        content_x,
                        visible_row_y,
                        content_w,
                        visible_row_h,
                        theme.f32_alpha(theme.accent, 0.22),
                        edge_row_radii(
                            visible_row_y,
                            visible_row_h,
                            content_y,
                            panel_bottom,
                            content_radius,
                        ),
                        DEPTH,
                        ORDER + 4,
                    );
                }
                let row_dim = if drag_source_row == Some(absolute_ix) {
                    0.32
                } else {
                    1.0
                };
                let chevron = if entry.is_dir {
                    Some(if self.open_dirs.contains(&entry.path) {
                        "\u{f078}"
                    } else {
                        "\u{f054}"
                    })
                } else {
                    None
                };
                // Markdown notes default to the note glyph (the picker's
                // "Note"); other file types (yaml, toml, images, …) keep
                // their real per-extension icon so they read as what they
                // are. Folders keep the folder icon. All overridable.
                let is_markdown_note = !entry.is_dir
                    && Path::new(&entry.label)
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .is_some_and(|ext| {
                            ext.eq_ignore_ascii_case("md")
                                || ext.eq_ignore_ascii_case("markdown")
                                || ext.eq_ignore_ascii_case("mdx")
                        });
                let icon = if entry.is_dir {
                    if self.open_dirs.contains(&entry.path) {
                        FOLDER_OPEN_ICON
                    } else {
                        FOLDER_CLOSED_ICON
                    }
                } else if is_markdown_note {
                    // Mash-up override for the DEFAULT note glyph only —
                    // per-note user icons (.neoism-icons.json, painted in
                    // the `entry.icon` branch below) still win.
                    crate::primitives::look::icon_override("note")
                        .and_then(|over| over.glyph)
                        .unwrap_or(NOTE_DEFAULT_ICON)
                } else {
                    icon_for_file(&entry.label).0
                };
                let icon_color = if entry.is_dir {
                    theme.u8(theme.folder)
                } else if is_markdown_note {
                    theme.u8_alpha(theme.fg, 0.72)
                } else {
                    icon_for_file(&entry.label).1
                };
                let label_color = if entry.is_dir || is_selected {
                    theme.u8(theme.fg)
                } else {
                    theme.u8(theme.dim)
                };
                let chevron_opts = DrawOpts {
                    font_size,
                    color: fade_u8(theme.u8(theme.muted), row_dim),
                    clip_rect: Some(panel_clip),
                    ..DrawOpts::default()
                };
                let icon_opts = DrawOpts {
                    font_size: icon_size,
                    color: fade_u8(icon_color, row_dim),
                    clip_rect: Some(panel_clip),
                    ..DrawOpts::default()
                };
                let label_opts = DrawOpts {
                    font_size,
                    color: fade_u8(label_color, row_dim),
                    clip_rect: Some(panel_clip),
                    ..DrawOpts::default()
                };
                // The drop-target folder wiggles under the drag.
                let base_x = content_x
                    + row_pad_x
                    + entry.depth as f32 * indent_px
                    + if is_drop_target { drag_wiggle_dx } else { 0.0 };
                let text_y = row_y + (row_h - font_size) / 2.0;
                let mut cursor_x = base_x;
                if let Some(chevron) = chevron {
                    draw_icon_centered_with_occlusion(
                        sugarloaf,
                        cursor_x,
                        [cursor_x, row_y, indent_px, row_h],
                        chevron,
                        &chevron_opts,
                        occlusion,
                        true,
                    );
                }
                cursor_x += indent_px;
                // The icon is a click target: a tap on it opens the
                // Notion-style icon/emoji picker for this entry.
                self.icon_rects
                    .push(([cursor_x - 2.0, row_y, icon_size + 4.0, row_h], absolute_ix));
                // A blank/whitespace-only custom icon is treated as "no
                // custom icon" so it falls back to the default glyph instead
                // of rendering an empty box (belt-and-suspenders: the icon
                // map already drops empty values in `load_notes_icons`).
                if let Some(custom) = entry
                    .icon
                    .as_deref()
                    .filter(|glyph| !glyph.trim().is_empty())
                {
                    let custom_opts = DrawOpts {
                        font_size: icon_size,
                        color: fade_u8(theme.u8(theme.fg), row_dim),
                        clip_rect: Some(panel_clip),
                        ..DrawOpts::default()
                    };
                    draw_icon_centered_with_occlusion(
                        sugarloaf,
                        cursor_x,
                        [cursor_x, row_y, icon_size, row_h],
                        custom,
                        &custom_opts,
                        occlusion,
                        true,
                    );
                } else {
                    draw_icon_centered_with_occlusion(
                        sugarloaf,
                        cursor_x,
                        [cursor_x, row_y, icon_size, row_h],
                        icon,
                        &icon_opts,
                        occlusion,
                        true,
                    );
                }
                cursor_x += icon_size + icon_gap;
                let budget = (content_x + content_w - cursor_x - row_pad_x).max(0.0);
                let label = truncate_label(&entry.label, budget, sugarloaf, &label_opts);
                draw_text_with_occlusion(
                    sugarloaf,
                    cursor_x,
                    text_y,
                    &label,
                    &label_opts,
                    occlusion,
                );
            }
        }

        // Whole-panel "drop into the vault root" affordance: the root owns
        // no row, so when the drag targets it we wash the list band in a
        // faint accent tint (rows still show through) instead of a per-row
        // band. Drawn above the rows, below the cursor ghost. Mirrors
        // file_tree's root wash.
        if self.is_notes_root_drop_target() {
            sugarloaf.quad(
                None,
                content_x,
                list_y,
                content_w,
                list_h,
                theme.f32_alpha(theme.accent, 0.14),
                [content_radius; 4],
                DEPTH,
                ORDER + 5,
            );
        }
        // Paint the lifted row above the list (its rect was already cut
        // out of the row text above). A soft shadow + faintly raised sheet
        // reads as the real row peeled up off the list — no synthetic
        // chrome. No clip: it follows the cursor past the panel edge.
        if let Some(l) = lifted {
            let s = self.scale;
            let [lx, ly, lw, lh] = l.rect;
            let pad = 10.0 * s;
            sugarloaf.quad(
                None,
                lx + 1.5 * s,
                ly + 3.0 * s,
                lw,
                lh,
                theme.f32_alpha(theme.bg, 0.40),
                [l.radius; 4],
                DEPTH,
                ORDER + 6,
            );
            sugarloaf.quad(
                None,
                lx,
                ly,
                lw,
                lh,
                theme.f32_alpha(theme.surface, 0.94),
                [l.radius; 4],
                DEPTH,
                ORDER + 7,
            );
            let chevron_opts = DrawOpts {
                font_size: icon_size,
                color: theme.u8(theme.muted),
                ..DrawOpts::default()
            };
            let icon_opts = DrawOpts {
                font_size: icon_size,
                color: l.icon_color,
                ..DrawOpts::default()
            };
            let label_opts = DrawOpts {
                font_size,
                color: l.label_color,
                ..DrawOpts::default()
            };
            let mut cx = lx + pad;
            let text_y = ly + (lh - font_size) / 2.0;
            if let Some(chev) = l.chevron {
                let width = sugarloaf.text_mut().measure(chev, &chevron_opts);
                draw_icon_centered_with_occlusion(
                    sugarloaf,
                    cx,
                    [cx, ly, width, lh],
                    chev,
                    &chevron_opts,
                    &[],
                    true,
                );
                cx += width + icon_gap;
            }
            let width = sugarloaf.text_mut().measure(&l.icon, &icon_opts);
            draw_icon_centered_with_occlusion(
                sugarloaf,
                cx,
                [cx, ly, width, lh],
                &l.icon,
                &icon_opts,
                &[],
                true,
            );
            cx += width + icon_gap;
            sugarloaf.text_mut().draw(cx, text_y, &l.label, &label_opts);
        }

        let footer_hover =
            self.focused && self.selector_selected && !self.settings_selected;
        let pointer_hover =
            mouse.is_some_and(|(mx, my)| rect_contains(workspace_rect, mx, my));
        let press_progress = self.vault_press_started_at.and_then(|started| {
            let progress = started.elapsed().as_secs_f32() / 0.36;
            if progress >= 1.0 {
                self.vault_press_started_at = None;
                None
            } else {
                Some(progress)
            }
        });
        if footer_hover || pointer_hover || press_progress.is_some() {
            let pulse = press_progress
                .map(|progress| (progress * std::f32::consts::PI).sin())
                .unwrap_or(0.0);
            let expand_x = pulse * 3.0 * self.scale;
            let expand_y = pulse * 1.5 * self.scale;
            sugarloaf.quad(
                None,
                workspace_rect[0] - expand_x,
                workspace_rect[1] - expand_y,
                workspace_rect[2] + expand_x * 2.0,
                workspace_rect[3] + expand_y * 2.0,
                theme.f32_alpha(
                    if pulse > 0.0 {
                        theme.accent
                    } else {
                        theme.hover
                    },
                    0.34 + pulse * 0.24,
                ),
                [8.0 * self.scale + pulse * 3.0 * self.scale; 4],
                DEPTH,
                ORDER + 2,
            );
            if pulse > 0.0 {
                sugarloaf.rect(
                    None,
                    workspace_rect[0],
                    workspace_rect[1] + workspace_rect[3] - 2.0 * self.scale,
                    workspace_rect[2],
                    (2.0 * self.scale).max(1.0),
                    theme.f32_alpha(theme.accent, 0.45 * pulse),
                    DEPTH,
                    ORDER + 3,
                );
            }
        }
        if footer_hover {
            let cursor_w = (font_size * 0.6).max(2.0);
            let cursor_x = content_x + (row_pad_x - cursor_w).max(0.0);
            let cursor_h = (row_h - 6.0 * self.scale).max(font_size).min(row_h);
            let cursor_y = footer_y + (row_h - cursor_h) / 2.0;
            self.selected_cursor_rect = Some([cursor_x, cursor_y, cursor_w, cursor_h]);
        }
        // Keep the vault label on the shared text baseline, but centre the
        // dropdown glyph by its visible ink so fallback-font bearings do
        // not pull it above the label.
        let footer_x = content_x + row_pad_x;
        let chevron_slot = muted_opts.font_size;
        let chevron_gap = 6.0 * self.scale;
        let name_budget = (workspace_rect[0] + workspace_rect[2]
            - footer_x
            - chevron_gap
            - chevron_slot)
            .max(0.0);
        let workspace_name =
            truncate_label(&self.workspace_name, name_budget, sugarloaf, &muted_opts);
        let name_w = sugarloaf.text_mut().measure(&workspace_name, &muted_opts);
        draw_text_with_occlusion(
            sugarloaf,
            footer_x,
            footer_y + (row_h - font_size * 0.86) * 0.5,
            &workspace_name,
            &muted_opts,
            occlusion,
        );
        let chevron_x = footer_x + name_w + chevron_gap;
        draw_icon_centered_with_occlusion(
            sugarloaf,
            chevron_x,
            [chevron_x, footer_y, chevron_slot, row_h],
            "\u{f078}",
            &muted_opts,
            occlusion,
            true,
        );
    }

    fn rebuild_rows(&mut self) {
        self.rows.clear();
        let by_parent = children_by_parent(&self.all_entries);
        let Some(root) = self.workspace_path.clone() else {
            return;
        };
        push_visible_children(
            &self.all_entries,
            &by_parent,
            &self.open_dirs,
            &root,
            &mut self.rows,
        );
    }

    fn row_entry(&self, row: usize) -> Option<&NoteSidebarEntry> {
        let entry_index = self.rows.get(row)?.entry_index;
        self.all_entries.get(entry_index)
    }

    fn row_index_for_path(&self, path: &Path) -> Option<usize> {
        self.rows.iter().position(|row| {
            self.all_entries
                .get(row.entry_index)
                .is_some_and(|entry| entry.path == path)
        })
    }

    fn max_scroll_top_for(&self, rows_visible: usize) -> usize {
        self.rows.len().saturating_sub(rows_visible.max(1))
    }

    /// Keep `selected_index` inside the visible window, feeding the lag
    /// spring (via `set_scroll_top`) so keyboard navigation that pushes
    /// the viewport eases like the file tree.
    fn clamp_scroll(&mut self, rows_visible: usize) {
        if self.rows.is_empty() {
            self.scroll_top = 0;
            return;
        }
        let rows_visible = rows_visible.max(1);
        if self.selected_index < self.scroll_top {
            self.set_scroll_top(self.selected_index);
        } else if self.selected_index >= self.scroll_top + rows_visible {
            self.set_scroll_top(self.selected_index.saturating_sub(rows_visible - 1));
        }
        let max_top = self.max_scroll_top_for(rows_visible);
        if self.scroll_top > max_top {
            self.set_scroll_top(max_top);
        }
    }

    /// Clamp `scroll_top` to the panel-height-aware bounds without
    /// touching the selection — called each frame before painting so a
    /// terminal resize that shrinks the panel never leaves a blank gap
    /// below the last row. Mirrors `file_tree::clamp_scroll_bounds`.
    fn clamp_scroll_bounds(&mut self, rows_visible: usize) {
        if self.rows.is_empty() {
            self.scroll_top = 0;
            return;
        }
        let max_top = self.max_scroll_top_for(rows_visible);
        if self.scroll_top > max_top {
            self.set_scroll_top(max_top);
        }
    }

    fn clamp_selection_and_scroll(&mut self) {
        if self.rows.is_empty() {
            self.selected_index = 0;
            self.scroll_top = 0;
        } else {
            self.selected_index =
                self.selected_index.min(self.rows.len().saturating_sub(1));
            self.scroll_top = self
                .scroll_top
                .min(self.max_scroll_top_for(self.last_panel_height_rows));
        }
    }
}

/// Mac Finder-style spring-loaded drag-and-drop for the notes sidebar.
/// A press on a page/folder row ARMS a drag; once the cursor travels
/// past [`NOTES_DRAG_ACTIVATION_PX`] it goes `live` and a ghost of the
/// dragged row follows the cursor. Dwelling on a closed folder for
/// [`NOTES_SPRING_OPEN_DWELL`] springs it open, and releasing over a
/// folder / the vault root MOVES the page or folder into it. Mirrors
/// `file_tree::drag` beat for beat — the host (desktop
/// `bridges/workspace/sidebar.rs`) drives begin/update/end and commits
/// the move; the ghost + wiggle paint in [`NotesSidebar::render`].
impl NotesSidebar {
    /// Arm a potential drag from the row at `row`. Every listed row is a
    /// real page/folder with a path under the vault, so all are
    /// draggable; returns `true` when armed. The caller DEFERS activation
    /// (open note / toggle folder) to release.
    pub fn begin_notes_drag(&mut self, row: usize, mouse_x: f32, mouse_y: f32) -> bool {
        let Some(entry) = self.row_entry(row).cloned() else {
            return false;
        };
        self.notes_drag = Some(NotesDragState {
            source_row: row,
            source_path: entry.path,
            source_label: entry.label,
            source_is_dir: entry.is_dir,
            live: false,
            start_x: mouse_x,
            start_y: mouse_y,
            current_x: mouse_x,
            current_y: mouse_y,
            hovered_dir: None,
            hovered_since: None,
            sprang: HashSet::new(),
        });
        true
    }

    /// Drive an armed/live drag: move the ghost, flip `live` past the
    /// activation threshold, resolve the hovered drop-target folder, and
    /// return the path of a closed folder that has been dwelled on long
    /// enough to spring open (returned once per folder). `hovered_row` is
    /// the row currently under the cursor (host hit-test via
    /// [`row_at`](Self::row_at)).
    pub fn update_notes_drag(
        &mut self,
        mouse_x: f32,
        mouse_y: f32,
        hovered_row: Option<usize>,
    ) -> Option<PathBuf> {
        // Resolve the drop target against the row model BEFORE taking a
        // mutable borrow of `self.notes_drag`.
        let source_path = self.notes_drag.as_ref()?.source_path.clone();
        let target = self.notes_drop_target(hovered_row, &source_path);

        let drag = self.notes_drag.as_mut()?;
        drag.current_x = mouse_x;
        drag.current_y = mouse_y;
        if !drag.live {
            let dx = mouse_x - drag.start_x;
            let dy = mouse_y - drag.start_y;
            if (dx * dx + dy * dy).sqrt() < NOTES_DRAG_ACTIVATION_PX {
                return None;
            }
            drag.live = true;
        }

        // Re-arm the dwell clock whenever the hovered folder changes.
        let target_path = target.as_ref().map(|(path, _)| path.clone());
        if drag.hovered_dir != target_path {
            drag.hovered_dir = target_path;
            drag.hovered_since = drag.hovered_dir.as_ref().map(|_| Instant::now());
        }

        if let (Some((dir, closed)), Some(since)) = (target, drag.hovered_since) {
            if closed
                && since.elapsed() >= NOTES_SPRING_OPEN_DWELL
                && drag.sprang.insert(dir.clone())
            {
                return Some(dir);
            }
        }
        None
    }

    /// Finish a drag. Returns the outcome and clears the drag state.
    pub fn end_notes_drag(&mut self) -> NotesDropOutcome {
        let Some(drag) = self.notes_drag.take() else {
            return NotesDropOutcome::Cancel;
        };
        if !drag.live {
            return NotesDropOutcome::Click;
        }
        match drag.hovered_dir {
            Some(dest_dir) if dest_dir != drag.source_path => NotesDropOutcome::Move {
                source: drag.source_path,
                dest_dir,
            },
            _ => NotesDropOutcome::Cancel,
        }
    }

    /// Cancel any in-flight drag without acting (e.g. on Escape or focus
    /// loss). No-op when nothing is being dragged.
    pub fn cancel_notes_drag(&mut self) {
        self.notes_drag = None;
    }

    pub fn notes_drag(&self) -> Option<&NotesDragState> {
        self.notes_drag.as_ref()
    }

    /// True only once a drag has crossed the activation threshold — the
    /// window in which the ghost paints and the cursor shows "grabbing".
    pub fn is_notes_dragging(&self) -> bool {
        self.notes_drag.as_ref().is_some_and(|drag| drag.live)
    }

    /// The row (by absolute index into the visible list) whose full-width
    /// rect contains `(x, y)`, or `None`. The host uses this to feed the
    /// hovered row into [`update_notes_drag`](Self::update_notes_drag).
    pub fn row_at(&self, x: f32, y: f32) -> Option<usize> {
        for (rect, index) in &self.note_rects {
            if rect_contains(*rect, x, y) {
                return Some(*index);
            }
        }
        None
    }

    /// The hovered folder resolved to `(path, is_closed)` when the row
    /// under the cursor is a LEGAL drop target for `source_path`, else
    /// `None`. A legal target is a folder that is not the source, not
    /// inside the source's own subtree, and not the source's current
    /// parent (a no-op move). A note (file) row, or empty space, falls
    /// through to the vault root.
    fn notes_drop_target(
        &self,
        hovered_row: Option<usize>,
        source_path: &Path,
    ) -> Option<(PathBuf, bool)> {
        if let Some(entry) = hovered_row.and_then(|row| self.row_entry(row)) {
            if entry.is_dir {
                let dir = entry.path.clone();
                if dir.as_path() == source_path {
                    return None;
                }
                // Inside the dragged folder's own subtree.
                if dir.starts_with(source_path) {
                    return None;
                }
                // Already living directly in this folder — a no-op.
                if source_path.parent() == Some(dir.as_path()) {
                    return None;
                }
                let closed = !self.open_dirs.contains(&dir);
                return Some((dir, closed));
            }
            // A note (file) row falls through to a vault-root drop below.
        }
        self.notes_root_drop_target(source_path)
    }

    /// The vault root as a drop target, or `None` when there is no vault,
    /// the source already lives directly at the root (a no-op move), or
    /// the source is the root itself. The root is always open — its
    /// children ARE the top-level rows — so it never spring-opens.
    fn notes_root_drop_target(&self, source_path: &Path) -> Option<(PathBuf, bool)> {
        let root = self.workspace_path.clone()?;
        if root.as_path() == source_path || root.starts_with(source_path) {
            return None;
        }
        if source_path.parent() == Some(root.as_path()) {
            return None;
        }
        Some((root, false))
    }

    /// True while a live drag's current drop target is the vault root.
    /// The root owns no row, so the renderer paints a whole-panel wash as
    /// its "release here" affordance instead of a per-row band.
    fn is_notes_root_drop_target(&self) -> bool {
        let Some(drag) = self.notes_drag.as_ref().filter(|drag| drag.live) else {
            return false;
        };
        match (drag.hovered_dir.as_deref(), self.workspace_path.as_deref()) {
            (Some(hovered), Some(root)) => hovered == root,
            _ => false,
        }
    }

    /// Absolute index of the row being dragged, resolved from its path so
    /// it survives row re-indexing. `None` until the drag is live. The
    /// renderer dims this row in place (it's been lifted out).
    fn notes_drag_source_row(&self) -> Option<usize> {
        let source = self
            .notes_drag
            .as_ref()
            .filter(|drag| drag.live)?
            .source_path
            .clone();
        self.rows.iter().position(|row| {
            self.all_entries
                .get(row.entry_index)
                .is_some_and(|entry| entry.path == source)
        })
    }

    /// Absolute index of the folder row the drag is currently hovering,
    /// resolved from its path so it survives the row re-indexing a
    /// spring-open causes. Drives the highlight + wiggle.
    fn notes_drag_hovered_row(&self) -> Option<usize> {
        let dir = self.notes_drag.as_ref()?.hovered_dir.clone()?;
        self.rows.iter().position(|row| {
            self.all_entries
                .get(row.entry_index)
                .is_some_and(|entry| entry.path == dir)
        })
    }
}

/// A note's `icon:` frontmatter emoji, read from the file head — the
/// same page icon the markdown editor renders above the title, mirrored
/// onto the sidebar row (Notion-style). Only the first KB is read; any
/// miss (no frontmatter, no icon, unreadable) is `None`. Explicit
/// `.neoism-icons.json` overrides still win at render time.
fn note_frontmatter_icon(path: &Path) -> Option<String> {
    if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
        return None;
    }
    let head = {
        use std::io::Read;
        let mut file = std::fs::File::open(path).ok()?;
        let mut buffer = [0u8; 1024];
        let read = file.read(&mut buffer).ok()?;
        String::from_utf8_lossy(&buffer[..read]).into_owned()
    };
    let mut lines = head.lines();
    if lines.next().map(str::trim) != Some("---") {
        return None;
    }
    for line in lines.take(32) {
        if line.trim() == "---" {
            return None;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if !key.trim().eq_ignore_ascii_case("icon") {
            continue;
        }
        let value = value.trim().trim_matches('"').trim_matches('\'');
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

fn collect_note_entries(
    root: &Path,
    path: &Path,
    depth: usize,
    out: &mut Vec<NoteSidebarEntry>,
) {
    if should_skip_note_entry(root, path) {
        return;
    }
    if path.is_dir() && path != root {
        let label = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("folder")
            .to_string();
        let parent = path.parent().unwrap_or(root).to_path_buf();
        out.push(NoteSidebarEntry {
            path: path.to_path_buf(),
            label,
            is_dir: true,
            icon: None,
            depth,
            parent,
        });
    }

    if path.is_file() {
        let label = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("file")
            .to_string();
        let parent = path.parent().unwrap_or(root).to_path_buf();
        out.push(NoteSidebarEntry {
            path: path.to_path_buf(),
            label,
            is_dir: false,
            icon: note_frontmatter_icon(path),
            depth,
            parent,
        });
        return;
    }

    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        collect_note_entries(root, &entry.path(), depth + usize::from(path != root), out);
    }
}

/// Read the vault's icon map (`.neoism-icons.json`: relative path → glyph).
/// Missing/invalid files mean no overrides; wasm has no fs so this is a
/// graceful no-op there.
///
/// Empty values and old/current default note glyphs are dropped. Older
/// builds persisted the default glyph instead of removing the key when a
/// note returned to its default icon. Because this map is applied last,
/// that stale root-note entry masked a later frontmatter `icon:` while
/// nested notes (without a stale entry) worked. Defaults are fallbacks,
/// not real overrides, so absence is their canonical representation.
fn load_notes_icons(root: &Path) -> HashMap<String, String> {
    let mut icons: HashMap<String, String> =
        std::fs::read_to_string(root.join(NOTES_ICONS_FILE))
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
    icons.retain(|_, glyph| {
        let glyph = glyph.trim();
        !glyph.is_empty()
            && glyph != NOTE_DEFAULT_ICON
            && glyph != LEGACY_NOTE_DEFAULT_ICON
    });
    icons
}

fn should_skip_note_entry(root: &Path, path: &Path) -> bool {
    if path == root {
        return false;
    }
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    // `project.toml` at the vault ROOT is vault metadata (code-project
    // links), not a note — hidden like dotfiles. A user's own
    // project.toml in a subfolder still shows.
    if name == "project.toml" && path.parent() == Some(root) {
        return true;
    }
    name.starts_with('.') || matches!(name, "target" | "node_modules")
}

fn children_by_parent(entries: &[NoteSidebarEntry]) -> HashMap<PathBuf, Vec<usize>> {
    let mut by_parent: HashMap<PathBuf, Vec<usize>> = HashMap::new();
    for (index, entry) in entries.iter().enumerate() {
        by_parent
            .entry(entry.parent.clone())
            .or_default()
            .push(index);
    }
    by_parent
}

fn push_visible_children(
    entries: &[NoteSidebarEntry],
    by_parent: &HashMap<PathBuf, Vec<usize>>,
    open_dirs: &HashSet<PathBuf>,
    parent: &Path,
    rows: &mut Vec<NoteSidebarRow>,
) {
    let Some(children) = by_parent.get(parent) else {
        return;
    };
    for &entry_index in children {
        let Some(entry) = entries.get(entry_index) else {
            continue;
        };
        rows.push(NoteSidebarRow { entry_index });
        if entry.is_dir && open_dirs.contains(&entry.path) {
            push_visible_children(entries, by_parent, open_dirs, &entry.path, rows);
        }
    }
}

fn draw_frame_top(
    sugarloaf: &mut Sugarloaf,
    rect: [f32; 4],
    outer_color: [f32; 4],
    inner_color: [f32; 4],
    radius: f32,
    stroke: f32,
) {
    let [x, y, w, h] = rect;
    sugarloaf.quad(
        None,
        x,
        y,
        w,
        h,
        outer_color,
        [radius, radius, 0.0, 0.0],
        DEPTH,
        ORDER,
    );
    sugarloaf.quad(
        None,
        x + stroke,
        y + stroke,
        (w - stroke * 2.0).max(0.0),
        (h - stroke * 2.0).max(0.0),
        inner_color,
        [
            (radius - stroke).max(0.0),
            (radius - stroke).max(0.0),
            0.0,
            0.0,
        ],
        DEPTH,
        ORDER + 1,
    );
}

fn rect_contains(rect: [f32; 4], x: f32, y: f32) -> bool {
    x >= rect[0] && y >= rect[1] && x <= rect[0] + rect[2] && y <= rect[1] + rect[3]
}

/// Scale a packed `[r, g, b, a]` color's alpha by `alpha` — used to dim
/// the drag SOURCE row to a placeholder while it rides the cursor.
/// Mirrors `file_tree::render::fade_u8`.
fn fade_u8(mut color: [u8; 4], alpha: f32) -> [u8; 4] {
    color[3] = (color[3] as f32 * alpha) as u8;
    color
}

#[cfg(test)]
mod tests {
    use super::*;

    const VAULT: &str = "/tmp/neoism-notes-test-vault";

    /// Build a sidebar with `n` flat note rows + one expandable folder
    /// ("dir") containing a single child, rooted at a synthetic vault.
    /// Uses `set_entries_from_host` so the test never touches the
    /// filesystem (mirrors the web host path).
    fn sidebar_with_notes(n: usize) -> NotesSidebar {
        let root = PathBuf::from(VAULT);
        let mut sidebar = NotesSidebar::default();
        sidebar.set_workspace("Test", Some(root.clone()));
        sidebar.set_visible(true);
        let mut entries: Vec<(PathBuf, bool)> = (0..n)
            .map(|i| (root.join(format!("note-{i:03}.md")), false))
            .collect();
        entries.push((root.join("folder"), true));
        entries.push((root.join("folder").join("child.md"), false));
        sidebar.set_entries_from_host(entries);
        sidebar
    }

    #[test]
    fn both_load_paths_surface_a_note_frontmatter_icon() {
        // A REAL note on disk with an `icon:` frontmatter. Both the
        // daemon-fed (`set_entries_from_host`, the desktop path) AND the
        // local fs-walk (`refresh_notes`) must put the icon on the row — the
        // "page emoji shows in the doc but the sidebar keeps the default md
        // icon" regression was `set_entries_from_host` hard-coding `None`.
        let root = std::env::temp_dir().join("neoism-icon-test-vault");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let note = root.join("TASKS.md");
        std::fs::write(
            &note,
            "---\nicon: \u{1f525}\ncover: creation\n---\n# TASKS\n",
        )
        .unwrap();

        let mut host = NotesSidebar::default();
        host.set_workspace("Test", Some(root.clone()));
        host.set_visible(true);
        host.set_entries_from_host(vec![(note.clone(), false)]);
        let host_icon = host
            .all_entries
            .iter()
            .find(|e| e.path == note)
            .and_then(|e| e.icon.clone());
        assert_eq!(
            host_icon.as_deref(),
            Some("\u{1f525}"),
            "set_entries_from_host must surface the frontmatter icon"
        );
        // The RENDERER reads the icon via `row_entry(row)` — verify the
        // root note's visible row maps to the icon-bearing entry (rules out a
        // `rebuild_rows`/`row_entry` root-specific mapping bug that the
        // `all_entries` check above would miss).
        let row = host
            .row_index_for_path(&note)
            .expect("the root note must have a visible row");
        let row_icon = host.row_entry(row).and_then(|e| e.icon.clone());
        assert_eq!(
            row_icon.as_deref(),
            Some("\u{1f525}"),
            "the rendered row must map to the icon-bearing entry"
        );

        let mut local = NotesSidebar::default();
        local.set_workspace("Test", Some(root.clone()));
        local.set_visible(true);
        local.refresh_notes();
        let local_icon = local
            .all_entries
            .iter()
            .find(|e| e.path == note)
            .and_then(|e| e.icon.clone());
        assert_eq!(
            local_icon.as_deref(),
            Some("\u{1f525}"),
            "refresh_notes must surface the frontmatter icon"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// REPRO scratch: mimic the picker writing `.neoism-icons.json`, the
    /// way `notes_menus.rs::set_notes_entry_icon` does (key = strip_prefix
    /// of the VAULT), for a note path.
    fn picker_write_icon(vault: &Path, note: &Path, icon: &str) {
        let rel = note
            .strip_prefix(vault)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let icons_path = vault.join(NOTES_ICONS_FILE);
        let mut icons: HashMap<String, String> = std::fs::read_to_string(&icons_path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        icons.insert(rel, icon.to_string());
        std::fs::write(&icons_path, serde_json::to_string_pretty(&icons).unwrap())
            .unwrap();
    }

    #[test]
    fn empty_icon_map_entry_never_clobbers_a_root_note_frontmatter_icon() {
        // Faithful to the user's real vault: `.neoism-icons.json` holds a
        // STALE empty-string value for the root note (`{"TASKS.md": ""}`,
        // left by an older reset that wrote `""` instead of removing the
        // key), while the note itself carries a real `icon:` frontmatter.
        // A NESTED note carries a frontmatter icon and has NO map entry.
        //
        // Before the fix, the map was applied LAST as the highest-priority
        // override and stored `Some("")`, clobbering the root note's real
        // frontmatter icon on every re-list (set -> collapse/expand ->
        // re-fetch reverts it). The nested note, absent from the map, kept
        // its icon — exactly the reported root-vs-folder asymmetry.
        let root = std::env::temp_dir().join("neoism-empty-icon-map-vault");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("folder")).unwrap();
        let root_note = root.join("TASKS.md");
        let nested_note = root.join("folder").join("nested.md");
        std::fs::write(
            &root_note,
            "---\nicon: \u{2b50}\ncover: creation\n---\n# TASKS\n",
        )
        .unwrap();
        std::fs::write(&nested_note, "---\nicon: \u{1f525}\n---\n# nested\n").unwrap();
        // The stale empty-string map entry for the ROOT note only.
        std::fs::write(root.join(NOTES_ICONS_FILE), "{\n  \"TASKS.md\": \"\"\n}")
            .unwrap();

        // Daemon-style entry list, exactly like daemon_sync.rs:
        // notes_root.join(relative_path) for each TreeListing entry.
        let entries = || {
            vec![
                (root.join("TASKS.md"), false),
                (root.join("folder"), true),
                (root.join("folder").join("nested.md"), false),
            ]
        };
        let root_icon = |s: &NotesSidebar| {
            s.all_entries
                .iter()
                .find(|e| e.path == root_note)
                .and_then(|e| e.icon.clone())
        };
        let nested_icon = |s: &NotesSidebar| {
            s.all_entries
                .iter()
                .find(|e| e.path == nested_note)
                .and_then(|e| e.icon.clone())
        };

        // Daemon path (set_entries_from_host) — the desktop re-list.
        let mut s = NotesSidebar::default();
        s.set_workspace("Test", Some(root.clone()));
        s.set_visible(true);
        s.set_entries_from_host(entries());
        s.reveal_dir(&root.join("folder"));
        assert_eq!(
            root_icon(&s).as_deref(),
            Some("\u{2b50}"),
            "root note frontmatter icon must survive an empty map entry (set_entries_from_host)"
        );
        assert_eq!(
            nested_icon(&s).as_deref(),
            Some("\u{1f525}"),
            "nested icon (host)"
        );

        // Collapse + expand + re-fetch (the toggle-panel cycle).
        s.open_dirs.remove(&root.join("folder"));
        s.rebuild_rows();
        s.reveal_dir(&root.join("folder"));
        s.set_entries_from_host(entries());
        assert_eq!(
            root_icon(&s).as_deref(),
            Some("\u{2b50}"),
            "root note icon reverted after collapse/expand re-fetch"
        );

        // Local fs-walk path (refresh_notes).
        s.refresh_notes();
        assert_eq!(
            root_icon(&s).as_deref(),
            Some("\u{2b50}"),
            "root note frontmatter icon must survive an empty map entry (refresh_notes)"
        );
        assert_eq!(
            nested_icon(&s).as_deref(),
            Some("\u{1f525}"),
            "nested icon (local)"
        );

        // A NON-empty map entry still wins over the frontmatter (picker set).
        picker_write_icon(&root, &root_note, "\u{f135}");
        s.refresh_notes();
        assert_eq!(
            root_icon(&s).as_deref(),
            Some("\u{f135}"),
            "a real picker icon must still override the frontmatter"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn legacy_default_map_icon_never_clobbers_a_root_note_frontmatter_icon() {
        let root = std::env::temp_dir().join("neoism-legacy-root-icon-vault");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let note = root.join("TASKS.md");
        std::fs::write(&note, "---\nicon: \u{1f4a1}\n---\n# TASKS\n").unwrap();
        std::fs::write(
            root.join(NOTES_ICONS_FILE),
            format!("{{\"TASKS.md\":\"{LEGACY_NOTE_DEFAULT_ICON}\"}}"),
        )
        .unwrap();

        let mut sidebar = NotesSidebar::default();
        sidebar.set_workspace("Test", Some(root.clone()));
        let icon = sidebar
            .all_entries
            .iter()
            .find(|entry| entry.path == note)
            .and_then(|entry| entry.icon.as_deref());
        assert_eq!(icon, Some("\u{1f4a1}"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn touchpad_scroll_accumulates_away_from_top_edge() {
        // Four quarter-row nudges sum to one row, so eight push two rows
        // — same accumulator behaviour as the file tree.
        let mut s = sidebar_with_notes(40);
        let row_h = s.row_height();
        for _ in 0..8 {
            s.scroll_pixels(-row_h / 4.0, 5);
        }
        assert_eq!(s.scroll_top, 2);
    }

    #[test]
    fn touchpad_overscroll_is_discarded_at_edges() {
        let mut s = sidebar_with_notes(40);
        let row_h = s.row_height();
        s.scroll_pixels(row_h / 2.0, 5);
        s.scroll_pixels(-row_h / 2.0, 5);
        assert_eq!(s.scroll_top, 0);
    }

    #[test]
    fn scroll_by_respects_panel_height_bottom() {
        let mut s = sidebar_with_notes(40);
        s.scroll_by(1000, 5);
        assert_eq!(s.scroll_top, s.max_scroll_top_for(5));
    }

    #[test]
    fn half_page_jump_moves_selection() {
        let mut s = sidebar_with_notes(40);
        s.set_selected(0);
        s.select_next_by(5);
        assert_eq!(s.selected_index, 5);
        s.select_prev_by(2);
        assert_eq!(s.selected_index, 3);
    }

    #[test]
    fn expansion_persists_across_close_open() {
        let mut s = sidebar_with_notes(4);
        let folder = PathBuf::from(VAULT).join("folder");
        // Open the folder, then close + reopen the panel with the SAME
        // vault. The expanded set must survive (regression: it used to
        // reset to all-closed).
        s.open_dirs.insert(folder.clone());
        s.rebuild_rows();
        assert!(s.open_dirs.contains(&folder));
        s.set_visible(false);
        s.set_visible(true);
        s.set_workspace("Test", Some(PathBuf::from(VAULT)));
        assert!(
            s.open_dirs.contains(&folder),
            "reopening the same vault collapsed an expanded folder"
        );
    }

    #[test]
    fn switching_vault_clears_expansion() {
        let mut s = sidebar_with_notes(4);
        let folder = PathBuf::from(VAULT).join("folder");
        s.open_dirs.insert(folder.clone());
        // A different vault path is a fresh tree — expansion should reset.
        s.set_workspace("Other", Some(PathBuf::from("/tmp/other-vault")));
        assert!(!s.open_dirs.contains(&folder));
    }

    #[test]
    fn mark_dirty_only_while_visible() {
        let mut s = sidebar_with_notes(4);
        assert!(!s.take_refresh());
        s.set_visible(false);
        s.mark_dirty();
        assert!(
            !s.take_refresh(),
            "hidden panel should not request a refresh"
        );
        s.set_visible(true);
        s.mark_dirty();
        assert!(s.take_refresh());
        assert!(!s.take_refresh(), "flag is one-shot");
    }

    /// Vault: `note.md`, `docs/` (closed), `assets/` (open) > `img/`
    /// (closed). Visible rows after rebuild: `assets`(0), `img`(1),
    /// `docs`(2), `note.md`(3) — dirs sort before files, and `assets` is
    /// expanded so its child folder is a real row. Mirrors the file
    /// tree's `sample()` used for its drag tests.
    fn drag_sidebar() -> NotesSidebar {
        let root = PathBuf::from(VAULT);
        let mut s = NotesSidebar::default();
        s.set_workspace("Test", Some(root.clone()));
        s.set_visible(true);
        s.set_entries_from_host(vec![
            (root.join("note.md"), false),
            (root.join("docs"), true),
            (root.join("assets"), true),
            (root.join("assets").join("img"), true),
        ]);
        s.reveal_dir(&root.join("assets"));
        s
    }

    #[test]
    fn press_becomes_live_only_past_threshold() {
        let mut s = drag_sidebar();
        s.begin_notes_drag(3, 100.0, 100.0); // note.md
        assert!(!s.is_notes_dragging());
        // A tiny nudge stays a click.
        s.update_notes_drag(101.0, 101.0, Some(2));
        assert!(!s.is_notes_dragging());
        assert!(matches!(s.end_notes_drag(), NotesDropOutcome::Click));
        // Past the threshold it goes live.
        s.begin_notes_drag(3, 100.0, 100.0);
        s.update_notes_drag(100.0, 120.0, Some(2));
        assert!(s.is_notes_dragging());
    }

    #[test]
    fn dropping_a_note_into_a_folder_moves_it() {
        let root = PathBuf::from(VAULT);
        let mut s = drag_sidebar();
        s.begin_notes_drag(3, 100.0, 100.0); // note.md
        s.update_notes_drag(100.0, 130.0, Some(2)); // over docs
        match s.end_notes_drag() {
            NotesDropOutcome::Move { source, dest_dir } => {
                assert_eq!(source, root.join("note.md"));
                assert_eq!(dest_dir, root.join("docs"));
            }
            _ => panic!("expected a move"),
        }
    }

    #[test]
    fn dropping_a_folder_into_a_folder_moves_the_subtree() {
        let root = PathBuf::from(VAULT);
        let mut s = drag_sidebar();
        s.begin_notes_drag(2, 100.0, 100.0); // docs
        s.update_notes_drag(100.0, 130.0, Some(0)); // over assets
        match s.end_notes_drag() {
            NotesDropOutcome::Move { source, dest_dir } => {
                assert_eq!(source, root.join("docs"));
                assert_eq!(dest_dir, root.join("assets"));
            }
            _ => panic!("expected a folder move"),
        }
    }

    #[test]
    fn dropping_into_the_current_parent_is_a_noop() {
        // note.md already lives at the vault root — a file/empty-space
        // drop resolves to root, which is its current parent.
        let mut s = drag_sidebar();
        s.begin_notes_drag(3, 100.0, 100.0); // note.md
        s.update_notes_drag(100.0, 130.0, None); // empty space => root
        assert!(matches!(s.end_notes_drag(), NotesDropOutcome::Cancel));
    }

    #[test]
    fn cannot_drop_a_folder_into_its_own_subtree() {
        let mut s = drag_sidebar();
        s.begin_notes_drag(0, 100.0, 100.0); // assets
        s.update_notes_drag(100.0, 130.0, Some(1)); // over assets/img
        assert!(matches!(s.end_notes_drag(), NotesDropOutcome::Cancel));
    }

    #[test]
    fn dropping_a_nested_note_at_root_moves_to_root() {
        // A note living inside a folder can be dragged out to the vault
        // root (empty-space / file-row drop).
        let root = PathBuf::from(VAULT);
        let mut s = NotesSidebar::default();
        s.set_workspace("Test", Some(root.clone()));
        s.set_visible(true);
        s.set_entries_from_host(vec![
            (root.join("docs"), true),
            (root.join("docs").join("nested.md"), false),
        ]);
        s.reveal_dir(&root.join("docs"));
        // rows: docs(0), docs/nested.md(1).
        s.begin_notes_drag(1, 100.0, 100.0); // docs/nested.md
        s.update_notes_drag(100.0, 400.0, None); // empty space => root
        match s.end_notes_drag() {
            NotesDropOutcome::Move { source, dest_dir } => {
                assert_eq!(source, root.join("docs").join("nested.md"));
                assert_eq!(dest_dir, root);
            }
            _ => panic!("expected a move to the vault root"),
        }
    }
}
