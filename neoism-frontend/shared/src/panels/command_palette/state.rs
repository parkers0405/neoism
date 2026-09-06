// Copyright (c) 2023-present, Raphael Amorim.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! `CommandPalette` struct, ctor, and state-mutating methods.
//!
//! Selection movement / hit-testing lives in [`super::update`]; drawing
//! lives in [`super::render`]. This file owns the fields and the
//! "enter mode" / "set query" entry points.

use web_time::Instant;
use unicode_segmentation::UnicodeSegmentation;

use crate::animation::CriticallyDampedSpring;

use super::actions::{
    PaletteBufferEntry, PaletteDirectoryEntry, PaletteHostCapabilities, PaletteHostEntry,
    PaletteServerEntry, PaletteShaderEntry, PaletteSurface, PaletteWorkspaceEntry,
    WorkspaceDirectoryTarget,
};
use super::modes::PaletteMode;
use super::MAX_RECENT_SEARCHES;
use super::RESULT_ITEM_HEIGHT;

/// Live state for an in-progress drag of a workspace row onto a host
/// header in the Workspaces modal (5D-drag).
///
/// Armed on mouse-press over a selectable `Workspace` row; promoted to
/// `active` once the cursor moves past `WORKSPACE_DRAG_ACTIVATION_PX`
/// (so a plain click never reads as a drag). While active, `drop_host_id`
/// tracks the host header currently under the cursor — the drop target —
/// for both the visual highlight and the release decision.
///
/// Mirrors the buffer-tabs `DragState` press/active pattern rather than
/// inventing a new one.
#[derive(Clone, Debug)]
pub(super) struct WorkspaceDrag {
    /// Id of the workspace being dragged (the pressed row's id).
    pub(super) workspace_id: String,
    /// The host the dragged workspace currently lives under. A drop back
    /// onto this same host is a no-op (you didn't move it anywhere).
    pub(super) source_host_id: String,
    /// Cursor position at press time, in logical (scale-divided) coords.
    /// The activation threshold is measured from here.
    pub(super) press_x: f32,
    pub(super) press_y: f32,
    /// `true` once the cursor has moved past the activation threshold —
    /// only then does the gesture suppress the click + paint a ghost.
    pub(super) active: bool,
    /// Host id of the header row currently under the cursor (the live
    /// drop target), or `None` when the cursor isn't over a host header.
    /// Drives the drop-target highlight in render.
    pub(super) drop_host_id: Option<String>,
}

/// Command palette UI component (Raycast-style)
pub struct CommandPalette {
    pub(super) enabled: bool,
    pub query: String,
    pub selected_index: usize,
    pub(super) hovered_index: Option<usize>,
    pub(super) server_edit_hit: Option<([f32; 4], String)>,
    pub(super) server_remove_hit: Option<([f32; 4], String)>,
    pub(super) scroll_offset: usize,
    pub has_adaptive_theme: bool,
    /// Which list the palette is showing (commands or fonts).
    pub(super) mode: PaletteMode,
    /// Timestamp for caret blinking
    pub(super) caret_blink_start: Instant,
    /// Timestamp of the last event that actually changed `scroll_offset`.
    /// Drives the scrollbar fade-in/fade-out, sharing the terminal
    /// scrollbar's 2 s delay + 300 ms fade envelope via
    /// `scrollbar::opacity_from_last_scroll`. `None` while the palette
    /// has never scrolled since it opened — scrollbar stays hidden.
    pub(super) last_scroll_time: Option<Instant>,
    /// Recently dispatched `/` search queries, most-recent first. Surfaced
    /// as suggestion rows in Search mode so the user can re-run a prior
    /// search without retyping. Capped to keep the list scannable.
    pub(super) recent_searches: Vec<String>,
    /// Live buffer matches for the current `/` query. Refreshed by the
    /// screen layer each frame from `editor.drain_search_matches()`.
    /// Shown instead of `recent_searches` in Search mode whenever the
    /// query is non-empty so the dropdown reads as a live picker
    /// rather than a history list.
    pub(super) buffer_matches: Vec<(u64, u64, String)>,
    /// Host-owned directory snapshot for the current Commands-mode `cd`
    /// query. The shared palette never touches the filesystem itself.
    pub(super) cd_directory_results: Vec<PaletteDirectoryEntry>,
    workspace_directory_recents: Vec<String>,
    workspace_directory_target: Option<WorkspaceDirectoryTarget>,
    pub(super) cd_error: Option<String>,
    /// UTF-8 byte insertion point in `query`.
    pub(super) query_cursor: usize,
    /// Measured input band height from the latest render. Geometry/hit tests
    /// use this instead of assuming one line.
    pub(super) input_band_height: f32,
    pub(super) viewport_height: f32,
    pub(super) cd_completion_selected: bool,
    /// Direction of the current Search-mode session: `false` for `/`
    /// (forward), `true` for `?` (backward). The commit dispatcher reads
    /// this to pick the search direction (and `v:searchforward` so
    /// `n`/`N` keep the right orientation afterwards). Survives palette
    /// close so the commit that just closed the palette still sees it.
    pub(super) search_backward: bool,
    /// Chrome zoom multiplier — driven by Ctrl+/Ctrl- via
    /// `Renderer::set_chrome_scale`. Applied uniformly to every length
    /// (width, padding, fonts, row heights) so the palette grows /
    /// shrinks in lockstep with the rest of the workspace chrome.
    /// Default 1.0 keeps existing screenshots unchanged.
    pub(super) scale: f32,
    pub(super) list_scroll_spring: CriticallyDampedSpring,
    pub(super) cursor_spring: CriticallyDampedSpring,
    pub(super) last_list_scroll_frame: Instant,
    pub(super) last_cursor_frame: Instant,
    pub(super) selected_cursor_rect: Option<[f32; 4]>,
    pub(super) wheel_accumulator: f32,
    /// Persistent sub-row offset for direct touch; unlike the wheel spring it
    /// does not decay after lift.
    pub(super) touch_scroll_offset: f32,
    pub(super) open_pop_started: Instant,
    pub(super) pop_on_open: bool,
    pub(super) top_anchor: f32,
    pub(super) surface: PaletteSurface,
    /// What the embedding host can execute — the second visibility
    /// axis next to `surface`. Defaults to everything (desktop); the
    /// web host narrows it via [`Self::set_host_capabilities`] so
    /// commands that cannot run there are not listed.
    pub(super) host_capabilities: PaletteHostCapabilities,
    /// In-progress workspace→host drag (5D-drag), or `None` when no drag
    /// is armed. Only ever populated in `Workspaces` mode.
    pub(super) workspace_drag: Option<WorkspaceDrag>,
    /// Wave 6A: workspace-less hosts (discovered tailnet peers) shown as
    /// header-only drop targets at the bottom of the Workspaces tree.
    /// Only read while `mode` is `Workspaces`; replaced on every
    /// `enter_workspaces_mode*` call.
    pub(super) workspace_peer_hosts: Vec<PaletteHostEntry>,
    /// In-flight / just-finished workspace move (promote/demote) so the
    /// Workspaces modal can show "moving…" on the target host header
    /// while the daemon copies things over, then the outcome. Survives
    /// palette close so reopening still shows the result.
    pub(super) workspace_move: Option<WorkspaceMoveStatus>,
}

/// Feedback state for a dispatched workspace move (drag-drop promote /
/// demote). Rendered on the matching host header in the Workspaces modal.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceMoveStatus {
    pub workspace_id: String,
    pub target_host_id: String,
    pub phase: WorkspaceMovePhase,
    pub since: Instant,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WorkspaceMovePhase {
    InFlight,
    Done { ok: bool, message: String },
}

/// How long a finished move's ✓/✗ result stays on the host header.
pub(super) const WORKSPACE_MOVE_RESULT_TTL: web_time::Duration =
    web_time::Duration::from_secs(4);

impl Default for CommandPalette {
    fn default() -> Self {
        Self {
            enabled: false,
            query: String::new(),
            selected_index: 0,
            hovered_index: None,
            server_edit_hit: None,
            server_remove_hit: None,
            scroll_offset: 0,
            has_adaptive_theme: false,
            mode: PaletteMode::Commands,
            caret_blink_start: Instant::now(),
            last_scroll_time: None,
            recent_searches: Vec::new(),
            buffer_matches: Vec::new(),
            cd_directory_results: Vec::new(),
            workspace_directory_recents: Vec::new(),
            workspace_directory_target: None,
            cd_error: None,
            query_cursor: 0,
            input_band_height: super::INPUT_HEIGHT,
            viewport_height: 900.0,
            cd_completion_selected: false,
            search_backward: false,
            scale: 1.0,
            list_scroll_spring: CriticallyDampedSpring::new(),
            cursor_spring: CriticallyDampedSpring::new(),
            last_list_scroll_frame: Instant::now(),
            last_cursor_frame: Instant::now(),
            selected_cursor_rect: None,
            wheel_accumulator: 0.0,
            touch_scroll_offset: 0.0,
            open_pop_started: Instant::now(),
            pop_on_open: false,
            top_anchor: super::PALETTE_MARGIN_TOP,
            surface: PaletteSurface::Terminal,
            host_capabilities: PaletteHostCapabilities::all(),
            workspace_drag: None,
            workspace_peer_hosts: Vec::new(),
            workspace_move: None,
        }
    }
}

impl CommandPalette {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn selected_cursor_rect(&self) -> Option<[f32; 4]> {
        self.selected_cursor_rect
    }

    /// Push the current chrome scale (Ctrl+/Ctrl-) into the palette.
    /// Clamped on the renderer side already, but re-clamped here so a
    /// stray test or future caller can't spike the multiplier.
    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale.clamp(0.5, 3.0);
        self.input_band_height = super::INPUT_HEIGHT * self.scale;
        self.reset_motion();
    }

    pub fn set_top_anchor(&mut self, top: f32) {
        self.top_anchor = top.max(0.0);
    }

    pub fn set_surface(&mut self, surface: PaletteSurface) {
        self.surface = surface;
    }

    /// Declare what the embedding host can execute. Commands whose
    /// capability flag is off are dropped from the Commands listing
    /// (see `command_visible_for_host`). Desktop never calls this and
    /// keeps the full catalog; the web bridge re-asserts its set every
    /// frame alongside `set_surface`.
    pub fn set_host_capabilities(&mut self, caps: PaletteHostCapabilities) {
        self.host_capabilities = caps;
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if enabled {
            self.query.clear();
            self.cd_directory_results.clear();
            self.workspace_directory_target = None;
            self.cd_error = None;
            self.query_cursor = 0;
            self.input_band_height = super::INPUT_HEIGHT * self.scale;
            self.selected_index = 0;
            self.scroll_offset = 0;
            self.caret_blink_start = Instant::now();
            // Clear scrollbar history so reopening the palette never
            // flashes a leftover scrollbar from the previous session.
            self.last_scroll_time = None;
            // Always re-open into Commands mode — a stale Fonts list
            // from a previous session would be misleading (fonts may
            // have changed) and surprising (user toggles palette and
            // finds themselves on the font list).
            self.mode = PaletteMode::Commands;
            self.reset_motion();
            self.start_open_pop();
        } else {
            self.selected_cursor_rect = None;
            self.pop_on_open = false;
            // Closing the palette drops any in-flight drag.
            self.workspace_drag = None;
        }
    }

    /// Open Commands mode with an exact initial query. Unlike `set_enabled`,
    /// this does not briefly expose an empty command list to the host.
    pub fn open_commands_with_query(&mut self, query: impl Into<String>) {
        self.set_enabled(true);
        self.set_query(query.into());
    }

    pub fn open_commands_for_workspace(
        &mut self,
        query: impl Into<String>,
        target: WorkspaceDirectoryTarget,
    ) {
        self.open_commands_with_query(query);
        self.workspace_directory_target = Some(target);
    }

    pub fn capture_workspace_directory_target(&mut self, target: WorkspaceDirectoryTarget) {
        if self.workspace_directory_target.is_none() {
            self.workspace_directory_target = Some(target);
        }
    }

    pub fn change_workspace_directory_intent(
        &self,
        destination: impl Into<String>,
    ) -> Option<super::actions::ChangeWorkspaceDirectoryIntent> {
        Some(super::actions::ChangeWorkspaceDirectoryIntent {
            target: self.workspace_directory_target.clone()?,
            destination: destination.into(),
        })
    }

    pub fn typed_change_workspace_directory_intent(
        &mut self,
    ) -> Option<super::actions::ChangeWorkspaceDirectoryIntent> {
        let suffix = self.cd_query_suffix()?.to_owned();
        match super::actions::parse_cd_operand(&suffix) {
            Ok(destination) => {
                self.cd_error = None;
                self.change_workspace_directory_intent(destination)
            }
            Err(error) => { self.cd_error = Some(error); None }
        }
    }

    pub fn cd_error(&self) -> Option<&str> { self.cd_error.as_deref() }
    pub fn set_cd_error(&mut self, error: impl Into<String>) {
        self.cd_error = Some(error.into());
    }

    /// Keep captured workspace mode alive after a successful dispatch and use
    /// the optimistic canonical destination as the base for the next command.
    pub fn continue_workspace_directory(&mut self, root: impl Into<String>) {
        let root = root.into();
        if let Some(target) = self.workspace_directory_target.as_mut() {
            target.root = root.clone();
        }
        self.record_workspace_directory(root);
        self.mode = PaletteMode::Commands;
        self.cd_error = None;
        self.set_query("cd ".into());
    }

    /// Keep the captured target while the daemon resolves and broadcasts a
    /// relative, home, or previous-directory transition.
    pub fn continue_workspace_directory_pending(&mut self) {
        self.mode = PaletteMode::Commands;
        self.cd_error = None;
        self.set_query("cd ".into());
    }

    pub fn record_workspace_directory(&mut self, root: impl Into<String>) {
        let root = root.into();
        if root.is_empty() { return; }
        self.workspace_directory_recents.retain(|entry| entry != &root);
        self.workspace_directory_recents.insert(0, root);
        self.workspace_directory_recents.truncate(8);
    }

    pub fn previous_workspace_directory(&self, current: &str) -> Option<&str> {
        self.workspace_directory_recents
            .iter()
            .find(|path| path.as_str() != current)
            .map(String::as_str)
    }

    pub fn compose_workspace_directory_choices(
        &mut self,
        _home: Option<String>,
        _workspace_root: Option<String>,
        completions: Vec<PaletteDirectoryEntry>,
    ) {
        if self.workspace_directory_target.is_none() { return; }
        let mut rows: Vec<PaletteDirectoryEntry> = Vec::new();
        for completion in completions {
            if rows.iter().any(|row| row.absolute_path == completion.absolute_path) { continue; }
            rows.push(completion);
        }
        self.set_cd_directory_results(rows);
    }

    pub fn workspace_directory_target(&self) -> Option<&WorkspaceDirectoryTarget> {
        self.workspace_directory_target.as_ref()
    }

    pub(super) fn start_open_pop(&mut self) {
        self.open_pop_started = Instant::now();
        self.pop_on_open = true;
    }

    /// Open the palette in raw ex-command mode. The shared command list
    /// owns normal `:`/Cmd+Shift+; now; this is for callers that
    /// intentionally want the typed command dispatched verbatim — e.g.
    /// the `Go to Line…` command, whose typed `:N` payload jumps there.
    pub fn enter_ex_mode(&mut self) {
        self.enabled = true;
        self.mode = PaletteMode::Ex;
        self.query.clear();
        self.selected_index = 0;
        self.hovered_index = None;
        self.scroll_offset = 0;
        self.caret_blink_start = Instant::now();
        self.last_scroll_time = None;
        self.reset_motion();
        self.start_open_pop();
    }

    /// `true` while the palette is capturing an ex command. The router
    /// reads this on Enter to decide between executing a `PaletteAction`
    /// and dispatching the query as a typed `:<query>`.
    pub fn is_ex_mode(&self) -> bool {
        matches!(self.mode, PaletteMode::Ex)
    }

    /// Open the palette in `/` search mode. Same shape as `enter_ex_mode`
    /// — empty query, fresh selection, palette opens. The router
    /// distinguishes the two via `is_search_mode`.
    pub fn enter_search_mode(&mut self) {
        self.enter_search_mode_with_direction(false);
    }

    /// Open the palette in `?` backward search mode. Identical to
    /// [`Self::enter_search_mode`] except the commit direction flips —
    /// the dispatcher jumps to the previous match and points `n`/`N`
    /// backward, mirroring vim's native `?`.
    pub fn enter_search_mode_backward(&mut self) {
        self.enter_search_mode_with_direction(true);
    }

    fn enter_search_mode_with_direction(&mut self, backward: bool) {
        self.enabled = true;
        self.mode = PaletteMode::Search;
        self.search_backward = backward;
        self.query.clear();
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.caret_blink_start = Instant::now();
        self.last_scroll_time = None;
        self.reset_motion();
        self.start_open_pop();
    }

    /// `true` while the palette is capturing a `/` search query.
    pub fn is_search_mode(&self) -> bool {
        matches!(self.mode, PaletteMode::Search)
    }

    /// `true` when the current (or just-closed) Search-mode session was
    /// opened with `?` — the commit should search backward.
    pub fn search_is_backward(&self) -> bool {
        self.search_backward
    }

    /// Replace the live `/`-search match list with a new snapshot.
    /// Called every frame the screen drained a `rio_search_matches`
    /// notification. Re-clamps `selected_index` so a list that just
    /// shrank doesn't leave the cursor pointing past the new end.
    pub fn set_buffer_matches(&mut self, matches: Vec<(u64, u64, String)>) {
        self.buffer_matches = matches;
        if self.selected_index >= self.buffer_matches.len() {
            self.selected_index = self.buffer_matches.len().saturating_sub(1);
            self.cursor_spring.reset();
        }
        // Match lists can be hundreds of items — re-clamp scroll too
        // so we don't paint a phantom row beyond the end.
        if self.scroll_offset > self.selected_index {
            self.scroll_offset = self.selected_index;
            self.list_scroll_spring.reset();
        }
    }

    /// Record `query` as the most-recent `/` search. Dedupes (so re-running
    /// the same search bubbles it back to the top instead of stacking) and
    /// caps the list at `MAX_RECENT_SEARCHES`.
    pub fn push_recent_search(&mut self, query: String) {
        if query.is_empty() {
            return;
        }
        self.recent_searches.retain(|q| q != &query);
        self.recent_searches.insert(0, query);
        self.recent_searches.truncate(MAX_RECENT_SEARCHES);
    }

    /// Swap the palette into font-browsing mode with the given family
    /// list. Clears the query so the full list is visible, keeps the
    /// palette open. Called by the router after the user picks the
    /// `List Fonts` command.
    pub fn enter_fonts_mode(&mut self, fonts: Vec<String>) {
        // Mirror `enter_themes_mode` / `enter_shaders_mode`: force the
        // palette open. The common trigger already has it open, but
        // dispatch paths that fire the action from a closed palette
        // (the Alt+K command sheet / a context menu going through
        // `execute_palette_action`) rely on this to actually show the
        // font list instead of silently entering a hidden mode.
        self.enabled = true;
        self.mode = PaletteMode::Fonts(fonts);
        self.query.clear();
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.caret_blink_start = Instant::now();
        self.last_scroll_time = None;
        self.reset_motion();
        self.start_open_pop();
    }

    pub fn enter_themes_mode(&mut self, themes: Vec<String>) {
        self.enabled = true;
        self.mode = PaletteMode::Themes(themes);
        self.query.clear();
        // The picker can open from a typed command or a previous session.
        // Its byte cursor must belong to the cleared query, not that old text.
        self.query_cursor = 0;
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.caret_blink_start = Instant::now();
        self.last_scroll_time = None;
        self.reset_motion();
        self.start_open_pop();
    }

    pub fn enter_mashups_mode(
        &mut self,
        mut packs: Vec<super::actions::PaletteMashupEntry>,
    ) {
        packs.insert(
            0,
            super::actions::PaletteMashupEntry {
                id: None,
                name: "None".to_string(),
                detail: "Deactivate pack; keep the current theme and clear its shader"
                    .to_string(),
                theme: None,
                shader_overlay: None,
                font_family: None,
            },
        );
        self.enabled = true;
        self.mode = PaletteMode::Mashups(packs);
        self.query.clear();
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.caret_blink_start = Instant::now();
        self.last_scroll_time = None;
        self.reset_motion();
        self.start_open_pop();
    }

    pub fn enter_shaders_mode(&mut self, shaders: Vec<PaletteShaderEntry>) {
        self.enabled = true;
        self.mode = PaletteMode::Shaders(shaders);
        self.query.clear();
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.caret_blink_start = Instant::now();
        self.last_scroll_time = None;
        self.reset_motion();
        self.start_open_pop();
    }

    pub fn enter_buffers_mode(&mut self, buffers: Vec<PaletteBufferEntry>) {
        self.enabled = true;
        self.mode = PaletteMode::Buffers(buffers);
        self.query.clear();
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.caret_blink_start = Instant::now();
        self.last_scroll_time = None;
        self.reset_motion();
        self.start_open_pop();
    }

    /// Open the palette as the grouped host→workspace tree.
    ///
    /// The first row of the tree is a (non-selectable) host header, so
    /// the initial selection is snapped forward to the first selectable
    /// workspace row — otherwise Enter would land on a separator.
    ///
    /// 5D-data seam: `workspaces` is still a flat `Vec` here. Callers
    /// build it from whatever host info they have (today: a single
    /// implicit Local host via [`PaletteWorkspaceEntry::local`]); when
    /// the real `HostWorkspaceTree` / `/tailnet-peers` payload is wired
    /// in, they populate the per-entry host fields and this method needs
    /// no change — the grouping happens in `grouped_workspace_rows`.
    pub fn enter_workspaces_mode(&mut self, workspaces: Vec<PaletteWorkspaceEntry>) {
        self.enter_workspaces_mode_with_hosts(workspaces, Vec::new());
    }

    /// Open the minimal server picker. The final Add row remains visible
    /// while filtering so a missing server can always be added directly.
    pub fn enter_servers_mode(&mut self, servers: Vec<PaletteServerEntry>) {
        self.enabled = true;
        self.mode = PaletteMode::Servers(servers);
        self.query.clear();
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.caret_blink_start = Instant::now();
        self.last_scroll_time = None;
        self.reset_motion();
        self.start_open_pop();
    }

    pub fn update_server_status(
        &mut self,
        server_id: &str,
        status: crate::panels::ServerIndicatorStatus,
    ) -> bool {
        let PaletteMode::Servers(servers) = &mut self.mode else {
            return false;
        };
        let Some(server) = servers.iter_mut().find(|server| server.id == server_id)
        else {
            return false;
        };
        if server.status == status {
            return false;
        }
        server.status = status;
        true
    }

    /// [`Self::enter_workspaces_mode`], plus workspace-less hosts (Wave
    /// 6A: discovered tailnet peers) appended to the tree as header-only
    /// drop targets. Dragging a workspace onto one of those headers
    /// emits `MoveWorkspaceToHost` with the peer's `daemon_url`, exactly
    /// like a drop on a populated remote host.
    pub fn enter_workspaces_mode_with_hosts(
        &mut self,
        workspaces: Vec<PaletteWorkspaceEntry>,
        peer_hosts: Vec<PaletteHostEntry>,
    ) {
        self.enabled = true;
        self.mode = PaletteMode::Workspaces(workspaces);
        self.workspace_peer_hosts = peer_hosts;
        self.query.clear();
        self.scroll_offset = 0;
        self.caret_blink_start = Instant::now();
        self.last_scroll_time = None;
        self.reset_motion();
        self.start_open_pop();
        // Snap off the leading host header onto the first real workspace.
        self.selected_index = self.first_selectable_index(0).unwrap_or(0);
    }

    /// True while the palette is open in the grouped Workspaces tree.
    /// Web hosts poll this to live-refresh the tree as daemon
    /// `HostWorkspaceTree` pushes arrive while the modal is up.
    pub fn workspaces_mode_open(&self) -> bool {
        self.enabled && matches!(self.mode, PaletteMode::Workspaces(_))
    }

    /// Swap in a fresh host→workspace tree WITHOUT resetting the query,
    /// selection, or open-pop animation — used when a daemon
    /// `HostWorkspaceTree` push lands while the modal is already open.
    /// No-op outside Workspaces mode so a stale push can't hijack the
    /// palette into a different mode.
    pub fn refresh_workspaces_tree(
        &mut self,
        workspaces: Vec<PaletteWorkspaceEntry>,
        peer_hosts: Vec<PaletteHostEntry>,
    ) {
        if !matches!(self.mode, PaletteMode::Workspaces(_)) {
            return;
        }
        self.mode = PaletteMode::Workspaces(workspaces);
        self.workspace_peer_hosts = peer_hosts;
        // The tree may have shrunk — re-snap the selection onto a
        // selectable row (row 0 is a host header, never selectable).
        self.selected_index = self
            .first_selectable_index(self.selected_index)
            .or_else(|| self.first_selectable_index(0))
            .unwrap_or(0);
    }

    pub fn set_query(&mut self, query: String) {
        let old_cd_suffix = self.cd_query_suffix().map(str::to_owned);
        self.query = query;
        self.query_cursor = self.query.len();
        self.cd_error = None;
        self.cd_completion_selected = false;
        if self.cd_query_suffix() != old_cd_suffix.as_deref() {
            // Never show results belonging to a previous suffix while the
            // host obtains the next filesystem snapshot.
            self.cd_directory_results.clear();
        }
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.caret_blink_start = Instant::now();
        if matches!(self.mode, PaletteMode::Search) {
            self.buffer_matches.clear();
        }
        // In the grouped Workspaces tree row 0 is a host header, so a
        // re-filter must re-snap the selection onto the first selectable
        // workspace row rather than parking on a separator.
        if matches!(self.mode, PaletteMode::Workspaces(_)) {
            self.selected_index = self.first_selectable_index(0).unwrap_or(0);
        }
        // Typing reshapes the list entirely — drop any scrollbar
        // fade state so the next scroll starts with a clean timer.
        self.last_scroll_time = None;
        self.reset_motion();
    }


    pub fn query_cursor(&self) -> usize { self.query_cursor }

    pub fn set_query_cursor(&mut self, byte: usize) {
        let requested = byte.min(self.query.len());
        let byte = if requested == self.query.len() {
            requested
        } else {
            self.query.grapheme_indices(true)
                .map(|(index, _)| index)
                .take_while(|index| *index <= requested)
                .last()
                .unwrap_or(0)
        };
        self.query_cursor = byte;
        self.caret_blink_start = Instant::now();
    }

    pub fn move_query_cursor_left(&mut self) {
        let prev = self.query[..self.query_cursor].grapheme_indices(true).next_back().map(|(i, _)| i).unwrap_or(0);
        self.set_query_cursor(prev);
    }

    pub fn move_query_cursor_right(&mut self) {
        let next = self.query[self.query_cursor..].grapheme_indices(true).nth(1).map(|(i, _)| self.query_cursor + i).unwrap_or(self.query.len());
        self.set_query_cursor(next);
    }

    pub fn insert_query_text(&mut self, text: &str) {
        if text.is_empty() || text.chars().any(char::is_control) { return; }
        let at = self.query_cursor;
        let mut next = self.query.clone();
        next.insert_str(at, text);
        self.set_query(next);
        self.query_cursor = at + text.len();
    }

    pub fn backspace_query(&mut self) -> bool {
        if self.query_cursor == 0 { return false; }
        let at = self.query_cursor;
        let prev = self.query[..at].grapheme_indices(true).next_back().map(|(i, _)| i).unwrap_or(0);
        let mut next = self.query.clone();
        next.replace_range(prev..at, "");
        self.set_query(next);
        self.query_cursor = prev;
        true
    }

    /// Whether Commands mode currently contains exactly `cd` or starts with
    /// `cd ` and therefore expects host-owned directory rows.
    pub fn is_cd_query(&self) -> bool {
        self.cd_query_suffix().is_some()
    }

    /// Raw target suffix following `cd `. Exact `cd` and `cd ` both yield an
    /// empty suffix. Matching is case-sensitive and only applies in Commands
    /// mode.
    pub fn cd_query_suffix(&self) -> Option<&str> {
        if !matches!(self.mode, PaletteMode::Commands) {
            return None;
        }
        if self.query == "cd" {
            Some("")
        } else {
            self.query.strip_prefix("cd ")
        }
    }

    /// Replace directory rows for the current `cd` suffix. Input order is
    /// preserved because filtering and ranking belong to the filesystem host.
    pub fn set_cd_directory_results(&mut self, results: Vec<PaletteDirectoryEntry>) {
        self.cd_directory_results = results;
        self.cd_completion_selected = false;
        self.selected_index = self
            .selected_index
            .min(self.cd_directory_results.len().saturating_sub(1));
        self.scroll_offset = self.scroll_offset.min(self.selected_index);
        self.reset_motion();
    }

    pub(super) fn row_height(&self) -> f32 {
        RESULT_ITEM_HEIGHT * self.scale
    }

    pub(super) fn reset_motion(&mut self) {
        self.list_scroll_spring.reset();
        self.cursor_spring.reset();
        self.last_list_scroll_frame = Instant::now();
        self.last_cursor_frame = Instant::now();
        self.selected_cursor_rect = None;
        self.wheel_accumulator = 0.0;
        self.touch_scroll_offset = 0.0;
        // A mode switch / requery / reopen invalidates any armed drag —
        // the dragged row may no longer exist in the new list.
        self.workspace_drag = None;
    }

    /// Record a dispatched workspace move so the Workspaces modal shows
    /// "moving…" on the target host header until [`Self::finish_workspace_move`].
    pub fn begin_workspace_move(&mut self, workspace_id: String, target_host_id: String) {
        self.workspace_move = Some(WorkspaceMoveStatus {
            workspace_id,
            target_host_id,
            phase: WorkspaceMovePhase::InFlight,
            since: Instant::now(),
        });
    }

    /// Flip the in-flight move to its outcome (shown for
    /// [`WORKSPACE_MOVE_RESULT_TTL`], then cleared by the tick).
    pub fn finish_workspace_move(&mut self, ok: bool, message: impl Into<String>) {
        if let Some(status) = self.workspace_move.as_mut() {
            status.phase = WorkspaceMovePhase::Done {
                ok,
                message: message.into(),
            };
            status.since = Instant::now();
        }
    }

    /// Current move feedback for render. `None` once a finished result
    /// has aged out.
    pub fn workspace_move_status(&self) -> Option<&WorkspaceMoveStatus> {
        self.workspace_move.as_ref()
    }

    /// Per-frame upkeep for the move feedback: expires finished results
    /// after [`WORKSPACE_MOVE_RESULT_TTL`]. Returns `true` while the
    /// status needs the host to keep redrawing (spinner animation or a
    /// visible result), including the frame that clears it.
    pub fn tick_workspace_move(&mut self) -> bool {
        match self.workspace_move.as_ref() {
            None => false,
            Some(status) => match &status.phase {
                WorkspaceMovePhase::InFlight => true,
                WorkspaceMovePhase::Done { .. } => {
                    if status.since.elapsed() >= WORKSPACE_MOVE_RESULT_TTL {
                        self.workspace_move = None;
                    }
                    true
                }
            },
        }
    }
}
