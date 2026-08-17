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
            Some(TopBarAction::OpenAgent) => {
                self.open_neoism_agent_tab();
                self.mark_dirty();
                true
            }
            Some(TopBarAction::OpenServers) => {
                self.request_server_manager();
                true
            }
            Some(TopBarAction::OpenSettings) => {
                self.open_settings_panel();
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
                // Mirror Cmd+P → Themes: searchable list + live preview.
                self.open_theme_picker();
                true
            }
            Some(TopBarAction::OpenExtensions) => {
                self.open_extensions_page();
                true
            }
            Some(TopBarAction::OpenNeoWorld) => {
                self.open_neoworld_page();
                true
            }
            Some(TopBarAction::OpenSearch) => {
                self.open_finder_files();
                true
            }
            Some(TopBarAction::OpenNotes) => {
                self.open_neoism_notes_sidebar();
                true
            }
            Some(TopBarAction::OpenAbout) => {
                self.open_about();
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

    /// Open the unified config file as a buffer tab WITHOUT touching
    /// the workspace's cwd. The settings editor is a one-off look at a
    /// global file; the user's actual project root should stay put so
    /// navigation / file-tree / status-line stay scoped to the project
    /// they were working in.
    pub fn open_settings_config_tab(&mut self) {
        let path = neoism_backend::config::config_file_path();
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
        // Native code editor hosts the config buffer now (nvim removed).
        self.activate_code_path(path);
        self.reapply_chrome_layout();
        self.renderer.trail_cursor.reset();
        self.mark_dirty();
    }

    /// Open the About modal — app name, version, and build commit.
    pub fn open_about(&mut self) {
        use neoism_ui::widgets::modal::{ModalAction, ModalButton, ModalSpec};
        let version = env!("CARGO_PKG_VERSION");
        let commit = option_env!("GIT_HASH").unwrap_or("dev");
        let body = format!(
            "Neoism  v{version}\n\nA terminal-first workspace for code, notes,\nagents, and multiplayer editing.\n\nCommit\n{commit}"
        );
        self.renderer.modal.open(ModalSpec {
            title: "About Neoism".to_string(),
            body,
            meta: String::new(),
            input: None,
            buttons: vec![ModalButton::new("OK", "Enter", ModalAction::Close)],
            busy: false,
            blocking: true,
        });
        self.mark_dirty();
    }

    /// Open the Zed-style GUI settings panel, seeded with the current
    /// config so its controls reflect what's on disk.
    pub fn open_settings_panel(&mut self) {
        // Raw config value so every key (terminal + agent) is present.
        let values = neoism_backend::config::load_config_json_value();
        self.renderer.settings.set_values(values);
        let families = self.sugarloaf.font_family_names();
        self.renderer.settings.set_font_families(families);
        self.renderer.settings.open();
        self.mark_dirty();
    }

    /// Click router for the settings panel — persists any control change
    /// straight to config.json (the watcher hot-reloads it live).
    pub fn handle_settings_click(&mut self) -> bool {
        if !self.renderer.settings.is_active() {
            return false;
        }
        let (mx, my) = self.mouse_logical_for_hit_test();
        let outcome = self.renderer.settings.pointer_down(mx, my);
        if let Some(action) = outcome.action {
            self.apply_settings_action(action);
        }
        self.mark_dirty();
        outcome.consumed
    }

    /// Hover router for the settings panel.
    #[allow(dead_code)]
    pub fn handle_settings_hover(&mut self) -> bool {
        if !self.renderer.settings.is_active() {
            return false;
        }
        let (mx, my) = self.mouse_logical_for_hit_test();
        self.renderer.settings.pointer_move(mx, my);
        true
    }

    pub(crate) fn apply_settings_action(
        &mut self,
        action: neoism_ui::panels::SettingsAction,
    ) {
        use neoism_ui::panels::SettingsAction;
        match action {
            SettingsAction::Set { key, value } => {
                if let Err(err) = neoism_backend::config::write_setting(key, value) {
                    tracing::warn!(target: "neoism::config", %err, key, "settings write failed");
                }
            }
            SettingsAction::SetKeybind { action, key, with } => {
                if let Err(err) =
                    neoism_backend::config::write_keybind(action, &key, &with)
                {
                    tracing::warn!(target: "neoism::config", %err, action, "keybind write failed");
                }
                let msg = if key.is_empty() {
                    format!("Reset {action} to its default — restart to apply")
                } else {
                    format!("Rebound {action} — restart to apply")
                };
                self.renderer.notifications.push(
                    msg,
                    neoism_ui::panels::notifications::NotificationLevel::Info,
                );
            }
            SettingsAction::OpenConfigFile => {
                self.renderer.settings.close();
                self.open_settings_config_tab();
            }
            SettingsAction::RunAction(action) => {
                if action == "open-model" {
                    // Reuse the agent pane's model + provider (connect) picker.
                    self.renderer.settings.close();
                    let _ = self.open_neoism_agent_tab();
                    if let Some(agent) =
                        self.context_manager.current_mut().neoism_agent.as_mut()
                    {
                        agent.open_model_picker();
                    }
                }
            }
        }
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

        let dist_index = web_dir.join("dist/index.html");
        if !dist_index.is_file() {
            self.renderer.notifications.push(
                "Neoism web build is missing. Run `npm run build` in neoism-frontend/web first.",
                neoism_ui::panels::notifications::NotificationLevel::Error,
            );
            self.mark_dirty();
            return;
        }

        let url = "http://127.0.0.1:5173";
        if !web_frontend_port_listening() {
            let child = std::process::Command::new("npm")
                .arg("run")
                .arg("preview")
                .arg("--")
                .arg("--host")
                .arg("0.0.0.0")
                .arg("--port")
                .arg("5173")
                .arg("--strictPort")
                .current_dir(&web_dir)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .spawn();
            let mut child = match child {
                Ok(child) => child,
                Err(err) => {
                    self.renderer.notifications.push(
                        format!("Failed to start built web server: {err}"),
                        neoism_ui::panels::notifications::NotificationLevel::Error,
                    );
                    self.mark_dirty();
                    return;
                }
            };

            let mut launch_error = None;
            for _ in 0..60 {
                if web_frontend_port_listening() {
                    break;
                }
                match child.try_wait() {
                    Ok(Some(status)) => {
                        use std::io::Read;
                        let mut stderr = String::new();
                        if let Some(mut pipe) = child.stderr.take() {
                            let _ = pipe.read_to_string(&mut stderr);
                        }
                        let detail = stderr
                            .lines()
                            .rev()
                            .find(|line| !line.trim().is_empty())
                            .unwrap_or("no error output");
                        launch_error = Some(format!(
                            "Built web server exited with {status}: {detail}"
                        ));
                        break;
                    }
                    Ok(None) => {}
                    Err(err) => {
                        launch_error = Some(format!(
                            "Could not check built web server status: {err}"
                        ));
                        break;
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }

            if launch_error.is_none() && !web_frontend_port_listening() {
                let _ = child.kill();
                launch_error = Some(
                    "Built web server did not become ready on port 5173.".to_string(),
                );
            }
            if let Some(error) = launch_error {
                self.renderer.notifications.push(
                    error,
                    neoism_ui::panels::notifications::NotificationLevel::Error,
                );
                self.mark_dirty();
                return;
            }
        }

        self.renderer.notifications.push(
            "Neoism web build is ready on http://127.0.0.1:5173.",
            neoism_ui::panels::notifications::NotificationLevel::Info,
        );
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
