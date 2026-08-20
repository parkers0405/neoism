use super::*;
use neoism_ui::chrome::EditorPaneKind;
use neoism_ui::editor::code::substitute::{
    apply_substitute, parse_substitute_command, split_replace_query, SubstituteRange,
    SubstituteSpec,
};
use neoism_ui::editor::code::{CodeInputMode, CodeMode};
use neoism_ui::editor::markdown::vim::{
    vim_search_backward, vim_search_forward, VimSearch,
};
use neoism_ui::editor::markdown::MarkdownPosition;
use neoism_ui::editor::notebook::NotebookCellType;
use neoism_ui::panels::command_palette::{PaletteHostCapabilities, PaletteSurface};
use neoism_ui::panels::finder::{FinderMode, ReferenceRow};
use neoism_ui::panels::notifications::NotificationLevel;
use neoism_ui::PanelKey;

thread_local! {
    /// Last `(pattern, selected row line)` the buffer-search live-drive
    /// acted on, `Some` only while the finder sits in BufferLines /
    /// BufferReplace mode. Wasm is single-threaded and `ChromeBridge`
    /// cannot grow fields from this module, so the memo lives here —
    /// same pattern as `editor_panes`' clipboard register cells.
    static BUFFER_SEARCH_SYNC: std::cell::RefCell<Option<(String, Option<u32>)>> =
        const { std::cell::RefCell::new(None) };
}

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
        if !self.chrome.finder.is_enabled() {
            return false;
        }
        let mode = match self.chrome.finder.mode() {
            FinderMode::Files => "files",
            FinderMode::Grep => "grep",
            FinderMode::GitChanges => "git_changes",
            // In-buffer modes commit against the hosted code pane
            // chrome-side (desktop `open_finder_selection` parity) —
            // their rows carry no file to hand to JS.
            FinderMode::BufferLines => return self.confirm_finder_buffer_search_web(),
            FinderMode::BufferReplace => {
                return self.confirm_finder_buffer_replace_web()
            }
            FinderMode::References => return self.confirm_finder_reference_jump_web(),
            // Symbols has no data source on web yet (GoToSymbol is
            // hidden by the host-capability filter).
            FinderMode::Symbols => return false,
        };
        let Some((path, line)) = self.chrome.finder.selected_open_target() else {
            return false;
        };
        let query = self.chrome.finder.query.clone();
        // Line-carrying hits (grep / git-changes rows, 1-based) must
        // land the caret on the hit line: arm the same deferred
        // cursor target the LSP cross-file goto uses — consumed when
        // the host routes the fetched file through `editor_open_file`.
        if let Some(hit_line) = line {
            super::editor_panes::arm_editor_pending_goto(
                path.clone(),
                (hit_line as usize).saturating_sub(1),
                0,
            );
        }
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

        // Ex mode wins — a highlighted suggestion row commits its
        // canonical name; otherwise the LITERAL typed query dispatches
        // (desktop parity: `Go to Line…` relies on a bare `:42` with
        // no suggestion rows reaching the dispatcher).
        if self.chrome.command_palette.is_ex_mode() {
            let command = self
                .chrome
                .command_palette
                .get_selected_ex_command()
                .or_else(|| {
                    let typed = self.chrome.command_palette.query.trim().to_string();
                    (!typed.is_empty()).then_some(typed)
                });
            if let Some(command) = command {
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
            // Server rows must keep their payload; everything else is
            // identified by name alone.
            match &action {
                PaletteAction::SelectServer { id }
                | PaletteAction::EditServer { id }
                | PaletteAction::RemoveServer { id } => {
                    let id = id.clone();
                    self.pending_palette_intents.push(PaletteIntent::Server {
                        action: palette_action_name(action),
                        id,
                    });
                    return true;
                }
                _ => {}
            }
            if !matches!(action, PaletteAction::OpenNeoismAgent) {
                self.pending_palette_intents.push(PaletteIntent::Action {
                    action: palette_action_name(action),
                });
            }
            return true;
        }

        // Commands-mode Ex suggestion rows (typing e.g. `vsplit` mixes
        // EX_COMMANDS rows into the list) — commit them like ex mode
        // does so Enter on one isn't a dead keystroke.
        if let Some(command) = self.chrome.command_palette.get_selected_ex_command() {
            self.pending_palette_intents
                .push(PaletteIntent::ExCommand { command });
            return true;
        }

        false
    }

    /// Drain queued palette intents as a JSON array. Called by the JS
    /// host at the top of every frame, which makes it double as the
    /// web's per-frame palette upkeep hook (`sync_palette_host_context`)
    /// — the desktop equivalents run from the router tick.
    ///
    /// Intents the bridge can satisfy chrome-side (mode re-opens,
    /// hosted-pane operations — the web analogue of desktop's
    /// `execute_palette_action` arms) are executed here and withheld;
    /// everything else is forwarded for JS to dispatch (toggle git
    /// diff panel, spawn PTYs, clipboard, fonts, …). Running the
    /// chrome-side arms at DRAIN time matters: the palette/finder
    /// Enter handlers close the modal after the pick, so a pick that
    /// re-opens the palette in another mode (Go to Line → ex mode)
    /// must apply after that close.
    /// Open the servers palette — the SAME shared surface desktop shows
    /// from its top-right corner (`command_palette.enter_servers_mode`).
    /// Web previously routed this to its own TS workplace panel, so the
    /// two frontends had different server UIs.
    ///
    /// `entries_json` is `[{id,name,address,local,status,active}]`;
    /// `status` is one of `online|connecting|offline|unknown`.
    pub fn open_servers_palette(&mut self, entries_json: String) {
        use neoism_ui::panels::command_palette::PaletteServerEntry;
        use neoism_ui::panels::ServerIndicatorStatus;
        #[derive(serde::Deserialize)]
        struct WireServer {
            id: String,
            name: String,
            #[serde(default)]
            address: String,
            #[serde(default)]
            local: bool,
            #[serde(default)]
            status: String,
            #[serde(default)]
            active: bool,
        }
        let Ok(rows) = serde_json::from_str::<Vec<WireServer>>(&entries_json) else {
            return;
        };
        let entries = rows
            .into_iter()
            .map(|row| PaletteServerEntry {
                id: row.id,
                name: row.name,
                address: row.address,
                local: row.local,
                status: match row.status.as_str() {
                    "online" => ServerIndicatorStatus::Online,
                    "connecting" => ServerIndicatorStatus::Connecting,
                    "offline" => ServerIndicatorStatus::Offline,
                    _ => ServerIndicatorStatus::Unknown,
                },
                active: row.active,
            })
            .collect();
        self.chrome.command_palette.enter_servers_mode(entries);
        self.relayout_chrome();
    }

    pub fn drain_palette_intents(&mut self) -> JsValue {
        self.sync_palette_host_context();
        let drained: Vec<PaletteIntent> =
            std::mem::take(&mut self.pending_palette_intents);
        let forward: Vec<PaletteIntent> = drained
            .into_iter()
            .filter(|intent| !self.execute_palette_intent_chrome_side(intent))
            .collect();
        serde_wasm_bindgen::to_value(&forward).unwrap_or(JsValue::NULL)
    }

    /// The full IDE theme catalog for the web pickers/settings —
    /// builtins first, then the bundled NvChad Base46 set (~97
    /// themes compiled into this wasm binary), then any runtime-
    /// registered customs (empty on wasm today). Same source of
    /// truth as the desktop palette (`all_ide_theme_names`), so the
    /// web offers the identical list instead of a hardcoded four.
    ///
    /// Returns `[{ name, dark, accent }]` where `dark` is the
    /// shared Rec. 601 background-luma split (`IdeTheme::is_dark`)
    /// and `accent` is the theme's accent as `#rrggbb` — the host
    /// seeds its presence cursor color from it so peers see the
    /// color this user's caret actually has.
    pub fn all_ide_theme_names(&self) -> JsValue {
        #[derive(serde::Serialize)]
        struct ThemeEntry {
            name: String,
            dark: bool,
            accent: String,
        }
        let entries: Vec<ThemeEntry> =
            neoism_ui::primitives::ide_theme::all_ide_theme_names()
                .into_iter()
                .map(|name| {
                    let theme = neoism_ui::primitives::IdeTheme::by_name(&name);
                    ThemeEntry {
                        dark: theme.is_dark(),
                        accent: format!("#{:06x}", theme.accent & 0xff_ff_ff),
                        name,
                    }
                })
                .collect();
        serde_wasm_bindgen::to_value(&entries).unwrap_or(JsValue::NULL)
    }
}

// Non-exported palette/finder internals (plain impl — `#[wasm_bindgen]`
// blocks may only contain exported methods).
impl ChromeBridge {
    /// Enter in BufferLines mode: desktop `confirm_finder_buffer_search`
    /// — jump to the selected row's match, arm `n`/`N` with the
    /// committed pattern, keep hlsearch bands, forget the origin. The
    /// caller (shim Enter handler / `modal_pointer_down`) closes the
    /// finder afterwards. An empty query behaves like Esc.
    fn confirm_finder_buffer_search_web(&mut self) -> bool {
        let query = self.chrome.finder.query.clone();
        let selected_line = self.chrome.finder.selected_line();
        BUFFER_SEARCH_SYNC.with(|cell| *cell.borrow_mut() = None);
        let Some(pane) = self.chrome.code_pane_mut() else {
            return true;
        };
        if query.is_empty() {
            if let Some((line, col)) = pane.search_origin.take() {
                pane.buffer.set_cursor_position(line, col, false);
                pane.buffer.follow_cursor = true;
            }
            pane.search_highlight = None;
            return true;
        }
        if let Some(row_line) = selected_line {
            let line_ix = (row_line as usize).saturating_sub(1);
            let col = pane
                .buffer
                .lines
                .get(line_ix)
                .and_then(|line| line.find(&query))
                .unwrap_or(0);
            pane.buffer.set_cursor_position(line_ix, col, false);
            pane.buffer.follow_cursor = true;
        }
        // No selected row (no matches): keep the cursor where the live
        // incsearch left it, but still commit the pattern.
        pane.search_highlight = Some(query.clone());
        pane.buffer.vim.search = Some(VimSearch {
            pattern: query,
            // `?` commits with the direction reversed: `n` continues
            // up, `N` back down (nvim semantics).
            forward: !pane.search_backward,
            whole_word: false,
        });
        pane.search_origin = None;
        true
    }

    /// Enter in BufferReplace mode: desktop
    /// `confirm_finder_buffer_replace` — parse `pattern/replacement`,
    /// run a whole-file global substitute through the `:s` engine (one
    /// undo step, count toast, hlsearch + `n` armed). An empty pattern
    /// behaves like Esc (restore the origin).
    fn confirm_finder_buffer_replace_web(&mut self) -> bool {
        let raw_query = self.chrome.finder.query.clone();
        let (pattern, replacement) = split_replace_query(&raw_query);
        BUFFER_SEARCH_SYNC.with(|cell| *cell.borrow_mut() = None);
        let notice = {
            let Some(pane) = self.chrome.code_pane_mut() else {
                return true;
            };
            if pattern.is_empty() {
                if let Some((line, col)) = pane.search_origin.take() {
                    pane.buffer.set_cursor_position(line, col, false);
                    pane.buffer.follow_cursor = true;
                }
                pane.search_highlight = None;
                return true;
            }
            pane.search_origin = None;
            let spec = SubstituteSpec {
                range: SubstituteRange::WholeFile,
                pattern: pattern.clone(),
                replacement: replacement.unwrap_or_default(),
                global: true,
                case_insensitive: false,
            };
            let outcome = apply_substitute(&mut pane.buffer, &spec);
            if outcome.substitutions > 0 {
                pane.search_highlight = Some(pattern.clone());
                pane.buffer.vim.search = Some(VimSearch {
                    pattern: pattern.clone(),
                    forward: true,
                    whole_word: false,
                });
                pane.buffer.follow_cursor = true;
            } else {
                pane.search_highlight = None;
            }
            substitute_outcome_message(&pattern, outcome)
        };
        self.chrome.notifications.push(notice, NotificationLevel::Info);
        true
    }

    /// Enter in References mode (Project Problems rows). Rows for the
    /// ACTIVE code pane jump chrome-side (the desktop
    /// `open_code_location` shape); rows for other files fall back to
    /// the generic finder-open intent so JS opens the tab.
    fn confirm_finder_reference_jump_web(&mut self) -> bool {
        let Some((path, line, col)) = self.chrome.finder.selected_reference_target()
        else {
            return false;
        };
        let is_active_pane = self
            .chrome
            .code_pane()
            .is_some_and(|pane| pane.path == path);
        if is_active_pane {
            if let Some(pane) = self.chrome.code_pane_mut() {
                pane.buffer.set_cursor_position(
                    (line as usize).saturating_sub(1),
                    col as usize,
                    false,
                );
                pane.buffer.follow_cursor = true;
            }
            return true;
        }
        let query = self.chrome.finder.query.clone();
        // Same deferred-goto mechanism as grep hits: the caret lands
        // on the problem/reference row's line (and column) once the
        // host opens the fetched file through `editor_open_file`.
        super::editor_panes::arm_editor_pending_goto(
            path.clone(),
            (line as usize).saturating_sub(1),
            col as usize,
        );
        self.pending_finder_open_intents.push(FinderOpenIntent {
            path: path.to_string_lossy().into_owned(),
            line: Some(line),
            mode: "grep",
            query,
        });
        true
    }

    // ------------------------------------------------------------
    // Per-frame palette upkeep (invoked from `drain_palette_intents`,
    // which the JS host calls at the top of every frame).
    // ------------------------------------------------------------

    /// Keep the shared palette's two visibility axes in lock-step with
    /// live chrome state:
    ///
    /// 1. Host capabilities — the web set, re-asserted so commands the
    ///    web cannot execute are never listed (desktop defaults to
    ///    all-true and is untouched).
    /// 2. Surface — the desktop `active_command_palette_surface`
    ///    mapping (notebook → Notebook, draw/markdown → Markdown,
    ///    code → Editor, else Terminal) derived from the hosted panes.
    ///
    /// Also drives the buffer-search live-preview (see
    /// `sync_finder_buffer_search`).
    fn sync_palette_host_context(&mut self) {
        self.chrome
            .command_palette
            .set_host_capabilities(PaletteHostCapabilities::web());
        let surface = match self.chrome.active_editor_pane_kind() {
            Some(EditorPaneKind::Code) => PaletteSurface::Editor,
            Some(EditorPaneKind::Notebook) => PaletteSurface::Notebook,
            // A `.neodraw` pane is a saveable document like markdown —
            // desktop reports it as the Markdown surface too.
            Some(EditorPaneKind::Draw) => PaletteSurface::Markdown,
            None => {
                if self.chrome.markdown_pane_mut().is_some() {
                    PaletteSurface::Markdown
                } else {
                    PaletteSurface::Terminal
                }
            }
        };
        self.chrome.command_palette.set_surface(surface);
        self.sync_finder_buffer_search();
    }

    /// Live-drive for the finder's BufferLines / BufferReplace modes —
    /// the web mirror of desktop's `finder_buffer_query_changed` (query
    /// edits move the cursor to the nearest match + hlsearch) and
    /// `finder_buffer_preview_selected` (arrowing rows previews them).
    /// Desktop hooks these off the input events; the web bridge has no
    /// such seam, so it diffs `(pattern, selected row)` per frame.
    ///
    /// Also owns Esc-cancel parity: when the finder leaves a buffer
    /// mode while the pane still has a `search_origin` (i.e. no commit
    /// ran), the origin cursor is restored and the bands cleared.
    fn sync_finder_buffer_search(&mut self) {
        let mode = self.chrome.finder.mode();
        let in_buffer_mode = self.chrome.finder.is_visible()
            && matches!(mode, FinderMode::BufferLines | FinderMode::BufferReplace);
        if !in_buffer_mode {
            let was_active =
                BUFFER_SEARCH_SYNC.with(|cell| cell.borrow_mut().take()).is_some();
            if was_active {
                if let Some(pane) = self.chrome.code_pane_mut() {
                    if let Some((line, col)) = pane.search_origin.take() {
                        pane.buffer.set_cursor_position(line, col, false);
                        pane.buffer.follow_cursor = true;
                        pane.search_highlight = None;
                    }
                }
            }
            return;
        }

        let raw_query = self.chrome.finder.query.clone();
        // Replace mode live-drives on the PATTERN half of the query.
        let query = if mode == FinderMode::BufferReplace {
            split_replace_query(&raw_query).0
        } else {
            raw_query
        };
        let selected = self.chrome.finder.selected_line();
        let key = (query.clone(), selected);
        let prev = BUFFER_SEARCH_SYNC.with(|cell| cell.borrow().clone());
        if prev.as_ref() == Some(&key) {
            return;
        }
        let query_changed = prev.as_ref().map(|(q, _)| q != &query).unwrap_or(true);
        BUFFER_SEARCH_SYNC.with(|cell| *cell.borrow_mut() = Some(key));

        if query_changed {
            let Some(pane) = self.chrome.code_pane_mut() else {
                return;
            };
            let Some((origin_line, origin_col)) = pane.search_origin else {
                return;
            };
            if query.is_empty() {
                pane.buffer
                    .set_cursor_position(origin_line, origin_col, false);
                pane.buffer.follow_cursor = true;
                pane.search_highlight = None;
                return;
            }
            pane.search_highlight = Some(query.clone());
            // Both helpers exclude the exact start position, so nudge
            // the origin one step the other way — a match AT the
            // origin is found either direction.
            let found = if pane.search_backward {
                vim_search_backward(
                    &pane.buffer.lines,
                    MarkdownPosition {
                        line: origin_line,
                        col: origin_col + 1,
                    },
                    &query,
                    false,
                )
            } else {
                let start = if origin_col > 0 {
                    MarkdownPosition {
                        line: origin_line,
                        col: origin_col - 1,
                    }
                } else if origin_line > 0 {
                    MarkdownPosition {
                        line: origin_line - 1,
                        col: usize::MAX,
                    }
                } else {
                    MarkdownPosition { line: 0, col: 0 }
                };
                vim_search_forward(&pane.buffer.lines, start, &query, false)
            };
            if let Some(found) = found {
                pane.buffer.set_cursor_position(found.line, found.col, false);
                pane.buffer.follow_cursor = true;
            }
        } else {
            // Selection moved: preview the selected row's match.
            let Some(row_line) = selected else {
                return;
            };
            let Some(pane) = self.chrome.code_pane_mut() else {
                return;
            };
            let line_ix = (row_line as usize).saturating_sub(1);
            let col = pane
                .buffer
                .lines
                .get(line_ix)
                .and_then(|line| line.find(&query))
                .unwrap_or(0);
            pane.buffer.set_cursor_position(line_ix, col, false);
            pane.buffer.follow_cursor = true;
        }
    }

    // ------------------------------------------------------------
    // Chrome-side palette dispatch — the web analogue of desktop's
    // `Screen::execute_palette_action` arms that never need the JS
    // host. Runs at drain time (after the modal-close that follows a
    // pick) so arms that re-open the palette in another mode work.
    // ------------------------------------------------------------

    /// Execute `intent` chrome-side when possible. `true` = consumed
    /// (withheld from JS); `false` = forward to the TS dispatcher.
    fn execute_palette_intent_chrome_side(&mut self, intent: &PaletteIntent) -> bool {
        match intent {
            PaletteIntent::Action { action } => {
                self.execute_palette_action_chrome_side(action)
            }
            PaletteIntent::ExCommand { command } => {
                self.execute_ex_command_chrome_side(command)
            }
            _ => false,
        }
    }

    fn execute_palette_action_chrome_side(&mut self, action: &str) -> bool {
        match action {
            // Desktop: `command_palette.enter_ex_mode()` — the typed
            // `:N` payload then jumps there (see the ex interception
            // below).
            "GoToLine" => {
                self.chrome.command_palette.enter_ex_mode();
                self.relayout_chrome();
                true
            }
            // Web has no pack files on disk — fall back to the shared
            // Themes picker (the same list the top-bar hamburger's
            // Themes entry opens); a pick flows through the normal
            // Theme intent into `set_ide_theme`.
            "OpenMashupPacks" => {
                let themes = neoism_ui::primitives::ide_theme::all_ide_theme_names();
                self.chrome.command_palette.enter_themes_mode(themes);
                self.relayout_chrome();
                self.chrome.notifications.push(
                    "Mash Up Packs need pack files on disk — showing Themes instead."
                        .to_string(),
                    NotificationLevel::Info,
                );
                true
            }
            // Desktop `toggle_code_word_wrap` (bridges/code/input.rs).
            "ToggleWordWrap" => {
                let Some(pane) = self.chrome.code_pane_mut() else {
                    return false;
                };
                pane.wrap = !pane.wrap;
                if pane.wrap {
                    pane.scroll_x = 0.0;
                }
                // Re-reveal: the cursor's visual row changes with the
                // layout.
                pane.buffer.follow_cursor = true;
                let wrap = pane.wrap;
                self.chrome.notifications.push(
                    if wrap { "Word wrap on" } else { "Word wrap off" }.to_string(),
                    NotificationLevel::Info,
                );
                true
            }
            // Desktop `toggle_code_vim_mode`: flip the CODE pane's
            // input mode when one owns focus; otherwise forward so the
            // terminal's scrollback vi mode toggles host-side.
            "ToggleViMode" => {
                let Some(pane) = self.chrome.code_pane_mut() else {
                    return false;
                };
                let entering = pane.input_mode == CodeInputMode::Standard;
                if entering {
                    pane.input_mode = CodeInputMode::Vim;
                    pane.buffer.mode = CodeMode::Normal;
                    pane.buffer.clear_selection();
                    pane.buffer.break_undo_group();
                    pane.buffer.snap_normal_cursor();
                } else {
                    pane.input_mode = CodeInputMode::Standard;
                    pane.buffer.mode = CodeMode::Insert;
                    pane.buffer.vim.clear_pending();
                    pane.buffer.clear_selection();
                }
                self.chrome.notifications.push(
                    if entering { "Vim mode on" } else { "Vim mode off" }.to_string(),
                    NotificationLevel::Info,
                );
                true
            }
            // Desktop `open_finder_buffer_search`: `/`-search over the
            // active code pane via the finder's BufferLines mode. No
            // code pane → forward (TS opens the grep finder instead).
            "SearchForward" | "SearchBackward" => {
                let backward = action == "SearchBackward";
                let lines = {
                    let Some(pane) = self.chrome.code_pane_mut() else {
                        return false;
                    };
                    pane.search_origin =
                        Some((pane.buffer.cursor_line, pane.buffer.cursor_col));
                    pane.search_backward = backward;
                    pane.buffer.lines.clone()
                };
                BUFFER_SEARCH_SYNC.with(|cell| *cell.borrow_mut() = None);
                self.chrome.command_palette.set_enabled(false);
                self.chrome.finder.open_buffer_lines(lines);
                self.relayout_chrome();
                true
            }
            // Desktop `open_finder_buffer_replace`: `pattern/replacement`
            // typed into the finder, Enter substitutes the whole file.
            "ReplaceInFile" => {
                let lines = {
                    let Some(pane) = self.chrome.code_pane_mut() else {
                        return false;
                    };
                    pane.search_origin =
                        Some((pane.buffer.cursor_line, pane.buffer.cursor_col));
                    pane.search_backward = false;
                    pane.buffer.lines.clone()
                };
                BUFFER_SEARCH_SYNC.with(|cell| *cell.borrow_mut() = None);
                self.chrome.command_palette.set_enabled(false);
                self.chrome.finder.open_buffer_replace(lines);
                self.relayout_chrome();
                true
            }
            // Desktop `open_neoworld_page`: the chrome-page tab. The
            // shared host falls back to a preview pet when the JS host
            // hasn't installed a persisted one yet, so this is safe to
            // route before the top-bar wiring lands.
            "OpenNeoWorld" => {
                self.chrome.open_neoworld_page_tab();
                self.relayout_chrome();
                true
            }
            // Desktop `open_project_problems`: diagnostics as a
            // References-mode finder list; Enter jumps to the line.
            "ProjectProblems" => {
                self.open_project_problems_web();
                true
            }
            // Desktop `open_neoism_notes_sidebar`. `toggle_notes_sidebar`
            // seeds the workspace + queues the data refresh the JS host
            // answers via `takeNotesRefresh`.
            "OpenNeoismNotes" => {
                if !self.chrome.notes_sidebar.is_visible() {
                    self.chrome.toggle_notes_sidebar();
                }
                self.relayout_chrome();
                true
            }
            "RunNotebookCell"
            | "InsertNotebookCodeCellAbove"
            | "InsertNotebookCodeCellBelow"
            | "InsertNotebookMarkdownCellAbove"
            | "InsertNotebookMarkdownCellBelow"
            | "DeleteNotebookCell"
            | "MoveNotebookCellUp"
            | "MoveNotebookCellDown"
            | "ClearNotebookCellOutput"
            | "ClearNotebookOutputs" => self.notebook_palette_op(action),
            _ => false,
        }
    }

    /// Chrome-side ex dispatch: bare `:N` line jumps (the Go to Line…
    /// payload) and the `:s` substitute family against the hosted
    /// panes. Everything else forwards to the TS ex dispatcher.
    fn execute_ex_command_chrome_side(&mut self, command: &str) -> bool {
        let trimmed = command.trim();
        if let Ok(line) = trimmed.parse::<usize>() {
            let line = line.max(1);
            if let Some(pane) = self.chrome.code_pane_mut() {
                let line_ix =
                    (line - 1).min(pane.buffer.lines.len().saturating_sub(1));
                pane.buffer.set_cursor_position(line_ix, 0, false);
                pane.buffer.follow_cursor = true;
                if pane.input_mode == CodeInputMode::Vim
                    && pane.buffer.mode == CodeMode::Normal
                {
                    pane.buffer.snap_normal_cursor();
                }
                return true;
            }
            if let Some(pane) = self.chrome.markdown_pane_mut() {
                pane.jump_to_line(line);
                return true;
            }
            return false;
        }
        if let Some(spec) = parse_substitute_command(trimmed) {
            let outcome = {
                let Some(pane) = self.chrome.code_pane_mut() else {
                    return false;
                };
                let outcome = apply_substitute(&mut pane.buffer, &spec);
                if outcome.substitutions > 0 {
                    pane.search_highlight = Some(spec.pattern.clone());
                    pane.buffer.vim.search = Some(VimSearch {
                        pattern: spec.pattern.clone(),
                        forward: true,
                        whole_word: false,
                    });
                    pane.buffer.follow_cursor = true;
                }
                outcome
            };
            self.chrome.notifications.push(
                substitute_outcome_message(&spec.pattern, outcome),
                NotificationLevel::Info,
            );
            return true;
        }
        false
    }

    /// Pane-local notebook cell operations (desktop
    /// `bridges/markdown/notebook.rs` arms). Errors surface on the
    /// shared notification stack, mirroring the notebook key path.
    fn notebook_palette_op(&mut self, action: &str) -> bool {
        let result: Option<Result<(), String>> = {
            let Some(pane) = self.chrome.notebook_pane_mut() else {
                return false;
            };
            match action {
                "RunNotebookCell" => Some(pane.run_current_cell()),
                "InsertNotebookCodeCellAbove" => {
                    Some(pane.insert_cell_above(NotebookCellType::Code).map(|_| ()))
                }
                "InsertNotebookCodeCellBelow" => {
                    Some(pane.insert_cell_below(NotebookCellType::Code).map(|_| ()))
                }
                "InsertNotebookMarkdownCellAbove" => Some(
                    pane.insert_cell_above(NotebookCellType::Markdown).map(|_| ()),
                ),
                "InsertNotebookMarkdownCellBelow" => Some(
                    pane.insert_cell_below(NotebookCellType::Markdown).map(|_| ()),
                ),
                "DeleteNotebookCell" => Some(pane.delete_current_cell().map(|_| ())),
                "MoveNotebookCellUp" => Some(pane.move_current_cell_up().map(|_| ())),
                "MoveNotebookCellDown" => {
                    Some(pane.move_current_cell_down().map(|_| ()))
                }
                "ClearNotebookCellOutput" => {
                    Some(pane.clear_current_output().map(|_| ()))
                }
                "ClearNotebookOutputs" => Some(pane.clear_all_outputs().map(|_| ())),
                _ => None,
            }
        };
        match result {
            None => false,
            Some(Ok(())) => true,
            Some(Err(err)) => {
                self.chrome.notifications.push(err, NotificationLevel::Error);
                true
            }
        }
    }

    /// Desktop `open_project_problems`, fed from the diagnostics the
    /// web host pushed for the ACTIVE buffer (`set_diagnostics`). Web
    /// diagnostics are per-active-buffer today, so the list covers the
    /// focused file; the References commit path jumps chrome-side.
    fn open_project_problems_web(&mut self) {
        use neoism_ui::panels::diagnostics_popup::Severity;
        let display = self.chrome.code_pane().map(|pane| {
            pane.path
                .strip_prefix(&self.workspace_root)
                .unwrap_or(&pane.path)
                .to_string_lossy()
                .into_owned()
        });
        let mut rows: Vec<ReferenceRow> = Vec::new();
        if let Some(display) = display {
            for item in &self.cached_diagnostics {
                let severity = match item.severity {
                    Severity::Error => "error",
                    Severity::Warn => "warn",
                    Severity::Info => "info",
                    Severity::Hint => "hint",
                };
                let mut message =
                    item.message.lines().next().unwrap_or("").to_string();
                if message.chars().count() > 160 {
                    message = message.chars().take(160).collect();
                }
                rows.push(ReferenceRow {
                    path: display.clone(),
                    line: item.lnum as u32,
                    column: 0,
                    text: format!("{severity}: {message}"),
                });
            }
        }
        if rows.is_empty() {
            self.chrome.notifications.push(
                "No problems reported".to_string(),
                NotificationLevel::Info,
            );
            return;
        }
        rows.sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
        self.chrome.command_palette.set_enabled(false);
        self.chrome
            .finder
            .open_references(self.workspace_root.clone(), rows);
        self.relayout_chrome();
    }
}

/// Desktop `run_code_substitute` toast wording, shared by the
/// BufferReplace commit and the chrome-side `:s` ex dispatch.
fn substitute_outcome_message(
    pattern: &str,
    outcome: neoism_ui::editor::code::substitute::SubstituteOutcome,
) -> String {
    if outcome.substitutions == 0 {
        format!("Pattern not found: {pattern}")
    } else {
        format!(
            "{} substitution{} on {} line{}",
            outcome.substitutions,
            if outcome.substitutions == 1 { "" } else { "s" },
            outcome.lines_changed,
            if outcome.lines_changed == 1 { "" } else { "s" },
        )
    }
}
