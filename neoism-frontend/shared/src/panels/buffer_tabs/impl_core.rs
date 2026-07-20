use super::*;

impl<A> BufferTabs<A> {
    pub fn new() -> Self {
        BufferTabs {
            visible: false,
            tabs: Vec::new(),
            active: 0,
            layout: Vec::new(),
            scale: 1.0,
            scroll_x: 0.0,
            scroll_target_x: 0.0,
            pending_ensure_active: false,
            drag: None,
            tear_out_anim: None,
            hover: None,
            hover_anim_started: None,
            hover_from: None,
            hover_to: None,
            focused: false,
            focused_index: 0,
            focused_cursor_rect: None,
            pending_activate: None,
            pending_closes: Vec::new(),
            new_tab_rect: None,
            panel_origin: (0.0, 0.0),
            pending_new_tab: false,
        }
    }

    /// Take the most recent activate intent. Returns `Some(idx)` once
    /// for each successful pointer-click on a tab body; subsequent
    /// calls without an intervening click return `None`. The host
    /// pulls this each frame and propagates to chrome / JS so tab N
    /// becomes the focused buffer.
    pub fn drain_active_change(&mut self) -> Option<usize> {
        self.pending_activate.take()
    }

    /// Take any close-button intents queued since the last drain.
    /// Indices reference the tab list at the time of the click, so the
    /// host should apply them in order against its bookkeeping list
    /// (typically rightmost-first to keep earlier indices valid).
    pub fn drain_close_requests(&mut self) -> Vec<usize> {
        std::mem::take(&mut self.pending_closes)
    }

    /// Take the pending "+" new-tab click intent. Returns `true` at
    /// most once per click; the host turns it into its native
    /// new-terminal action (desktop `TabCreateNew` parity).
    pub fn drain_new_tab_request(&mut self) -> bool {
        std::mem::take(&mut self.pending_new_tab)
    }

    /// `true` while the strip is mid-animation. Drivers use this to
    /// keep requesting redraws until the slide settles.
    pub fn is_animating(&self) -> bool {
        if (self.scroll_x - self.scroll_target_x).abs() > 0.5 {
            return true;
        }
        if let Some(anim) = self.tear_out_anim.as_ref() {
            if anim.started_at.elapsed().as_millis() < TEAR_OUT_ANIM_MS as u128 {
                return true;
            }
        }
        if self.hover_anim_started.is_some_and(|started| {
            started.elapsed() < Duration::from_millis(TAB_HOVER_ANIM_MS)
        }) {
            return true;
        }
        self.drag.as_ref().is_some_and(|d| d.active)
    }

    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale.clamp(0.5, 3.0);
        self.layout.clear();
        self.pending_ensure_active = !self.tabs.is_empty();
    }

    /// Effective strip height in logical pixels (base * scale).
    pub fn height(&self) -> f32 {
        BUFFER_TABS_HEIGHT * self.scale
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn set_visible(&mut self, v: bool) {
        self.visible = v;
        if !v {
            self.hover = None;
            self.hover_anim_started = None;
            self.hover_from = None;
            self.hover_to = None;
            self.focused = false;
            self.focused_cursor_rect = None;
        }
    }

    #[allow(dead_code)]
    pub fn set_tabs(&mut self, tabs: Vec<BufferTab<A>>, active: usize) {
        self.tabs = tabs;
        self.active = if self.tabs.is_empty() {
            0
        } else {
            active.min(self.tabs.len() - 1)
        };
        self.layout.clear();
        self.drag = None;
        self.hover = None;
        self.hover_anim_started = None;
        self.hover_from = None;
        self.hover_to = None;
        self.focused_index = self.active;
        self.focused_cursor_rect = None;
    }

    pub fn set_hover(&mut self, hover: Option<TabHit>) -> bool {
        let hover = hover.filter(|hit| match *hit {
            TabHit::Activate(ix) | TabHit::Close(ix) => ix < self.tabs.len(),
            // The "+" slot is always present (the strip shows whenever
            // the workspace has a terminal), so it is always hoverable.
            TabHit::NewTab => true,
        });
        if self.hover == hover {
            return false;
        }
        let old_ix = self.hover.map(tab_hit_index);
        let new_ix = hover.map(tab_hit_index);
        if old_ix != new_ix {
            self.hover_from = old_ix;
            self.hover_to = new_ix;
            self.hover_anim_started = Some(Instant::now());
        }
        self.hover = hover;
        true
    }

    /// Clear hover without playing the normal hover-out animation.
    /// Used when a modal opens above the strip; otherwise stale tab
    /// hover animation keeps repainting behind the modal and reads as
    /// a blinking active-tab color.
    pub fn clear_hover_immediate(&mut self) -> bool {
        let changed = self.hover.is_some()
            || self.hover_anim_started.is_some()
            || self.hover_from.is_some()
            || self.hover_to.is_some();
        self.hover = None;
        self.hover_anim_started = None;
        self.hover_from = None;
        self.hover_to = None;
        changed
    }

    pub fn tabs(&self) -> &[BufferTab<A>] {
        &self.tabs
    }

    pub fn active(&self) -> usize {
        self.active
    }

    /// Override the active tab's display title (e.g. so the virtual
    /// note-graph tab reads "Neoism Graph" instead of its filename).
    pub fn set_active_title(&mut self, title: impl Into<String>) {
        let ix = self.active;
        if let Some(tab) = self.tabs.get_mut(ix) {
            tab.title = title.into();
        }
    }

    /// Override the display title of the tab at `ix` (right-click →
    /// Rename). Returns `true` when a tab was actually relabelled.
    pub fn set_title(&mut self, ix: usize, title: impl Into<String>) -> bool {
        if let Some(tab) = self.tabs.get_mut(ix) {
            tab.title = title.into();
            true
        } else {
            false
        }
    }

    /// The agent session id backing the tab at `ix`, if this is an
    /// agent tab whose title should be published at the daemon level on
    /// rename. Terminal-agent tabs key off `terminal_route_id`; native
    /// neoism-agent tabs key off `neoism_agent_route_id`. Both are
    /// stringified so the daemon `SetTitle { session_id, .. }` path
    /// receives the route identifier the host can resolve to a session.
    pub fn agent_session_id_at(&self, ix: usize) -> Option<String> {
        let tab = self.tabs.get(ix)?;
        if let Some(route) = tab.neoism_agent_route_id {
            return Some(route.to_string());
        }
        if tab.agent_kind.is_some() {
            if let Some(route) = tab.terminal_route_id {
                return Some(route.to_string());
            }
        }
        None
    }

    pub fn is_focused(&self) -> bool {
        self.focused
    }

    pub fn set_focused(&mut self, focused: bool) {
        if focused && !self.tabs.is_empty() {
            self.focused_index = self.active.min(self.tabs.len() - 1);
            self.pending_ensure_active = true;
        }
        self.focused = focused && !self.tabs.is_empty() && self.visible;
        if !self.focused {
            self.focused_cursor_rect = None;
        }
    }

    /// Index of the focus cursor, clamped into the valid range. The
    /// cursor can land on any tab `0..tabs.len()` OR on the trailing
    /// "+" new-tab slot at exactly `tabs.len()`, so the clamp ceiling is
    /// `tabs.len()` (one past the last tab) rather than the last tab.
    pub fn focused_index(&self) -> usize {
        self.focused_index.min(self.tabs.len())
    }

    /// `true` when the focus cursor is parked on the trailing "+"
    /// new-tab slot rather than on a real tab. Hosts check this before
    /// activating `focused_index()` so Enter on the "+" opens a new
    /// terminal instead of activating tab `tabs.len()` (which is out of
    /// range).
    pub fn focused_on_new_tab(&self) -> bool {
        self.focused && self.focused_index.min(self.tabs.len()) == self.tabs.len()
    }

    pub fn focused_cursor_rect(&self) -> Option<[f32; 4]> {
        self.focused.then_some(())?;
        self.focused_cursor_rect
    }

    pub fn move_focused(&mut self, previous: bool) -> bool {
        // The focus cursor cycles over `tabs.len() + 1` slots: every tab
        // plus the trailing "+" new-tab slot at index `tabs.len()`. With
        // a single tab there are still two slots (the tab and the "+"),
        // so only bail when there's nothing at all to focus.
        let slots = self.tabs.len() + 1;
        if !self.focused || slots <= 1 {
            return false;
        }
        let current = self.focused_index.min(slots - 1);
        self.focused_index = if previous {
            if current == 0 {
                slots - 1
            } else {
                current - 1
            }
        } else {
            (current + 1) % slots
        };
        self.pending_ensure_active = true;
        true
    }

    pub fn select_relative(&mut self, previous: bool) -> bool {
        let operation = if previous {
            BufferTabPolicyOperation::SelectPrevious
        } else {
            BufferTabPolicyOperation::SelectNext
        };
        let result = apply_buffer_tab_policy(
            BufferTabPolicyInput {
                len: self.tabs.len(),
                active: self.active,
                closeable: Vec::new(),
            },
            operation,
        );
        if !result.changed {
            return false;
        }
        self.set_active(result.active);
        true
    }

    pub fn select_index(&mut self, index: usize) -> bool {
        let result = apply_buffer_tab_policy(
            BufferTabPolicyInput {
                len: self.tabs.len(),
                active: self.active,
                closeable: Vec::new(),
            },
            BufferTabPolicyOperation::SelectIndex { index },
        );
        if !result.changed {
            return false;
        }
        self.set_active(result.active);
        true
    }

    pub fn set_active(&mut self, ix: usize) {
        if !self.tabs.is_empty() {
            self.active = ix.min(self.tabs.len() - 1);
            if self.focused {
                self.focused_index = self.active;
            }
            self.pending_ensure_active = true;
        }
    }

    pub fn terminal_index(&self) -> Option<usize> {
        self.tabs
            .iter()
            .position(|tab| tab.is_terminal() && tab.terminal_route_id.is_none())
    }

    pub fn is_terminal_at(&self, ix: usize) -> bool {
        self.tabs.get(ix).is_some_and(BufferTab::is_terminal)
    }

    pub fn is_root_terminal_at(&self, ix: usize) -> bool {
        self.tabs
            .get(ix)
            .is_some_and(|tab| tab.is_terminal() && tab.terminal_route_id.is_none())
    }

    pub fn terminal_route_at(&self, ix: usize) -> Option<usize> {
        self.tabs.get(ix).and_then(|tab| tab.terminal_route_id)
    }

    pub fn active_is_terminal(&self) -> bool {
        self.is_terminal_at(self.active)
    }

    /// Resolve the active close command before host-side IO runs.
    ///
    /// Desktop can have the editor focused while the strip still points at
    /// a terminal tab, usually after a route transition. In that case closing
    /// should target the remembered editor buffer when possible, then the first
    /// non-terminal tab, and only ignore if the strip truly has no closeable
    /// editor/agent/markdown target. Web uses the same state rule for
    /// tab close commands before replaying PTY/editor content.
    pub fn active_close_plan(
        &mut self,
        current_is_editor: bool,
        remembered_path: Option<&Path>,
    ) -> BufferTabClosePlan {
        let active_ix = self.active;
        if !current_is_editor {
            if let Some(route_id) = self.terminal_route_at(active_ix) {
                return BufferTabClosePlan::CloseTerminalRoute { route_id };
            }
        }

        if self.is_terminal_at(active_ix) && current_is_editor {
            if let Some(path) = remembered_path {
                if let Some(ix) = self.find_path(path) {
                    self.set_active(ix);
                    return BufferTabClosePlan::CloseTab { index: ix };
                }
            }

            if let Some(ix) = self
                .tabs
                .iter()
                .enumerate()
                .find_map(|(ix, tab)| tab.target().map(|_| ix))
            {
                self.set_active(ix);
                return BufferTabClosePlan::CloseTab { index: ix };
            }
        }

        let active_ix = self.active;
        if self.is_terminal_at(active_ix) {
            BufferTabClosePlan::Ignore
        } else {
            BufferTabClosePlan::CloseTab { index: active_ix }
        }
    }

    pub fn has_file_tabs(&self) -> bool {
        self.tabs.iter().any(|tab| tab.target().is_some())
    }

    pub fn target_at(&self, ix: usize) -> Option<BufferTabTarget> {
        self.tabs.get(ix).and_then(BufferTab::target)
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.as_ref().is_some_and(|d| d.active)
    }

    pub fn drag_state(&self) -> Option<&DragState> {
        self.drag.as_ref()
    }

    pub fn find_path(&self, path: &Path) -> Option<usize> {
        self.tabs
            .iter()
            .position(|t| t.path.as_deref() == Some(path))
    }

    pub fn active_path(&self) -> Option<&Path> {
        self.tabs.get(self.active).and_then(|t| t.path.as_deref())
    }

    pub fn active_shows_breadcrumbs(&self) -> bool {
        self.tabs
            .get(self.active)
            .and_then(BufferTab::target)
            .is_some_and(|target| {
                matches!(
                    target,
                    BufferTabTarget::File(_) | BufferTabTarget::Markdown(_)
                )
            })
    }

    pub fn has_modified_tabs(&self) -> bool {
        self.tabs.iter().any(|tab| tab.modified)
    }

    /// Modified tabs whose buffer lives SERVER-side (code files) —
    /// the only ones a server switch would actually lose. Markdown
    /// panes are client-owned: their unsaved lines ride along across
    /// a connection swap, so counting them in the switch gate traps
    /// the user (view flips, connection refuses to follow, saves land
    /// on the wrong daemon).
    pub fn has_modified_server_tabs(&self) -> bool {
        self.tabs.iter().any(|tab| {
            tab.modified && matches!(tab.target(), Some(BufferTabTarget::File(_)))
        })
    }

    /// Flip the modified flag on the tab whose `path` matches.
    pub fn set_modified(&mut self, path: &Path, modified: bool) -> bool {
        if let Some(ix) = self.find_path(path) {
            if self.tabs[ix].modified != modified {
                self.tabs[ix].modified = modified;
                return true;
            }
        }
        false
    }

    /// Width of every tab slot for `count` tabs in `available_width`.
    ///
    /// Tabs keep their natural `NATURAL_TAB_WIDTH` regardless of count:
    /// we no longer shrink-to-fit and truncate titles when the strip is
    /// crowded. When the tabs overflow the strip, the strip scrolls
    /// horizontally instead (it is already a scroll surface). The only
    /// remaining clamp is the strip width itself — a fixed natural-width
    /// tab on a phone-width strip left a huge empty band on its right, so
    /// a lone tab never exceeds the strip.
    pub fn tab_width_for(count: usize, available_width: f32) -> f32 {
        let _ = count;
        // Never wider than the strip (keeps a single tab from leaving a
        // dead band), never narrower than MIN so titles stay legible.
        let upper = NATURAL_TAB_WIDTH.min(available_width.max(MIN_TAB_WIDTH));
        NATURAL_TAB_WIDTH.clamp(MIN_TAB_WIDTH.min(upper), upper)
    }

    /// Truncate `title` so that it (plus a single-char ellipsis) fits
    /// in `max_width` pixels at the strip font.
    pub fn fit_title<'a>(
        title: &'a str,
        max_width: f32,
        mut char_width: impl FnMut(char) -> f32,
    ) -> std::borrow::Cow<'a, str> {
        let suffix_width = char_width(TITLE_ELLIPSIS);
        let mut accumulated: f32 = 0.0;
        let mut truncate_ix: usize = 0;
        for (ix, c) in title.char_indices() {
            if accumulated + suffix_width <= max_width {
                truncate_ix = ix;
            }
            accumulated += char_width(c);
            if accumulated > max_width {
                let mut out =
                    String::with_capacity(truncate_ix + TITLE_ELLIPSIS.len_utf8());
                out.push_str(&title[..truncate_ix]);
                out.push(TITLE_ELLIPSIS);
                return std::borrow::Cow::Owned(out);
            }
        }
        std::borrow::Cow::Borrowed(title)
    }

    /// Bump the scroll target by `delta` logical pixels.
    pub fn scroll_by(&mut self, delta: f32) {
        self.scroll_target_x += delta;
    }

    /// Move the scroll target so that the active tab is fully visible.
    pub fn ensure_active_visible(&mut self, available_width: f32) {
        self.ensure_index_visible(self.active, available_width);
    }

    /// Move the scroll target so the tab at `ix` is fully visible. Only the
    /// scroll *target* is moved; the per-frame lerp toward it gives the same
    /// smooth, touchpad-style reveal when arrow-navigating into off-screen
    /// tabs. Used for both the active tab and the keyboard focus cursor.
    pub fn ensure_index_visible(&mut self, ix: usize, available_width: f32) {
        if self.tabs.is_empty() || available_width <= 0.0 {
            return;
        }
        let ix = ix.min(self.tabs.len() - 1);
        let tab_width = Self::tab_width_for(self.tabs.len(), available_width);
        let total_w = tab_width * self.tabs.len() as f32;
        let max_scroll = (total_w - available_width).max(0.0);
        let tab_left = ix as f32 * tab_width;
        let tab_right = tab_left + tab_width;
        let mut target = self.scroll_target_x;
        if tab_left < target {
            target = tab_left;
        } else if tab_right > target + available_width {
            target = tab_right - available_width;
        }
        self.scroll_target_x = target.clamp(0.0, max_scroll);
    }

    pub fn move_active(&mut self, previous: bool) -> bool {
        let operation = if previous {
            BufferTabPolicyOperation::MovePrevious
        } else {
            BufferTabPolicyOperation::MoveNext
        };
        let result = apply_buffer_tab_policy(
            BufferTabPolicyInput {
                len: self.tabs.len(),
                active: self.active,
                closeable: Vec::new(),
            },
            operation,
        );
        let (Some(from), Some(to)) = (result.move_from, result.move_to) else {
            return false;
        };

        self.tabs.swap(from, to);
        self.active = result.active;
        self.drag = None;
        self.layout.clear();
        self.pending_ensure_active = true;
        true
    }

    pub fn ensure_terminal_tab(&mut self) -> usize {
        if let Some(ix) = self.terminal_index() {
            return ix;
        }
        self.tabs.insert(
            0,
            BufferTab {
                title: TERMINAL_TITLE.to_string(),
                modified: false,
                path: None,
                markdown: false,
                terminal_route_id: None,
                neoism_agent_route_id: None,
                chrome_page: None,
                agent_kind: None,
            },
        );
        self.active = self.active.saturating_add(1).min(self.tabs.len() - 1);
        self.visible = true;
        self.layout.clear();
        0
    }

    pub fn open_terminal(&mut self, route_id: usize) -> usize {
        if let Some(ix) = self
            .tabs
            .iter()
            .position(|tab| tab.terminal_route_id == Some(route_id))
        {
            self.active = ix;
            self.pending_ensure_active = true;
            return ix;
        }

        let number = self.tabs.iter().filter(|tab| tab.is_terminal()).count() + 1;
        self.tabs.push(BufferTab {
            title: format!("Terminal {number}"),
            modified: false,
            path: None,
            markdown: false,
            terminal_route_id: Some(route_id),
            neoism_agent_route_id: None,
            chrome_page: None,
            agent_kind: None,
        });
        self.active = self.tabs.len() - 1;
        self.visible = true;
        self.layout.clear();
        self.pending_ensure_active = true;
        self.active
    }

    pub fn open_neoism_agent(&mut self, route_id: usize) -> usize {
        if let Some(ix) = self
            .tabs
            .iter()
            .position(|tab| tab.neoism_agent_route_id == Some(route_id))
        {
            self.active = ix;
            self.pending_ensure_active = true;
            return ix;
        }

        let number = self
            .tabs
            .iter()
            .filter(|tab| tab.neoism_agent_route_id.is_some())
            .count()
            + 1;
        self.tabs.push(BufferTab {
            title: if number == 1 {
                "Neoism Agent".to_string()
            } else {
                format!("Neoism Agent {number}")
            },
            modified: false,
            path: None,
            markdown: false,
            terminal_route_id: None,
            neoism_agent_route_id: Some(route_id),
            chrome_page: None,
            agent_kind: None,
        });
        self.active = self.tabs.len() - 1;
        self.visible = true;
        self.layout.clear();
        self.pending_ensure_active = true;
        self.active
    }

    /// Open (or activate) a chrome helper-page tab. Singleton per
    /// `ChromePageKind`: a second open with the same `kind` activates
    /// the existing tab instead of pushing a duplicate. Title comes
    /// from [`ChromePageKind::title`].
    pub fn open_chrome_page(&mut self, kind: ChromePageKind, route_id: usize) -> usize {
        if let Some(ix) = self
            .tabs
            .iter()
            .position(|t| t.chrome_page.is_some_and(|p| p.kind == kind))
        {
            self.active = ix;
            self.visible = true;
            self.pending_ensure_active = true;
            // Refresh route_id in case the context was respawned.
            if let Some(tab) = self.tabs.get_mut(ix) {
                tab.chrome_page = Some(ChromePageRef { kind, route_id });
            }
            return ix;
        }
        self.tabs.push(BufferTab {
            title: kind.title().to_string(),
            modified: false,
            path: None,
            markdown: false,
            terminal_route_id: None,
            neoism_agent_route_id: None,
            chrome_page: Some(ChromePageRef { kind, route_id }),
            agent_kind: None,
        });
        self.active = self.tabs.len() - 1;
        self.visible = true;
        self.layout.clear();
        self.pending_ensure_active = true;
        self.active
    }

    pub fn remove_terminal_route(&mut self, route_id: usize) -> bool {
        let Some(ix) = self
            .tabs
            .iter()
            .position(|tab| tab.terminal_route_id == Some(route_id))
        else {
            return false;
        };

        self.drag = None;
        self.hover = None;
        self.hover_anim_started = None;
        self.hover_from = None;
        self.hover_to = None;
        self.tabs.remove(ix);
        if self.tabs.is_empty() {
            self.active = 0;
            self.visible = false;
        } else if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        } else if ix < self.active {
            self.active = self.active.saturating_sub(1);
        }
        self.layout.clear();
        self.pending_ensure_active = true;
        true
    }

    /// Add a buffer for `path` (or activate if already present).
    /// Re-point every tab holding `old` at `new` (title follows the new
    /// file name). Used by the markdown title-edit rename so the open
    /// tab tracks the renamed file instead of orphaning.
    pub fn rename_path(&mut self, old: &Path, new: PathBuf) {
        let title = new
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| new.display().to_string());
        for tab in &mut self.tabs {
            if tab.path.as_deref() == Some(old) {
                tab.path = Some(new.clone());
                tab.title = title.clone();
            }
        }
    }

    pub fn open_path(&mut self, path: PathBuf) -> usize {
        if is_markdown_path(&path) {
            return self.open_markdown(path);
        }
        let ix = if let Some(ix) = self.find_path(&path) {
            self.active = ix;
            ix
        } else {
            let title = path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            self.tabs.push(BufferTab {
                title,
                modified: false,
                path: Some(path),
                markdown: false,
                terminal_route_id: None,
                neoism_agent_route_id: None,
                chrome_page: None,
                agent_kind: None,
            });
            self.active = self.tabs.len() - 1;
            self.visible = true;
            self.layout.clear();
            self.active
        };
        self.pending_ensure_active = true;
        ix
    }

    pub fn open_markdown(&mut self, path: PathBuf) -> usize {
        let ix = if let Some(ix) = self.find_path(&path) {
            self.active = ix;
            if let Some(tab) = self.tabs.get_mut(ix) {
                tab.markdown = true;
            }
            ix
        } else {
            let title = path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            self.tabs.push(BufferTab {
                title,
                modified: false,
                path: Some(path),
                markdown: true,
                terminal_route_id: None,
                neoism_agent_route_id: None,
                chrome_page: None,
                agent_kind: None,
            });
            self.active = self.tabs.len() - 1;
            self.visible = true;
            self.layout.clear();
            self.active
        };
        self.pending_ensure_active = true;
        ix
    }

    /// Close the tab at `ix`. Returns `(removed_target, new_active_target)`.
    pub fn close_at(
        &mut self,
        ix: usize,
    ) -> (Option<BufferTabTarget>, Option<BufferTabTarget>) {
        if ix >= self.tabs.len() {
            return (None, None);
        }
        if self.is_terminal_at(ix) {
            self.set_active(ix);
            return (None, None);
        }
        self.drag = None;
        self.hover = None;
        self.hover_anim_started = None;
        self.hover_from = None;
        self.hover_to = None;
        let result = apply_buffer_tab_policy(
            BufferTabPolicyInput {
                len: self.tabs.len(),
                active: self.active,
                closeable: (0..self.tabs.len())
                    .map(|tab_ix| !self.is_terminal_at(tab_ix))
                    .collect(),
            },
            BufferTabPolicyOperation::CloseIndex { index: ix },
        );
        let Some(remove_ix) = result.remove_index else {
            self.set_active(ix);
            return (None, None);
        };
        let removed = self.tabs.remove(remove_ix).target();
        if self.tabs.is_empty() {
            self.active = 0;
            self.visible = false;
            self.layout.clear();
            return (removed, None);
        }
        self.active = result.active.min(self.tabs.len() - 1);
        self.layout.clear();
        let new_active = self.tabs[self.active].target();
        (removed, new_active)
    }

    /// Map a click in window coordinates to a `TabHit`.
    pub fn hit_test(
        &self,
        mouse_x: f32,
        mouse_y: f32,
        x_left: f32,
        y_top: f32,
        available_width: f32,
    ) -> Option<TabHit> {
        if !self.visible {
            return None;
        }
        // The trailing "+" button lives past the last tab. Its rect is
        // captured each render in absolute window coords, so test it
        // directly before the per-tab math (which clamps to the tab
        // range and would otherwise never reach the "+").
        if let Some([nx, ny, nw, nh]) = self.new_tab_rect {
            if mouse_x >= nx && mouse_x <= nx + nw && mouse_y >= ny && mouse_y <= ny + nh
            {
                return Some(TabHit::NewTab);
            }
        }
        if self.tabs.is_empty() {
            return None;
        }
        let strip_h = self.height();
        let tab_pad_x = TAB_PADDING_X * self.scale;
        let close_size = CLOSE_BTN_SIZE * self.scale;
        let close_hit_size = CLOSE_HIT_SIZE * self.scale;
        if mouse_y < y_top || mouse_y > y_top + strip_h {
            return None;
        }
        let raw_local_x = mouse_x - x_left;
        if raw_local_x < 0.0 || raw_local_x > available_width {
            return None;
        }
        let local_x = raw_local_x + self.scroll_x;
        let tab_width = Self::tab_width_for(self.tabs.len(), available_width);
        let total_w = tab_width * self.tabs.len() as f32;
        if local_x < 0.0 || local_x > total_w {
            return None;
        }
        let raw_ix = (local_x / tab_width).floor() as usize;
        let ix = raw_ix.min(self.tabs.len() - 1);
        let tab_left = x_left - self.scroll_x + ix as f32 * tab_width;
        if self.is_root_terminal_at(ix) {
            return Some(TabHit::Activate(ix));
        }
        let close_x = tab_left + tab_width - tab_pad_x;
        let close_y = y_top + (strip_h - close_size) / 2.0;
        let close_hit_left = close_x - close_hit_size / 2.0;
        let close_hit_top = close_y + close_size / 2.0 - close_hit_size / 2.0;
        let on_close_x =
            mouse_x >= close_hit_left && mouse_x <= close_hit_left + close_hit_size;
        let on_close_y =
            mouse_y >= close_hit_top && mouse_y <= close_hit_top + close_hit_size;
        if on_close_x && on_close_y {
            Some(TabHit::Close(ix))
        } else {
            Some(TabHit::Activate(ix))
        }
    }

    /// Begin a potential drag-to-reorder.
    pub fn begin_drag(
        &mut self,
        ix: usize,
        mouse_x: f32,
        mouse_y: f32,
        x_left: f32,
        available_width: f32,
    ) {
        if ix >= self.tabs.len() || !self.visible {
            return;
        }
        let tab_width = Self::tab_width_for(self.tabs.len(), available_width);
        let local_x = mouse_x - x_left + self.scroll_x;
        let slot_left = ix as f32 * tab_width;
        let grab_offset = (local_x - slot_left).clamp(0.0, tab_width);
        self.drag = Some(DragState {
            current_ix: ix,
            press_local_x: local_x,
            current_local_x: local_x,
            press_y: mouse_y,
            current_y: mouse_y,
            grab_offset,
            active: false,
            tear_out_armed: false,
            tear_out_horizontal: true,
        });
    }

    /// Update an in-progress drag. Returns `true` when the dragged
    /// tab swapped slots OR the drag's render state changed.
    pub fn update_drag(
        &mut self,
        mouse_x: f32,
        mouse_y: f32,
        x_left: f32,
        y_top: f32,
        available_width: f32,
    ) -> bool {
        let Some(drag) = self.drag.as_mut() else {
            return false;
        };
        let count = self.tabs.len();
        if count == 0 {
            return false;
        }
        let tab_width = Self::tab_width_for(count, available_width);
        let local_x = mouse_x - x_left + self.scroll_x;
        drag.current_local_x = local_x;
        drag.current_y = mouse_y;
        let strip_h = BUFFER_TABS_HEIGHT * self.scale;
        let strip_bottom = y_top + strip_h;
        let prev_armed = drag.tear_out_armed;
        drag.tear_out_armed = mouse_y > strip_bottom + TEAR_OUT_DROP_THRESHOLD_PX;
        let prev_horiz = drag.tear_out_horizontal;
        let dx = local_x - drag.press_local_x;
        drag.tear_out_horizontal = dx <= TEAR_OUT_VERTICAL_X_THRESHOLD_PX;
        let armed_changed =
            prev_armed != drag.tear_out_armed || prev_horiz != drag.tear_out_horizontal;
        if !drag.active
            && ((local_x - drag.press_local_x).abs() > DRAG_ACTIVATION_THRESHOLD_PX
                || (mouse_y - drag.press_y).abs() > DRAG_ACTIVATION_THRESHOLD_PX)
        {
            drag.active = true;
        }
        if !drag.active {
            return armed_changed;
        }
        if drag.tear_out_armed {
            return true;
        }
        let dragged_left = (local_x - drag.grab_offset).clamp(0.0, f32::MAX);
        let dragged_center = dragged_left + tab_width * 0.5;
        let cur = drag.current_ix;
        let mut swapped = false;
        if cur + 1 < count && dragged_center > (cur as f32 + 1.0) * tab_width {
            let result = apply_buffer_tab_policy(
                BufferTabPolicyInput {
                    len: count,
                    active: self.active,
                    closeable: Vec::new(),
                },
                BufferTabPolicyOperation::Reorder {
                    from: cur,
                    to: cur + 1,
                },
            );
            self.tabs.swap(cur, cur + 1);
            self.active = result.active.min(count - 1);
            if let Some(d) = self.drag.as_mut() {
                d.current_ix += 1;
            }
            self.layout.clear();
            swapped = true;
        } else if cur > 0 && dragged_center < cur as f32 * tab_width {
            let result = apply_buffer_tab_policy(
                BufferTabPolicyInput {
                    len: count,
                    active: self.active,
                    closeable: Vec::new(),
                },
                BufferTabPolicyOperation::Reorder {
                    from: cur,
                    to: cur - 1,
                },
            );
            self.tabs.swap(cur, cur - 1);
            self.active = result.active.min(count - 1);
            if let Some(d) = self.drag.as_mut() {
                d.current_ix -= 1;
            }
            self.layout.clear();
            swapped = true;
        }
        swapped
    }

    /// End an in-progress drag.
    pub fn end_drag(&mut self, drop_on_other_strip: bool) -> DragRelease<A> {
        let Some(d) = self.drag.take() else {
            return DragRelease::None;
        };
        if !d.active {
            return DragRelease::None;
        }
        let armed = d.tear_out_armed;
        let ix = d.current_ix;
        let is_tearable = self
            .tabs
            .get(ix)
            .map(|t| {
                t.path.is_some()
                    || (t.agent_kind.is_some() && t.terminal_route_id.is_some())
                    // Neoism-agent tabs are native Rust surfaces with no
                    // path and no `agent_kind`/PTY route; they are
                    // identified solely by `neoism_agent_route_id`. They
                    // are just as movable/tearable as file and
                    // terminal-agent tabs, so include them here or
                    // `end_drag` collapses every agent-pane drag back to a
                    // no-op `Reorder`.
                    || t.neoism_agent_route_id.is_some()
            })
            .unwrap_or(false);
        if !is_tearable || (!armed && !drop_on_other_strip) {
            return DragRelease::Reorder;
        }
        if ix >= self.tabs.len() {
            return DragRelease::Reorder;
        }
        let tab_width = self
            .layout
            .get(ix)
            .map(|(_, w)| *w)
            .unwrap_or(MIN_TAB_WIDTH * self.scale);
        let strip_h = BUFFER_TABS_HEIGHT * self.scale;
        let from_x = d.current_local_x;
        let from_y = (d.current_y - strip_h * 0.5).max(0.0);
        let tab = self.tabs.remove(ix);
        if self.active >= self.tabs.len() && !self.tabs.is_empty() {
            self.active = self.tabs.len() - 1;
        }
        self.layout.clear();
        self.tear_out_anim = Some(TearOutAnim {
            started_at: Instant::now(),
            from_x,
            from_y,
            width: tab_width,
            title: tab.title.clone(),
        });
        if drop_on_other_strip {
            DragRelease::MoveOut { tab }
        } else {
            DragRelease::TearOut {
                ix,
                tab,
                split_down: d.tear_out_horizontal,
            }
        }
    }
}
