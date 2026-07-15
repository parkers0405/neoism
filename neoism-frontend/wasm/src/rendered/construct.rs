use super::*;
use neoism_ui::layout::Rect as ChromeRect;
use neoism_ui::panels::agent_pane::state::NeoismAgentPane;
use neoism_ui::panels::breadcrumbs::Breadcrumbs;
use neoism_ui::panels::buffer_tabs::BufferTab;
use neoism_ui::panels::completion_menu::CompletionMenu;
use neoism_ui::panels::context_menu::ContextMenu;
use neoism_ui::panels::cursorline_overlay::CursorlineOverlay;
use neoism_ui::panels::diagnostics_popup::DiagnosticsPopup;
use neoism_ui::panels::editor_scroll::EditorScroll;
use neoism_ui::panels::minimap::Minimap;
use neoism_ui::panels::notifications::Notifications;
use neoism_ui::panels::search::SearchOverlay;
use neoism_ui::panels::trail_cursor::TrailCursor;
use neoism_ui::panels::yank_flash::YankFlash;
use neoism_ui::panels::{
    DiagnosticCounts, FileTree, GitChangeSummary, Mode, PrimaryKind, StatusInfo,
};
use neoism_ui::primitives::IdeTheme;
use neoism_ui::terminal_blocks::TerminalInputBuffer;
use neoism_ui::widgets::island::Island;
use neoism_ui::{Chrome, CustomCursor, GitBranch};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

#[wasm_bindgen]
impl ChromeBridge {
    /// Async constructor. Builds the sugarloaf-on-canvas terminal
    /// renderer, installs a `FileTree` rooted at `workspace_root`,
    /// and runs an initial layout for the given cell grid.
    #[wasm_bindgen(js_name = "new")]
    pub async fn new(
        canvas: web_sys::HtmlCanvasElement,
        cols: u32,
        rows: u32,
        scale: f32,
        workspace_root: String,
    ) -> Result<ChromeBridge, JsValue> {
        let rendered = RenderedTerminal::new(canvas, cols, rows, scale).await?;

        // Default-everything chrome, then install the file tree
        // sidebar at the requested root. Hosts that want a wider
        // / narrower sidebar can flip via `set_file_tree_width`
        // through a follow-up bridge method (omitted for now).
        let mut chrome: Chrome<()> = Chrome::new();
        // Default IdeTheme — pastel_dark. JS pushes the user's
        // preferred theme name via `ChromeBridge::set_ide_theme`
        // shortly after construction; this seeds the static
        // ACTIVE_IDE_THEME cell so any first-paint shims that fire
        // before that call still read a consistent palette.
        chrome.set_ide_theme(IdeTheme::default().name.as_str());
        // The lifted CommandPalette ships its own native COMMANDS
        // catalog (mode-aware: ex commands, fonts, buffers, etc.) —
        // no host-side `set_commands` install needed.
        chrome.buffer_tabs.set_visible(true);
        chrome.buffer_tabs.set_tabs(
            vec![BufferTab {
                title: "Terminal 1".to_string(),
                modified: false,
                path: None,
                markdown: false,
                scratch_id: None,
                terminal_route_id: None,
                neoism_agent_route_id: None,
                chrome_page: None,
                agent_kind: None,
            }],
            0,
        );
        let root = if workspace_root.is_empty() {
            PathBuf::from(".")
        } else {
            PathBuf::from(workspace_root)
        };
        // Mirror the desktop's first-paint state. `StatusInfo` only
        // has the fields below — `position`/`file_type`/`encoding`/
        // `eol`/`indent` don't exist on this struct yet, so they're
        // intentionally omitted. `lsp_status: None` is how the
        // panel encodes "LSP Off" (the `LspStatus` enum has no
        // `Off` variant; `None` hides the pill entirely).
        chrome.status_line.set_info(StatusInfo {
            mode: Mode::Terminal,
            primary: "Terminal".to_string(),
            primary_kind: PrimaryKind::Terminal,
            cwd_label: Some(root.to_string_lossy().into_owned()),
            project: Some("neoism".to_string()),
            branch: Some("main".to_string()),
            git_changes: Some(GitChangeSummary::default()),
            lsp_status: None,
            diagnostics: DiagnosticCounts::default(),
            cursor_lines: None,
            ..StatusInfo::default()
        });
        chrome.install_file_tree(FileTree::new(root.clone()));
        chrome.set_workspace_root_path(Some(root.clone()));
        chrome.install_agent_pane(NeoismAgentPane::with_directory(Some(
            root.to_string_lossy().into_owned(),
        )));

        // W3-A: install the remaining slim panels so the JS-side
        // bridge state pushes (W3-B) have a uniform install
        // surface to target. Each `install_*` replaces the
        // auto-constructed default with a fresh instance.
        // Constructed with `::new()` defaults — the bridge's
        // state-push methods seed per-frame data (toast messages,
        // popup items, cursor positions, etc.) afterwards.
        chrome.install_breadcrumbs(Breadcrumbs::new());
        chrome.install_completion_menu(CompletionMenu::new());
        chrome.install_minimap(Minimap::new());
        chrome.install_notifications(Notifications::new());
        chrome.install_diagnostics_popup(DiagnosticsPopup::new());
        chrome.install_context_menu(ContextMenu::new());
        chrome.install_search(SearchOverlay::default());
        chrome.install_git_branch(GitBranch::new());
        chrome.install_custom_cursor(CustomCursor::new());
        chrome.install_cursorline_overlay(CursorlineOverlay::new());
        chrome.install_trail_cursor(TrailCursor::new());
        chrome.install_yank_flash(YankFlash::new());
        chrome.install_editor_scroll(EditorScroll::new());

        let shared = SharedState(Rc::new(RefCell::new(JsServiceState::new())));

        // Initial viewport: the cell grid times the 8x16 logical
        // cell-size approximation `RenderedTerminal::resize` uses.
        // The host can call `resize()` with explicit pixel
        // dimensions once they're known.
        let viewport = ChromeRect::new(0.0, 0.0, cols as f32 * 8.0, rows as f32 * 16.0);
        let island_theme = IdeTheme::default();
        let mut bridge = ChromeBridge {
            rendered,
            chrome,
            files: Box::new(JsFilesService(shared.clone())),
            clipboard: Box::new(JsClipboardService(shared.clone())),
            commands: Box::new(JsCommandService(shared.clone())),
            git: Box::new(JsGitService(shared.clone())),
            clock: Box::new(JsClockService(shared.clone())),
            search: Box::new(JsSearchService(shared.clone())),
            notifications: Box::new(JsNotificationService(shared.clone())),
            workspace_root: root,
            splash_tui_guard: true,
            services_state: shared,
            viewport,
            active_tab_index: 0,
            tab_contents: std::collections::HashMap::new(),
            last_markdown_viewport_h: 600.0,
            markdown_crdt_client_id: generate_crdt_client_id(),
            editor_viewport_textoff: 0,
            markdown_crdt_binding: None,
            crdt_outbound: Vec::new(),
            tab_paths: std::collections::HashMap::new(),
            tab_kinds: std::collections::HashMap::new(),
            active_font_scale: 1.0,
            editor_grid_snapshot: None,
            editor_grid_snapshots:
                neoism_ui::editor_snapshot::EditorGridSnapshotStore::new(),
            editor_grid_surface_id: None,
            editor_default_fg: 0x00FF_FFFF,
            editor_default_bg: 0x0000_0000,
            editor_viewport_topline: 0,
            editor_viewport_botline: 0,
            editor_viewport_line_count: 0,
            pending_grid_scroll_animation_rows: 0,
            nvim_send: None,
            pty_outbox: None,
            last_dpr_scale: 1.0,
            terminal_blocks: TerminalInputBuffer::default(),
            pending_agent_tab_opens: 0,
            pending_finder_open_intents: Vec::new(),
            pending_palette_intents: Vec::new(),
            agent_state: AgentBridgeState::default(),
            cached_diagnostics: Vec::new(),
            editor_surfaces: Vec::new(),
            workspace_island: Island::new(
                island_theme.f32(island_theme.muted),
                island_theme.f32(island_theme.fg),
                island_theme.f32(island_theme.border),
                true,
            ),
            workspace_island_tabs: Vec::new(),
            workspace_island_active_id: None,
            pending_workspace_island_intents: Vec::new(),
        };
        bridge.relayout_chrome();
        Ok(bridge)
    }

    /// User-facing font zoom. Ctrl+= / Ctrl+- / Ctrl+0 in JS funnel
    /// through here. `scale` is the absolute multiplier (1.0 =
    /// default cell size); the bridge clamps to `[0.5, 3.0]` so a
    /// runaway ramp can't make the chrome unreadable / unrenderable.
    ///
    /// Recomputes cell metrics from the logical 8x16 base (the same
    /// base `resize`/`RenderedTerminal::resize` use as their first-
    /// paint estimate) scaled by the new factor, pushes them into
    /// chrome via `set_cell_metrics`, and re-runs `set_layout` so
    /// panels reflow before the next paint.
    ///
    /// TODO: wire actual sugarloaf scale_factor when the API lands
    /// — today `Sugarloaf::rescale` is the device-pixel-ratio knob
    /// and conflating it with a user font zoom would warp the
    /// surface size. For now only chrome cell metrics re-flow; the
    /// terminal grid keeps its current advance, so glyphs don't
    /// visually grow until that follow-up arrives.
    pub fn set_font_scale(&mut self, scale: f32) {
        let clamped = scale.clamp(0.5, 3.0);
        self.active_font_scale = clamped;
        let cell_w = 8.0 * clamped;
        let cell_h = 16.0 * clamped;
        self.chrome.set_chrome_scale(clamped);
        self.chrome.set_cell_metrics(cell_w, cell_h);
        self.workspace_island.set_scale(clamped);
        self.relayout_chrome();
    }

    /// Current font scale (last value passed to `set_font_scale`,
    /// or `1.0` if never set). JS reads this back to fold subsequent
    /// zoom presses geometrically.
    pub fn font_scale(&self) -> f32 {
        self.active_font_scale
    }

    /// Toggle Crosswords vi mode for the terminal surface. This is
    /// the web equivalent of desktop's `PaletteAction::ToggleViMode`.
    pub fn toggle_vi_mode(&mut self) {
        self.rendered.terminal.inner.toggle_vi_mode();
    }

    /// Replace the command-palette contents with a font-family list
    /// and keep the palette open. JSON shape: `["Family", ...]`.
    pub fn enter_palette_fonts_mode(&mut self, fonts_json: &str) -> Result<(), JsValue> {
        let fonts: Vec<String> = serde_json::from_str(fonts_json)
            .map_err(|e| JsValue::from_str(&format!("palette fonts parse: {e}")))?;
        self.chrome.command_palette.enter_fonts_mode(fonts);
        // Reflow so `layout.command_palette` is Some — the palette
        // only draws inside its layout rect, and the Enter that
        // picked the command closed the palette and reflowed it
        // away. Without this the new mode stays invisible until
        // the next input event re-runs the layout.
        self.relayout_chrome();
        Ok(())
    }

    /// Replace the command-palette contents with an IDE theme list
    /// and keep the palette open. JSON shape: `["pastel_dark", ...]`.
    pub fn enter_palette_themes_mode(
        &mut self,
        themes_json: &str,
    ) -> Result<(), JsValue> {
        let themes: Vec<String> = serde_json::from_str(themes_json)
            .map_err(|e| JsValue::from_str(&format!("palette themes parse: {e}")))?;
        self.chrome.command_palette.enter_themes_mode(themes);
        // Reflow — see enter_palette_fonts_mode.
        self.relayout_chrome();
        Ok(())
    }

    /// Replace the command-palette contents with runtime shader
    /// choices and keep the palette open. JSON shape:
    /// `[{ "title": String, "detail": String, "filter": Option<String> }]`.
    pub fn enter_palette_shaders_mode(
        &mut self,
        shaders_json: &str,
    ) -> Result<(), JsValue> {
        let shaders: Vec<neoism_ui::panels::command_palette::PaletteShaderEntry> =
            serde_json::from_str(shaders_json)
                .map_err(|e| JsValue::from_str(&format!("palette shaders parse: {e}")))?;
        self.chrome.command_palette.enter_shaders_mode(shaders);
        // Reflow — see enter_palette_fonts_mode.
        self.relayout_chrome();
        Ok(())
    }

    /// Replace the command-palette contents with the host's buffer
    /// list and keep the palette open. JSON shape:
    /// `[{ "title": String, "detail": String, "tab_index": usize }, ...]`.
    pub fn enter_palette_buffers_mode(
        &mut self,
        buffers_json: &str,
    ) -> Result<(), JsValue> {
        use neoism_ui::panels::command_palette::{
            PaletteBufferEntry, PaletteBufferTarget,
        };

        #[derive(serde::Deserialize)]
        struct JsBuffer {
            title: String,
            #[serde(default)]
            detail: String,
            tab_index: usize,
        }

        let parsed: Vec<JsBuffer> = serde_json::from_str(buffers_json)
            .map_err(|e| JsValue::from_str(&format!("palette buffers parse: {e}")))?;
        let entries = parsed
            .into_iter()
            .map(|b| PaletteBufferEntry {
                title: b.title,
                detail: b.detail,
                target: PaletteBufferTarget::Pane {
                    route_id: 0,
                    tab_index: b.tab_index,
                },
            })
            .collect();
        self.chrome.command_palette.enter_buffers_mode(entries);
        // Reflow — see enter_palette_fonts_mode.
        self.relayout_chrome();
        Ok(())
    }

    /// Open the command palette as the grouped host→workspace tree
    /// — the desktop's Ctrl+Shift+W "Workspaces" modal
    /// (`Screen::open_daemon_workspaces_picker`). The JS host builds
    /// the payload from its `HostWorkspaceTree` state. JSON shape:
    ///
    /// ```json
    /// {
    ///   "workspaces": [{
    ///     "title": "neoism", "detail": "/home/x/neoism",
    ///     "workspace_id": "ws-1", "host_id": "host-a",
    ///     "host_label": "framework", "host_kind": "local",
    ///     "daemon_url": null, "host_online": true
    ///   }],
    ///   "peer_hosts": [{
    ///     "host_id": "tailnet:mac", "label": "mac",
    ///     "kind": "remote",
    ///     "daemon_url": "ws://100.64.0.2:7878/session",
    ///     "online": true
    ///   }]
    /// }
    /// ```
    ///
    /// `host_kind` / `kind` accept `"local" | "remote" | "cloud"`
    /// (anything else falls back to `remote`). Selecting a
    /// workspace row queues a `PaletteIntent::Workspace` for
    /// `drain_palette_intents`. Drag-to-move is rendered but
    /// view-only on web for now (no host-side move dispatch).
    pub fn open_workspaces_palette(&mut self, payload_json: &str) -> Result<(), JsValue> {
        let (entries, peer_hosts) = parse_workspaces_payload(payload_json)?;
        self.chrome
            .command_palette
            .enter_workspaces_mode_with_hosts(entries, peer_hosts);
        // Reflow — see enter_palette_fonts_mode.
        self.relayout_chrome();
        Ok(())
    }

    /// True while the command palette is open in Workspaces mode.
    /// The web host checks this on every daemon `HostWorkspaceTree`
    /// push so the open modal live-refreshes instead of going stale.
    pub fn workspaces_palette_open(&self) -> bool {
        self.chrome.command_palette.workspaces_mode_open()
    }

    /// Swap a fresh host→workspace tree into the ALREADY-OPEN
    /// Workspaces modal, preserving the user's query and selection.
    /// No-op when the palette isn't in Workspaces mode.
    pub fn refresh_workspaces_palette(
        &mut self,
        payload_json: &str,
    ) -> Result<(), JsValue> {
        let (entries, peer_hosts) = parse_workspaces_payload(payload_json)?;
        self.chrome
            .command_palette
            .refresh_workspaces_tree(entries, peer_hosts);
        // Reflow — the refreshed tree can change the modal height.
        self.relayout_chrome();
        Ok(())
    }
}
