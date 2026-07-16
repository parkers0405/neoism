use std::collections::HashSet;
use std::path::PathBuf;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;

use web_time::Instant;

use crate::widgets::diff_card;

use super::state::GitDiffPanel;
use super::types::{
    FileChange, FocusSection, PanelData, PanelHit, ScrollbarKind, VisualRow,
    VisualRowKind,
};
use super::{
    FILE_ROW_HEIGHT, FILE_SCROLL_OFF_ROWS, PANEL_MAX_WIDTH, PANEL_MIN_WIDTH,
    PANEL_OPEN_ANIMATION_LENGTH, REFRESH_DEBOUNCE_MS, RESIZE_HIT_HALF,
    SCROLL_ANIMATION_LENGTH,
};

/// Build the flattened tree rows from the (path-sorted) file list and
/// the collapsed-directory set. Directory group nodes are emitted just
/// before their first child; a collapsed directory swallows every file
/// under it (the sorted order guarantees those files are contiguous).
pub(super) fn build_visual_rows(
    files: &[FileChange],
    collapsed: &HashSet<String>,
) -> Vec<VisualRow> {
    let mut rows: Vec<VisualRow> = Vec::with_capacity(files.len() + 8);
    // Directory paths currently "open" along the walk (deepest last).
    let mut open_stack: Vec<String> = Vec::new();
    // While set, files whose path starts with this prefix are hidden
    // (they live under a collapsed directory).
    let mut skip_prefix: Option<String> = None;

    for (fi, f) in files.iter().enumerate() {
        if let Some(pref) = &skip_prefix {
            if f.path.starts_with(pref.as_str()) {
                continue;
            }
            skip_prefix = None;
        }

        // Cumulative directory paths for this file's parent chain.
        let mut want: Vec<String> = Vec::new();
        let mut cum = String::new();
        let comps: Vec<&str> = f.path.split('/').collect();
        let dir_count = comps.len().saturating_sub(1);
        for c in &comps[..dir_count] {
            if !cum.is_empty() {
                cum.push('/');
            }
            cum.push_str(c);
            want.push(cum.clone());
        }

        // Truncate the open stack to the shared prefix, then push (and
        // emit rows for) the newly-entered directories.
        let mut common = 0;
        while common < open_stack.len()
            && common < want.len()
            && open_stack[common] == want[common]
        {
            common += 1;
        }
        open_stack.truncate(common);
        let mut hidden_by_collapse = false;
        for (depth, path) in want.iter().enumerate().skip(common) {
            let is_collapsed = collapsed.contains(path);
            rows.push(VisualRow {
                depth,
                kind: VisualRowKind::Dir {
                    path: path.clone(),
                    collapsed: is_collapsed,
                },
            });
            open_stack.push(path.clone());
            if is_collapsed {
                skip_prefix = Some(format!("{path}/"));
                hidden_by_collapse = true;
                break;
            }
        }
        if hidden_by_collapse {
            continue;
        }

        rows.push(VisualRow {
            depth: dir_count,
            kind: VisualRowKind::File { file_index: fi },
        });
    }
    rows
}

impl GitDiffPanel {
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// True while the panel reserves its right-edge column. Same as
    /// `is_visible` today; kept as a separate predicate so a future
    /// slide-out animation can reserve the column past the visibility
    /// flip without changing call sites.
    pub fn is_present(&self) -> bool {
        self.visible
    }

    pub fn is_focused(&self) -> bool {
        self.focused && self.visible
    }

    pub fn set_focused(&mut self, f: bool) {
        self.focused = f && self.visible;
        if !self.focused {
            self.clear_pending();
        }
    }

    /// Effective panel width in window-logical pixels. Used by the
    /// screen layer's chrome-layout pass to reserve a right margin so
    /// editor/terminal panes don't paint underneath. Honours the
    /// user-resizable `width` field.
    pub fn effective_width(&self, window_w: f32) -> f32 {
        if !self.is_present() {
            return 0.0;
        }
        // Scale + window-relative cap so a tiny window can never have
        // the panel eat more than 80% of the editor area.
        let scaled = self.width * self.scale;
        scaled.min(window_w * 0.8)
    }

    /// Current panel width in logical pixels (pre-scale).
    pub fn width(&self) -> f32 {
        self.width
    }

    /// Adjust the panel's width by `delta` logical pixels. Clamped to
    /// `[PANEL_MIN_WIDTH, PANEL_MAX_WIDTH]`. Mouse-drag resize and
    /// keyboard resize both call this so the chrome layout sees a
    /// single source of truth.
    pub fn resize(&mut self, delta: f32) {
        self.width = (self.width + delta).clamp(PANEL_MIN_WIDTH, PANEL_MAX_WIDTH);
    }

    /// Hit-test for the leading-edge resize gripper. Active only when
    /// the panel is visible — mirrors `is_hovering_file_tree_resize_edge`.
    pub fn is_hovering_resize_edge(&self, mx: f32, my: f32) -> bool {
        if !self.visible || self.panel_rect.w <= 0.0 {
            return false;
        }
        let edge_x = self.panel_rect.x;
        let in_y = my >= self.panel_rect.y && my <= self.panel_rect.y + self.panel_rect.h;
        in_y && (mx - edge_x).abs() <= RESIZE_HIT_HALF
    }

    pub fn select_next(&mut self) {
        self.rebuild_visual_rows();
        let leaves = self.visible_leaf_indices();
        if leaves.is_empty() {
            return;
        }
        let pos = leaves.iter().position(|&fi| fi == self.selected);
        match pos {
            Some(p) if p + 1 < leaves.len() => {
                let next = leaves[p + 1];
                let _ = self.select_file(next);
            }
            Some(_) => {
                // On the last visible file — let the diff card take the
                // keystroke so ↓ keeps reading the last file's diff.
                self.scroll_diff_rows(2);
            }
            None => {
                // Selection is hidden under a collapsed folder — land on
                // the first visible leaf.
                let first = leaves[0];
                let _ = self.select_file(first);
            }
        }
    }

    pub fn select_prev(&mut self) {
        self.rebuild_visual_rows();
        let leaves = self.visible_leaf_indices();
        if leaves.is_empty() {
            return;
        }
        let pos = leaves.iter().position(|&fi| fi == self.selected);
        match pos {
            Some(0) | None => self.scroll_diff_rows(-2),
            Some(p) => {
                let prev = leaves[p - 1];
                let _ = self.select_file(prev);
            }
        }
    }

    /// Move the file selection by `delta` visible leaves, clamped to the
    /// first/last file. The backbone of the vim count moves + half-page
    /// jumps. Safe on an empty list.
    fn move_selection_by(&mut self, delta: i32) {
        self.rebuild_visual_rows();
        let leaves = self.visible_leaf_indices();
        if leaves.is_empty() {
            return;
        }
        let cur = leaves
            .iter()
            .position(|&fi| fi == self.selected)
            .unwrap_or(0) as i32;
        let last = leaves.len() as i32 - 1;
        let target = (cur + delta).clamp(0, last) as usize;
        let _ = self.select_file(leaves[target]);
    }

    /// Vim `<count>j`: move the selection down `n` files (clamped).
    pub fn select_next_by(&mut self, n: usize) {
        self.move_selection_by(n as i32);
    }

    /// Vim `<count>k`: move the selection up `n` files (clamped).
    pub fn select_prev_by(&mut self, n: usize) {
        self.move_selection_by(-(n as i32));
    }

    /// Number of file rows that fit in the files card body — used to
    /// size Ctrl+D / Ctrl+U half-page jumps.
    fn files_visible_rows(&self) -> usize {
        let row_h = FILE_ROW_HEIGHT * self.scale;
        if row_h <= 0.0 {
            return 1;
        }
        ((self.files_body_rect.h / row_h).floor() as usize).max(1)
    }

    /// Ctrl+D — jump the file selection down half a page. Consuming this
    /// in the panel is also what keeps Ctrl+D from reaching the terminal
    /// behind it as an EOF that would close the shell.
    pub fn select_half_page_down(&mut self) {
        self.select_next_by((self.files_visible_rows() / 2).max(1));
    }

    /// Ctrl+U — jump the file selection up half a page.
    pub fn select_half_page_up(&mut self) {
        self.select_prev_by((self.files_visible_rows() / 2).max(1));
    }

    /// Vim `gg` / `1` — select the first visible file.
    pub fn select_first_file(&mut self) {
        self.clear_pending();
        self.rebuild_visual_rows();
        if let Some(&fi) = self.visible_leaf_indices().first() {
            let _ = self.select_file(fi);
        }
    }

    /// Vim `$` / `G` — select the last visible file.
    pub fn select_last_file(&mut self) {
        self.clear_pending();
        self.rebuild_visual_rows();
        if let Some(&fi) = self.visible_leaf_indices().last() {
            let _ = self.select_file(fi);
        }
    }

    /// Vim `<count>G` — select the `one_based`-th visible file (clamped).
    pub fn goto_file(&mut self, one_based: usize) {
        self.clear_pending();
        self.rebuild_visual_rows();
        let leaves = self.visible_leaf_indices();
        if leaves.is_empty() {
            return;
        }
        let ix = one_based
            .saturating_sub(1)
            .min(leaves.len().saturating_sub(1));
        let _ = self.select_file(leaves[ix]);
    }

    /// Feed a typed digit into the pending vim count. A leading `0` with
    /// no count in progress is ignored. Returns true when absorbed.
    pub fn push_count_digit(&mut self, digit: u32) -> bool {
        self.pending_g = false;
        if self.pending_count.is_none() && digit == 0 {
            return false;
        }
        let acc = self.pending_count.unwrap_or(0);
        self.pending_count = Some(acc.saturating_mul(10).saturating_add(digit as usize));
        true
    }

    /// Consume the pending count, defaulting to 1 when none was typed.
    pub fn take_count(&mut self) -> usize {
        self.pending_g = false;
        self.pending_count.take().unwrap_or(1).max(1)
    }

    /// Peek at the pending count without consuming it.
    pub fn pending_count(&self) -> Option<usize> {
        self.pending_count
    }

    /// Register a `g` keypress. Returns true when it completes a `gg`.
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

    /// Drop any half-entered count / `gg`.
    pub fn clear_pending(&mut self) {
        self.pending_count = None;
        self.pending_g = false;
    }

    /// Rebuild the cached tree rows from the current file list +
    /// collapsed-set. Cheap; called before any read of `visual_rows`
    /// that must reflect the latest data (navigation, hit-testing,
    /// scroll-into-view) and once per frame inside `render`.
    pub(super) fn rebuild_visual_rows(&mut self) {
        let rows = match self.data.lock() {
            Ok(d) => build_visual_rows(&d.files, &self.collapsed_dirs),
            Err(_) => return,
        };
        self.visual_rows = rows;
    }

    /// File indices of every *visible* leaf row, in tree order (files
    /// hidden under a collapsed folder are excluded).
    pub(super) fn visible_leaf_indices(&self) -> Vec<usize> {
        self.visual_rows
            .iter()
            .filter_map(|r| match r.kind {
                VisualRowKind::File { file_index } => Some(file_index),
                VisualRowKind::Dir { .. } => None,
            })
            .collect()
    }

    /// Visual-row index of the currently-selected file, if it's visible.
    pub(super) fn selected_visual_index(&self) -> Option<usize> {
        self.visual_rows.iter().position(|r| {
            matches!(r.kind, VisualRowKind::File { file_index } if file_index == self.selected)
        })
    }

    /// Toggle the collapsed state of the folder at `visual_ix`. No-op if
    /// the row isn't a directory. Rebuilds the tree so the change lands
    /// immediately.
    pub fn toggle_folder(&mut self, visual_ix: usize) {
        let path = match self.visual_rows.get(visual_ix) {
            Some(VisualRow {
                kind: VisualRowKind::Dir { path, .. },
                ..
            }) => path.clone(),
            _ => return,
        };
        if !self.collapsed_dirs.remove(&path) {
            self.collapsed_dirs.insert(path);
        }
        self.section = FocusSection::Files;
        self.checkbox_focused = false;
        self.rebuild_visual_rows();
        self.scroll_selected_into_view();
    }

    /// Returns the (path, repo_root) of the currently-selected file so
    /// the screen layer can `:edit` it on Enter. `None` if the panel
    /// has no files yet.
    pub fn selected_file_target(&self) -> Option<(PathBuf, PathBuf)> {
        let data = self.data.lock().ok()?;
        let f = data.files.get(self.selected)?;
        let root = data.repo_root.clone()?;
        let abs = root.join(&f.path);
        Some((abs, root))
    }

    pub fn is_animating(&self) -> bool {
        self.file_scroll_spring.position != 0.0
            || self.diff_scroll_spring.position != 0.0
            || self.open_progress() < 1.0
    }

    /// Cursor caret rect (window-logical) so the screen layer can
    /// animate the trail-cursor over to the panel's selected file row
    /// — same path the file_tree uses to make the terminal caret jump
    /// when the user navigates over.
    pub fn selected_cursor_rect(&self) -> Option<[f32; 4]> {
        self.selected_cursor_rect
    }

    pub fn needs_redraw(&self) -> bool {
        if !self.visible {
            return false;
        }
        if self.is_animating() {
            return true;
        }
        if let Ok(data) = self.data.lock() {
            if data.loading {
                return true;
            }
        }
        false
    }

    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale.clamp(0.5, 3.0);
        self.file_scroll_spring.reset();
        self.diff_scroll_spring.reset();
    }

    pub fn open(&mut self, repo_root: Option<PathBuf>, branch: Option<String>) {
        self.visible = true;
        self.focused = true;
        self.open_started_at = Some(Instant::now());
        self.selected = 0;
        self.file_scroll = 0.0;
        self.file_scroll_spring.reset();
        self.diff_scroll = 0.0;
        self.diff_scroll_spring.reset();
        self.commit_focused = false;
        self.section = FocusSection::Files;
        self.checkbox_focused = false;
        self.branch_menu_open = false;
        self.branch_filter.clear();
        self.clear_pending();
        self.refresh(repo_root, branch);
    }

    pub fn toggle(&mut self, repo_root: Option<PathBuf>, branch: Option<String>) {
        if self.visible {
            self.close();
        } else {
            self.open(repo_root, branch);
        }
    }

    pub fn close(&mut self) {
        if !self.visible {
            return;
        }
        self.visible = false;
        self.focused = false;
        self.commit_focused = false;
        self.branch_menu_open = false;
        self.open_started_at = None;
        self.clear_pending();
    }

    pub fn reset_for_server_switch(&mut self) {
        self.close();
        if let Ok(mut data) = self.data.lock() {
            *data = PanelData::default();
        }
        self.selected = 0;
        self.clear_pending();
    }

    pub(super) fn open_progress(&self) -> f32 {
        if !self.visible {
            return 0.0;
        }
        let Some(started) = self.open_started_at else {
            return 1.0;
        };
        let t = (Instant::now()
            .saturating_duration_since(started)
            .as_secs_f32()
            / PANEL_OPEN_ANIMATION_LENGTH.max(0.001))
        .clamp(0.0, 1.0);
        let inv = 1.0 - t;
        1.0 - inv * inv * inv
    }

    pub fn refresh(&mut self, repo_root: Option<PathBuf>, branch: Option<String>) {
        let now = Instant::now();
        let id = {
            let mut data = match self.data.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            let same_repo = data.repo_root.as_deref() == repo_root.as_deref();
            if let Some(last) = data.last_refresh {
                if same_repo
                    && now.saturating_duration_since(last).as_millis()
                        < REFRESH_DEBOUNCE_MS
                    && !data.files.is_empty()
                {
                    data.branch = branch.clone();
                    return;
                }
            }
            data.refresh_id = data.refresh_id.wrapping_add(1);
            data.loading = true;
            data.error = None;
            data.branch = branch;
            data.repo_root = repo_root.clone();
            data.last_refresh = Some(now);
            data.refresh_id
        };

        let Some(root) = repo_root else {
            if let Ok(mut data) = self.data.lock() {
                data.loading = false;
                data.files.clear();
                data.diffs.clear();
                data.error = Some("Not a git repository".to_string());
            }
            return;
        };

        // Web/wasm has no `GitDiffIo` provider installed by default —
        // the daemon pushes data directly into `self.data` instead.
        // Native fork installs an `Arc<dyn GitDiffIo>` so we can shell
        // out to `git status` here on a background thread.
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(io) = self.io.clone() else {
                if let Ok(mut data) = self.data.lock() {
                    data.loading = false;
                }
                return;
            };
            let arc = Arc::clone(&self.data);
            std::thread::spawn(move || {
                let files = io.collect_files(&root);
                let first_diff = files
                    .first()
                    .map(|f| (f.path.clone(), super::parse::load_diff(&root, f)));
                let Ok(mut data) = arc.lock() else { return };
                if data.refresh_id != id {
                    return;
                }
                data.loading = false;
                data.files = files;
                data.diffs.clear();
                if let Some((path, diff)) = first_diff {
                    data.diffs.insert(path, diff);
                }
            });
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = root;
            let _ = id;
        }
    }

    /// Host push (web): replace the changed-file list. On wasm there
    /// is no `GitDiffIo` provider — the daemon is the data source and
    /// the host stores results back here, mirroring the native
    /// refresh thread's store-back.
    pub fn host_set_files(&mut self, files: Vec<super::types::FileChange>) {
        let file_count = files.len();
        if let Ok(mut data) = self.data.lock() {
            data.loading = false;
            data.error = None;
            data.files = files;
            data.diffs.clear();
        }
        if self.selected >= file_count {
            self.selected = 0;
            self.file_scroll = 0.0;
        }
    }

    /// Host push (web): the diff body for one file, parsed from raw
    /// `git diff` patch text (hunk `@@` headers included).
    pub fn host_set_diff_text(&mut self, path: &str, patch: &str) {
        let mut lines = Vec::new();
        super::parse::parse_diff_into(patch.as_bytes(), &mut lines);
        if let Ok(mut data) = self.data.lock() {
            data.diffs.insert(path.to_string(), lines);
        }
    }

    /// Host push (web): surface a daemon-side failure in the panel
    /// body instead of spinning on `loading` forever.
    pub fn host_set_error(&mut self, message: String) {
        if let Ok(mut data) = self.data.lock() {
            data.loading = false;
            data.error = Some(message);
        }
    }

    // ── Commit box (bottom region) ───────────────────────────────────

    /// True when the commit-message box owns keyboard input.
    pub fn commit_box_focused(&self) -> bool {
        self.commit_focused && self.visible
    }

    /// Focus / blur the commit box. Focusing also takes panel focus so
    /// the caret animation and key routing land on the panel, and moves
    /// the section cursor onto the commit region.
    pub fn focus_commit_box(&mut self, focus: bool) {
        self.commit_focused = focus && self.visible;
        if focus {
            self.focused = true;
            self.section = FocusSection::Commit;
            self.close_branch_menu();
        }
    }

    /// Park the section cursor on the file list (mouse click on a file /
    /// folder row). Keeps Alt+Up/Down navigation consistent with where
    /// the pointer last acted.
    pub fn focus_files_section(&mut self) {
        self.section = FocusSection::Files;
        self.checkbox_focused = false;
        self.commit_focused = false;
    }

    /// The current commit-message text.
    pub fn commit_input_text(&self) -> &str {
        self.commit_input.text()
    }

    /// Insert typed text into the commit box (only when it is focused).
    pub fn commit_input_insert(&mut self, text: &str) {
        if !self.commit_focused || text.is_empty() {
            return;
        }
        self.commit_input.insert_str(text);
    }

    /// Delete the character before the caret in the commit box.
    pub fn commit_input_backspace(&mut self) {
        if !self.commit_focused {
            return;
        }
        self.commit_input.backspace();
    }

    /// Toggle stage/unstage for the file at `idx`. Stages an unstaged
    /// file (`git add`), unstages a staged one (`git restore --staged`),
    /// then refreshes the list. No-op on wasm (no IO provider).
    pub fn toggle_stage(&mut self, idx: usize) {
        let target = self
            .data
            .lock()
            .ok()
            .and_then(|d| d.files.get(idx).map(|f| (f.path.clone(), f.staged)));
        let Some((path, is_staged)) = target else {
            return;
        };
        #[cfg(not(target_arch = "wasm32"))]
        self.spawn_mutation(move |io, root| {
            if is_staged {
                io.unstage(root, &path)
            } else {
                io.stage(root, &path)
            }
        });
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (path, is_staged);
        }
    }

    /// Toggle stage/unstage for the currently-selected file.
    pub fn toggle_stage_selected(&mut self) {
        self.toggle_stage(self.selected);
    }

    /// Stage every currently-unstaged file, then refresh.
    pub fn stage_all(&mut self) {
        let paths: Vec<String> = self
            .data
            .lock()
            .ok()
            .map(|d| {
                d.files
                    .iter()
                    .filter(|f| !f.staged)
                    .map(|f| f.path.clone())
                    .collect()
            })
            .unwrap_or_default();
        if paths.is_empty() {
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        self.spawn_mutation(move |io, root| {
            for p in &paths {
                io.stage(root, p)?;
            }
            Ok(())
        });
        #[cfg(target_arch = "wasm32")]
        {
            let _ = paths;
        }
    }

    /// True when there is at least one file and *every* file is already
    /// staged. Drives the bottom-bar button's reversible label/action:
    /// all-staged → "Unstage All", otherwise → "Stage All".
    pub fn all_files_staged(&self) -> bool {
        self.data
            .lock()
            .ok()
            .map(|d| !d.files.is_empty() && d.files.iter().all(|f| f.staged))
            .unwrap_or(false)
    }

    /// Bottom-bar Stage/Unstage toggle. Unstages everything when all
    /// files are already staged; otherwise stages the unstaged ones.
    /// Mirrors the button's computed label so the click always matches
    /// what the user sees.
    pub fn stage_all_toggle(&mut self) {
        if self.all_files_staged() {
            self.unstage_all();
        } else {
            self.stage_all();
        }
    }

    /// Unstage every currently-staged file, then refresh.
    pub fn unstage_all(&mut self) {
        let paths: Vec<String> = self
            .data
            .lock()
            .ok()
            .map(|d| {
                d.files
                    .iter()
                    .filter(|f| f.staged)
                    .map(|f| f.path.clone())
                    .collect()
            })
            .unwrap_or_default();
        if paths.is_empty() {
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        self.spawn_mutation(move |io, root| {
            for p in &paths {
                io.unstage(root, p)?;
            }
            Ok(())
        });
        #[cfg(target_arch = "wasm32")]
        {
            let _ = paths;
        }
    }

    // ── Focus sections (Alt+Up/Down) ─────────────────────────────────

    /// Move focus to the previous section (Alt+Up): Commit → Diff →
    /// Files → Branch. Stops at Branch (the topmost section).
    pub fn section_focus_prev(&mut self) {
        self.section = match self.section {
            FocusSection::Commit => FocusSection::Diff,
            FocusSection::Diff => FocusSection::Files,
            FocusSection::Files | FocusSection::Branch => FocusSection::Branch,
        };
        self.sync_section_focus();
    }

    /// Move focus to the next section (Alt+Down): Branch → Files → Diff
    /// → Commit. Stops at Commit (the bottommost section). The Diff
    /// section parks focus on the changes card so ↑/↓ scroll the diff.
    pub fn section_focus_next(&mut self) {
        self.section = match self.section {
            FocusSection::Branch => FocusSection::Files,
            FocusSection::Files => FocusSection::Diff,
            FocusSection::Diff | FocusSection::Commit => FocusSection::Commit,
        };
        self.sync_section_focus();
    }

    /// True when the diff/changes card holds keyboard focus (drives its
    /// focus ring and routes plain ↑/↓ into `scroll_diff_keys`).
    pub fn diff_section_focused(&self) -> bool {
        self.visible && self.focused && self.section == FocusSection::Diff
    }

    /// Keep the derived focus flags (commit-box ownership, checkbox
    /// column, branch dropdown) in step with the active section.
    fn sync_section_focus(&mut self) {
        self.commit_focused = self.section == FocusSection::Commit && self.visible;
        if self.section != FocusSection::Files {
            self.checkbox_focused = false;
        }
        if self.section != FocusSection::Branch {
            self.close_branch_menu();
        }
    }

    /// Alt+Right inside the panel — hop focus onto the row checkbox
    /// column while in the Files section. Returns `true` when it moved
    /// (so the caller keeps focus in the panel); `false` means "already
    /// at the rightmost target, let the global chain continue".
    pub fn section_move_right(&mut self) -> bool {
        if self.section == FocusSection::Files && !self.checkbox_focused {
            self.checkbox_focused = true;
            return true;
        }
        false
    }

    /// Alt+Left inside the panel — hop focus back off the checkbox
    /// column. Returns `true` when it moved; `false` means the caller
    /// should leave the panel (focus the editor to the left).
    pub fn section_move_left(&mut self) -> bool {
        if self.section == FocusSection::Files && self.checkbox_focused {
            self.checkbox_focused = false;
            return true;
        }
        false
    }

    /// True when the branch-selector section holds focus (drives the
    /// focus ring on the branch button).
    pub fn branch_section_focused(&self) -> bool {
        self.visible && self.focused && self.section == FocusSection::Branch
    }

    /// True when the file-list checkbox column holds focus.
    pub fn checkbox_column_focused(&self) -> bool {
        self.visible && self.focused && self.checkbox_focused
    }

    // ── Branch dropdown ──────────────────────────────────────────────

    pub fn branch_menu_is_open(&self) -> bool {
        self.branch_menu_open && self.visible
    }

    /// Open the branch dropdown and kick off a background fetch of the
    /// local branch list. Native shells out to `git for-each-ref`; wasm
    /// leaves the list empty.
    pub fn open_branch_menu(&mut self) {
        if !self.visible {
            return;
        }
        self.branch_menu_open = true;
        self.section = FocusSection::Branch;
        self.commit_focused = false;
        self.branch_filter.clear();
        self.branch_menu_selected = 0;
        self.load_branches();
    }

    pub fn close_branch_menu(&mut self) {
        self.branch_menu_open = false;
        self.branch_filter.clear();
    }

    pub fn toggle_branch_menu(&mut self) {
        if self.branch_menu_open {
            self.close_branch_menu();
        } else {
            self.open_branch_menu();
        }
    }

    /// Branch names matching the dropdown's filter, in list order.
    pub(super) fn filtered_branches(&self) -> Vec<String> {
        let needle = self.branch_filter.text().to_ascii_lowercase();
        let all = self
            .data
            .lock()
            .map(|d| d.branches.clone())
            .unwrap_or_default();
        if needle.is_empty() {
            return all;
        }
        all.into_iter()
            .filter(|b| b.to_ascii_lowercase().contains(&needle))
            .collect()
    }

    /// Move the branch-dropdown highlight by `delta` rows, clamped.
    pub fn branch_menu_move(&mut self, delta: i32) {
        let count = self.filtered_branches().len();
        if count == 0 {
            self.branch_menu_selected = 0;
            return;
        }
        let cur = self.branch_menu_selected.min(count - 1) as i32;
        let next = (cur + delta).clamp(0, count as i32 - 1);
        self.branch_menu_selected = next as usize;
    }

    /// Insert typed text into the branch filter and reset the highlight.
    pub fn branch_filter_insert(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.branch_filter.insert_str(text);
        self.branch_menu_selected = 0;
    }

    pub fn branch_filter_backspace(&mut self) {
        self.branch_filter.backspace();
        self.branch_menu_selected = 0;
    }

    /// Switch to the highlighted branch in the dropdown (Enter). No-op
    /// when the filtered list is empty.
    pub fn branch_menu_activate(&mut self) {
        let branches = self.filtered_branches();
        let Some(branch) = branches.get(self.branch_menu_selected).cloned() else {
            return;
        };
        self.switch_branch(branch);
    }

    /// Switch by explicit name (dropdown row click).
    pub fn branch_menu_select_name(&mut self, branch: String) {
        self.switch_branch(branch);
    }

    /// Switch to the branch at `slot` in the rendered dropdown row list
    /// (row click). No-op if the slot is stale.
    pub fn activate_branch_row(&mut self, slot: usize) {
        if let Some((name, _)) = self.branch_menu_row_rects.get(slot).cloned() {
            self.switch_branch(name);
        }
    }

    /// Fetch the local branch list off-thread into `data.branches`.
    pub fn load_branches(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(io) = self.io.clone() else {
                return;
            };
            let Some(root) = self.data.lock().ok().and_then(|d| d.repo_root.clone())
            else {
                return;
            };
            let arc = Arc::clone(&self.data);
            std::thread::spawn(move || {
                let branches = io.list_branches(&root);
                if let Ok(mut data) = arc.lock() {
                    data.branches = branches;
                }
            });
        }
    }

    /// Check out `branch`, closing the dropdown and refreshing the file
    /// list. The branch label updates optimistically to the target.
    pub fn switch_branch(&mut self, branch: String) {
        self.close_branch_menu();
        self.selected = 0;
        self.file_scroll = 0.0;
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(io) = self.io.clone() else {
                return;
            };
            let (root, id) = {
                let mut data = match self.data.lock() {
                    Ok(g) => g,
                    Err(_) => return,
                };
                let Some(root) = data.repo_root.clone() else {
                    return;
                };
                // Skip a no-op switch to the branch we're already on.
                if data.branch.as_deref() == Some(branch.as_str()) {
                    return;
                }
                data.refresh_id = data.refresh_id.wrapping_add(1);
                data.loading = true;
                data.error = None;
                data.last_refresh = Some(Instant::now());
                (root, data.refresh_id)
            };
            let arc = Arc::clone(&self.data);
            let target = branch.clone();
            std::thread::spawn(move || {
                let result = io.checkout(&root, &target);
                let files = io.collect_files(&root);
                let first_diff = files
                    .first()
                    .map(|f| (f.path.clone(), super::parse::load_diff(&root, f)));
                let Ok(mut data) = arc.lock() else { return };
                if data.refresh_id != id {
                    return;
                }
                data.loading = false;
                match result {
                    Ok(()) => data.branch = Some(target),
                    Err(e) => data.error = Some(e),
                }
                data.files = files;
                data.diffs.clear();
                if let Some((path, diff)) = first_diff {
                    data.diffs.insert(path, diff);
                }
            });
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = branch;
        }
    }

    /// Commit the staged changes with the commit-box message. Clears
    /// the box + refreshes on success; surfaces git's error otherwise.
    pub fn commit(&mut self) {
        let message = self.commit_input.text().trim().to_string();
        if message.is_empty() {
            if let Ok(mut data) = self.data.lock() {
                data.error = Some("Commit message is empty".to_string());
            }
            return;
        }
        // The staged files vanish from the list after a commit, so
        // reset the selection to avoid pointing past the new list.
        self.selected = 0;
        self.file_scroll = 0.0;
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.spawn_mutation(move |io, root| io.commit(root, &message));
            self.commit_input.clear();
            self.commit_focused = false;
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = message;
        }
    }

    /// Run a mutating git op on a background thread, then re-collect the
    /// file list (bypassing the refresh debounce) so the staged state +
    /// diff update. Native-only; wasm has no `GitDiffIo` provider.
    #[cfg(not(target_arch = "wasm32"))]
    fn spawn_mutation<F>(&mut self, op: F)
    where
        F: FnOnce(
                &Arc<dyn super::state::GitDiffIo>,
                &std::path::Path,
            ) -> Result<(), String>
            + Send
            + 'static,
    {
        let Some(io) = self.io.clone() else {
            return;
        };
        let (root, id) = {
            let mut data = match self.data.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            let Some(root) = data.repo_root.clone() else {
                return;
            };
            data.refresh_id = data.refresh_id.wrapping_add(1);
            data.loading = true;
            data.error = None;
            data.last_refresh = Some(Instant::now());
            (root, data.refresh_id)
        };
        let arc = Arc::clone(&self.data);
        std::thread::spawn(move || {
            let result = op(&io, &root);
            let files = io.collect_files(&root);
            let first_diff = files
                .first()
                .map(|f| (f.path.clone(), super::parse::load_diff(&root, f)));
            let Ok(mut data) = arc.lock() else { return };
            if data.refresh_id != id {
                return;
            }
            data.loading = false;
            if let Err(e) = result {
                data.error = Some(e);
            }
            data.files = files;
            data.diffs.clear();
            if let Some((path, diff)) = first_diff {
                data.diffs.insert(path, diff);
            }
        });
    }

    pub fn active_rect(&self) -> Option<[f32; 4]> {
        if self.panel_rect.w <= 0.0 || self.panel_rect.h <= 0.0 {
            return None;
        }
        Some(self.panel_rect.as_array())
    }

    pub fn hit_test(&self, mx: f32, my: f32) -> PanelHit {
        if !self.visible || !self.panel_rect.contains(mx, my) {
            return PanelHit::Outside;
        }
        // Branch dropdown overlays the cards while open — its rows +
        // search box take priority over everything they cover.
        if self.branch_menu_open {
            for (slot, (_name, rect)) in self.branch_menu_row_rects.iter().enumerate() {
                if rect.contains(mx, my) {
                    return PanelHit::BranchMenuRow(slot);
                }
            }
            if self.branch_filter_rect.contains(mx, my) {
                return PanelHit::BranchFilterBox;
            }
        }
        if self.branch_button_rect.contains(mx, my) {
            return PanelHit::BranchButton;
        }
        if self.close_rect.contains(mx, my) {
            return PanelHit::Close;
        }
        // Commit region buttons + box take priority over anything they
        // overlap (they sit in the reserved bottom band).
        if self.commit_button_rect.contains(mx, my) {
            return PanelHit::CommitButton;
        }
        if self.stage_all_rect.contains(mx, my) {
            return PanelHit::StageAllButton;
        }
        if self.commit_box_rect.contains(mx, my) {
            return PanelHit::CommitBox;
        }
        // Per-row checkbox before the row itself so a checkbox click
        // toggles staging instead of moving the selection.
        for (idx, rect) in &self.file_checkbox_rects {
            if rect.contains(mx, my) {
                return PanelHit::FileCheckbox(*idx);
            }
        }
        for (idx, rect) in &self.file_row_rects {
            if rect.contains(mx, my) {
                return PanelHit::FileRow(*idx);
            }
        }
        for (visual_ix, rect) in &self.folder_row_rects {
            if rect.contains(mx, my) {
                return PanelHit::FolderToggle(*visual_ix);
            }
        }
        PanelHit::Inside
    }

    /// Programmatic select-by-index. Used by `select_next/prev` and
    /// click handlers; lazy-loads the file's diff and springs the
    /// selected row into the file-list viewport.
    pub fn select_file(&mut self, idx: usize) -> bool {
        let (path, repo_root, needs_load) = {
            let data = match self.data.lock() {
                Ok(g) => g,
                Err(_) => return false,
            };
            if idx >= data.files.len() {
                return false;
            }
            let f = &data.files[idx];
            let needs_load = !data.diffs.contains_key(&f.path);
            (f.path.clone(), data.repo_root.clone(), needs_load)
        };
        let changed = idx != self.selected;
        self.selected = idx;
        // Reset diff scroll so a freshly-selected file lands at the
        // top of its diff body — otherwise the bottom card would
        // start mid-diff for the new file.
        self.diff_scroll = 0.0;
        self.diff_scroll_spring.reset();
        self.scroll_selected_into_view();
        if needs_load {
            // Background load of the per-file diff. Native only —
            // wasm relies on the daemon pushing diffs into
            // `self.data.diffs` ahead of time.
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(root) = repo_root {
                let arc = Arc::clone(&self.data);
                let path_for_thread = path.clone();
                std::thread::spawn(move || {
                    let file = {
                        let Ok(d) = arc.lock() else { return };
                        d.files.iter().find(|f| f.path == path_for_thread).cloned()
                    };
                    let Some(file) = file else { return };
                    let diff = super::parse::load_diff(&root, &file);
                    if let Ok(mut d) = arc.lock() {
                        d.diffs.insert(path_for_thread, diff);
                    }
                });
            }
            #[cfg(target_arch = "wasm32")]
            {
                let _ = (path, repo_root);
            }
        }
        changed
    }

    pub fn scroll_diff_rows(&mut self, rows: i32) {
        if rows == 0 {
            return;
        }
        let line_h = diff_card::LINE_HEIGHT * self.scale;
        if line_h <= 0.0 {
            return;
        }
        let total_lines = self.current_diff_len();
        let visible_h =
            (self.diff_card_rect.h - diff_card::HEADER_HEIGHT * self.scale).max(0.0);
        let visible = ((visible_h / line_h).floor() as usize).max(1);
        let max_top = total_lines.saturating_sub(visible);
        let max_scroll = max_top as f32 * line_h;
        let target = (self.diff_scroll + rows as f32 * line_h).clamp(0.0, max_scroll);
        let drow = target - self.diff_scroll;
        if drow.abs() < 0.5 {
            return;
        }
        let was_idle = self.diff_scroll_spring.position == 0.0;
        self.diff_scroll = target;
        self.diff_scroll_spring.position += drow;
        if was_idle {
            self.last_diff_scroll_frame = Instant::now();
        }
    }

    pub fn scroll_at(&mut self, mx: f32, my: f32, delta: f32) -> bool {
        if !self.visible || !self.panel_rect.contains(mx, my) {
            return false;
        }
        // Diff card sits below the files card — route wheel events to
        // whichever card the mouse is over so the user can scroll the
        // file list and the diff independently.
        if self.diff_card_rect.contains(mx, my) {
            self.scroll_diff_pixels(delta);
        } else if self.files_card_rect.contains(mx, my) {
            self.scroll_files_pixels(delta);
        }
        true
    }

    /// Pixel-precise wheel/trackpad scroll for the file list. Applies
    /// the raw pixel delta straight to `file_scroll` — no whole-row
    /// quantization or sub-row dead-zone — then feeds the critically
    /// damped spring so trackpad, wheel and keyboard all glide the same
    /// way. Positive `delta` scrolls toward the top (content moves
    /// down), matching the overlay wheel convention.
    fn scroll_files_pixels(&mut self, delta: f32) {
        if delta == 0.0 {
            return;
        }
        let row_h = FILE_ROW_HEIGHT * self.scale;
        if row_h <= 0.0 {
            return;
        }
        let total = self.visual_rows.len();
        let visible = ((self.files_body_rect.h / row_h).floor() as usize).max(1);
        let max_scroll = total.saturating_sub(visible) as f32 * row_h;
        let target = (self.file_scroll - delta).clamp(0.0, max_scroll);
        let d = target - self.file_scroll;
        if d == 0.0 {
            return;
        }
        let was_idle = self.file_scroll_spring.position == 0.0;
        self.file_scroll = target;
        self.file_scroll_spring.position += d;
        if was_idle {
            self.last_file_scroll_frame = Instant::now();
        }
    }

    /// Pixel-precise wheel/trackpad scroll for the diff card. Same
    /// spring-lagged, dead-zone-free path as the file list so trackpad,
    /// wheel and held ↑/↓ (via `scroll_diff_keys`) all share one feel.
    fn scroll_diff_pixels(&mut self, delta: f32) {
        if delta == 0.0 {
            return;
        }
        let line_h = diff_card::LINE_HEIGHT * self.scale;
        if line_h <= 0.0 {
            return;
        }
        let total_lines = self.current_diff_len();
        let visible_h =
            (self.diff_card_rect.h - diff_card::HEADER_HEIGHT * self.scale).max(0.0);
        let visible = ((visible_h / line_h).floor() as usize).max(1);
        let max_scroll = total_lines.saturating_sub(visible) as f32 * line_h;
        let target = (self.diff_scroll - delta).clamp(0.0, max_scroll);
        let d = target - self.diff_scroll;
        if d == 0.0 {
            return;
        }
        let was_idle = self.diff_scroll_spring.position == 0.0;
        self.diff_scroll = target;
        self.diff_scroll_spring.position += d;
        if was_idle {
            self.last_diff_scroll_frame = Instant::now();
        }
    }

    /// Held ↑/↓ while the Diff section owns focus — smooth pixel scroll
    /// of the changes card. Routes through the same pixel path as the
    /// wheel so keyboard, wheel and trackpad feel identical (no ±row
    /// jumps). `down` scrolls toward the end of the diff.
    pub fn scroll_diff_keys(&mut self, down: bool) {
        let line_h = diff_card::LINE_HEIGHT * self.scale;
        let step = (line_h * 2.0).max(1.0);
        // `scroll_diff_pixels`: +delta scrolls up, so "down" => -step.
        self.scroll_diff_pixels(if down { -step } else { step });
    }

    pub(super) fn scroll_selected_into_view(&mut self) {
        let row_h = FILE_ROW_HEIGHT * self.scale;
        if row_h <= 0.0 || self.files_body_rect.h <= 0.0 {
            return;
        }
        let visible = ((self.files_body_rect.h / row_h).floor() as usize).max(1);
        // Scroll-off: keep `scroll_off` rows of context above and
        // below the cursor, mirroring the file_tree's behaviour. The
        // band shrinks gracefully on tiny viewports so the cursor can
        // still reach the very top/bottom row.
        let scroll_off = FILE_SCROLL_OFF_ROWS.min(visible.saturating_sub(1) / 2);
        // Tree rows (folders + files) drive the scroll space, not the
        // raw file count — a collapsed folder shrinks the list.
        let total = self.visual_rows.len();
        let selected_row = self.selected_visual_index().unwrap_or(0);
        let last_idx = total.saturating_sub(1);

        // Selected row's logical y inside the scroll space.
        let row_y = selected_row as f32 * row_h;
        let view_top = self.file_scroll;
        let view_bot = view_top + self.files_body_rect.h;

        // Distance from the *padded* viewport edges so the cursor
        // can never touch them unless we're at the actual list bound.
        let pad = scroll_off as f32 * row_h;
        let target = if selected_row <= scroll_off {
            // Near the very top — pin to 0 so the first row stays
            // anchored at the top of the viewport.
            0.0
        } else if last_idx.saturating_sub(selected_row) <= scroll_off {
            // Near the very bottom — pin so the last row sits at
            // the viewport bottom.
            (total as f32 * row_h - self.files_body_rect.h).max(0.0)
        } else if row_y < view_top + pad {
            (row_y - pad).max(0.0)
        } else if row_y + row_h > view_bot - pad {
            (row_y + row_h + pad - self.files_body_rect.h).max(0.0)
        } else {
            self.file_scroll
        };
        let max_top = total.saturating_sub(visible);
        let max_scroll = max_top as f32 * row_h;
        let target = target.clamp(0.0, max_scroll);
        let drow = target - self.file_scroll;
        if drow.abs() < 0.5 {
            return;
        }
        let was_idle = self.file_scroll_spring.position == 0.0;
        self.file_scroll = target;
        self.file_scroll_spring.position += drow;
        if was_idle {
            self.last_file_scroll_frame = Instant::now();
        }
    }

    /// Hit-test for the right-edge scrollbar of either card. Returns
    /// the kind so the screen layer can route a drag to the right
    /// scroll axis.
    pub fn scrollbar_hit(&self, mx: f32, my: f32) -> Option<ScrollbarKind> {
        if !self.visible {
            return None;
        }
        if super::render::hit_scrollbar_thumb(&self.files_scrollbar_thumb_rect, mx, my) {
            return Some(ScrollbarKind::Files);
        }
        if super::render::hit_scrollbar_thumb(&self.diff_scrollbar_thumb_rect, mx, my) {
            return Some(ScrollbarKind::Diff);
        }
        None
    }

    /// Drag a scrollbar thumb to a new vertical position. `mouse_y`
    /// is window-logical. Maps the thumb's track position onto the
    /// underlying scroll range and snaps the spring so the drag feels
    /// 1:1 instead of springing back.
    pub fn drag_scrollbar(&mut self, kind: ScrollbarKind, mouse_y: f32) {
        match kind {
            ScrollbarKind::Files => {
                let row_h = FILE_ROW_HEIGHT * self.scale;
                let total = self.visual_rows.len();
                let visible = ((self.files_body_rect.h / row_h).floor() as usize).max(1);
                if total <= visible || self.files_body_rect.h <= 0.0 {
                    return;
                }
                let max_top = total.saturating_sub(visible);
                let max_scroll = max_top as f32 * row_h;
                // Map `mouse_y` linearly across the track height.
                let progress = ((mouse_y - self.files_body_rect.y)
                    / self.files_body_rect.h.max(1.0))
                .clamp(0.0, 1.0);
                let target = (progress * max_scroll).clamp(0.0, max_scroll);
                self.file_scroll = target;
                self.file_scroll_spring.reset();
            }
            ScrollbarKind::Diff => {
                let line_h = diff_card::LINE_HEIGHT * self.scale;
                let total_lines = self.current_diff_len();
                let body_h = (self.diff_card_rect.h
                    - diff_card::HEADER_HEIGHT * self.scale)
                    .max(0.0);
                let visible = ((body_h / line_h).floor() as usize).max(1);
                if total_lines <= visible || body_h <= 0.0 {
                    return;
                }
                let max_top = total_lines.saturating_sub(visible);
                let max_scroll = max_top as f32 * line_h;
                let track_top =
                    self.diff_card_rect.y + diff_card::HEADER_HEIGHT * self.scale;
                let progress = ((mouse_y - track_top) / body_h.max(1.0)).clamp(0.0, 1.0);
                let target = (progress * max_scroll).clamp(0.0, max_scroll);
                self.diff_scroll = target;
                self.diff_scroll_spring.reset();
            }
        }
    }

    pub(super) fn current_diff_len(&self) -> usize {
        self.data
            .lock()
            .map(|d| {
                d.files
                    .get(self.selected)
                    .and_then(|f| d.diffs.get(&f.path))
                    .map(|v| {
                        diff_card::visual_row_count(
                            v,
                            diff_card::body_text_width(self.diff_card_rect.w, self.scale),
                            self.scale,
                        )
                    })
                    .unwrap_or(0)
            })
            .unwrap_or(0)
    }

    pub(super) fn tick_file_scroll(&mut self) -> f32 {
        if self.file_scroll_spring.position == 0.0 {
            self.last_file_scroll_frame = Instant::now();
            return 0.0;
        }
        let now = Instant::now();
        let dt = now
            .saturating_duration_since(self.last_file_scroll_frame)
            .as_secs_f32()
            .min(0.05);
        self.last_file_scroll_frame = now;
        self.file_scroll_spring.update(dt, SCROLL_ANIMATION_LENGTH);
        self.file_scroll_spring.position
    }

    pub(super) fn tick_diff_scroll(&mut self) -> f32 {
        if self.diff_scroll_spring.position == 0.0 {
            self.last_diff_scroll_frame = Instant::now();
            return 0.0;
        }
        let now = Instant::now();
        let dt = now
            .saturating_duration_since(self.last_diff_scroll_frame)
            .as_secs_f32()
            .min(0.05);
        self.last_diff_scroll_frame = now;
        self.diff_scroll_spring.update(dt, SCROLL_ANIMATION_LENGTH);
        self.diff_scroll_spring.position
    }
}
