// Window-top chrome bar (panel toggle + hamburger menu) — thin
// click/move bridge that hands desktop `MouseState` coordinates to
// the shared [`neoism_ui::panels::chrome_topbar::ChromeTopBar`] and
// applies any resulting `TopBarAction`.
//
// Render lives in `host/run.rs`; layout sits above the buffer-tabs
// row (see `Renderer::top_bar_strip_height`).

use super::super::*;
use neoism_ui::panels::chrome_topbar::TopBarAction;

impl Screen<'_> {
    /// Hit-test the current mouse position against the top bar and
    /// apply any queued action. Returns `true` when the click landed
    /// on the bar so the caller short-circuits further panel dispatch.
    pub fn handle_top_bar_click(&mut self) -> bool {
        if !self.renderer.top_bar.is_visible() {
            return false;
        }
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();

        let consumed = self.renderer.top_bar.pointer_down(mouse_x, mouse_y);
        match self.renderer.top_bar.take_action() {
            Some(TopBarAction::TogglePanel) => {
                self.toggle_file_tree();
                true
            }
            Some(TopBarAction::ToggleRightPanel) => {
                if let Some(agent) =
                    self.context_manager.current_mut().neoism_agent.as_mut()
                {
                    agent.side_panel_mut().toggle_visibility();
                    self.mark_dirty();
                }
                true
            }
            Some(TopBarAction::OpenSettings) => {
                self.open_settings_config_tab();
                true
            }
            Some(TopBarAction::OpenWorkspaces) => {
                self.open_daemon_workspaces_picker();
                true
            }
            Some(TopBarAction::StartWebServer) => {
                self.start_web_frontend_server();
                true
            }
            Some(TopBarAction::OpenThemes) => {
                // Mirror the Cmd+P → Themes flow: open the real
                // theme-picker modal (the rich card with the live
                // swatch + preview), not just the palette in
                // "themes mode".
                self.open_theme_picker();
                true
            }
            Some(TopBarAction::OpenExtensions) => {
                self.open_extensions_page();
                true
            }
            None => {
                if consumed {
                    self.mark_dirty();
                }
                consumed
            }
        }
    }

    /// Open `~/.config/neoism/config.toml` as a buffer tab WITHOUT
    /// touching the workspace's cwd or nvim's `:cd`. The settings
    /// editor is a one-off look at a global file; the user's actual
    /// project root should stay put so navigation / file-tree /
    /// status-line stay scoped to the project they were working in.
    pub fn open_settings_config_tab(&mut self) {
        let path = neoism_backend::config::config_file_path();
        let workspace_root = self
            .active_pane_workspace_root()
            .or_else(|| self.active_workspace_root.clone());
        let already_active = self
            .renderer
            .buffer_tabs
            .active_path()
            .is_some_and(|active| active == path.as_path());
        self.renderer.buffer_tabs.ensure_terminal_tab();
        if !already_active {
            self.renderer.buffer_tabs.open_path(path.clone());
        } else {
            self.renderer.file_tree.set_active_path(Some(path.clone()));
        }

        // Path A: there's already a primary editor pane in this
        // workspace — just switch to it and tell nvim to `:edit` the
        // config file. No `:cd` so the project's cwd stays put.
        if let Some((editor_node, editor_route)) = self.primary_editor_node_and_route() {
            self.context_manager
                .current_grid_mut()
                .set_current_node(editor_node, &mut self.sugarloaf);
            self.context_manager.select_route_from_current_grid();
            self.send_editor_command_to_route(
                editor_route,
                neoism_backend::performer::nvim::vim_edit_command(
                    &path.display().to_string(),
                ),
            );
            self.reapply_chrome_layout();
            self.renderer.trail_cursor.reset();
            self.mark_dirty();
            return;
        }

        // Path B: no editor pane exists yet — spawn one as a stacked
        // peer of the terminal and open the config file in it. The
        // editor inherits the workspace root so its `getcwd()` keeps
        // matching the project the user opened.
        let rich_text_id = next_rich_text_id();
        let _ = self.sugarloaf.text(Some(rich_text_id));
        let old_index = self.context_manager.current_index();
        if self.context_manager.add_stacked_editor(
            path.clone(),
            rich_text_id,
            &mut self.sugarloaf,
            workspace_root.clone(),
        ) {
            self.reapply_chrome_layout();
        } else if let Some(new_ix) = self.context_manager.add_editor_tab(
            path.clone(),
            rich_text_id,
            workspace_root,
        ) {
            self.context_manager.switch_context_visibility(
                &mut self.sugarloaf,
                old_index,
                new_ix,
            );
        }
        self.renderer.trail_cursor.reset();
        self.mark_dirty();
    }

    /// Mouse hover bridge — keeps the bar's hover highlights in sync
    /// with the desktop pointer even when no click fires.
    pub fn handle_top_bar_hover(&mut self) -> bool {
        if !self.renderer.top_bar.is_visible() {
            return false;
        }
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();
        self.renderer.top_bar.pointer_move(mouse_x, mouse_y);
        true
    }

    pub fn start_web_frontend_server(&mut self) {
        let web_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|path| path.parent())
            .map(|repo| repo.join("neoism-frontend/web"));
        let Some(web_dir) = web_dir else {
            self.renderer.notifications.push(
                "Could not locate neoism-frontend/web.",
                neoism_ui::panels::notifications::NotificationLevel::Error,
            );
            self.mark_dirty();
            return;
        };

        let url = "http://127.0.0.1:5173";
        if !web_frontend_port_listening() {
            match std::process::Command::new("npm")
                .arg("run")
                .arg("dev")
                .arg("--")
                .arg("--host")
                .arg("0.0.0.0")
                .current_dir(&web_dir)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                Ok(_) => self.renderer.notifications.push(
                    "Starting Neoism web server on 127.0.0.1:5173...",
                    neoism_ui::panels::notifications::NotificationLevel::Info,
                ),
                Err(err) => {
                    self.renderer.notifications.push(
                        format!("Failed to start web server: {err}"),
                        neoism_ui::panels::notifications::NotificationLevel::Error,
                    );
                    self.mark_dirty();
                    return;
                }
            }
        }

        open_url_in_browser(url);
        self.mark_dirty();
    }
}

fn web_frontend_port_listening() -> bool {
    std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, 5173)).is_ok()
}

fn open_url_in_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg(url);
        command
    };

    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    };

    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(url);
        command
    };

    let _ = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}
