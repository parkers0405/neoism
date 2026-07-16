use std::collections::HashSet;
use std::path::{Path, PathBuf};
use web_time::Instant;

use crate::event::{
    KeyState, LogicalKey, Modifiers, NamedKey, PointerButton, UiEvent, WheelMode,
};
use crate::layout::Rect;
use crate::panels::PanelContext;
use crate::services::{DirEntry, IoError, RequestId};

use super::git::git_statuses_for;
use super::scan::{
    apply_git_statuses, entries_from_dir_listing, normalize_path, same_entry_layout,
    scan_dir_result,
};
use super::state::{FileTree, RevealFlash};
use super::types::{
    FileTreeGitRefreshRequest, FileTreeGitRefreshResult, GitStatus, NodeKind,
    PendingDirKind, PendingDirRequest, TreeEntry,
};
use super::virtuals::{
    is_workspace_note_path, scan_root_with_workspace, virtual_workspace_path,
    workspace_virtual_children,
};
use super::{
    CURSOR_ANIMATION_LENGTH, FILE_TREE_MAX_WIDTH, FILE_TREE_MIN_WIDTH,
    FILE_TREE_RESIZE_STEP, FRAME_STROKE, REVEAL_FLASH_MS, ROW_HEIGHT,
    SCROLL_ANIMATION_LENGTH, SCROLL_OFF_ROWS,
};

impl FileTree {
    /// Mark `path` as the buffer nvim currently has open, so its row
    /// gets the active-buffer accent. Pass `None` to clear (e.g. when
    /// switching to a terminal pane).
    pub fn set_active_path(&mut self, path: Option<PathBuf>) {
        self.active_path = path;
    }

    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale.clamp(0.5, 3.0);
        self.clear_label_truncation_cache();
        self.scroll.reset();
        self.cursor_spring.reset();
    }

    /// Effective row height in logical pixels (base * scale).
    pub fn row_height(&self) -> f32 {
        ROW_HEIGHT * self.scale
    }

    pub fn visible_rows_for_panel_height(&self, panel_height: f32) -> usize {
        let frame_stroke = (FRAME_STROKE * self.scale).max(2.0);
        let content_h = (panel_height - frame_stroke * 2.0).max(0.0);
        self.rows_per_panel(content_h).max(1)
    }

    /// Current panel width in logical pixels.
    pub fn width(&self) -> f32 {
        self.width
    }

    /// Set an absolute panel width in logical pixels.
    pub fn set_width(&mut self, width: f32) {
        let next = width.clamp(FILE_TREE_MIN_WIDTH, FILE_TREE_MAX_WIDTH);
        if self.width.to_bits() != next.to_bits() {
            self.clear_label_truncation_cache();
            self.width = next;
        }
    }

    /// Resize the panel by `delta` logical pixels (positive = wider).
    /// Clamped to `[FILE_TREE_MIN_WIDTH, FILE_TREE_MAX_WIDTH]`.
    pub fn resize(&mut self, delta: f32) {
        self.set_width(self.width + delta);
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn set_visible(&mut self, v: bool) {
        self.visible = v;
        if !v {
            self.scroll.reset();
            self.cursor_spring.reset();
            self.wheel_accumulator = 0.0;
            self.selected_cursor_rect = None;
        }
    }

    #[allow(dead_code)]
    pub fn toggle_visible(&mut self) {
        self.visible = !self.visible;
    }

    pub fn set_focused(&mut self, f: bool) {
        self.focused = f;
        if !f {
            self.clear_pending();
        }
    }

    pub fn is_focused(&self) -> bool {
        self.focused
    }

    pub fn set_entries(&mut self, entries: Vec<TreeEntry>) {
        if self.selected >= entries.len() {
            self.selected = entries.len().saturating_sub(1);
        }
        if self
            .reveal_flash
            .as_ref()
            .is_some_and(|flash| flash.index >= entries.len())
        {
            self.reveal_flash = None;
        }
        self.entries = entries;
        self.scroll.reset();
        self.cursor_spring.reset();
        self.wheel_accumulator = 0.0;
        self.clamp_scroll(self.last_panel_height_rows);
    }

    /// Entry replacement for LIVE refreshes (fs-watch pushes, git
    /// refresh, remote relists): keeps the user's scroll position,
    /// in-flight scroll animation, and wheel residual instead of
    /// snapping the viewport back to `scroll_top`'s resting frame.
    /// A background relist landing mid-gesture must be invisible.
    pub fn set_entries_preserve_scroll(&mut self, entries: Vec<TreeEntry>) {
        if self.selected >= entries.len() {
            self.selected = entries.len().saturating_sub(1);
        }
        if self
            .reveal_flash
            .as_ref()
            .is_some_and(|flash| flash.index >= entries.len())
        {
            self.reveal_flash = None;
        }
        self.entries = entries;
        self.clamp_scroll(self.last_panel_height_rows);
    }

    fn track_pending_dir(
        &mut self,
        id: RequestId,
        path: PathBuf,
        depth: u8,
        kind: PendingDirKind,
    ) {
        self.pending_dir_requests.insert(
            id,
            PendingDirRequest {
                path: normalize_path(&path),
                depth,
                kind,
            },
        );
    }

    /// Replace the git-status map and re-badge the EXISTING rows in
    /// place — no filesystem rescan. Remote (daemon-served) trees use
    /// this with statuses fetched over the git plane; a rescan-based
    /// refresh would blank them (listings are async there).
    pub fn apply_git_statuses_map(
        &mut self,
        root: &Path,
        git_statuses: std::collections::HashMap<PathBuf, GitStatus>,
    ) -> bool {
        if self.root.as_deref() != Some(&normalize_path(root)) {
            return false;
        }
        self.git_statuses = git_statuses;
        apply_git_statuses(&mut self.entries, &self.git_statuses)
    }

    /// Live re-list for async (daemon-served) trees: request fresh
    /// listings for the root and every open dir WITHOUT touching the
    /// current entries — each `Pending` reply splices in through
    /// [`Self::handle_service_reply`], whose Root branch preserves
    /// expansion. `refresh` is wrong for this: its scan swallows
    /// `Pending` into empty listings and would blank the tree.
    pub fn relist_open_dirs(&mut self, ctx: &PanelContext) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let open_dirs: Vec<(PathBuf, u8)> = self
            .entries
            .iter()
            .filter(|entry| matches!(entry.kind, NodeKind::Dir { open: true }))
            .filter_map(|entry| entry.path.clone().map(|path| (path, entry.depth)))
            .collect();
        match ctx.services.files.list_dir(&root) {
            Err(IoError::Pending(id)) => {
                self.track_pending_dir(id, root.clone(), 0, PendingDirKind::Root);
            }
            Ok(read) => {
                // Synchronous source (local fs): a plain refresh does
                // the whole job in one pass.
                let _ = read;
                self.refresh(ctx);
                return;
            }
            Err(_) => {}
        }
        for (path, depth) in open_dirs {
            match ctx.services.files.list_dir(&path) {
                Err(IoError::Pending(id)) => {
                    self.track_pending_dir(id, path, depth + 1, PendingDirKind::Expand);
                }
                // Synchronous listings were already covered by the
                // refresh above; other errors degrade to "keep what we
                // have".
                _ => {}
            }
        }
    }

    fn parse_dir_entries(payload: &serde_json::Value) -> Option<Vec<DirEntry>> {
        if let Ok(entries) = serde_json::from_value::<Vec<DirEntry>>(payload.clone()) {
            return Some(entries);
        }
        if let Some(entries) = payload.get("entries") {
            if let Ok(entries) = serde_json::from_value::<Vec<DirEntry>>(entries.clone())
            {
                return Some(entries);
            }
        }
        if let Some(entries) = payload
            .get("DirListing")
            .and_then(|listing| listing.get("entries"))
        {
            if let Ok(entries) = serde_json::from_value::<Vec<DirEntry>>(entries.clone())
            {
                return Some(entries);
            }
        }
        None
    }

    pub fn handle_service_reply(
        &mut self,
        request_id: RequestId,
        payload: &serde_json::Value,
    ) -> bool {
        let Some(request) = self.pending_dir_requests.remove(&request_id) else {
            return false;
        };
        let Some(entries) = Self::parse_dir_entries(payload) else {
            return false;
        };
        let children = entries_from_dir_listing(
            &request.path,
            request.depth,
            &self.git_statuses,
            entries,
        );

        match request.kind {
            PendingDirKind::Root => {
                if self.root.as_deref() != Some(request.path.as_path()) {
                    return false;
                }
                // A re-root queued this listing (remote/tailnet join):
                // the rows are about to appear — run the reveal now.
                if std::mem::take(&mut self.root_transition_armed) {
                    self.root_transition_started = Some(Instant::now());
                }
                // Carry expansion across the replace: a live re-list
                // (daemon fs-watch push) must not collapse every open
                // dir. Dirs that were open stay open and keep their
                // previous subtree rows; a concurrent Expand re-list
                // for each open dir splices fresh children in.
                let open_dirs: std::collections::HashSet<PathBuf> = self
                    .entries
                    .iter()
                    .filter(|entry| matches!(entry.kind, NodeKind::Dir { open: true }))
                    .filter_map(|entry| entry.path.clone())
                    .collect();
                if open_dirs.is_empty() {
                    // Preserve scroll and keep the selection on the same
                    // PATH — remote roots re-list repeatedly (fs-watch
                    // pushes, liveness re-checks) and a plain set_entries
                    // snapped the viewport back to the top on every reply,
                    // which made scrolling a joined tree fight the user.
                    let selected_path =
                        self.selected().and_then(|entry| entry.path.clone());
                    self.set_entries_preserve_scroll(children);
                    if let Some(path) = selected_path {
                        if let Some(ix) = self.entries.iter().position(|entry| {
                            entry.path.as_deref() == Some(path.as_path())
                        }) {
                            self.selected = ix;
                        }
                    }
                    return true;
                }
                let mut merged = Vec::with_capacity(self.entries.len());
                for mut child in children {
                    let child_path = child.path.clone();
                    let reopen = child_path
                        .as_ref()
                        .is_some_and(|path| open_dirs.contains(path));
                    if reopen {
                        child.kind = NodeKind::Dir { open: true };
                    }
                    merged.push(child);
                    if reopen {
                        if let Some(path) = child_path {
                            if let Some(ix) = self.entries.iter().position(|entry| {
                                entry.path.as_deref() == Some(path.as_path())
                            }) {
                                let depth = self.entries[ix].depth;
                                let mut end = ix + 1;
                                while end < self.entries.len()
                                    && self.entries[end].depth > depth
                                {
                                    end += 1;
                                }
                                merged.extend(self.entries[ix + 1..end].iter().cloned());
                            }
                        }
                    }
                }
                let selected_path = self.selected().and_then(|entry| entry.path.clone());
                self.set_entries_preserve_scroll(merged);
                // Row indices shift when the host adds/removes files —
                // keep the SELECTION on the same path, not the same
                // index, so a live relist never moves the cursor row.
                if let Some(path) = selected_path {
                    if let Some(ix) = self
                        .entries
                        .iter()
                        .position(|entry| entry.path.as_deref() == Some(path.as_path()))
                    {
                        self.selected = ix;
                    }
                }
                true
            }
            PendingDirKind::Expand => {
                let Some(parent_ix) = self.entries.iter().position(|entry| {
                    entry.path.as_deref() == Some(request.path.as_path())
                }) else {
                    return false;
                };
                let parent_depth = self.entries[parent_ix].depth;
                if !matches!(self.entries[parent_ix].kind, NodeKind::Dir { open: true }) {
                    return false;
                }
                let mut end = parent_ix + 1;
                while end < self.entries.len() && self.entries[end].depth > parent_depth {
                    end += 1;
                }
                self.entries.splice(parent_ix + 1..end, children);
                if self.selected >= self.entries.len() {
                    self.selected = self.entries.len().saturating_sub(1);
                    self.cursor_spring.reset();
                }
                self.clamp_scroll(self.last_panel_height_rows);
                true
            }
        }
    }

    /// Replace entries with a single-level scan of `root`. Directories
    /// sort first, then files; both are alphabetical (case-insensitive).
    /// Hidden entries (leading `.`) are skipped — matches nvim-tree's
    /// default. Errors and unreadable entries are silently skipped: the
    /// tree degrades to "empty panel" rather than refusing to render.
    pub fn populate_from_dir(&mut self, root: &Path, ctx: &PanelContext) {
        let root = normalize_path(root);
        let root_changed = self.root.as_deref() != Some(root.as_path());
        // A remote root listing for this exact root is already in flight
        // — re-dispatching just queues another reply that will stomp the
        // tree again (and again, via the liveness re-check timer).
        if !root_changed
            && self.pending_dir_requests.values().any(|request| {
                matches!(request.kind, PendingDirKind::Root) && request.path == root
            })
        {
            return;
        }
        if root_changed {
            self.selected = 0;
            self.scroll_top = 0;
            self.reveal_flash = None;
            // Statuses are absolute paths. Never let badges from the old
            // workspace appear while the new root's worker is running.
            self.git_statuses.clear();
            self.begin_root_transition();
        }
        // Deliberately do not call `git_statuses_for` here. Native Git can
        // spend minutes enumerating untracked files in a large repository;
        // the desktop bridge immediately launches `git_refresh_request` on
        // its bounded worker and applies the result back to this tree. Keep
        // cached statuses when repopulating the same root so badges do not
        // flicker while that refresh is in flight.
        self.root = Some(root.clone());
        match scan_dir_result(&root, 0, &self.git_statuses, ctx.services.files) {
            Ok(_) => {
                let entries = scan_root_with_workspace(
                    &root,
                    &self.git_statuses,
                    &self.open_dirs(),
                    false,
                    ctx.services.files,
                );
                self.set_entries(entries);
            }
            Err(IoError::Pending(id)) => {
                self.track_pending_dir(id, root, 0, PendingDirKind::Root);
                if self.entries.is_empty() {
                    self.set_entries(Vec::new());
                }
                if root_changed {
                    // Remote listing in flight — sweep when it lands, not
                    // over the skeleton.
                    self.root_transition_started = None;
                    self.root_transition_armed = true;
                }
            }
            Err(_) => self.set_entries(Vec::new()),
        }
    }

    /// Start the staggered row-reveal sweep — the tree-equivalent of the
    /// status line's mode-swap transition. Called automatically when a
    /// populate re-roots the tree; hosts call it directly when they swap
    /// in a whole preserved tree (per-workspace tree caches, server
    /// switches) where the root inside the struct never changes.
    pub fn begin_root_transition(&mut self) {
        self.root_transition_started = Some(Instant::now());
        self.root_transition_armed = false;
    }

    /// Refresh the visible tree without discarding open folders where
    /// possible. Used after filesystem operations and git-status changes.
    pub fn refresh(&mut self, ctx: &PanelContext) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let open_dirs: HashSet<PathBuf> = self
            .entries
            .iter()
            .filter_map(|entry| match entry.kind {
                NodeKind::Dir { open: true } => {
                    entry.path.as_ref().map(|path| normalize_path(path))
                }
                _ => None,
            })
            .collect();
        let selected_path = self.selected().and_then(|entry| entry.path.clone());

        self.git_statuses = git_statuses_for(&root, ctx.services.git);
        let entries = scan_root_with_workspace(
            &root,
            &self.git_statuses,
            &open_dirs,
            false,
            ctx.services.files,
        );
        self.set_entries_preserve_scroll(entries);
        if let Some(path) = selected_path {
            if let Some(ix) = self
                .entries
                .iter()
                .position(|entry| entry.path.as_deref() == Some(path.as_path()))
            {
                self.selected = ix;
            }
        }
        self.clamp_scroll(self.last_panel_height_rows);
    }

    /// Refresh only git badges on currently visible rows. Cheaper than
    /// rebuilding the tree and enough after nvim writes a buffer.
    #[allow(dead_code)]
    pub fn refresh_git_status(&mut self, ctx: &PanelContext) -> bool {
        let Some(root) = self.root.clone() else {
            return false;
        };
        let open_dirs = self.open_dirs();
        let selected_path = self.selected().and_then(|entry| entry.path.clone());
        self.git_statuses = git_statuses_for(&root, ctx.services.git);
        let next_entries = scan_root_with_workspace(
            &root,
            &self.git_statuses,
            &open_dirs,
            false,
            ctx.services.files,
        );
        if !same_entry_layout(&self.entries, &next_entries) {
            self.set_entries(next_entries);
            if let Some(path) = selected_path {
                if let Some(ix) = self
                    .entries
                    .iter()
                    .position(|entry| entry.path.as_deref() == Some(path.as_path()))
                {
                    self.selected = ix;
                }
            }
            self.clamp_scroll(self.last_panel_height_rows);
            return true;
        }
        apply_git_statuses(&mut self.entries, &self.git_statuses)
    }

    pub fn git_refresh_request(&self) -> Option<FileTreeGitRefreshRequest> {
        Some(FileTreeGitRefreshRequest {
            root: self.root.clone()?,
            open_dirs: self.open_dirs(),
            default_open_workspace: false,
        })
    }

    pub fn run_git_refresh_request(
        request: FileTreeGitRefreshRequest,
        ctx: &PanelContext,
    ) -> FileTreeGitRefreshResult {
        let git_statuses = git_statuses_for(&request.root, ctx.services.git);
        let entries = scan_root_with_workspace(
            &request.root,
            &git_statuses,
            &request.open_dirs,
            request.default_open_workspace,
            ctx.services.files,
        );
        FileTreeGitRefreshResult {
            root: request.root,
            git_statuses,
            entries,
        }
    }

    pub fn apply_git_refresh_result(&mut self, result: FileTreeGitRefreshResult) -> bool {
        if self.root.as_deref() != Some(result.root.as_path()) {
            return false;
        }

        let selected_path = self.selected().and_then(|entry| entry.path.clone());
        let FileTreeGitRefreshResult {
            git_statuses,
            entries,
            ..
        } = result;

        self.git_statuses = git_statuses;
        if !same_entry_layout(&self.entries, &entries) {
            self.set_entries(entries);
            if let Some(path) = selected_path {
                if let Some(ix) = self
                    .entries
                    .iter()
                    .position(|entry| entry.path.as_deref() == Some(path.as_path()))
                {
                    self.selected = ix;
                }
            }
            self.clamp_scroll(self.last_panel_height_rows);
            return true;
        }

        apply_git_statuses(&mut self.entries, &self.git_statuses)
    }

    /// Toggle expand/collapse on the entry at `index`. Files are a no-op.
    /// Opening: walks the directory and inserts children at depth+1
    /// right after the parent. Closing: removes contiguous descendant
    /// rows. Returns the path of the toggled directory if it was a dir
    /// (useful for caller damage-tracking); `None` for files.
    pub fn toggle_dir_at(&mut self, index: usize, ctx: &PanelContext) -> Option<PathBuf> {
        let (parent_depth, parent_path, was_open) = {
            let entry = self.entries.get(index)?;
            let open = match entry.kind {
                NodeKind::Dir { open } => open,
                NodeKind::File => return None,
            };
            (entry.depth, entry.path.clone()?, open)
        };

        if was_open {
            // Collapse: drop all descendants — contiguous run after
            // `index` whose depth > parent_depth.
            let mut end = index + 1;
            while end < self.entries.len() && self.entries[end].depth > parent_depth {
                end += 1;
            }
            self.entries.drain(index + 1..end);
            if let Some(e) = self.entries.get_mut(index) {
                e.kind = NodeKind::Dir { open: false };
            }
        } else {
            if self.entries[index].is_neoism_workspace_virtual_root() {
                if let Some(root) = self.root.clone() {
                    let children = workspace_virtual_children(
                        &root,
                        parent_depth + 1,
                        &self.git_statuses,
                        &self.open_dirs(),
                        ctx.services.files,
                    );
                    for (i, child) in children.into_iter().enumerate() {
                        self.entries.insert(index + 1 + i, child);
                    }
                }
            } else {
                // Expand: scan the directory and splice children in.
                match scan_dir_result(
                    &parent_path,
                    parent_depth + 1,
                    &self.git_statuses,
                    ctx.services.files,
                ) {
                    Ok(children) => {
                        for (i, child) in children.into_iter().enumerate() {
                            self.entries.insert(index + 1 + i, child);
                        }
                    }
                    Err(IoError::Pending(id)) => {
                        self.track_pending_dir(
                            id,
                            parent_path.clone(),
                            parent_depth + 1,
                            PendingDirKind::Expand,
                        );
                    }
                    Err(_) => {}
                }
            }
            if let Some(e) = self.entries.get_mut(index) {
                e.kind = NodeKind::Dir { open: true };
            }
        }
        // Selection clamp for the case where we collapsed past it.
        if self.selected >= self.entries.len() {
            self.selected = self.entries.len().saturating_sub(1);
            self.cursor_spring.reset();
        }
        self.clamp_scroll(self.last_panel_height_rows);
        Some(parent_path)
    }

    pub(super) fn open_dirs(&self) -> HashSet<PathBuf> {
        self.entries
            .iter()
            .filter_map(|entry| match entry.kind {
                NodeKind::Dir { open: true } => {
                    entry.path.as_ref().map(|path| normalize_path(path))
                }
                _ => None,
            })
            .collect()
    }

    pub fn entries(&self) -> &[TreeEntry] {
        &self.entries
    }

    /// Compatibility accessor for the slim file-tree surface. Newer
    /// callers can use [`entries`](Self::entries); `nodes` is kept for
    /// tests and consumers that still use the old panel vocabulary.
    pub fn nodes(&self) -> &[TreeEntry] {
        self.entries()
    }

    /// Apply a single directory listing under `path`.
    ///
    /// When `path` is the tree root this replaces the top-level rows.
    /// When `path` is an open directory already present in the tree,
    /// its visible descendants are replaced in place.
    pub fn apply_listing(&mut self, path: &Path, listing: Vec<DirEntry>) {
        let path = normalize_path(path);
        if self.root.is_none() {
            self.root = Some(path.clone());
        }
        let depth = if self.root.as_deref() == Some(path.as_path()) {
            0
        } else {
            self.entries
                .iter()
                .find(|entry| entry.path.as_deref() == Some(path.as_path()))
                .map(|entry| entry.depth + 1)
                .unwrap_or(0)
        };
        let children =
            entries_from_dir_listing(&path, depth, &self.git_statuses, listing);

        if self.root.as_deref() == Some(path.as_path()) {
            self.set_entries(children);
            return;
        }

        let Some(parent_ix) = self
            .entries
            .iter()
            .position(|entry| entry.path.as_deref() == Some(path.as_path()))
        else {
            return;
        };
        let parent_depth = self.entries[parent_ix].depth;
        let mut end = parent_ix + 1;
        while end < self.entries.len() && self.entries[end].depth > parent_depth {
            end += 1;
        }
        self.entries.splice(parent_ix + 1..end, children);
        if self.selected >= self.entries.len() {
            self.selected = self.entries.len().saturating_sub(1);
            self.cursor_spring.reset();
        }
        self.clamp_scroll(self.last_panel_height_rows);
    }

    pub fn selected_path(&self) -> Option<&Path> {
        self.selected().and_then(|entry| entry.path.as_deref())
    }

    pub fn pending_len(&self) -> usize {
        self.pending_dir_requests.len()
    }

    /// True while a directory listing is in flight and there is
    /// nothing to show yet — the window between a (remote) workspace
    /// switch and the daemon's first `DirListing` reply. Native local
    /// scans are synchronous and never enter this state.
    pub fn is_loading(&self) -> bool {
        self.entries.is_empty() && !self.pending_dir_requests.is_empty()
    }

    pub fn is_expanded(&self, path: &Path) -> bool {
        let path = normalize_path(path);
        self.entries.iter().any(|entry| {
            entry.path.as_deref() == Some(path.as_path())
                && matches!(entry.kind, NodeKind::Dir { open: true })
        })
    }

    pub fn open_dir(&mut self, path: &Path, ctx: &PanelContext) -> bool {
        let path = normalize_path(path);
        let Some(index) = self.entries.iter().position(|entry| {
            entry.path.as_deref() == Some(path.as_path())
                && matches!(entry.kind, NodeKind::Dir { .. })
        }) else {
            return false;
        };
        if matches!(self.entries[index].kind, NodeKind::Dir { open: true }) {
            return true;
        }
        self.toggle_dir_at(index, ctx).is_some()
    }

    pub fn close_dir(&mut self, path: &Path) -> bool {
        let path = normalize_path(path);
        let Some(index) = self.entries.iter().position(|entry| {
            entry.path.as_deref() == Some(path.as_path())
                && matches!(entry.kind, NodeKind::Dir { open: true })
        }) else {
            return false;
        };
        let parent_depth = self.entries[index].depth;
        let mut end = index + 1;
        while end < self.entries.len() && self.entries[end].depth > parent_depth {
            end += 1;
        }
        self.entries.drain(index + 1..end);
        self.entries[index].kind = NodeKind::Dir { open: false };
        if self.selected >= self.entries.len() {
            self.selected = self.entries.len().saturating_sub(1);
            self.cursor_spring.reset();
        }
        self.clamp_scroll(self.last_panel_height_rows);
        true
    }

    pub fn reveal_directory(&mut self, path: &Path, ctx: &PanelContext) -> Option<usize> {
        let target = normalize_path(path);
        let root = self.root.clone()?;
        let mut current = if is_workspace_note_path(&root, &target) {
            virtual_workspace_path(&root)
        } else {
            root.clone()
        };
        let relative = target.strip_prefix(&current).ok()?;
        if relative.as_os_str().is_empty() {
            return None;
        }

        let mut target_index = None;
        for component in relative.components() {
            current.push(component.as_os_str());
            let idx = self
                .entries
                .iter()
                .position(|entry| entry.path.as_deref() == Some(current.as_path()))?;
            let is_target = current == target;
            if !is_target && self.entries[idx].kind == (NodeKind::Dir { open: false }) {
                self.toggle_dir_at(idx, ctx);
            }
            if is_target {
                target_index = Some(idx);
                break;
            }
        }

        let idx = target_index?;
        if self.entries[idx].kind == (NodeKind::Dir { open: false }) {
            self.toggle_dir_at(idx, ctx);
        }
        self.set_selected(idx);
        self.reveal_flash = Some(RevealFlash {
            index: idx,
            started: Instant::now(),
        });
        self.clamp_scroll(self.last_panel_height_rows);
        Some(idx)
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn selected(&self) -> Option<&TreeEntry> {
        self.entries.get(self.selected)
    }

    pub fn selected_cursor_rect(&self) -> Option<[f32; 4]> {
        self.selected_cursor_rect
    }

    pub fn select_next(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        self.move_selection_to((self.selected + 1).min(self.entries.len() - 1));
    }

    pub fn select_prev(&mut self) {
        self.move_selection_to(self.selected.saturating_sub(1));
    }

    /// Vim-style half-page jump down: move selection by `n` rows,
    /// clamped to the last entry. Wired to Ctrl+D in tree mode.
    pub fn select_next_by(&mut self, n: usize) {
        if self.entries.is_empty() {
            return;
        }
        self.move_selection_to(
            self.selected.saturating_add(n).min(self.entries.len() - 1),
        );
    }

    /// Vim-style half-page jump up: move selection by `n` rows,
    /// clamped to row 0. Wired to Ctrl+U in tree mode.
    pub fn select_prev_by(&mut self, n: usize) {
        self.move_selection_to(self.selected.saturating_sub(n));
    }

    /// Jump to the first row (vim `gg` / `1`).
    pub fn select_first(&mut self) {
        self.clear_pending();
        if !self.entries.is_empty() {
            self.move_selection_to(0);
        }
    }

    /// Jump to the last row (vim `$` / `G`).
    pub fn select_last(&mut self) {
        self.clear_pending();
        if !self.entries.is_empty() {
            self.move_selection_to(self.entries.len().saturating_sub(1));
        }
    }

    /// Jump to a 1-based row (vim `<count>G`), clamped to the last row.
    pub fn goto_row(&mut self, one_based: usize) {
        self.clear_pending();
        if !self.entries.is_empty() {
            self.move_selection_to(one_based.saturating_sub(1));
        }
    }

    /// Feed a typed digit into the pending vim count. A leading `0` with
    /// no count in progress is ignored (in vim `0` is a motion). Returns
    /// true when the digit was absorbed.
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

    /// Register a `g` keypress. Returns true when it completes a `gg`
    /// (caller jumps to the top); false when it merely arms the first `g`.
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

    /// Move the selection to a specific row index. Mouse-click hit
    /// tests use this to put the cursor on the row the user actually
    /// pointed at before activating.
    pub fn set_selected(&mut self, row: usize) {
        if self.entries.is_empty() {
            return;
        }
        self.move_selection_to(row.min(self.entries.len() - 1));
    }

    fn move_selection_to(&mut self, new_selected: usize) {
        if self.entries.is_empty() {
            return;
        }
        let new_selected = new_selected.min(self.entries.len() - 1);
        if new_selected == self.selected {
            return;
        }
        self.reveal_flash = None;

        let was_idle = self.cursor_spring.position == 0.0;
        let rows = self.selected as i32 - new_selected as i32;
        self.cursor_spring.position += rows as f32 * self.row_height();
        if was_idle {
            self.last_cursor_frame = Instant::now();
        }
        self.selected = new_selected;
        self.clamp_scroll(self.last_panel_height_rows);
    }

    /// Bump scroll_top by `delta` rows in either direction. Used by
    /// the wheel handler in `screen`; if the viewport actually moves,
    /// the selection is kept inside the scrolloff band.
    pub fn scroll_by(&mut self, delta: i32, panel_height_rows: usize) {
        let old = self.scroll_top;
        let max_top = self.max_scroll_top(panel_height_rows);
        if delta < 0 {
            self.scroll_top = self
                .scroll_top
                .saturating_sub(delta.unsigned_abs() as usize);
        } else {
            self.scroll_top = self.scroll_top.saturating_add(delta as usize).min(max_top);
        }
        if old != self.scroll_top {
            self.reveal_flash = None;
            self.push_scroll_lag(old, self.scroll_top);
        }
    }

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
        let max_top = self.max_scroll_top(panel_height_rows);
        if (self.scroll_top == 0 && self.wheel_accumulator > 0.0)
            || (self.scroll_top == max_top && self.wheel_accumulator < 0.0)
        {
            self.wheel_accumulator = 0.0;
        }
    }

    fn set_scroll_top(&mut self, new_top: usize) {
        let old = self.scroll_top;
        self.scroll_top = new_top;
        self.push_scroll_lag(old, self.scroll_top);
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

    fn scrolloff_for(panel_height_rows: usize) -> usize {
        if panel_height_rows <= 2 {
            return 0;
        }
        SCROLL_OFF_ROWS.min(panel_height_rows.saturating_sub(1) / 2)
    }

    pub(super) fn tick_scroll(&mut self) -> f32 {
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

    pub(super) fn tick_cursor(&mut self) -> f32 {
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

    pub fn is_animating(&self) -> bool {
        let reveal_animating = self.reveal_flash.as_ref().is_some_and(|flash| {
            Instant::now()
                .saturating_duration_since(flash.started)
                .as_secs_f32()
                * 1000.0
                < REVEAL_FLASH_MS
        });
        self.visible
            && (self.scroll.position != 0.0
                || self.cursor_spring.position != 0.0
                || reveal_animating
                || self.root_transition_started.is_some()
                || self.is_loading())
    }

    /// Number of rows that fit in `panel_height` logical pixels.
    pub(super) fn rows_per_panel(&self, panel_height: f32) -> usize {
        let row_h = self.row_height();
        if row_h <= 0.0 {
            return 0;
        }
        (panel_height / row_h).floor().max(0.0) as usize
    }

    /// Keep `selected` inside the visible window. Caller passes the
    /// current panel pixel height; the scroll cursor adjusts so the
    /// selection is on screen, with no jumping when it already is.
    pub(super) fn clamp_scroll(&mut self, panel_height_rows: usize) {
        if self.entries.is_empty() {
            self.scroll_top = 0;
            return;
        }
        if panel_height_rows == 0 {
            return;
        }
        let scrolloff = Self::scrolloff_for(panel_height_rows);
        if self.selected < self.scroll_top.saturating_add(scrolloff) {
            self.set_scroll_top(self.selected.saturating_sub(scrolloff));
        } else if self.selected.saturating_add(scrolloff)
            >= self.scroll_top.saturating_add(panel_height_rows)
        {
            self.set_scroll_top(self.selected + scrolloff + 1 - panel_height_rows);
        }
        let max_top = self.max_scroll_top(panel_height_rows);
        if self.scroll_top > max_top {
            self.set_scroll_top(max_top);
        }
    }

    pub(super) fn clamp_scroll_bounds(&mut self, panel_height_rows: usize) {
        if self.entries.is_empty() {
            self.scroll_top = 0;
            return;
        }
        let max_top = self.max_scroll_top(panel_height_rows);
        if self.scroll_top > max_top {
            self.set_scroll_top(max_top);
        }
    }

    pub(super) fn max_scroll_top(&self, panel_height_rows: usize) -> usize {
        let visible = panel_height_rows.max(1);
        self.entries.len().saturating_sub(visible)
    }

    pub fn handle_ui_event(
        &mut self,
        event: &UiEvent,
        ctx: &PanelContext,
        bounds: Option<Rect>,
    ) -> bool {
        match event {
            UiEvent::ServiceReply {
                request_id,
                payload,
            } => self.handle_service_reply(*request_id, payload),
            UiEvent::PointerDown {
                button: PointerButton::Left,
                x,
                y,
                click_count,
                ..
            } => {
                let Some(bounds) = bounds else {
                    return false;
                };
                self.set_focused(true);
                let Some(row) = self
                    .hit_test_in_bounds(*x, *y, bounds.x, bounds.y, bounds.w, bounds.h)
                else {
                    return true;
                };
                self.set_selected(row);
                if *click_count >= 1 {
                    self.activate_selected(ctx);
                }
                true
            }
            UiEvent::Wheel { dy, mode, .. } => {
                let Some(bounds) = bounds else {
                    return false;
                };
                let rows = self.visible_rows_for_panel_height(bounds.h);
                let pixels = match mode {
                    WheelMode::Pixel => -*dy,
                    WheelMode::Line => -*dy * self.row_height(),
                    WheelMode::Page => -*dy * bounds.h,
                };
                self.scroll_pixels(pixels, rows);
                true
            }
            UiEvent::Key(key) if key.state == KeyState::Pressed && self.is_focused() => {
                let alt = key.modifiers.contains(Modifiers::ALT);
                let ctrl = key.modifiers.contains(Modifiers::CTRL);
                let meta = key.modifiers.contains(Modifiers::META);
                let plain = !alt && !ctrl && !meta;
                let rows_visible = bounds
                    .map(|bounds| self.visible_rows_for_panel_height(bounds.h))
                    .unwrap_or(self.last_panel_height_rows)
                    .max(1);
                let half_page = (rows_visible / 2).max(1);
                match &key.logical {
                    LogicalKey::Named(NamedKey::ArrowLeft) if alt && ctrl => {
                        self.resize(-FILE_TREE_RESIZE_STEP);
                        true
                    }
                    LogicalKey::Named(NamedKey::ArrowRight) if alt && ctrl => {
                        self.resize(FILE_TREE_RESIZE_STEP);
                        true
                    }
                    LogicalKey::Named(NamedKey::ArrowDown) => {
                        self.select_next();
                        true
                    }
                    LogicalKey::Named(NamedKey::ArrowUp) => {
                        self.select_prev();
                        true
                    }
                    LogicalKey::Named(NamedKey::PageDown) => {
                        self.select_next_by(half_page);
                        true
                    }
                    LogicalKey::Named(NamedKey::PageUp) => {
                        self.select_prev_by(half_page);
                        true
                    }
                    LogicalKey::Named(NamedKey::Enter)
                    | LogicalKey::Named(NamedKey::Space) => self.activate_selected(ctx),
                    LogicalKey::Named(NamedKey::Escape) => {
                        self.set_focused(false);
                        true
                    }
                    LogicalKey::Character(ch) if ch.as_str() == "j" && plain => {
                        self.select_next();
                        true
                    }
                    LogicalKey::Character(ch) if ch.as_str() == "k" && plain => {
                        self.select_prev();
                        true
                    }
                    LogicalKey::Character(ch) if ch.as_str() == "d" && ctrl => {
                        self.select_next_by(half_page);
                        true
                    }
                    LogicalKey::Character(ch) if ch.as_str() == "u" && ctrl => {
                        self.select_prev_by(half_page);
                        true
                    }
                    LogicalKey::Character(ch) if ch.as_str() == "e" && plain => {
                        self.activate_selected(ctx)
                    }
                    LogicalKey::Character(ch)
                        if plain
                            && matches!(
                                ch.as_str(),
                                "m" | "c" | "p" | "d" | "n" | "f" | "r"
                            ) =>
                    {
                        true
                    }
                    LogicalKey::Character(ch)
                        if (ctrl || meta)
                            && ch
                                .chars()
                                .next()
                                .is_some_and(|c| c.is_ascii_alphabetic()) =>
                    {
                        true
                    }
                    LogicalKey::Character(_) if plain => true,
                    LogicalKey::Named(_) if plain => true,
                    _ => false,
                }
            }
            _ => false,
        }
    }

    fn activate_selected(&mut self, ctx: &PanelContext) -> bool {
        let Some(selected) = self.selected().cloned() else {
            return true;
        };
        match selected.kind {
            NodeKind::Dir { .. } => {
                let idx = self.selected_index();
                let _ = self.toggle_dir_at(idx, ctx);
            }
            NodeKind::File => {
                // Queue an open-intent for the host. Native turns this
                // into `:edit <path>` for the attached nvim; web pulls
                // the path via `drain_open_paths` and either spawns a
                // markdown editor tab (for `.md`) or a generic
                // file-viewer tab.
                if let Some(path) = selected.path.clone() {
                    self.pending_opens.push(path);
                }
                self.set_focused(false);
            }
        }
        true
    }

    /// Map a window-space click to a row index. `y_top` is the top of
    /// the panel in logical pixels — should match the value passed to
    /// `render`. Returns `None` when outside the panel or past the
    /// last row.
    pub fn hit_test(
        &self,
        mouse_x: f32,
        mouse_y: f32,
        y_top: f32,
        panel_height: f32,
    ) -> Option<usize> {
        self.hit_test_in_bounds(mouse_x, mouse_y, 0.0, y_top, self.width, panel_height)
    }

    pub fn hit_test_in_bounds(
        &self,
        mouse_x: f32,
        mouse_y: f32,
        x_left: f32,
        y_top: f32,
        panel_width: f32,
        panel_height: f32,
    ) -> Option<usize> {
        if !self.visible {
            return None;
        }
        let frame_stroke = (FRAME_STROKE * self.scale).max(2.0);
        let content_x = x_left + frame_stroke;
        let content_y = y_top + frame_stroke;
        let content_w = (panel_width - frame_stroke * 2.0).max(0.0);
        let content_h = (panel_height - frame_stroke * 2.0).max(0.0);
        if mouse_x < content_x || mouse_x > content_x + content_w {
            return None;
        }
        if mouse_y < content_y || mouse_y > content_y + content_h {
            return None;
        }
        let row_h = self.row_height();
        let local_y = mouse_y - content_y - self.scroll.position;
        let row = (local_y / row_h).floor() as isize + self.scroll_top as isize;
        if row < 0 {
            return None;
        }
        let row = row as usize;
        if row >= self.entries.len() {
            return None;
        }
        Some(row)
    }
}
