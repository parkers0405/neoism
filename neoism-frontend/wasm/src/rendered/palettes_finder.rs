use super::*;
use neoism_ui::PanelKey;

#[wasm_bindgen]
impl ChromeBridge {
    pub fn show_search_palette(&mut self) {
        self.chrome.finder.set_enabled(false);
        self.chrome.command_palette.enter_search_mode();
        self.relayout_chrome();
    }

    pub fn show_finder(&mut self) {
        self.show_finder_files();
    }

    pub fn show_finder_files(&mut self) {
        self.chrome.command_palette.set_enabled(false);
        self.chrome.finder.open_files(self.workspace_root.clone());
        self.relayout_chrome();
    }

    pub fn show_finder_grep(&mut self) {
        self.chrome.command_palette.set_enabled(false);
        self.chrome.finder.open_grep(self.workspace_root.clone());
        self.relayout_chrome();
    }

    pub fn show_finder_git_changes(&mut self) {
        self.chrome.command_palette.set_enabled(false);
        self.chrome
            .finder
            .open_git_changes(&*self.search, self.workspace_root.clone());
        self.relayout_chrome();
    }

    /// Click/tap router for the center modals (command palette /
    /// finder). Returns 0 = no modal consumed the press, 1 = a row
    /// was picked and committed, 2 = the press landed inside the
    /// modal chrome/input (consume; host may raise the soft
    /// keyboard for query typing). Presses OUTSIDE the card return
    /// 0 — chrome's light-dismiss closes the modal when the
    /// forwarded PointerDown hits the blocker.
    pub fn modal_pointer_down(&mut self, x: f32, y: f32) -> i32 {
        let Some((pw, ph, sf)) = self.rendered.sugarloaf_mut().map(|s| {
            let size = s.window_size();
            (size.width as f32, size.height as f32, s.scale_factor())
        }) else {
            return 0;
        };
        if self.chrome.command_palette.is_visible() {
            return match self.chrome.command_palette.hit_test(x, y, pw, sf) {
                Ok(Some(_)) => {
                    self.chrome.command_palette.hover(x, y, pw, sf);
                    self.pick_palette_action();
                    self.chrome.command_palette.set_enabled(false);
                    self.relayout_chrome();
                    1
                }
                Ok(None) => 2,
                Err(()) => 0,
            };
        }
        if self.chrome.finder.is_visible() {
            return match self.chrome.finder.hit_test(x, y, (pw, ph, sf)) {
                Ok(Some(index)) => {
                    self.chrome.finder.select_index(index);
                    self.pick_finder_selection();
                    self.chrome.finder.close();
                    self.relayout_chrome();
                    1
                }
                Ok(None) => 2,
                Err(()) => 0,
            };
        }
        0
    }

    /// Wheel / touch-drag scroll for the center modals. `delta`
    /// uses DOM sign (positive scrolls the list down).
    pub fn modal_scroll(&mut self, x: f32, y: f32, delta_pixels: f32) -> bool {
        let Some((pw, ph, sf)) = self.rendered.sugarloaf_mut().map(|s| {
            let size = s.window_size();
            (size.width as f32, size.height as f32, s.scale_factor())
        }) else {
            return false;
        };
        if self.chrome.command_palette.is_visible() {
            if let Some(rect) = self.chrome.command_palette.active_rect(pw, sf) {
                if x >= rect[0]
                    && x <= rect[0] + rect[2]
                    && y >= rect[1]
                    && y <= rect[1] + rect[3]
                {
                    self.chrome.command_palette.scroll_pixels(delta_pixels);
                    return true;
                }
            }
            return false;
        }
        if self.chrome.finder.is_visible() {
            if let Some([rx, ry, rw, rh]) = self.chrome.finder.active_rect((pw, ph, sf)) {
                if x >= rx && x <= rx + rw && y >= ry && y <= ry + rh {
                    self.chrome.finder.scroll_pixels(delta_pixels);
                    return true;
                }
            }
        }
        false
    }

    /// Drain the file-tree's queue of activated paths (the user
    /// double-clicked or pressed Enter on a file row). Returns a
    /// JSON array of absolute path strings — the JS host turns each
    /// one into an open-buffer intent (markdown editor for `.md`,
    /// generic viewer otherwise) and fetches contents via the
    /// FilesService bridge.
    pub fn drain_file_tree_opens(&mut self) -> JsValue {
        let Some(tree) = self.chrome.file_tree.as_mut() else {
            return serde_wasm_bindgen::to_value(&Vec::<String>::new())
                .unwrap_or(JsValue::NULL);
        };
        let paths: Vec<String> = tree
            .drain_open_paths()
            .into_iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        serde_wasm_bindgen::to_value(&paths).unwrap_or(JsValue::NULL)
    }

    /// Hit-test a window-space coordinate against the file tree and
    /// return the entry under that point as JSON. Used by the web
    /// host's right-click handler to surface a CRUD context menu
    /// (Rename / New File / New Folder / Delete) for the targeted
    /// row.
    ///
    /// Returns `null` when the panel is hidden or `(x, y)` falls
    /// outside its bounds (or past the last row). Otherwise:
    ///
    /// ```text
    /// {
    ///   path: string | null,    // absolute path for the row
    ///   is_dir: bool,           // true for directory rows
    ///   parent_dir: string,     // dir that should host New File/Dir
    ///   label: string,          // display label (for menu header)
    /// }
    /// ```
    ///
    /// `parent_dir` is the row's parent directory for files, and
    /// the row itself for directory entries — so "New File" /
    /// "New Folder" can use it as the creation target verbatim.
    /// Selection is also nudged onto the hit row, so the keyboard
    /// shortcuts (F2 / Delete) operate on the same entry that the
    /// user just right-clicked.
    pub fn file_tree_context_target(&mut self, x: f32, y: f32) -> JsValue {
        #[derive(serde::Serialize)]
        struct Target {
            path: Option<String>,
            is_dir: bool,
            parent_dir: String,
            label: String,
        }
        let bounds = match self.chrome.layout().file_tree {
            Some(rect) => rect,
            None => return JsValue::NULL,
        };
        let tree = match self.chrome.file_tree.as_mut() {
            Some(t) => t,
            None => return JsValue::NULL,
        };
        let row =
            match tree.hit_test_in_bounds(x, y, bounds.x, bounds.y, bounds.w, bounds.h) {
                Some(r) => r,
                None => return JsValue::NULL,
            };
        let entries = tree.entries();
        let entry = match entries.get(row) {
            Some(e) => e.clone(),
            None => return JsValue::NULL,
        };
        tree.set_selected(row);
        let is_dir = matches!(
            entry.kind,
            neoism_ui::panels::file_tree::NodeKind::Dir { .. }
        );
        let path_str = entry
            .path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned());
        let parent_dir = if is_dir {
            path_str
                .clone()
                .unwrap_or_else(|| self.workspace_root.to_string_lossy().into_owned())
        } else {
            entry
                .path
                .as_ref()
                .and_then(|p| p.parent())
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| self.workspace_root.to_string_lossy().into_owned())
        };
        let target = Target {
            path: path_str,
            is_dir,
            parent_dir,
            label: entry.label.clone(),
        };
        serde_wasm_bindgen::to_value(&target).unwrap_or(JsValue::NULL)
    }

    /// Return the absolute path of the currently-selected file
    /// tree row, or `null` when the tree is hidden / nothing is
    /// selected / the row has no backing path (virtual entry).
    ///
    /// Drives the keyboard shortcuts (F2 rename, Delete) so the
    /// host can act on the focused entry without re-hit-testing.
    pub fn file_tree_selected_path(&self) -> JsValue {
        let Some(tree) = self.chrome.file_tree.as_ref() else {
            return JsValue::NULL;
        };
        let Some(path) = tree.selected_path() else {
            return JsValue::NULL;
        };
        JsValue::from_str(&path.to_string_lossy())
    }

    /// Return the workspace root path the chrome was constructed
    /// with. Used by the host to default "New File / New Folder"
    /// targets when the user right-clicks outside any row.
    pub fn file_tree_workspace_root(&self) -> JsValue {
        JsValue::from_str(&self.workspace_root.to_string_lossy())
    }

    /// True when the file tree owns chrome focus. Used by the
    /// host's F2 / Delete keyboard shortcuts to gate CRUD actions
    /// so the keys only fire when the user is actually in the
    /// tree, not in the terminal / editor / palette.
    pub fn file_tree_focused(&self) -> bool {
        self.chrome.focused() == Some(PanelKey::FileTree)
    }

    /// Drain the buffer-tab strip's queued click intents. Returns
    /// `{ activate: number | null, close: number[], new_tab: bool }`
    /// so the JS host can mirror tab-bar clicks into its own
    /// bookkeeping list — switch the visible content for
    /// `activate`, splice + replay `set_buffer_tabs` for each
    /// `close`, and spawn a terminal tab for `new_tab` (the
    /// strip's trailing "+" button, desktop TabCreateNew parity).
    pub fn drain_buffer_tab_intents(&mut self) -> JsValue {
        #[derive(serde::Serialize)]
        struct Intents {
            activate: Option<u32>,
            close: Vec<u32>,
            new_tab: bool,
        }
        let activate = self.chrome.drain_buffer_tab_activate().map(|ix| ix as u32);
        let close: Vec<u32> = self
            .chrome
            .drain_buffer_tab_closes()
            .into_iter()
            .map(|ix| ix as u32)
            .collect();
        let new_tab = self.chrome.drain_buffer_tab_new();
        // Keep the wasm-side notion of the active tab in lock-step
        // with what we just told JS, so subsequent
        // `set_tab_content` calls land on the right slot when the
        // host hasn't yet acknowledged the activation.
        if let Some(ix) = activate {
            self.sync_active_tab_state(ix as usize);
        }
        serde_wasm_bindgen::to_value(&Intents {
            activate,
            close,
            new_tab,
        })
        .unwrap_or(JsValue::NULL)
    }

    pub fn drain_agent_tab_opens(&mut self) -> u32 {
        let count = self.pending_agent_tab_opens;
        self.pending_agent_tab_opens = 0;
        count
    }

    /// Snapshot the finder's currently-highlighted row and queue
    /// an open intent. Mirrors `Screen::open_finder_selection` on
    /// desktop, minus the host-side bookkeeping (nvim ex command,
    /// editor route activation, breadcrumb refresh) which the TS
    /// host owns. Returns `true` when an intent was queued.
    ///
    /// Called BEFORE `chrome.handle_event` for Enter so the
    /// finder's own Enter handler (`Finder::close()` in
    /// chrome_shim) can still run inside `handle_event` and close
    /// the panel — we deliberately do not mutate panel state here
    /// so `chrome.event_priority_order` still routes the Enter to
    /// the modal and swallows it (otherwise Enter would leak to
    /// background panels). Matches the `palette_enter_action`
    /// capture-only pattern in `handle_event`.
    pub fn pick_finder_selection(&mut self) -> bool {
        use neoism_ui::panels::finder::FinderMode;
        if !self.chrome.finder.is_enabled() {
            return false;
        }
        let Some((path, line)) = self.chrome.finder.selected_open_target() else {
            return false;
        };
        let mode = match self.chrome.finder.mode() {
            FinderMode::Files => "files",
            FinderMode::Grep => "grep",
            FinderMode::GitChanges => "git_changes",
            // Web has no native code pane yet; the desktop owns these.
            FinderMode::BufferLines
            | FinderMode::BufferReplace
            | FinderMode::References
            | FinderMode::Symbols => return false,
        };
        let query = self.chrome.finder.query.clone();
        self.pending_finder_open_intents.push(FinderOpenIntent {
            path: path.to_string_lossy().into_owned(),
            line,
            mode,
            query,
        });
        true
    }

    /// Drain queued finder open intents as a JSON array. JS turns
    /// each one into a buffer-tab append plus an `Editor::OpenBuffer`
    /// envelope (and optionally a follow-up `:<line>` jump for grep
    /// hits — that part lives in `TerminalPanel.ts`).
    pub fn drain_finder_open_intents(&mut self) -> JsValue {
        let drained: Vec<FinderOpenIntent> =
            std::mem::take(&mut self.pending_finder_open_intents);
        serde_wasm_bindgen::to_value(&drained).unwrap_or(JsValue::NULL)
    }

    /// Snapshot the command palette's currently-highlighted row
    /// and queue an execute intent. Mirrors `Screen::handle_palette_click`
    /// on desktop. Returns `true` when an intent was queued.
    ///
    /// Called BEFORE `chrome.handle_event` for Enter so the
    /// palette's own Enter handler (`set_enabled(false)` in
    /// chrome_shim) can still run inside `handle_event` and close
    /// the panel — we deliberately do not mutate panel state here
    /// so `chrome.event_priority_order` still routes the Enter to
    /// the modal and swallows it. Matches the `palette_enter_action`
    /// capture-only pattern in `handle_event`.
    pub fn pick_palette_action(&mut self) -> bool {
        if !self.chrome.command_palette.is_enabled() {
            return false;
        }

        // Ex mode wins — the suggestion list is ex commands and
        // Enter forwards the selection to nvim.
        if self.chrome.command_palette.is_ex_mode() {
            if let Some(command) = self.chrome.command_palette.get_selected_ex_command() {
                self.pending_palette_intents
                    .push(PaletteIntent::ExCommand { command });
                return true;
            }
            return false;
        }

        // `/` search mode — pick either a live buffer match or
        // (failing that) a recent / freeform search term.
        if self.chrome.command_palette.is_search_mode() {
            let location = self.chrome.command_palette.selected_buffer_match_location();
            if location.is_some() {
                let query = self.chrome.command_palette.query.clone();
                self.pending_palette_intents.push(PaletteIntent::Search {
                    query,
                    match_location: location,
                });
                return true;
            } else if let Some(term) =
                self.chrome.command_palette.get_selected_search_term()
            {
                self.pending_palette_intents.push(PaletteIntent::Search {
                    query: term,
                    match_location: None,
                });
                return true;
            }
            return false;
        }

        if let Some(family) = self.chrome.command_palette.get_selected_font() {
            self.pending_palette_intents
                .push(PaletteIntent::Font { family });
            return true;
        }

        if let Some(name) = self.chrome.command_palette.get_selected_theme() {
            self.pending_palette_intents
                .push(PaletteIntent::Theme { name });
            return true;
        }

        if let Some(shader) = self.chrome.command_palette.get_selected_shader() {
            self.pending_palette_intents.push(PaletteIntent::Shader {
                title: shader.title,
                filter: shader.filter,
            });
            return true;
        }

        // Workspaces mode — picking a row switches the daemon
        // workspace. Mirrors the desktop router's
        // `get_selected_workspace_target` → `switch_daemon_host_workspace`
        // arm; the JS host owns the actual switch (it holds the
        // protocol client + HostWorkspaceTree bookkeeping).
        if let Some(target) = self.chrome.command_palette.get_selected_workspace_target()
        {
            self.pending_palette_intents.push(PaletteIntent::Workspace {
                workspace_id: target.workspace_id,
            });
            return true;
        }

        if let Some(target) = self.chrome.command_palette.get_selected_buffer_target() {
            use neoism_ui::panels::command_palette::PaletteBufferTarget;
            let target = match target {
                PaletteBufferTarget::Pane {
                    route_id,
                    tab_index,
                } => PaletteBufferIntent::Pane {
                    route_id,
                    tab_index,
                },
                PaletteBufferTarget::Workspace(tab_index) => {
                    PaletteBufferIntent::Workspace { tab_index }
                }
            };
            self.pending_palette_intents
                .push(PaletteIntent::Buffer { target });
            return true;
        }

        if let Some(action) = self.chrome.command_palette.get_selected_action() {
            use neoism_ui::panels::command_palette::PaletteAction;
            // OpenNeoismAgent has its own dedicated tab-open queue
            // path (see `handle_event`'s post-dispatch check); skip
            // the Action intent so JS doesn't double-fire it.
            if !matches!(action, PaletteAction::OpenNeoismAgent) {
                self.pending_palette_intents.push(PaletteIntent::Action {
                    action: palette_action_name(action),
                });
            }
            return true;
        }

        false
    }

    /// Drain queued palette intents as a JSON array. JS dispatches
    /// each one against host-side state (toggle git diff panel,
    /// open finder, run ex command via the editor envelope, etc.).
    pub fn drain_palette_intents(&mut self) -> JsValue {
        let drained: Vec<PaletteIntent> =
            std::mem::take(&mut self.pending_palette_intents);
        serde_wasm_bindgen::to_value(&drained).unwrap_or(JsValue::NULL)
    }
}
