use super::*;

use crate::animation::CriticallyDampedSpring;
use crate::input::SimpleInputBuffer;
use crate::layout::{ChromeLayout, Rect};
use crate::panels::agent_pane::state::NeoismAgentPane;
use crate::panels::breadcrumbs::Breadcrumbs;
use crate::panels::buffer_tabs::BufferTabTarget;
use crate::panels::completion_menu::CompletionMenu;
use crate::panels::context_menu::ContextMenu;
use crate::panels::diagnostics_popup::DiagnosticsPopup;
use crate::panels::minimap::Minimap;
use crate::panels::notifications::Notifications;
use crate::panels::search::SearchOverlay;
use crate::panels::splash_overlay::SplashOverlay;
use crate::panels::trail_cursor::TrailCursor;
use crate::panels::yank_flash::YankFlash;

use crate::panels::chrome_topbar::{ChromeTopBar, TopBarAction};
use crate::panels::git_diff::GitDiffPanel;
use crate::panels::notes_sidebar::NotesSidebar;
use crate::panels::pane_grid::PaneGrid;
use crate::panels::{
    BufferTabs, CommandComposer, CommandPalette, FileTree, Finder, GitDiff, StatusLine,
};
use crate::primitives::IdeTheme;
use crate::theme::ChromeTheme;

impl<A: Send + Copy + 'static> Chrome<A> {
    pub fn new() -> Self {
        let ide_theme = IdeTheme::pastel_dark();
        let theme = ChromeTheme::from_ide_theme(&ide_theme);
        if let Ok(mut g) = ACTIVE_IDE_THEME.write() {
            *g = Some(ide_theme);
        }

        Self {
            layout: ChromeLayout {
                top_bar: None,
                file_tree: None,
                notes_sidebar: None,
                buffer_tabs: Rect::new(0.0, 0.0, 0.0, 0.0),
                breadcrumbs: None,
                status_line: Rect::new(0.0, 0.0, 0.0, 0.0),
                terminal: Rect::new(0.0, 0.0, 0.0, 0.0),
                command_palette: None,
                finder: None,
                git_diff: None,
                command_composer: None,
                panes: Vec::new(),
            },
            theme,
            ide_theme,
            cursor_color_override: None,
            cursor_style: crate::cursor_style::CursorStyle::default(),
            file_tree_width: DEFAULT_FILE_TREE_WIDTH,
            cell_w: 8.0,
            cell_h: 16.0,
            chrome_scale: 1.0,
            top_workspace_strip_h: 0.0,
            bottom_content_inset: 0.0,
            mobile_web_agent_panel_enabled: false,
            mobile_agent_narrow: false,
            desktop_agent_panel_open_before_narrow: None,
            animation_phase: 0.0,
            active_tab_index: 0,
            tab_content: None,
            terminal_input: SimpleInputBuffer::default(),
            tab_lang: crate::syntax::Lang::Other,
            markdown_pane: None,
            code_pane: None,
            notebook_pane: None,
            draw_pane: None,
            editor_pane_tab: None,
            editor_pane_animating: false,
            code_doc_binding: None,
            code_lsp: Default::default(),
            lsp_popup: crate::panels::lsp_popup::LspPopup::new(),
            parked_code_panes: std::collections::HashMap::new(),
            parked_notebook_panes: std::collections::HashMap::new(),
            parked_draw_panes: std::collections::HashMap::new(),
            status_line: StatusLine::new(),
            buffer_tabs: BufferTabs::<A>::new(),
            top_bar: ChromeTopBar::new(),
            file_tree: None,
            command_palette: CommandPalette::new(),
            finder: Finder::new(),
            git_diff: GitDiff::new(),
            git_diff_panel: GitDiffPanel::new(),
            notes_sidebar: NotesSidebar::default(),
            command_composer: CommandComposer::new(),
            terminal_cwd_label: None,
            pane_grid: PaneGrid::new(crate::session_layout::SessionLeafKind::Terminal, 0),
            pane_surfaces: Vec::new(),
            pane_tabs: std::collections::HashMap::new(),
            pane_breadcrumbs: std::collections::HashMap::new(),
            host_drawn_panes: Vec::new(),
            agent_pane: None,
            splash_overlay: SplashOverlay::new(),
            terminal_splash_dismissed: false,
            breadcrumbs: Breadcrumbs::new(),
            notifications: Notifications::new(),
            completion_menu: CompletionMenu::new(),
            search_overlay: SearchOverlay::default(),
            minimap: Minimap::new(),
            yank_flash: YankFlash::new(),
            trail_cursor: TrailCursor::new(),
            diagnostics_popup: DiagnosticsPopup::new(),
            context_menu: ContextMenu::new(),
            git_branch: GitBranch::new(),
            custom_cursor: CustomCursor::new(),
            settings_page: crate::panels::settings_page::NeoismSettingsPane::new(),
            extensions_page: crate::panels::extensions_page::NeoismExtensionsPane::new(),
            neoworld_pane: None,
            modal: crate::widgets::modal::UniversalModal::new(),
            connection_gate_active: false,
            file_browser: crate::panels::file_browser::FileBrowserModal::new(),
            pending_settings_actions: Vec::new(),
            pending_extensions_actions: Vec::new(),
            pending_neoworld_snapshots: Vec::new(),
            focus_stack: Vec::new(),
            scroll_spring: CriticallyDampedSpring::new(),
            scroll_offset_px: 0.0,
            last_pointer_pos: (0.0, 0.0),
            pending_buffer_tab_closes: Vec::new(),
            pending_buffer_tab_activate: None,
            pending_buffer_tab_new: false,
            pending_top_bar_action: None,
            pending_panel_open_paths: Vec::new(),
            pending_git_panel_refresh: false,
            pending_notes_refresh: false,
            last_viewport: None,
            workspace_root_path: None,
            notes_vault_root: None,
            share_sheet: crate::panels::share_sheet::ShareSheet::new(),
            last_draw_time: None,
        }
    }

    pub(crate) fn apply_top_bar_action(&mut self, action: TopBarAction) {
        match action {
            TopBarAction::TogglePanel => {
                // Strict visibility toggle — click 1 opens, click 2
                // closes, regardless of focus state. The chrome's
                // pointer-down handler defocuses the tree whenever
                // the click lands outside its rect (including on the
                // top-bar's panel button), so the focus-aware
                // `toggle_file_tree` would treat the second click as
                // "tree visible but unfocused → just refocus" and the
                // panel would never close. Pin to plain show/hide.
                let visible = self.file_tree.as_ref().is_some_and(|t| t.is_visible());
                if visible {
                    self.hide_file_tree();
                } else {
                    self.show_file_tree();
                }
            }
            TopBarAction::OpenAgent => {
                self.pending_top_bar_action = Some(TopBarAction::OpenAgent);
            }
            TopBarAction::ToggleAgentSidePanel => {
                if self.mobile_agent_narrow && self.is_neoism_agent_tab_active() {
                    if let Some(pane) = self.agent_pane.as_mut() {
                        pane.toggle_side_panel();
                    }
                    self.relayout();
                }
            }
            TopBarAction::OpenThemes => {
                // Open the SAME theme picker the user gets from Cmd+P →
                // Themes, hosted entirely in shared chrome so it works on
                // web. The desktop bridge handles the top-bar click on its
                // own path (`bridges/top_bar.rs`) and opens a richer modal,
                // so this branch is web-facing: the desktop never reaches
                // `apply_top_bar_action` for the hamburger menu. The
                // `Modal` widget has no host in the web build, but the
                // palette themes mode IS hosted + rendered here, and its
                // Enter handler already applies the selected theme — so
                // this closes the "hamburger menu modals don't appear on
                // web" gap for Themes without depending on JS wiring.
                let themes = crate::primitives::ide_theme::all_ide_theme_names();
                self.command_palette.enter_themes_mode(themes);
                self.relayout();
            }
            TopBarAction::OpenServers => {
                self.pending_top_bar_action = Some(TopBarAction::OpenServers);
            }
            TopBarAction::ShareWithPhone => {
                // The host has to resolve a reachable URL from the daemon
                // before anything can be drawn, so this is queued rather
                // than handled here.
                self.pending_top_bar_action = Some(TopBarAction::ShareWithPhone);
            }
            TopBarAction::OpenNotes => {
                // Strict visibility toggle, same contract as
                // `TogglePanel` above: click 1 opens, click 2 closes.
                // This used to be open-ONLY (`if !visible { toggle }`),
                // so the notes button could never close the sidebar it
                // had just opened. `toggle_notes_sidebar` on its own is
                // focus-aware (`toggle_focus_or_visibility`) and would
                // only move focus for an already-open panel, which is
                // why the close path is spelled out here.
                if self.notes_sidebar.is_visible() {
                    self.notes_sidebar.set_visible(false);
                    self.notes_sidebar.set_focused(false);
                    self.relayout();
                } else {
                    self.toggle_notes_sidebar();
                    self.pending_notes_refresh = true;
                }
            }
            TopBarAction::OpenSearch => {
                // Desktop's top-bar search opens the Files finder. Consume
                // this in shared chrome rather than round-tripping through
                // the WASM host, so the first press opens/focuses immediately
                // and the second press strictly closes it.
                if self.finder.is_visible() {
                    self.finder.set_enabled(false);
                } else {
                    self.command_palette.set_enabled(false);
                    self.finder
                        .open_files(self.workspace_root_path.clone().unwrap_or_default());
                }
                self.relayout();
            }
            other => {
                // No shared-hosted surface yet — store for the bridge to
                // drain and route. Desktop opens Settings (config tab),
                // Workspaces (daemon picker), Extensions (page), and the
                // web-server launcher; web has no shared host for those
                // yet (see report — documented gap).
                self.pending_top_bar_action = Some(other);
            }
        }
    }

    /// Drain a host-routable top bar intent (Settings / Themes /
    /// Extensions). Returns `None` when there is nothing pending.
    pub fn drain_top_bar_action(&mut self) -> Option<TopBarAction> {
        self.pending_top_bar_action.take()
    }

    /// Replace the chrome theme. Cheap clone; called when the system
    /// light/dark setting flips or the user picks a new theme.
    pub fn set_theme(&mut self, theme: ChromeTheme) {
        self.theme = theme;
    }

    pub fn theme(&self) -> &ChromeTheme {
        &self.theme
    }

    /// Replace the active IdeTheme (richer palette than `ChromeTheme`).
    /// Resolves `name` through [`IdeThemeName::from_str`] — unknown names
    /// fall back to `pastel_dark`. Also updates the derived `ChromeTheme`
    /// so panels reading through `PanelContext::theme` see the new palette,
    /// and publishes to the process-wide [`ACTIVE_IDE_THEME`] cell so the
    /// slim adapter shims pick up the same theme on their next paint.
    pub fn set_ide_theme(&mut self, name: &str) {
        let resolved = IdeTheme::by_name(name);
        self.ide_theme = resolved;
        self.theme = ChromeTheme::from_ide_theme(&resolved);
        if let Ok(mut g) = ACTIVE_IDE_THEME.write() {
            *g = Some(resolved);
        }
    }

    pub fn ide_theme(&self) -> &IdeTheme {
        &self.ide_theme
    }

    /// Configure the user cursor style: an optional `#RRGGBB` override
    /// (beats the theme color, survives theme switches) and a preset
    /// name (`"rainbow"` animates and ignores the color entirely;
    /// anything else is solid).
    pub fn set_cursor_style_config(&mut self, color_hex: Option<&str>, style: &str) {
        self.cursor_color_override = color_hex
            .and_then(crate::cursor_style::parse_hex_color)
            .map(crate::cursor_style::hex_to_f32);
        self.cursor_style = crate::cursor_style::CursorStyle::from_str(style);
    }

    /// The color the LOCAL cursor wears this frame: rainbow preset >
    /// user override > theme fg (the web chrome's historical default).
    pub(crate) fn live_cursor_color(&self) -> [f32; 4] {
        match self.cursor_style {
            crate::cursor_style::CursorStyle::Rainbow => {
                crate::cursor_style::rainbow_color_f32(
                    crate::cursor_style::rainbow_now_seconds(),
                )
            }
            crate::cursor_style::CursorStyle::Solid => self
                .cursor_color_override
                .unwrap_or_else(|| self.ide_theme.f32(self.ide_theme.fg)),
        }
    }

    /// True when any cursor on screen is rainbow-animated — the LOCAL
    /// preset, or a remote peer broadcasting the rainbow flag — so the
    /// host keeps repainting while otherwise idle.
    pub(crate) fn rainbow_cursor_active(&self) -> bool {
        self.cursor_style.is_animated()
            || self.markdown_pane.as_ref().is_some_and(|pane| {
                pane.remote_cursors.iter().any(|cursor| cursor.rainbow)
            })
    }

    /// Install a file tree at the given root. Replaces any existing
    /// tree. Call [`Chrome::set_layout`] after this for the sidebar
    /// column to appear.
    pub fn install_file_tree(&mut self, mut tree: FileTree) {
        tree.set_width(self.file_tree_width);
        tree.set_scale(self.chrome_scale);
        self.file_tree = Some(tree);
    }

    /// Install the shared Neoism Agent pane state. The pane paints into
    /// the main terminal rect whenever a Neoism Agent buffer tab is active.
    pub fn install_agent_pane(&mut self, pane: NeoismAgentPane) {
        self.agent_pane = Some(pane);
    }

    /// Enable web/mobile responsive Agent chrome. Desktop intentionally never
    /// calls this, even if its window happens to be phone-width.
    pub fn set_mobile_web_agent_panel_enabled(&mut self, enabled: bool) {
        self.mobile_web_agent_panel_enabled = enabled;
        if !enabled {
            if let Some(was_open) = self.desktop_agent_panel_open_before_narrow.take() {
                if let Some(pane) = self.agent_pane.as_mut() {
                    pane.side_panel_mut().set_user_hidden(!was_open);
                }
            }
            self.mobile_agent_narrow = false;
            self.top_bar.set_mobile_agent_panel_button_visible(false);
        }
        self.relayout();
    }

    pub fn agent_pane(&self) -> Option<&NeoismAgentPane> {
        self.agent_pane.as_ref()
    }

    pub fn agent_pane_mut(&mut self) -> Option<&mut NeoismAgentPane> {
        self.agent_pane.as_mut()
    }

    // --------- Slim-panel installers ---------------------------------
    //
    // Each `install_<name>` replaces the panel's auto-constructed
    // instance with one supplied by the host. Mirrors the
    // `install_file_tree` pattern: handy when the host wants a
    // pre-seeded panel (theme-driven scale, custom defaults, etc.)
    // instead of the chrome's stock construction. Panels without
    // per-instance state (`GitBranch`, `CustomCursor`) accept the
    // unit-struct handle so the bridge's install ordering is uniform.

    pub fn install_breadcrumbs(&mut self, panel: Breadcrumbs) {
        self.breadcrumbs = panel;
    }

    pub fn install_completion_menu(&mut self, panel: CompletionMenu) {
        self.completion_menu = panel;
    }

    pub fn install_minimap(&mut self, panel: Minimap) {
        self.minimap = panel;
    }

    pub fn install_notifications(&mut self, panel: Notifications) {
        self.notifications = panel;
    }

    pub fn install_diagnostics_popup(&mut self, panel: DiagnosticsPopup) {
        self.diagnostics_popup = panel;
    }

    pub fn install_context_menu(&mut self, panel: ContextMenu) {
        self.context_menu = panel;
    }

    pub fn install_search(&mut self, panel: SearchOverlay) {
        self.search_overlay = panel;
    }

    pub fn install_git_branch(&mut self, panel: GitBranch) {
        self.git_branch = panel;
    }

    pub fn install_custom_cursor(&mut self, panel: CustomCursor) {
        self.custom_cursor = panel;
    }

    pub fn install_trail_cursor(&mut self, panel: TrailCursor) {
        self.trail_cursor = panel;
    }

    pub fn install_yank_flash(&mut self, panel: YankFlash) {
        self.yank_flash = panel;
    }

    /// Desktop-parity file-tree toggle:
    ///
    /// - hidden -> show and focus
    /// - visible + focused -> hide
    /// - visible + unfocused -> focus without changing width/layout
    pub fn toggle_file_tree(&mut self) -> bool {
        let Some(tree) = self.file_tree.as_mut() else {
            return false;
        };
        let (focus_tree, visibility_changed) = if !tree.is_visible() {
            tree.set_visible(true);
            tree.set_focused(true);
            (true, true)
        } else if tree.is_focused() {
            tree.set_focused(false);
            tree.set_visible(false);
            (false, true)
        } else {
            tree.set_focused(true);
            (true, false)
        };
        if focus_tree {
            self.focus(PanelKey::FileTree);
        } else {
            self.blur(PanelKey::FileTree);
        }
        visibility_changed
    }

    pub fn show_file_tree(&mut self) -> bool {
        let Some(tree) = self.file_tree.as_mut() else {
            return false;
        };
        let visibility_changed = !tree.is_visible();
        tree.set_visible(true);
        tree.set_focused(true);
        self.focus(PanelKey::FileTree);
        visibility_changed
    }

    pub fn hide_file_tree(&mut self) -> bool {
        let Some(tree) = self.file_tree.as_mut() else {
            return false;
        };
        let visibility_changed = tree.is_visible();
        tree.set_focused(false);
        tree.set_visible(false);
        self.blur(PanelKey::FileTree);
        visibility_changed
    }

    /// Override the file-tree sidebar width. Takes effect on the next
    /// `set_layout` call. Width is clamped to `[120.0, 600.0]` so a
    /// rogue caller can't shrink the column off-screen or eat the
    /// whole window.
    pub fn set_file_tree_width(&mut self, width: f32) {
        self.file_tree_width = width.clamp(120.0, 600.0);
        if let Some(tree) = self.file_tree.as_mut() {
            tree.set_width(self.file_tree_width);
        }
    }

    pub fn file_tree_width(&self) -> f32 {
        self.file_tree
            .as_ref()
            .map(FileTree::width)
            .unwrap_or(self.file_tree_width)
    }

    /// Snapshot of the current per-panel layout rects.
    pub fn layout(&self) -> &ChromeLayout {
        &self.layout
    }

    /// Set the resolved logical-pixel cell metrics for the active
    /// terminal font. Surfaces that paint over the terminal grid
    /// (notably the splash wordmark + menu) use these instead of
    /// hard-coded defaults so glyph + image placement aligns with the
    /// host's actual cell grid.
    pub fn set_cell_metrics(&mut self, cell_w: f32, cell_h: f32) {
        self.cell_w = cell_w.max(1.0);
        self.cell_h = cell_h.max(1.0);
    }

    pub fn set_top_workspace_strip_height(&mut self, height: f32) {
        self.top_workspace_strip_h = height.max(0.0);
    }

    pub fn cell_metrics(&self) -> (f32, f32) {
        (self.cell_w, self.cell_h)
    }

    pub fn set_animation_phase(&mut self, phase: f32) {
        self.animation_phase = if phase.is_finite() {
            phase.rem_euclid(10_000.0)
        } else {
            0.0
        };
    }

    pub fn set_chrome_scale(&mut self, scale: f32) {
        let clamped = scale.clamp(0.5, 3.0);
        self.chrome_scale = clamped;
        if let Some(tree) = self.file_tree.as_mut() {
            tree.set_scale(clamped);
        }
        self.buffer_tabs.set_scale(clamped);
        self.top_bar.set_scale(clamped);
        self.breadcrumbs.set_scale(clamped);
        self.notifications.set_scale(clamped);
        self.finder.set_scale(clamped);
        self.command_palette.set_scale(clamped);
        self.context_menu.set_scale(clamped);
        self.completion_menu.set_scale(clamped);
        self.status_line.set_scale(clamped);
        self.command_composer.set_scale(clamped);
        self.diagnostics_popup.set_scale(clamped);
        self.minimap.set_scale(clamped);
        self.git_diff_panel.set_scale(clamped);
        self.notes_sidebar.set_scale(clamped);
    }

    pub fn chrome_scale(&self) -> f32 {
        self.chrome_scale
    }

    /// Select which buffer-tab the user is viewing. `0` is the live
    /// terminal pane; any other index switches the terminal rect into
    /// a file-viewer that paints [`Chrome::tab_content`] instead of
    /// the terminal cells.
    pub fn set_active_tab_index(&mut self, idx: usize) {
        if idx != self.active_tab_index {
            // Reset the file-viewer scroll so a new tab opens at the
            // top instead of inheriting the previous tab's offset.
            self.scroll_offset_px = 0.0;
            self.scroll_spring.reset();
        }
        self.active_tab_index = idx;
    }

    /// Remove the buffer tab at `idx` from the strip and queue a
    /// close intent for the host to drain. The host owns the
    /// canonical buffer list (titles + cached contents), so chrome
    /// only mirrors the user's request — the host will replay
    /// `set_buffer_tabs` with the new list after acting.
    pub fn close_buffer_tab(&mut self, idx: usize) {
        let _ = self.buffer_tabs.close_at(idx);
        // The strip's own `close_at` shifts `active` down if the
        // removed tab was below it; mirror that into chrome's own
        // `active_tab_index` so the file-viewer paint stays in sync.
        let new_active = self.buffer_tabs.active();
        if new_active != self.active_tab_index {
            self.set_active_tab_index(new_active);
        }
        self.pending_buffer_tab_closes.push(idx);
    }

    /// Take the queued buffer-tab close intents. The host bridge
    /// pulls these once per frame and updates JS-side bookkeeping.
    pub fn drain_buffer_tab_closes(&mut self) -> Vec<usize> {
        std::mem::take(&mut self.pending_buffer_tab_closes)
    }

    /// Take the most recent buffer-tab activate intent.
    pub fn drain_buffer_tab_activate(&mut self) -> Option<usize> {
        self.pending_buffer_tab_activate.take()
    }

    /// Take the pending "+" new-tab click intent. `true` at most once
    /// per click on the strip's trailing new-tab button.
    pub fn drain_buffer_tab_new(&mut self) -> bool {
        std::mem::take(&mut self.pending_buffer_tab_new)
    }

    pub fn active_tab_index(&self) -> usize {
        self.active_tab_index
    }

    /// True when the selected buffer tab is the Rust-rendered Neoism
    /// Agent surface. The agent is a tab in the main strip, so it
    /// consumes the terminal rect instead of docking as a side pane.
    ///
    /// Suspended (false) while a full-screen chrome overlay is up so
    /// the agent pane's pointer/key routes stand down under the
    /// settings page / About modal, mirroring the terminal gate.
    pub fn is_neoism_agent_tab_active(&self) -> bool {
        if self.chrome_overlay_active() {
            return false;
        }
        matches!(
            self.buffer_tabs.target_at(self.active_tab_index),
            Some(BufferTabTarget::NeoismAgent(_))
        )
    }

    /// True when the active tab is a live terminal pane. Hosts read
    /// this to decide whether to draw terminal cells + splash, or
    /// paint the cached tab content instead.
    ///
    /// Keyed on the tab's TARGET, not its index: terminal tabs are the
    /// only targetless tabs, and restored workspaces routinely put a
    /// file tab at slot 0 (and fresh "Terminal 2" tabs past it). The
    /// old `index == 0` shortcut painted the splash under a markdown
    /// tab and left every additional terminal tab black.
    ///
    /// A full-screen chrome overlay (settings page / About modal)
    /// suspends the terminal surface while it is up — desktop parity:
    /// terminal compose is gated on `!settings.is_active()` there, and
    /// on web this same gate keeps the terminal grid from eating the
    /// overlay's pointer input.
    pub fn is_terminal_tab_active(&self) -> bool {
        if self.chrome_overlay_active() {
            return false;
        }
        self.is_terminal_tab_selected()
    }

    /// Terminal target identity without transient overlay suppression. Hosts
    /// use this to maintain the composer's configured shell-state request
    /// while a Settings/modal layer temporarily owns the keyboard.
    pub fn is_terminal_tab_selected(&self) -> bool {
        self.buffer_tabs.target_at(self.active_tab_index).is_none()
    }
}

#[cfg(test)]
mod top_bar_toggle_tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn file_tree_action_opens_focused_then_closes() {
        let mut chrome = Chrome::<()>::new();
        chrome.install_file_tree(FileTree::new(PathBuf::from("/workspace")));

        chrome.apply_top_bar_action(TopBarAction::TogglePanel);
        let tree = chrome.file_tree.as_ref().unwrap();
        assert!(tree.is_visible());
        assert!(tree.is_focused());

        chrome.apply_top_bar_action(TopBarAction::TogglePanel);
        assert!(!chrome.file_tree.as_ref().unwrap().is_visible());
    }

    #[test]
    fn notes_action_opens_focused_then_closes() {
        let mut chrome = Chrome::<()>::new();
        chrome.apply_top_bar_action(TopBarAction::OpenNotes);
        assert!(chrome.notes_sidebar.is_visible());
        assert!(chrome.notes_sidebar.is_focused());

        chrome.apply_top_bar_action(TopBarAction::OpenNotes);
        assert!(!chrome.notes_sidebar.is_visible());
        assert!(!chrome.notes_sidebar.is_focused());
    }

    #[test]
    fn search_action_opens_files_finder_then_closes_without_host_intent() {
        let mut chrome = Chrome::<()>::new();
        chrome.set_workspace_root_path(Some(PathBuf::from("/workspace")));

        chrome.apply_top_bar_action(TopBarAction::OpenSearch);
        assert!(chrome.finder.is_visible());
        assert_eq!(
            chrome.finder.mode(),
            crate::panels::finder::FinderMode::Files
        );
        assert_eq!(chrome.drain_top_bar_action(), None);

        chrome.apply_top_bar_action(TopBarAction::OpenSearch);
        assert!(!chrome.finder.is_visible());
        assert_eq!(chrome.drain_top_bar_action(), None);
    }
}
