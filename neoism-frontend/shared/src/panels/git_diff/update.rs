use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use web_time::Instant;

use crate::widgets::diff_card;

use super::state::GitDiffPanel;
use super::types::{
    FileChange, FocusSection, PanelData, PanelHit, Rect, ScrollbarKind, VisualRow,
    VisualRowKind,
};
use super::{
    FILE_ROW_HEIGHT, FILE_SCROLL_OFF_ROWS, PANEL_MAX_WIDTH, PANEL_MIN_WIDTH,
    PANEL_OPEN_ANIMATION_LENGTH, REFRESH_DEBOUNCE_MS, RESIZE_HIT_HALF,
    SCROLL_ANIMATION_LENGTH,
};

/// Build section-local directory trees. Indices always address the host list;
/// folder membership includes hidden descendants and never crosses a section.
pub(super) fn build_visual_rows(
    files: &[FileChange],
    collapsed: &HashSet<String>,
) -> Vec<VisualRow> {
    fn append(
        rows: &mut Vec<VisualRow>,
        files: &[FileChange],
        indices: &[usize],
        section: &str,
        prefix: &str,
        depth: usize,
        collapsed: &HashSet<String>,
    ) {
        let mut dirs = std::collections::BTreeMap::<String, Vec<usize>>::new();
        let mut leaves = Vec::new();
        for &index in indices {
            let relative = &files[index].path[prefix.len()..];
            if let Some((dir, _)) = relative.split_once('/') {
                dirs.entry(format!("{prefix}{dir}"))
                    .or_default()
                    .push(index);
            } else {
                leaves.push(index);
            }
        }
        for (path, children) in dirs {
            let key = format!("{section}/{path}");
            let is_collapsed = collapsed.contains(&key);
            rows.push(VisualRow {
                depth,
                kind: VisualRowKind::Group {
                    label: key,
                    path: Some(path.clone()),
                    file_indices: children.clone(),
                    collapsed: is_collapsed,
                },
            });
            if !is_collapsed {
                append(
                    rows,
                    files,
                    &children,
                    section,
                    &format!("{path}/"),
                    depth + 1,
                    collapsed,
                );
            }
        }
        leaves.sort_by(|&a, &b| files[a].path.cmp(&files[b].path));
        rows.extend(leaves.into_iter().map(|file_index| VisualRow {
            depth,
            kind: VisualRowKind::File { file_index },
        }));
    }

    let mut rows = Vec::with_capacity(files.len() + 2);
    for (label, untracked) in [("Tracked", false), ("Untracked", true)] {
        let indices: Vec<_> = files
            .iter()
            .enumerate()
            .filter(|(_, f)| {
                (f.status == super::types::FileStatus::Untracked) == untracked
            })
            .map(|(index, _)| index)
            .collect();
        if indices.is_empty() {
            continue;
        }
        let is_collapsed = collapsed.contains(label);
        rows.push(VisualRow {
            depth: 0,
            kind: VisualRowKind::Group {
                label: label.into(),
                path: None,
                file_indices: indices.clone(),
                collapsed: is_collapsed,
            },
        });
        if !is_collapsed {
            append(&mut rows, files, &indices, label, "", 1, collapsed);
        }
    }
    rows
}

impl GitDiffPanel {
    pub fn update_latest_commit_hover(&mut self, x: f32, y: f32) -> bool {
        let pr_changed = self.update_pull_request_hover(x, y);
        let hovered = self.visible
            && (self.latest_commit_rect.contains(x, y)
                || (self.latest_commit_hover
                    && self.latest_commit_popup_rect.contains(x, y)));
        let changed = self.latest_commit_hover != hovered || pr_changed;
        self.latest_commit_hover = hovered;
        changed
    }
    pub fn copy_latest_commit(&mut self) {
        self.latest_commit_copy = self.latest_commit().map(|commit| commit.sha);
    }
    pub fn take_latest_commit_copy(&mut self) -> Option<String> {
        self.latest_commit_copy.take()
    }
    pub fn request_latest_commit(&self) {
        if let Some(io) = self.io.as_ref().filter(|io| io.is_host_driven()) {
            io.request_latest_commit();
        }
    }
    pub fn latest_commit(&self) -> Option<neoism_protocol::git::CommitSummary> {
        self.data
            .lock()
            .ok()
            .and_then(|data| data.latest_commit.clone())
    }
    pub fn host_set_latest_commit(
        &mut self,
        commit: Option<neoism_protocol::git::CommitSummary>,
    ) {
        if let Ok(mut data) = self.data.lock() {
            data.latest_commit = commit;
        }
    }
    pub fn host_commit_detail(&mut self, detail: neoism_protocol::git::GitCommitDetail) {
        if let Ok(mut data) = self.data.lock() {
            data.commit_loading = false;
            if let Some(root) = data.repo_root.clone() {
                data.completed_commit_details.push((root, detail));
            }
        }
    }
    pub fn take_commit_details(
        &mut self,
    ) -> Vec<(PathBuf, neoism_protocol::git::GitCommitDetail)> {
        self.data
            .lock()
            .ok()
            .map(|mut data| std::mem::take(&mut data.completed_commit_details))
            .unwrap_or_default()
    }
    pub fn open_latest_commit(&mut self) {
        let Some(commit) = self.latest_commit() else {
            return;
        };
        let Some(root) = self.repo_root() else {
            return;
        };
        let Some(io) = self.io.clone() else {
            return;
        };
        if let Ok(mut data) = self.data.lock() {
            if data.commit_loading {
                return;
            }
            data.commit_loading = true;
        }
        if io.is_host_driven() {
            io.request_commit(&commit.sha);
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let data = self.data.clone();
            std::thread::spawn(move || {
                let result = io.read_commit(&root, &commit.sha);
                if let Ok(mut data) = data.lock() {
                    if data.repo_root.as_ref() != Some(&root) {
                        return;
                    }
                    data.commit_loading = false;
                    match result {
                        Ok(detail) => data.completed_commit_details.push((root, detail)),
                        Err(error) => data.error = Some(error),
                    }
                }
            });
        }
        #[cfg(target_arch = "wasm32")]
        if let Ok(mut data) = self.data.lock() {
            data.commit_loading = false;
            data.error = Some(format!(
                "Commit details require a Git host provider for {}",
                root.display()
            ));
        }
    }

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
            self.close_actions_menu();
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
        self.scroll_selected_into_view();
        match pos {
            Some(p) if p + 1 < leaves.len() => {
                let next = leaves[p + 1];
                let _ = self.select_file_with_reveal(next, true);
            }
            Some(_) => {}
            None => {
                // Selection is hidden under a collapsed folder — land on
                // the first visible leaf.
                let first = leaves[0];
                let _ = self.select_file_with_reveal(first, true);
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
        self.scroll_selected_into_view();
        match pos {
            Some(0) | None => {}
            Some(p) => {
                let prev = leaves[p - 1];
                let _ = self.select_file_with_reveal(prev, true);
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
        let _ = self.select_file_with_reveal(leaves[target], true);
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
            let _ = self.select_file_with_reveal(fi, true);
        }
    }

    /// Vim `$` / `G` — select the last visible file.
    pub fn select_last_file(&mut self) {
        self.clear_pending();
        self.rebuild_visual_rows();
        if let Some(&fi) = self.visible_leaf_indices().last() {
            let _ = self.select_file_with_reveal(fi, true);
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
        let _ = self.select_file_with_reveal(leaves[ix], true);
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
                VisualRowKind::Group { .. } => None,
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
                kind: VisualRowKind::Group { label, .. },
                ..
            }) => label.clone(),
            _ => return,
        };
        if !self.collapsed_dirs.remove(&path) {
            self.collapsed_dirs.insert(path);
        }
        self.section = FocusSection::Files;
        self.checkbox_focused = false;
        self.rebuild_visual_rows();
    }

    pub fn toggle_folder_stage(&mut self, visual_ix: usize) {
        let Some(VisualRow {
            kind: VisualRowKind::Group { file_indices, .. },
            ..
        }) = self.visual_rows.get(visual_ix)
        else {
            return;
        };
        let files = self.files();
        let children: Vec<_> = file_indices
            .iter()
            .filter_map(|&index| files.get(index).cloned())
            .collect();
        if children.is_empty() {
            return;
        }
        let all_staged = children
            .iter()
            .all(|f| f.staged && f.status != super::types::FileStatus::Mixed);
        self.run_mutation(move |io, root| {
            for file in children {
                if all_staged {
                    io.unstage(root, &file.path)?;
                } else if !file.staged || file.status == super::types::FileStatus::Mixed {
                    io.stage(root, &file.path)?;
                }
            }
            Ok(())
        });
    }

    pub fn begin_divider_drag(&mut self, x: f32, y: f32) -> bool {
        self.divider_dragging = self.is_hovering_divider(x, y);
        self.divider_dragging
    }

    /// Compatibility shim: the sidebar no longer contains a split divider.
    pub fn drag_divider(&mut self, _y: f32) -> bool {
        false
    }

    pub fn end_divider_drag(&mut self) -> bool {
        std::mem::take(&mut self.divider_dragging)
    }

    pub fn is_hovering_divider(&self, _x: f32, _y: f32) -> bool {
        false
    }

    /// Returns the (path, repo_root) of the currently-selected file so
    /// the screen layer can `:edit` it on Enter. `None` if the panel
    /// has no files yet.
    pub fn selected_file_target(&self) -> Option<(PathBuf, PathBuf)> {
        let data = self.data.lock().ok()?;
        let f = data.files.get(self.selected)?;
        let root = data.repo_root.clone()?;
        let abs = if self.io.as_ref().is_some_and(|io| io.is_host_driven()) {
            neoism_protocol::host_path::HostPath::new(root.to_string_lossy())
                .join(&f.path)
                .as_str()
                .into()
        } else {
            root.join(&f.path)
        };
        Some((abs, root))
    }

    pub fn branch(&self) -> Option<String> {
        self.data.lock().ok()?.branch.clone()
    }

    /// Current branch tracking ref. Feed this after each status/branch refresh.
    pub fn set_upstream(&mut self, upstream: Option<String>) {
        if let Ok(mut data) = self.data.lock() {
            data.upstream = upstream.filter(|s| !s.trim().is_empty());
        }
    }

    pub fn upstream(&self) -> Option<String> {
        self.data.lock().ok()?.upstream.clone()
    }

    pub fn push_button_label(&self) -> &'static str {
        if self.upstream().is_some() {
            "Push"
        } else {
            "Publish"
        }
    }

    pub fn push_button_enabled(&self) -> bool {
        self.git_action_enabled(super::types::GitAction::Push)
            && self.branch().is_some_and(|b| !b.trim().is_empty())
    }

    /// Dispatch the primary footer action (the arrow remains ActionsButton).
    pub fn activate_push_button(&mut self) {
        if !self.push_button_enabled() {
            return;
        }
        self.close_actions_menu();
        if self.upstream().is_some() {
            self.push();
        } else {
            self.publish();
        }
    }

    pub fn publish(&mut self) {
        self.run_mutation(|io, root| io.publish(root));
    }

    /// Snapshot for separate diff tabs. Accepts repo-relative or absolute paths.
    /// None means not loaded; Some(empty) is a loaded, empty patch.
    pub fn diff_lines_for_path(
        &self,
        path: &std::path::Path,
    ) -> Option<Vec<crate::widgets::diff_card::DiffLine>> {
        let data = self.data.lock().ok()?;
        let key = data
            .repo_root
            .as_ref()
            .and_then(|root| {
                neoism_protocol::host_path::HostPath::new(root.to_string_lossy())
                    .relative(&path.to_string_lossy())
            })
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        if !data.loading
            && data.error.is_none()
            && !data.files.iter().any(|file| file.path == key)
        {
            return Some(Vec::new());
        }
        data.diffs.get(&key).cloned()
    }

    /// Changes whenever cached patches are replaced/invalidated.
    pub fn diff_cache_generation(&self) -> u64 {
        self.data
            .lock()
            .map(|d| d.diff_cache_generation)
            .unwrap_or(0)
    }

    pub fn repo_root(&self) -> Option<PathBuf> {
        self.data.lock().ok()?.repo_root.clone()
    }

    pub fn files(&self) -> Vec<super::types::FileChange> {
        self.data
            .lock()
            .map(|data| data.files.clone())
            .unwrap_or_default()
    }

    pub fn selected_file_index(&self) -> Option<usize> {
        self.data
            .lock()
            .ok()
            .and_then(|data| (self.selected < data.files.len()).then_some(self.selected))
    }

    pub fn loading(&self) -> bool {
        self.data.lock().is_ok_and(|data| data.loading)
    }

    pub fn error(&self) -> Option<String> {
        self.data.lock().ok()?.error.clone()
    }

    pub fn is_animating(&self) -> bool {
        self.file_scroll_spring.position != 0.0 || self.open_progress() < 1.0
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
        if self.is_animating() || self.pull_request_needs_redraw() {
            return true;
        }
        if let Ok(data) = self.data.lock() {
            if data.loading
                || data.commit_loading
                || !data.completed_commit_details.is_empty()
            {
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
        self.close_actions_menu();
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
        self.divider_dragging = false;
        self.focused = false;
        self.commit_focused = false;
        self.branch_menu_open = false;
        self.close_actions_menu();
        self.open_started_at = None;
        self.clear_pending();
    }

    pub fn reset_for_server_switch(&mut self) {
        self.close();
        // Detach old workers: a reply from a previous endpoint must never
        // paint a newly selected workspace even if refresh ids coincide.
        self.data = Arc::new(std::sync::Mutex::new(PanelData::default()));
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
            if !same_repo || data.branch != branch {
                data.upstream = None;
                data.latest_commit = None;
                data.completed_commit_message = None;
            }
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
                data.diff_cache_generation = data.diff_cache_generation.wrapping_add(1);
                data.diffs.clear();
                data.base_texts.clear();
                data.base_pending.clear();
                data.base_errors.clear();
                data.error = Some("Not a git repository".to_string());
            }
            return;
        };

        if let Some(io) = self.io.as_ref().filter(|io| io.is_host_driven()) {
            let _ = io.collect_files(&root);
            io.request_latest_commit();
            return;
        }

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
                let upstream = io.upstream(&root);
                let latest_commit = io.latest_commit(&root);
                let first_diff = files
                    .first()
                    .map(|f| (f.path.clone(), super::parse::load_diff(&root, f)));
                let Ok(mut data) = arc.lock() else { return };
                if data.refresh_id != id {
                    return;
                }
                data.loading = false;
                data.files = files;
                data.upstream = upstream;
                data.latest_commit = latest_commit;
                data.diff_cache_generation = data.diff_cache_generation.wrapping_add(1);
                data.diffs.clear();
                data.base_texts.clear();
                data.base_pending.clear();
                data.base_errors.clear();
                if let Some((path, diff)) = first_diff {
                    data.diffs.insert(path, diff);
                }
            });
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = id;
            // With a JS-backed provider installed, `collect_files` fires
            // the daemon request and returns immediately — the reply
            // lands through `host_set_files` & co. Without one (legacy
            // hosts) the daemon-push path still owns the data, so leave
            // `loading` set for the host to clear.
            if let Some(io) = self.io.clone() {
                let _ = io.collect_files(&root);
                io.request_latest_commit();
            }
        }
    }

    /// Host push (web): replace the changed-file list. On wasm there
    /// is no `GitDiffIo` provider — the daemon is the data source and
    /// the host stores results back here, mirroring the native
    /// refresh thread's store-back.
    pub fn host_set_repo(&mut self, root: Option<PathBuf>, branch: Option<String>) {
        if let Ok(mut data) = self.data.lock() {
            if data.repo_root != root {
                data.commit_loading = false;
                data.completed_commit_details.clear();
            }
            if data.repo_root != root || data.branch != branch {
                data.base_texts.clear();
                data.base_pending.clear();
                data.base_errors.clear();
                data.upstream = None;
                data.completed_commit_message = None;
            }
            data.repo_root = root;
            data.branch = branch;
        }
    }

    pub fn host_set_files(&mut self, files: Vec<super::types::FileChange>) {
        let file_count = files.len();
        if let Ok(mut data) = self.data.lock() {
            data.loading = false;
            data.error = None;
            data.files = files;
            data.review_load_id = None;
            data.diff_cache_generation = data.diff_cache_generation.wrapping_add(1);
            data.diffs.clear();
            data.base_texts.clear();
            data.base_pending.clear();
            data.base_errors.clear();
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
        super::parse::parse_diff_into_with_headers(patch.as_bytes(), &mut lines);
        if let Ok(mut data) = self.data.lock() {
            data.diff_cache_generation = data.diff_cache_generation.wrapping_add(1);
            data.diffs.insert(path.to_string(), lines);
        }
    }

    /// Host push (web): surface a daemon-side failure in the panel
    /// body instead of spinning on `loading` forever.
    pub fn host_set_error(&mut self, message: String) {
        if let Ok(mut data) = self.data.lock() {
            data.loading = false;
            data.commit_loading = false;
            data.error = Some(message);
        }
    }

    /// Host push (web): the local branch list for the dropdown —
    /// mirrors the native `load_branches` thread's store-back. Clears
    /// `loading` because the wasm branch fetch raises it as a
    /// keep-alive for the redraw loop.
    pub fn host_set_branches(&mut self, branches: Vec<String>) {
        if let Ok(mut data) = self.data.lock() {
            data.branches = branches;
            data.loading = false;
        }
    }

    /// Host push (web): the current branch label — e.g. after a daemon
    /// checkout completed, mirroring the native `switch_branch`
    /// thread's `data.branch = Some(target)`.
    pub fn host_set_branch(&mut self, branch: Option<String>) {
        if let Ok(mut data) = self.data.lock() {
            if data.branch != branch {
                data.upstream = None;
            }
            data.branch = branch;
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
            self.close_actions_menu();
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
        self.run_mutation(move |io, root| {
            if is_staged {
                io.unstage(root, &path)
            } else {
                io.stage(root, &path)
            }
        });
    }

    /// Explicit review intent, validated against the current repository snapshot.
    pub fn stage_path(&mut self, root: &std::path::Path, path: &str, unstage: bool) {
        if self.loading() || self.repo_root().as_deref() != Some(root) {
            return;
        }
        let valid = self.files().iter().any(|f| {
            f.path == path
                && (f.staged && f.status != super::types::FileStatus::Mixed) == unstage
        });
        if !valid {
            return;
        }
        let path = path.to_owned();
        self.run_mutation(move |io, root| {
            if unstage {
                io.unstage(root, &path)
            } else {
                io.stage(root, &path)
            }
        });
    }

    /// Call only after confirmation. Index and untracked files are never discarded.
    pub fn restore_file(&mut self, root: &std::path::Path, path: &str) {
        if self.loading() || self.repo_root().as_deref() != Some(root) {
            return;
        }
        let valid = self.files().iter().any(|f| {
            f.path == path
                && f.status != super::types::FileStatus::Untracked
                && (!f.staged || f.status == super::types::FileStatus::Mixed)
        });
        if !valid {
            return;
        }
        let path = path.to_owned();
        self.run_mutation(move |io, root| io.restore(root, &path));
    }

    /// Hosts must complete canonical save/restore preparation before dispatch.
    pub fn apply_block(
        &mut self,
        root: &std::path::Path,
        path: &str,
        block: neoism_protocol::git::GitChangeBlock,
        restore: bool,
    ) -> bool {
        if self.loading()
            || self.io.is_none()
            || self.repo_root().as_deref() != Some(root)
            || (block.old_lines.is_empty() && block.new_lines.is_empty())
            || path.is_empty()
            // A retained canonical source can have new unsaved changes before
            // Git status sees them. The backend validates the exact block.
            || (restore && self.files().iter().any(|file| {
                file.path == path && file.status == super::types::FileStatus::Untracked
            }))
        {
            return false;
        }
        let path = path.to_owned();
        self.run_mutation(move |io, root| io.apply_block(root, &path, &block, restore));
        true
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
                    .filter(|f| !f.staged || f.status == super::types::FileStatus::Mixed)
                    .map(|f| f.path.clone())
                    .collect()
            })
            .unwrap_or_default();
        if paths.is_empty() {
            return;
        }
        self.run_mutation(move |io, root| {
            for p in &paths {
                io.stage(root, p)?;
            }
            Ok(())
        });
    }

    /// True when there is at least one file and *every* file is already
    /// staged. Drives the bottom-bar button's reversible label/action:
    /// all-staged → "Unstage All", otherwise → "Stage All".
    pub fn all_files_staged(&self) -> bool {
        self.data
            .lock()
            .ok()
            .map(|d| {
                !d.files.is_empty()
                    && d.files
                        .iter()
                        .all(|f| f.staged && f.status != super::types::FileStatus::Mixed)
            })
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
        self.run_mutation(move |io, root| {
            for p in &paths {
                io.unstage(root, p)?;
            }
            Ok(())
        });
    }

    // ── Focus sections (Alt+Up/Down) ─────────────────────────────────

    /// Alt+Up: Commit → footer Branch → Files; clamp at Files.
    pub fn section_focus_prev(&mut self) {
        self.section = match self.section {
            FocusSection::Commit => FocusSection::Branch,
            FocusSection::Branch => FocusSection::Files,
            FocusSection::Files => FocusSection::Files,
        };
        self.sync_section_focus();
    }

    /// Alt+Down: Files → footer Branch → Commit; clamp at Commit.
    pub fn section_focus_next(&mut self) {
        self.section = match self.section {
            FocusSection::Files => FocusSection::Branch,
            FocusSection::Branch | FocusSection::Commit => FocusSection::Commit,
        };
        self.sync_section_focus();
    }

    /// Legacy host query: the sidebar never owns a diff focus section.
    pub fn diff_section_focused(&self) -> bool {
        false // Diff previews live in route tabs, never an invisible sidebar stop.
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

    // The same availability check drives menu rows and execution, so a stale
    // popup cannot start an operation after a host refresh changes the state.
    pub(super) fn git_action_enabled(&self, action: super::types::GitAction) -> bool {
        use super::types::GitAction;
        let Ok(data) = self.data.lock() else {
            return false;
        };
        if self.io.is_none() || data.repo_root.is_none() || data.loading {
            return false;
        }
        match action {
            GitAction::Commit => {
                !self.commit_input.text().trim().is_empty()
                    && data.files.iter().any(|f| f.staged)
            }
            GitAction::StageAll => data
                .files
                .iter()
                .any(|f| !f.staged || f.status == super::types::FileStatus::Mixed),
            GitAction::UnstageAll => data.files.iter().any(|f| f.staged),
            GitAction::ToggleStageSelected => data.files.get(self.selected).is_some(),
            GitAction::OpenPullRequest => !data.pr.pending,
            GitAction::CreatePullRequest => !data.pr.pending && !matches!(&data.pr.discovered, Some(neoism_protocol::pull_request::PullRequestReply::Discovered { pull_request: Some(_), .. })),
            _ => true,
        }
    }

    pub(super) fn git_action_label(
        &self,
        action: super::types::GitAction,
    ) -> &'static str {
        use super::types::GitAction;
        match action {
            GitAction::Commit => "Commit",
            GitAction::StageAll => "Stage all changes",
            GitAction::UnstageAll => "Unstage all changes",
            GitAction::ToggleStageSelected => {
                if self
                    .data
                    .lock()
                    .ok()
                    .is_some_and(|d| d.files.get(self.selected).is_some_and(|f| f.staged))
                {
                    "Unstage selected file"
                } else {
                    "Stage selected file"
                }
            }
            GitAction::Fetch => "Fetch",
            GitAction::Pull => "Pull (fast-forward only)",
            GitAction::Push => "Push",
            GitAction::SwitchBranch => "Switch branch...",
            GitAction::Refresh => "Refresh",
            GitAction::OpenPullRequest => "Open pull request...",
            GitAction::CreatePullRequest => "Create pull request...",
            GitAction::BrowsePullRequests => "Pull Requests...",
        }
    }

    pub fn actions_menu_is_open(&self) -> bool {
        self.visible && self.actions_menu.is_interactive()
    }

    pub fn actions_menu_contains(&self, mx: f32, my: f32) -> bool {
        self.actions_menu_is_open() && self.actions_menu_rect.contains(mx, my)
    }

    pub fn open_actions_menu(&mut self) {
        if !self.visible {
            return;
        }
        self.close_branch_menu();
        self.focused = true;
        self.section = FocusSection::Commit;
        self.commit_focused = false;
        self.clear_pending();
        self.refresh_actions_menu();
        self.actions_menu.open_instant();
    }

    pub(super) fn refresh_actions_menu(&mut self) {
        let items: Vec<_> = super::types::GitAction::ALL
            .into_iter()
            .map(|action| {
                let mut item = crate::widgets::menu::MenuItem::new(
                    self.git_action_label(action),
                    action,
                );
                item.enabled = self.git_action_enabled(action);
                item
            })
            .collect();
        let changed = self
            .actions_menu
            .content()
            .items()
            .iter()
            .zip(&items)
            .any(|(old, new)| old.enabled != new.enabled || old.label != new.label)
            || self.actions_menu.content().len() != items.len();
        if changed {
            let selected = self.actions_menu.content().selected_index();
            self.actions_menu.content_mut().set_items(items);
            self.actions_menu.content_mut().set_selected_index(selected);
        }
    }

    pub fn close_actions_menu(&mut self) {
        self.actions_menu.close_instant();
        self.actions_menu_scroll_accumulator = 0.0;
        self.actions_menu_rect = Rect::ZERO;
        self.actions_menu_row_rects.clear();
    }

    pub fn toggle_actions_menu(&mut self) {
        if self.actions_menu_is_open() {
            self.close_actions_menu();
        } else {
            self.open_actions_menu();
        }
    }

    pub fn actions_menu_move(&mut self, delta: i32) {
        self.actions_menu.content_mut().move_selection(delta);
    }

    pub fn actions_menu_activate(&mut self) {
        let action = self.actions_menu.content().selected_action().copied();
        if let Some(action) = action {
            self.run_git_action(action);
        }
    }

    pub fn activate_action_row(&mut self, slot: usize) {
        if self.actions_menu.content_mut().set_selected_index(slot) {
            self.actions_menu_activate();
        }
    }

    fn run_git_action(&mut self, action: super::types::GitAction) {
        use super::types::GitAction;
        if !self.git_action_enabled(action) {
            return;
        }
        self.close_actions_menu();
        match action {
            GitAction::Commit => self.commit(),
            GitAction::StageAll => self.stage_all(),
            GitAction::UnstageAll => self.unstage_all(),
            GitAction::ToggleStageSelected => self.toggle_stage_selected(),
            GitAction::Fetch => self.fetch(),
            GitAction::Pull => self.pull(),
            GitAction::Push => self.push(),
            GitAction::SwitchBranch => self.open_branch_menu(),
            GitAction::OpenPullRequest => self.start_pull_request_lookup(),
            GitAction::BrowsePullRequests => { self.open_pull_request_inbox(); }
            GitAction::CreatePullRequest => { self.start_pull_request_creation(); }
            GitAction::Refresh => {
                let target = self.data.lock().ok().map(|mut d| {
                    d.last_refresh = None;
                    (d.repo_root.clone(), d.branch.clone())
                });
                if let Some((root, branch)) = target {
                    self.refresh(root, branch);
                }
            }
        }
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
        self.close_actions_menu();
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
        if let Some(io) = self.io.as_ref().filter(|io| io.is_host_driven()) {
            let root = self.data.lock().ok().and_then(|d| d.repo_root.clone());
            if let Some(root) = root {
                let _ = io.list_branches(&root);
            }
            return;
        }
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
        #[cfg(target_arch = "wasm32")]
        {
            // JS-backed provider: fires the daemon request; the list
            // lands back through `host_set_branches`. `loading` doubles
            // as the redraw keep-alive so the reply paints without a
            // user event (it has no visual effect while files exist).
            let Some(io) = self.io.clone() else {
                return;
            };
            let Some(root) = self.data.lock().ok().and_then(|d| d.repo_root.clone())
            else {
                return;
            };
            if let Ok(mut data) = self.data.lock() {
                data.loading = true;
            }
            let _ = io.list_branches(&root);
        }
    }

    /// Check out `branch`, closing the dropdown and refreshing the file
    /// list. The branch label updates optimistically to the target.
    pub fn switch_branch(&mut self, branch: String) {
        self.close_branch_menu();
        self.selected = 0;
        self.file_scroll = 0.0;
        if self.io.as_ref().is_some_and(|io| io.is_host_driven()) {
            self.run_mutation(move |io, root| io.checkout(root, &branch));
            return;
        }
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
                let upstream = io.upstream(&root);
                let latest_commit = io.latest_commit(&root);
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
                data.upstream = upstream;
                data.latest_commit = latest_commit;
                data.diff_cache_generation = data.diff_cache_generation.wrapping_add(1);
                data.diffs.clear();
                data.base_texts.clear();
                data.base_pending.clear();
                data.base_errors.clear();
                if let Some((path, diff)) = first_diff {
                    data.diffs.insert(path, diff);
                }
            });
        }
        #[cfg(target_arch = "wasm32")]
        {
            // Skip a no-op switch to the branch we're already on, then
            // queue the checkout through the JS-backed provider. The
            // refreshed file list + new branch label land back via the
            // `ChangedFiles` host push.
            let same = self
                .data
                .lock()
                .ok()
                .is_some_and(|d| d.branch.as_deref() == Some(branch.as_str()));
            if same {
                return;
            }
            self.run_mutation(move |io, root| io.checkout(root, &branch));
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
        if !self.git_action_enabled(super::types::GitAction::Commit) {
            return;
        }
        // The staged files vanish from the list after a commit, so
        // reset the selection to avoid pointing past the new list.
        self.selected = 0;
        self.file_scroll = 0.0;
        let confirmation = Arc::clone(&self.data);
        let expected_id = self.data.lock().ok().map(|d| d.refresh_id.wrapping_add(1));
        self.run_mutation(move |io, root| {
            let result = io.commit(root, &message);
            // A host-driven Ok merely queues a request. Only local IO can
            // acknowledge here; hosts deliver explicit success separately.
            if result.is_ok() && !io.is_host_driven() {
                if let Ok(mut data) = confirmation.lock() {
                    if Some(data.refresh_id) == expected_id {
                        data.completed_commit_message = Some(message);
                    }
                }
            }
            result
        });
        // Keep the draft until explicit local/host confirmation.
    }

    pub(super) fn apply_commit_confirmation(&mut self) {
        let completed = self
            .data
            .lock()
            .ok()
            .and_then(|mut data| data.completed_commit_message.take());
        if let Some(message) = completed {
            self.host_commit_succeeded(&message);
        }
    }

    /// Call only after the commit backend confirms success. Do not erase a
    /// newer draft typed while the original request was in flight.
    pub fn host_commit_succeeded(&mut self, submitted_message: &str) {
        if self.commit_input.text().trim() == submitted_message.trim() {
            self.commit_input.clear();
        }
    }

    pub fn fetch(&mut self) {
        self.run_mutation(|io, root| io.fetch(root));
    }

    pub fn pull(&mut self) {
        self.run_mutation(|io, root| io.pull(root));
    }

    pub fn push(&mut self) {
        self.run_mutation(|io, root| io.push(root));
    }

    /// Run a mutating git op through the installed `GitDiffIo`
    /// provider, then refresh the file list (bypassing the refresh
    /// debounce) so the staged state + diff update.
    ///
    /// Native: runs on a background thread and re-collects the list
    /// itself. Wasm: runs synchronously — the JS-backed provider just
    /// queues the daemon request and returns immediately, and the
    /// refreshed list (staged bits, branch, error) lands back through
    /// the `host_set_*` push path; `loading` stays raised meanwhile as
    /// the redraw keep-alive.
    pub(super) fn run_mutation<F>(&mut self, op: F)
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
        if io.is_host_driven() {
            if let Err(error) = op(&io, &root) {
                self.host_set_error(error);
            }
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let arc = Arc::clone(&self.data);
            std::thread::spawn(move || {
                let result = op(&io, &root);
                let files = io.collect_files(&root);
                let upstream = io.upstream(&root);
                let latest_commit = io.latest_commit(&root);
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
                data.upstream = upstream;
                data.latest_commit = latest_commit;
                data.diff_cache_generation = data.diff_cache_generation.wrapping_add(1);
                data.diffs.clear();
                data.base_texts.clear();
                data.base_pending.clear();
                data.base_errors.clear();
                if let Some((path, diff)) = first_diff {
                    data.diffs.insert(path, diff);
                }
            });
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = id;
            if let Err(e) = op(&io, &root) {
                if let Ok(mut data) = self.data.lock() {
                    data.loading = false;
                    data.error = Some(e);
                }
            }
        }
    }

    pub fn active_rect(&self) -> Option<[f32; 4]> {
        if self.panel_rect.w <= 0.0 || self.panel_rect.h <= 0.0 {
            return None;
        }
        Some(self.panel_rect.as_array())
    }

    /// Pixel extent, not rounded row counts: fractional viewports must reveal
    /// the entire last row without adding a blank row beneath it.
    pub(super) fn max_file_scroll(&self) -> f32 {
        if self.files_body_rect.h <= 0.0 {
            return 0.0;
        }
        (self.visual_rows.len() as f32 * FILE_ROW_HEIGHT * self.scale
            - self.files_body_rect.h)
            .max(0.0)
    }

    pub(super) fn clamp_file_scroll(&mut self) {
        let clamped = self.file_scroll.clamp(0.0, self.max_file_scroll());
        if clamped != self.file_scroll {
            self.file_scroll = clamped;
            self.file_scroll_spring.reset();
        }
    }

    pub fn pointer_kind_at(
        &self,
        mx: f32,
        my: f32,
    ) -> Option<crate::widgets::modal::ModalPointerKind> {
        use crate::widgets::modal::ModalPointerKind::{Default, Pointer, Text};
        match self.hit_test(mx, my) {
            PanelHit::Outside => None,
            PanelHit::CommitBox | PanelHit::BranchFilterBox => Some(Text),
            PanelHit::Inside => Some(Default),
            _ => Some(Pointer),
        }
    }

    pub fn hit_test(&self, mx: f32, my: f32) -> PanelHit {
        if !self.branch_menu_open && !self.actions_menu_is_open() && self.pull_request_hit(mx, my) { return PanelHit::PullRequest; }
        if self.visible
            && self.latest_commit_hover
            && self.latest_commit_copy_rect.contains(mx, my)
        {
            return PanelHit::LatestCommitCopy;
        }
        if self.visible && self.latest_commit_rect.contains(mx, my) {
            return PanelHit::LatestCommit;
        }
        if !self.visible || !self.panel_rect.contains(mx, my) {
            return PanelHit::Outside;
        }
        if self.actions_menu_is_open() && self.actions_menu_rect.contains(mx, my) {
            for (slot, rect) in &self.actions_menu_row_rects {
                if rect.contains(mx, my) {
                    return PanelHit::ActionMenuRow(*slot);
                }
            }
            return PanelHit::Inside;
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
            if self.branch_menu_rect.contains(mx, my) {
                return PanelHit::Inside;
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
        if self.push_button_rect.contains(mx, my) {
            return PanelHit::PushButton;
        }
        if self.actions_button_rect.contains(mx, my) {
            return PanelHit::ActionsButton;
        }
        if self.commit_box_rect.contains(mx, my) {
            return PanelHit::CommitBox;
        }
        // Per-row checkbox before the row itself so a checkbox click
        // toggles staging instead of moving the selection.
        for (idx, rect) in &self.folder_checkbox_rects {
            if rect.contains(mx, my) {
                return PanelHit::FolderCheckbox(*idx);
            }
        }
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

    /// Pointer/host selection loads the diff without changing the user's
    /// file-list scroll. Keyboard navigation uses the reveal variant below.
    pub fn select_file(&mut self, idx: usize) -> bool {
        self.select_file_with_reveal(idx, false)
    }

    fn select_file_with_reveal(&mut self, idx: usize, reveal: bool) -> bool {
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
        if changed {
            self.diff_scroll = 0.0;
            self.diff_scroll_spring.reset();
        }
        if reveal {
            self.scroll_selected_into_view();
        }
        if needs_load {
            if let Some(io) = self.io.as_ref().filter(|io| io.is_host_driven()) {
                io.request_diff(&path);
                return changed;
            }
            // Background load of the per-file diff. Native only —
            // wasm relies on the daemon pushing diffs into
            // `self.data.diffs` ahead of time.
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(root) = repo_root {
                let arc = Arc::clone(&self.data);
                let refresh_id = arc.lock().ok().map(|d| d.refresh_id);
                let path_for_thread = path.clone();
                std::thread::spawn(move || {
                    let file = {
                        let Ok(d) = arc.lock() else { return };
                        d.files.iter().find(|f| f.path == path_for_thread).cloned()
                    };
                    let Some(file) = file else { return };
                    let diff = super::parse::load_diff(&root, &file);
                    if let Ok(mut d) = arc.lock() {
                        if Some(d.refresh_id) != refresh_id {
                            return;
                        }
                        d.diff_cache_generation = d.diff_cache_generation.wrapping_add(1);
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
        if rows == 0 || self.diff_card_rect.h <= 0.0 {
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
        if self.actions_menu_contains(mx, my) {
            self.actions_menu.content_mut().scroll_pixels(
                delta,
                28.0 * self.scale,
                &mut self.actions_menu_scroll_accumulator,
            );
            return true;
        }
        if self.branch_menu_is_open() && self.branch_menu_rect.contains(mx, my) {
            if delta != 0.0 {
                self.branch_menu_move(if delta < 0.0 { 1 } else { -1 });
            }
            return true;
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

    /// Finger-direct scroll with no spring tail. The panel retains ownership
    /// at its bounds so a covered editor never starts moving mid-gesture.
    pub fn scroll_touch_at(&mut self, mx: f32, my: f32, finger_delta: f32) -> bool {
        if !self.visible || !self.panel_rect.contains(mx, my) {
            return false;
        }
        if self.actions_menu_contains(mx, my)
            || (self.branch_menu_is_open() && self.branch_menu_rect.contains(mx, my))
        {
            return self.scroll_at(mx, my, finger_delta);
        }
        if self.diff_card_rect.contains(mx, my) {
            self.scroll_diff_pixels(finger_delta);
            self.diff_scroll_spring.reset();
        } else if self.files_card_rect.contains(mx, my) {
            self.scroll_files_pixels(finger_delta);
            self.file_scroll_spring.reset();
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
        let max_scroll = self.max_file_scroll();
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
        if delta == 0.0 || self.diff_card_rect.h <= 0.0 {
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
        let max_scroll = self.max_file_scroll();
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
        if !self.visible
            || self.actions_menu_contains(mx, my)
            || (self.branch_menu_is_open() && self.branch_menu_rect.contains(mx, my))
        {
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
                let max_scroll = self.max_file_scroll();
                if max_scroll <= 0.0 {
                    return;
                }
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
}

#[cfg(test)]
mod tests {
    use super::super::types::{FileStatus, Rect};
    use super::*;

    #[test]
    fn file_viewport_uses_pixel_bounds_and_reveals_complete_last_row() {
        let mut panel = GitDiffPanel::new();
        for scale in [1.0, 1.25, 2.0] {
            panel.scale = scale;
            let row_h = FILE_ROW_HEIGHT * scale;
            panel.visual_rows = (0..10)
                .map(|file_index| VisualRow {
                    depth: 0,
                    kind: VisualRowKind::File { file_index },
                })
                .collect();
            panel.files_body_rect.h = row_h * 3.5;
            let max = row_h * 6.5;
            assert!((panel.max_file_scroll() - max).abs() < 0.001);
            panel.file_scroll = 0.0;
            panel.selected = 9;
            panel.scroll_selected_into_view();
            assert!((panel.file_scroll - max).abs() < 0.001);
            assert!(
                (10.0 * row_h - panel.file_scroll - panel.files_body_rect.h).abs()
                    < 0.001
            );
            panel.scroll_files_pixels(-10000.0);
            assert_eq!(panel.file_scroll, max);
            panel.drag_scrollbar(ScrollbarKind::Files, 10000.0);
            assert_eq!(panel.file_scroll, max);
            // Collapsing to a fitting list resets both target and spring.
            panel.visual_rows.truncate(2);
            panel.file_scroll_spring.position = 40.0;
            panel.clamp_file_scroll();
            assert_eq!(panel.file_scroll, 0.0);
            assert_eq!(panel.file_scroll_spring.position, 0.0);
            panel.files_body_rect.h = 0.0;
            assert_eq!(panel.max_file_scroll(), 0.0);
        }
    }

    #[test]
    fn git_pointer_classification_reserves_ibeam_for_inputs() {
        use crate::widgets::modal::ModalPointerKind::{Default, Pointer, Text};
        let mut panel = GitDiffPanel::new();
        panel.visible = true;
        panel.panel_rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 300.0,
            h: 500.0,
        };
        let rect = Rect {
            x: 10.0,
            y: 10.0,
            w: 100.0,
            h: 20.0,
        };
        assert_eq!(panel.pointer_kind_at(250.0, 250.0), Some(Default));
        assert_eq!(panel.pointer_kind_at(400.0, 250.0), None);
        panel.file_row_rects.push((0, rect));
        assert_eq!(panel.pointer_kind_at(20.0, 20.0), Some(Pointer));
        panel.commit_box_rect = rect;
        assert_eq!(panel.pointer_kind_at(20.0, 20.0), Some(Text));
        panel.branch_menu_open = true;
        panel.branch_filter_rect = rect;
        assert_eq!(panel.pointer_kind_at(20.0, 20.0), Some(Text));
        panel.branch_menu_row_rects.push(("main".into(), rect));
        assert_eq!(panel.pointer_kind_at(20.0, 20.0), Some(Pointer));
        panel.visible = false;
        assert_eq!(panel.pointer_kind_at(20.0, 20.0), None);
    }

    #[test]
    fn group_checkbox_stages_and_unstages_only_its_members() {
        use std::path::Path;
        use std::sync::Mutex;
        struct Io(Mutex<Vec<String>>);
        impl super::super::state::GitDiffIo for Io {
            fn is_host_driven(&self) -> bool {
                true
            }
            fn collect_files(&self, _: &Path) -> Vec<FileChange> {
                vec![]
            }
            fn stage(&self, _: &Path, path: &str) -> Result<(), String> {
                self.0.lock().unwrap().push(format!("stage {path}"));
                Ok(())
            }
            fn unstage(&self, _: &Path, path: &str) -> Result<(), String> {
                self.0.lock().unwrap().push(format!("unstage {path}"));
                Ok(())
            }
            fn commit(&self, _: &Path, _: &str) -> Result<(), String> {
                Ok(())
            }
            fn list_branches(&self, _: &Path) -> Vec<String> {
                vec![]
            }
            fn checkout(&self, _: &Path, _: &str) -> Result<(), String> {
                Ok(())
            }
            fn fetch(&self, _: &Path) -> Result<(), String> {
                Ok(())
            }
            fn pull(&self, _: &Path) -> Result<(), String> {
                Ok(())
            }
            fn push(&self, _: &Path) -> Result<(), String> {
                Ok(())
            }
        }
        let io = Arc::new(Io(Mutex::new(vec![])));
        let mut panel = GitDiffPanel::new();
        panel.set_io_provider(io.clone());
        {
            let mut data = panel.data.lock().unwrap();
            data.repo_root = Some(PathBuf::from("/repo"));
            data.files = ["src/a.rs", "src/nested/b.rs", "src-other/c.rs"]
                .into_iter()
                .map(|path| FileChange {
                    path: path.into(),
                    status: FileStatus::Modified,
                    additions: 0,
                    deletions: 0,
                    staged: false,
                })
                .collect();
        }
        panel.rebuild_visual_rows();
        panel.toggle_folder_stage(0);
        {
            let mut data = panel.data.lock().unwrap();
            for file in &mut data.files {
                file.staged = true;
            }
        }
        panel.toggle_folder_stage(0);
        assert_eq!(
            *io.0.lock().unwrap(),
            [
                "stage src/a.rs",
                "stage src/nested/b.rs",
                "stage src-other/c.rs",
                "unstage src/a.rs",
                "unstage src/nested/b.rs",
                "unstage src-other/c.rs",
            ]
        );
        io.0.lock().unwrap().clear();
        {
            let mut data = panel.data.lock().unwrap();
            data.files[1].status = FileStatus::Mixed;
            data.files.push(FileChange {
                path: "src/new.rs".into(),
                status: FileStatus::Untracked,
                additions: 0,
                deletions: 0,
                staged: false,
            });
        }
        panel.rebuild_visual_rows();
        let src = panel.visual_rows.iter().position(|row| matches!(
            &row.kind, VisualRowKind::Group { label, .. } if label == "Tracked/src"
        )).unwrap();
        panel.toggle_folder(src);
        panel.toggle_folder_stage(src);
        assert_eq!(*io.0.lock().unwrap(), ["stage src/nested/b.rs"]);
        io.0.lock().unwrap().clear();
        panel.data.lock().unwrap().files[1].status = FileStatus::Staged;
        panel.toggle_folder_stage(src);
        assert_eq!(
            *io.0.lock().unwrap(),
            ["unstage src/a.rs", "unstage src/nested/b.rs"]
        );
    }

    #[test]
    fn pointer_and_folder_actions_do_not_reveal_keyboard_selection() {
        let mut panel = GitDiffPanel::new();
        panel.data.lock().unwrap().files = (0..20)
            .map(|i| FileChange {
                path: format!("dir/file-{i:02}.rs"),
                status: FileStatus::Modified,
                additions: 1,
                deletions: 0,
                staged: false,
            })
            .collect();
        panel.files_body_rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 200.0,
            h: 96.0,
        };
        panel.rebuild_visual_rows();
        panel.file_scroll = 300.0;
        panel.select_file(0);
        assert_eq!(panel.file_scroll, 300.0);
        panel.toggle_folder(0);
        assert_eq!(panel.file_scroll, 300.0);
        panel.toggle_folder(0);
        assert_eq!(panel.file_scroll, 300.0);
        panel.select_next();
        assert_eq!(panel.selected, 1);
        // Section + directory now precede the leaves; keyboard scroll-off
        // includes both headers instead of pretending this is a flat list.
        assert_eq!(panel.file_scroll, FILE_ROW_HEIGHT);
    }
}
