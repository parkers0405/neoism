// Auto-split from screen/mod.rs. See sibling mod.rs for the Screen struct and
// the constructor/core methods. This file is part of the impl Screen<'_> block.

use super::super::*;
use neoism_backend::clipboard::{Clipboard, ClipboardType};
use neoism_terminal_core::crosswords::pos::Direction;
use neoism_ui::panels::command_palette::PaletteAction;
use neoism_window::keyboard::{Key, ModifiersState};

impl Screen<'_> {
    pub(crate) fn open_edit_server_form(
        &mut self,
        id: String,
        address: String,
        name: String,
        token: String,
    ) {
        use neoism_ui::widgets::modal::{ModalFormField, ModalFormSpec};
        self.renderer.modal.open_form(ModalFormSpec {
            title: "Edit server".into(),
            fields: vec![
                ModalFormField {
                    id: "server_id".into(),
                    label: String::new(),
                    value: id,
                    placeholder: String::new(),
                    secret: true,
                },
                ModalFormField {
                    id: "address".into(),
                    label: "Server address".into(),
                    value: address,
                    placeholder: "http://localhost:7878".into(),
                    secret: false,
                },
                ModalFormField {
                    id: "name".into(),
                    label: "Server name (optional)".into(),
                    value: name,
                    placeholder: "Home workstation".into(),
                    secret: false,
                },
                ModalFormField {
                    id: "token".into(),
                    label: "Password (optional)".into(),
                    value: token,
                    placeholder: "password".into(),
                    secret: true,
                },
            ],
            submit_label: "Save server".into(),
        });
    }

    pub(crate) fn open_add_server_prompt(&mut self) {
        use neoism_ui::widgets::modal::{ModalFormField, ModalFormSpec};

        self.renderer.modal.open_form(ModalFormSpec {
            title: "Add server".to_string(),
            fields: vec![
                ModalFormField {
                    id: "address".into(),
                    label: "Server address".into(),
                    value: String::new(),
                    placeholder: "ws://192.168.1.20:7981/session".into(),
                    secret: false,
                },
                ModalFormField {
                    id: "name".into(),
                    label: "Server name (optional)".into(),
                    value: String::new(),
                    placeholder: "Home workstation".into(),
                    secret: false,
                },
                ModalFormField {
                    id: "token".into(),
                    label: "Password (optional)".into(),
                    value: String::new(),
                    placeholder: "password".into(),
                    secret: true,
                },
            ],
            submit_label: "Join server".into(),
        });
        self.renderer.modal.set_form_tabs(
            vec!["Join server".to_string(), "Create server".to_string()],
            0,
        );
        self.mark_dirty();
    }

    /// The CREATE half of the Create/Join pair: pick a folder, get a
    /// server hosting it from this machine, and auto-join. Submission
    /// lands in the `ServerFormSubmit` arm (the `create_dir` field is
    /// the discriminator) → `create_and_join_local_server`.
    pub(crate) fn open_create_server_prompt(&mut self) {
        use neoism_ui::widgets::modal::{ModalFormField, ModalFormSpec};

        let default_dir = self
            .workspace_root_for_new_shell()
            .map(|root| root.display().to_string())
            .unwrap_or_default();
        self.renderer.modal.open_form(ModalFormSpec {
            title: "Add server".to_string(),
            fields: vec![
                ModalFormField {
                    id: "create_dir".into(),
                    label: "Project folder".into(),
                    value: default_dir,
                    placeholder: "~/code/myproject".into(),
                    secret: false,
                },
                ModalFormField {
                    id: "name".into(),
                    label: "Server name (optional)".into(),
                    value: String::new(),
                    placeholder: "Team server".into(),
                    secret: false,
                },
                ModalFormField {
                    id: "token".into(),
                    label: "Password (optional)".into(),
                    value: String::new(),
                    placeholder: "password".into(),
                    secret: true,
                },
            ],
            submit_label: "Create & join".into(),
        });
        self.renderer.modal.set_form_tabs(
            vec!["Join server".to_string(), "Create server".to_string()],
            1,
        );
        self.mark_dirty();
    }

    /// Create-and-join: spawn this machine's own daemon (+ agent, when
    /// the binary is present) on a free LAN-visible port for `dir`,
    /// register it, and hand off to the normal join path. The spawned
    /// processes outlive the app on purpose — a server the user
    /// created keeps serving until they stop it.
    pub(crate) fn create_and_join_local_server(
        &mut self,
        dir: String,
        name: Option<String>,
        password: Option<String>,
    ) {
        use neoism_ui::panels::notifications::NotificationLevel;

        let dir = if let Some(rest) = dir.strip_prefix("~/") {
            match std::env::var("HOME") {
                Ok(home) => std::path::PathBuf::from(home).join(rest),
                Err(_) => std::path::PathBuf::from(dir),
            }
        } else {
            std::path::PathBuf::from(dir)
        };
        if !dir.is_dir() {
            self.renderer.notifications.push(
                format!("`{}` is not a folder on this machine.", dir.display()),
                NotificationLevel::Warn,
            );
            self.mark_dirty();
            return;
        }

        // First port that binds; dropped immediately so the daemon can
        // take it (the tiny race is fine — a failed spawn surfaces in
        // the join error).
        let Some(port) = (9877u16..9900).find(|port| {
            std::net::TcpListener::bind(("0.0.0.0", *port)).is_ok()
        }) else {
            self.renderer.notifications.push(
                "No free port between 9877 and 9899 — stop an old server first."
                    .to_string(),
                NotificationLevel::Error,
            );
            self.mark_dirty();
            return;
        };

        // Installed and dev layouts both put the daemon next to the
        // desktop binary; PATH is the fallback.
        let sibling = |bin: &str| -> std::path::PathBuf {
            std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|dir| dir.join(bin)))
                .filter(|path| path.is_file())
                .unwrap_or_else(|| std::path::PathBuf::from(bin))
        };
        let daemon_bin = sibling("neoism-workspace-daemon");
        let state_dir = neoism_backend::config::config_dir_path()
            .join("hosted-servers")
            .join(port.to_string());
        let _ = std::fs::create_dir_all(&state_dir);

        let mut daemon = std::process::Command::new(&daemon_bin);
        daemon
            .arg("--addr")
            .arg(format!("0.0.0.0:{port}"))
            .arg("--no-unix-socket")
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("--workspace")
            .arg(&dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if let Some(password) = password.as_deref() {
            daemon
                .env("NEOISM_DAEMON_TOKEN", password)
                .env("NEOISM_REQUIRE_AUTH", "1");
        }
        if let Err(error) = daemon.spawn() {
            self.renderer.notifications.push(
                format!(
                    "Could not start the server daemon ({}): {error}",
                    daemon_bin.display()
                ),
                NotificationLevel::Error,
            );
            self.mark_dirty();
            return;
        }

        // Co-hosted agent on port + 1 (the convention joined clients
        // derive). Missing binary just means no remote agent — the
        // server itself is unaffected.
        let agent_bin = sibling("neoism-agent");
        if agent_bin.is_file() || agent_bin.components().count() == 1 {
            let _ = std::process::Command::new(&agent_bin)
                .arg("serve")
                .arg("--port")
                .arg((port + 1).to_string())
                .arg("--hostname")
                .arg("0.0.0.0")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }

        // Hand off to the normal add+join path (it dials with retry, so
        // the daemon's startup wins the race).
        let address = format!("ws://127.0.0.1:{port}/session");
        let name = name.or_else(|| {
            dir.file_name()
                .map(|n| n.to_string_lossy().into_owned())
        });
        self.request_server_add(address, name, password.clone());

        // Best-effort LAN address for sharing (UDP connect never sends
        // a packet; it just resolves the outbound interface).
        let lan_ip = std::net::UdpSocket::bind("0.0.0.0:0")
            .and_then(|socket| {
                socket.connect("8.8.8.8:80")?;
                socket.local_addr()
            })
            .map(|addr| addr.ip().to_string())
            .unwrap_or_else(|_| "<your-ip>".to_string());
        let share = match password.as_deref() {
            Some(_) => format!(
                "Serving {} — share ws://{lan_ip}:{port}/session (+ the password)",
                dir.display()
            ),
            None => format!(
                "Serving {} — share ws://{lan_ip}:{port}/session",
                dir.display()
            ),
        };
        self.renderer
            .notifications
            .push(share, NotificationLevel::Info);
        self.mark_dirty();
    }

    pub(crate) fn active_command_palette_surface(
        &self,
    ) -> neoism_ui::panels::command_palette::PaletteSurface {
        use neoism_ui::panels::command_palette::PaletteSurface;

        let current = self.context_manager.current();
        // A `.neodraw` pane is a saveable document like markdown — report
        // it as the Markdown surface so Write File / save shows up.
        if current.notebook.is_some() {
            PaletteSurface::Notebook
        } else if current.markdown.is_some() || current.draw.is_some() {
            PaletteSurface::Markdown
        } else if current.editor.is_some() {
            PaletteSurface::Editor
        } else {
            PaletteSurface::Terminal
        }
    }

    pub(crate) fn open_command_palette(&mut self) {
        let surface = self.active_command_palette_surface();
        self.renderer.command_palette.set_surface(surface);
        self.renderer.command_palette.set_workspace_visibility(
            self.context_manager
                .workspace_visibility_for_index(self.context_manager.current_index()),
        );
        self.renderer.command_palette.set_enabled(true);
        self.mark_dirty();
    }

    /// Direction-aware `/`-palette commit command. Forward sessions use
    /// the stock `vim_search_commit_command`; a `?` session routes the
    /// same `rio.search.commit` with the backward flag so the jump goes
    /// to the previous match and `n`/`N` stay reversed afterwards.
    pub(crate) fn palette_search_commit_command(
        &self,
        query: &str,
        location: Option<(u64, u64)>,
    ) -> String {
        if !self.renderer.command_palette.search_is_backward() {
            return neoism_backend::performer::nvim::vim_search_commit_command(
                query, location,
            );
        }
        let query = neoism_backend::performer::nvim::lua_string_literal(query);
        match location {
            Some((lnum, col)) => format!(
                "lua pcall(function() require('rio.search').commit({query}, {lnum}, {col}, true) end)"
            ),
            None => format!(
                "lua pcall(function() require('rio.search').commit({query}, nil, nil, true) end)"
            ),
        }
    }

    pub fn run_palette_ex_query(&mut self, query: &str) -> bool {
        let cmd = query.trim().trim_start_matches(':').trim();
        if cmd.is_empty() {
            return false;
        }

        if self.try_intercept_ex_command(cmd) {
            return true;
        }

        if self.context_manager.current().editor.is_some() {
            self.send_editor_command_raw(
                neoism_backend::performer::nvim::vim_run_ex_command(cmd),
            );
            return true;
        }

        false
    }

    pub fn open_workspace_buffers_picker(&mut self) {
        let entries = self.workspace_buffer_picker_entries();
        self.renderer.command_palette.enter_buffers_mode(entries);
        self.mark_dirty();
    }

    pub fn activate_palette_buffer(
        &mut self,
        target: neoism_ui::panels::command_palette::PaletteBufferTarget,
    ) {
        match target {
            neoism_ui::panels::command_palette::PaletteBufferTarget::Workspace(ix) => {
                let _ = self.activate_workspace_buffer_tab(ix);
            }
            neoism_ui::panels::command_palette::PaletteBufferTarget::Pane {
                route_id,
                tab_index,
            } => self.pane_tab_activate(route_id, tab_index),
        }
        self.renderer.file_tree.set_focused(false);
        self.mark_dirty();
    }

    pub(crate) fn is_command_palette_key(
        key: &neoism_window::event::KeyEvent,
        mods: ModifiersState,
    ) -> bool {
        mods.super_key()
            && !mods.shift_key()
            && !mods.control_key()
            && !mods.alt_key()
            && (matches!(key.physical_key, PhysicalKey::Code(KeyCode::KeyP))
                || matches!(key.key_without_modifiers().as_ref(), Key::Character(ch) if ch.eq_ignore_ascii_case("p")))
    }

    pub(crate) fn collect_popup_items(
        &self,
        pill: neoism_ui::panels::status_line::DiagnosticPill,
    ) -> Vec<neoism_ui::panels::diagnostics_popup::PopupItem> {
        use neoism_ui::panels::status_line::DiagnosticPill;
        let current = self.context_manager.current();
        let Some(diags) = current.editor_diagnostics.as_ref() else {
            return Vec::new();
        };
        // nvim's `vim.diagnostic.severity`: 1=error, 2=warn. The popup
        // only opens for those two pills.
        let target_severity: u8 = match pill {
            DiagnosticPill::Error => 1,
            DiagnosticPill::Warn => 2,
        };
        // Lift each backend `DiagnosticItem` into the shared POD shape
        // first so the lifted `PopupItem::From<&_>` impl picks it up.
        diags
            .items
            .iter()
            .filter(|d| d.severity == target_severity)
            .map(|d| crate::bridges::translate::diagnostic_item_from_nvim(d))
            .map(|snap| neoism_ui::panels::diagnostics_popup::PopupItem::from(&snap))
            .collect()
    }

    pub(crate) fn open_lsp_rename_prompt(&mut self) {
        self.renderer
            .modal
            .open(neoism_ui::widgets::modal::ModalSpec {
                title: "Rename Symbol".to_string(),
                body: "Enter the new name for the symbol under the cursor.".to_string(),
                meta: "Runs the attached LSP rename request.".to_string(),
                input: Some(neoism_ui::widgets::modal::ModalInputSpec {
                    value: String::new(),
                    placeholder: "new_name".to_string(),
                }),
                buttons: vec![
                neoism_ui::widgets::modal::ModalButton::new(
                    "Rename",
                    "Enter",
                    neoism_ui::widgets::modal::ModalAction::RunEditorCommandWithInput {
                        command: "Rename".to_string(),
                        value: String::new(),
                    },
                ),
                neoism_ui::widgets::modal::ModalButton::new(
                    "Close",
                    "Esc",
                    neoism_ui::widgets::modal::ModalAction::Close,
                ),
            ],
                busy: false,
                blocking: true,
            });
        self.mark_dirty();
    }

    pub(crate) fn open_lsp_workspace_symbols_prompt(&mut self) {
        self.renderer
            .modal
            .open(neoism_ui::widgets::modal::ModalSpec {
                title: "Workspace Symbols".to_string(),
                body: "Search symbols across the active LSP workspace.".to_string(),
                meta: "Results open as Neoism actions.".to_string(),
                input: Some(neoism_ui::widgets::modal::ModalInputSpec {
                    value: String::new(),
                    placeholder: "symbol query".to_string(),
                }),
                buttons: vec![
                neoism_ui::widgets::modal::ModalButton::new(
                    "Search",
                    "Enter",
                    neoism_ui::widgets::modal::ModalAction::RunEditorCommandWithInput {
                        command: "WorkspaceSymbols".to_string(),
                        value: String::new(),
                    },
                ),
                neoism_ui::widgets::modal::ModalButton::new(
                    "Close",
                    "Esc",
                    neoism_ui::widgets::modal::ModalAction::Close,
                ),
            ],
                busy: false,
                blocking: true,
            });
        self.mark_dirty();
    }

    pub fn open_theme_picker(&mut self) {
        // Pick up theme files dropped since launch before the spec
        // snapshots the registry.
        crate::mashup::sync_custom_ide_themes();
        self.renderer
            .modal
            .open(neoism_ui::panels::command_palette::theme_picker_modal_spec());
        self.mark_dirty();
    }

    pub fn open_mashup_picker(&mut self) {
        crate::mashup::sync_custom_ide_themes();
        self.renderer.modal.open(
            neoism_ui::panels::command_palette::mashup_packs_modal_spec(
                crate::mashup::mashup_palette_entries(),
            ),
        );
        self.mark_dirty();
    }

    pub fn open_shader_picker(&mut self) {
        let shader_overlay_paths: Vec<&str> = self
            .shader_overlay_paths
            .iter()
            .map(String::as_str)
            .collect();

        self.renderer
            .modal
            .open(neoism_ui::panels::command_palette::shaders_modal_spec(
                shader_overlay_paths,
            ));
        self.mark_dirty();
    }

    pub fn handle_palette_click(&mut self, clipboard: &mut Clipboard) -> bool {
        if !self.renderer.command_palette.is_enabled() {
            return false;
        }

        let scale_factor = self.sugarloaf.scale_factor();
        let window_width = self.sugarloaf.window_size().width;
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();

        if let Some(action) = self
            .renderer
            .command_palette
            .server_action_at(mouse_x, mouse_y)
        {
            match action {
                PaletteAction::EditServer { id } => {
                    self.request_server_edit(id);
                    self.renderer.command_palette.set_enabled(false);
                }
                PaletteAction::RemoveServer { id } => {
                    use neoism_ui::widgets::modal::{
                        ModalAction, ModalButton, ModalSpec,
                    };
                    self.renderer.command_palette.set_enabled(false);
                    self.renderer.modal.open(ModalSpec {
                        title: "Remove server?".into(),
                        body: "This removes the saved address and its local credential. The remote server is not changed.".into(),
                        meta: String::new(),
                        buttons: vec![
                            ModalButton::new("Remove", "Enter", ModalAction::ServerRemoveConfirm { id }),
                            ModalButton::new("Cancel", "Esc", ModalAction::Close),
                        ],
                        input: None,
                        busy: false,
                        blocking: true,
                    });
                }
                _ => {}
            }
            self.mark_dirty();
            return true;
        }

        // 5D-drag: a press on a workspace row in the Workspaces modal
        // arms a drag instead of switching immediately. The switch (or
        // the move-to-host intent) is decided on release in
        // `handle_palette_drag_release` — a plain click still switches,
        // a press+drag onto a host header emits MoveWorkspaceToHost.
        if self.renderer.command_palette.workspace_drag_press(
            mouse_x,
            mouse_y,
            window_width,
            scale_factor,
        ) {
            // Still select the pressed row so the highlight tracks the
            // press point even if the user ends up just clicking.
            if let Ok(Some(index)) = self.renderer.command_palette.hit_test(
                mouse_x,
                mouse_y,
                window_width,
                scale_factor,
            ) {
                self.renderer.command_palette.select_clicked(index);
            }
            self.mark_dirty();
            return true;
        }

        match self.renderer.command_palette.hit_test(
            mouse_x,
            mouse_y,
            window_width,
            scale_factor,
        ) {
            Ok(Some(index)) => {
                // Clicked a result row — select and execute the same
                // activation path Enter would take for that palette mode.
                // `select_clicked` snaps past non-selectable host header
                // rows in the grouped Workspaces tree.
                self.renderer.command_palette.select_clicked(index);

                if self.renderer.command_palette.is_ex_mode() {
                    if let Some(command) =
                        self.renderer.command_palette.get_selected_ex_command()
                    {
                        self.renderer.command_palette.set_enabled(false);
                        let trimmed = command.trim();
                        if trimmed.eq_ignore_ascii_case("ThemePicker")
                            || trimmed.eq_ignore_ascii_case("theme picker")
                        {
                            self.open_theme_picker();
                        } else if trimmed.eq_ignore_ascii_case("Shaders")
                            || trimmed.eq_ignore_ascii_case("ShaderPicker")
                            || trimmed.eq_ignore_ascii_case("shader picker")
                        {
                            self.open_shader_picker();
                        } else if !self.try_intercept_ex_command(trimmed)
                            && !trimmed.is_empty()
                        {
                            self.send_editor_command(
                                neoism_backend::performer::nvim::vim_run_ex_command(
                                    trimmed,
                                ),
                            );
                        }
                        self.mark_dirty();
                    }
                    return true;
                }

                if self.renderer.command_palette.is_search_mode() {
                    let is_markdown =
                        self.context_manager.current().active_markdown().is_some();
                    if let Some(location) = self
                        .renderer
                        .command_palette
                        .selected_buffer_match_location()
                    {
                        let query = self.renderer.command_palette.query.clone();
                        self.renderer.command_palette.set_enabled(false);
                        if !query.is_empty() {
                            self.renderer
                                .command_palette
                                .push_recent_search(query.clone());
                            if is_markdown {
                                if let Some(md) = self
                                    .context_manager
                                    .current_mut()
                                    .active_markdown_mut()
                                {
                                    md.search_commit(location.0, location.1);
                                }
                            } else {
                                let cmd = self.palette_search_commit_command(
                                    &query,
                                    Some(location),
                                );
                                self.send_editor_command(cmd);
                            }
                        }
                        self.mark_dirty();
                    } else if let Some(term) =
                        self.renderer.command_palette.get_selected_search_term()
                    {
                        self.renderer.command_palette.set_enabled(false);
                        if !term.is_empty() {
                            self.renderer
                                .command_palette
                                .push_recent_search(term.clone());
                            if is_markdown {
                                if let Some(md) = self
                                    .context_manager
                                    .current_mut()
                                    .active_markdown_mut()
                                {
                                    let first = md
                                        .search_scan(&term)
                                        .first()
                                        .map(|(lnum, col, _)| (*lnum, *col));
                                    match first {
                                        Some((lnum, col)) => md.search_commit(lnum, col),
                                        None => md.search_cancel(),
                                    }
                                }
                            } else {
                                let cmd = self.palette_search_commit_command(&term, None);
                                self.send_editor_command(cmd);
                            }
                        }
                        self.mark_dirty();
                    }
                    return true;
                }

                if let Some(font) = self.renderer.command_palette.get_selected_font() {
                    clipboard
                        .set(neoism_backend::clipboard::ClipboardType::Clipboard, font);
                    self.renderer.command_palette.set_enabled(false);
                    self.mark_dirty();
                    return true;
                }

                let selected_buffer =
                    self.renderer.command_palette.get_selected_buffer_target();
                if let Some(target) = selected_buffer {
                    self.renderer.command_palette.set_enabled(false);
                    self.activate_palette_buffer(target);
                    self.mark_dirty();
                    return true;
                }

                if let Some(target) = self
                    .renderer
                    .command_palette
                    .get_selected_workspace_target()
                {
                    self.renderer.command_palette.set_enabled(false);
                    // 8C: own grid → select it; foreign workspace →
                    // ADOPT it as a real Island workspace (attach its
                    // live daemon sessions) instead of only flipping
                    // the daemon's active pointer.
                    self.open_or_adopt_daemon_workspace(target.workspace_id);
                    if let Some(workspace_id) = self.current_workspace_id() {
                        self.report_workspace_subscription(workspace_id);
                    }
                    self.mark_dirty();
                    return true;
                }

                if let Some(action) = self.renderer.command_palette.get_selected_action()
                {
                    use neoism_ui::panels::command_palette::PaletteAction;
                    match action {
                        PaletteAction::ListFonts => {
                            let fonts = self.sugarloaf.font_family_names();
                            self.renderer.command_palette.enter_fonts_mode(fonts);
                        }
                        PaletteAction::ListBuffers => {
                            self.open_workspace_buffers_picker();
                        }
                        PaletteAction::ShowWorkplaces => {
                            self.open_daemon_workspaces_picker();
                        }
                        PaletteAction::ShowServers => {
                            self.request_server_manager();
                            self.renderer.command_palette.set_enabled(false);
                        }
                        action => {
                            self.renderer.command_palette.set_enabled(false);
                            self.execute_palette_action(action, clipboard);
                        }
                    }
                }
                self.mark_dirty();
                true
            }
            Ok(None) => {
                // Clicked inside palette but not on a result (e.g. input area)
                true
            }
            Err(()) => {
                // Clicked outside — close palette
                self.renderer.command_palette.set_enabled(false);
                self.mark_dirty();
                true
            }
        }
    }

    /// 5D-drag: advance an armed workspace drag as the cursor moves.
    /// Returns `true` when the palette consumed the move (a drag is
    /// armed) so the caller short-circuits other hover/drag paths.
    pub fn handle_palette_drag_move(&mut self) -> bool {
        if !self.renderer.command_palette.is_enabled() {
            return false;
        }
        let scale_factor = self.sugarloaf.scale_factor();
        let window_width = self.sugarloaf.window_size().width;
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();
        if self.renderer.command_palette.workspace_drag_move(
            mouse_x,
            mouse_y,
            window_width,
            scale_factor,
        ) {
            self.mark_dirty();
        }
        // Consume the event whenever a drag is in flight so the cursor
        // keeps gripping the row instead of falling through to hover.
        self.renderer.command_palette.is_dragging_workspace()
    }

    /// 5D-drag: finish a workspace drag on mouse release. Returns `true`
    /// when the release belonged to an *active* drag (so the caller
    /// short-circuits the normal release path); a plain
    /// armed-but-not-dragged press is treated as a click — the workspace
    /// switch fires here, mirroring the old press-to-switch behavior.
    pub fn handle_palette_drag_release(&mut self, _clipboard: &mut Clipboard) -> bool {
        if !self.renderer.command_palette.is_enabled() {
            return false;
        }
        let scale_factor = self.sugarloaf.scale_factor();
        let window_width = self.sugarloaf.window_size().width;
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();

        let (was_active, action) = self.renderer.command_palette.workspace_drag_release(
            mouse_x,
            mouse_y,
            window_width,
            scale_factor,
        );

        if !was_active {
            // Either nothing was armed (return false → let the normal
            // release path run) or it was a plain click on a workspace
            // row (no threshold crossed) → switch to that workspace now,
            // mirroring the pre-drag press-to-switch behavior.
            if let Some(target) = self
                .renderer
                .command_palette
                .get_selected_workspace_target()
            {
                self.renderer.command_palette.set_enabled(false);
                // 8C: same adopt path as the Enter pick.
                self.open_or_adopt_daemon_workspace(target.workspace_id);
                if let Some(workspace_id) = self.current_workspace_id() {
                    self.report_workspace_subscription(workspace_id);
                }
                self.mark_dirty();
                return true;
            }
            return false;
        }

        // Active drag released. Emit the move intent if the drop landed
        // on a different host; otherwise it's a no-op cancel. 5D-wire:
        // dispatch the real promote/demote via `execute_move_workspace`.
        // The modal stays OPEN so the target host header can show
        // "moving…" while the daemon copies the workspace over, then
        // the ✓/✗ outcome (task #8).
        if let Some(action) = action {
            if let neoism_ui::panels::command_palette::PaletteAction::MoveWorkspaceToHost {
                workspace_id,
                target_host_id,
                target_daemon_url,
                target_is_local,
            } = action
            {
                self.renderer
                    .command_palette
                    .begin_workspace_move(workspace_id.clone(), target_host_id);
                self.context_manager.move_workspace_to_host(
                    workspace_id,
                    target_daemon_url,
                    target_is_local,
                );
            }
        }
        self.mark_dirty();
        true
    }

    pub fn execute_palette_action(
        &mut self,
        action: neoism_ui::panels::command_palette::PaletteAction,
        clipboard: &mut Clipboard,
    ) {
        use neoism_ui::panels::command_palette::PaletteAction;
        match action {
            PaletteAction::TabCreate => {
                self.create_workspace_terminal_tab();
                self.cancel_search(clipboard);
            }
            PaletteAction::TabClose => self.close_tab(clipboard),
            PaletteAction::TabCloseUnfocused => {
                if self.ctx().len() > 1 {
                    self.context_manager.close_unfocused_tabs();
                    self.resize_top_or_bottom_line(1);
                }
            }
            PaletteAction::SelectNextTab => {
                self.clear_selection();
                self.select_top_level_workspace(false);
            }
            PaletteAction::SelectPrevTab => {
                self.clear_selection();
                self.select_top_level_workspace(true);
            }
            PaletteAction::SplitRight => self.split_right(),
            PaletteAction::SplitDown => self.split_down(),
            PaletteAction::SelectNextSplit => {
                self.context_manager.select_next_split();
            }
            PaletteAction::SelectPrevSplit => {
                self.context_manager.select_prev_split();
            }
            PaletteAction::CloseCurrentSplitOrTab => self.close_split_or_tab(clipboard),
            PaletteAction::ConfigEditor => {
                // Open the active config (config.json, or a legacy
                // config.toml) as a tab in the nvim editor — create the
                // default file first if neither exists.
                let config_path = neoism_backend::config::config_file_path();
                if !config_path.exists() {
                    neoism_backend::config::create_config_file(None);
                }
                self.open_path_in_editor(config_path);
            }
            PaletteAction::WindowCreateNew => {
                self.context_manager.create_new_window();
            }
            PaletteAction::IncreaseFontSize => {
                self.change_font_size(FontSizeAction::Increase);
            }
            PaletteAction::DecreaseFontSize => {
                self.change_font_size(FontSizeAction::Decrease);
            }
            PaletteAction::ResetFontSize => {
                self.change_font_size(FontSizeAction::Reset);
            }
            PaletteAction::ToggleViMode => {
                let context = self.context_manager.current_mut();
                let mut terminal = context.terminal.lock();
                terminal.toggle_vi_mode();
                drop(terminal);
                context
                    .renderable_content
                    .pending_update
                    .set_terminal_damage(
                        neoism_terminal_core::damage::TerminalDamage::Full,
                    );
            }
            PaletteAction::ToggleFullscreen => {
                self.context_manager.toggle_full_screen();
            }
            PaletteAction::ToggleAppearanceTheme => {
                self.context_manager.toggle_appearance_theme();
            }
            PaletteAction::OpenThemePicker => {
                self.open_theme_picker();
            }
            PaletteAction::OpenShaders => {
                self.open_shader_picker();
            }
            PaletteAction::OpenMashupPacks => {
                self.open_mashup_picker();
            }
            PaletteAction::Copy => {
                if self.context_manager.current().editor.is_some() {
                    self.send_editor_command(
                        neoism_backend::performer::nvim::vim_copy_active_command(),
                    );
                } else {
                    self.copy_selection(ClipboardType::Clipboard, clipboard);
                }
            }
            PaletteAction::Paste => {
                let content = clipboard.get(ClipboardType::Clipboard);
                if let Some(markdown) =
                    self.context_manager.current_mut().markdown.as_mut()
                {
                    markdown.enter_insert();
                    markdown.insert_text(&content);
                    self.sync_active_markdown_modified();
                    self.renderer.trail_cursor.reset();
                    self.mark_dirty();
                } else {
                    self.paste(&content, true);
                }
            }
            PaletteAction::SaveDocument => {
                self.save_current_document();
            }
            PaletteAction::RunNotebookCell => {
                self.run_current_notebook_cell();
            }
            PaletteAction::RunNotebookCellAndBelow => {
                self.run_current_and_below_notebook_cells();
            }
            PaletteAction::RunAllNotebookCells => {
                self.run_all_notebook_cells();
            }
            PaletteAction::InsertNotebookCodeCellAbove => {
                self.insert_notebook_code_cell_above();
            }
            PaletteAction::InsertNotebookCodeCellBelow => {
                self.insert_notebook_code_cell_below();
            }
            PaletteAction::InsertNotebookMarkdownCellAbove => {
                self.insert_notebook_markdown_cell_above();
            }
            PaletteAction::InsertNotebookMarkdownCellBelow => {
                self.insert_notebook_markdown_cell_below();
            }
            PaletteAction::DeleteNotebookCell => {
                self.delete_current_notebook_cell();
            }
            PaletteAction::MoveNotebookCellUp => {
                self.move_current_notebook_cell_up();
            }
            PaletteAction::MoveNotebookCellDown => {
                self.move_current_notebook_cell_down();
            }
            PaletteAction::InterruptNotebookKernel => {
                self.interrupt_current_notebook_kernel();
            }
            PaletteAction::ClearNotebookCellOutput => {
                self.clear_current_notebook_cell_output();
            }
            PaletteAction::ClearNotebookOutputs => {
                self.clear_current_notebook_outputs();
            }
            PaletteAction::RestartNotebookKernel => {
                self.restart_current_notebook_kernel();
            }
            PaletteAction::SearchForward => {
                if self.context_manager.current().editor.is_some() {
                    self.renderer.command_palette.enter_search_mode();
                    self.mark_dirty();
                } else {
                    self.start_search(Direction::Right);
                }
            }
            PaletteAction::SearchBackward => {
                if self.context_manager.current().editor.is_some() {
                    self.renderer.command_palette.enter_search_mode_backward();
                    self.mark_dirty();
                } else {
                    self.start_search(Direction::Left);
                }
            }
            PaletteAction::NvimEx(cmd) => {
                self.run_palette_ex_query(cmd);
            }
            PaletteAction::GoToLine => {
                self.renderer.command_palette.enter_ex_mode();
                self.mark_dirty();
            }
            PaletteAction::SearchFiles => {
                self.open_finder_files();
            }
            PaletteAction::SearchWords => {
                self.open_finder_grep();
            }
            PaletteAction::SearchGitChanges => {
                // Surface every changed file *with* its per-file diff by
                // opening the rich Git Diff panel (the same panel Alt+G
                // toggles), rather than the plain pick-a-file finder list.
                self.open_git_diff_panel();
            }
            PaletteAction::ToggleGitDiffPanel => {
                self.toggle_git_diff_panel();
            }
            PaletteAction::CreateNeoismNote => {
                self.create_current_neoism_note();
            }
            PaletteAction::DrawOnNote => {
                self.draw_on_current_note();
            }
            PaletteAction::OpenNeoismNotes => {
                self.open_neoism_notes_sidebar();
            }
            PaletteAction::LspHover => {
                self.execute_lsp_context_action(
                    neoism_ui::panels::context_menu::LspContextAction::Hover,
                );
            }
            PaletteAction::LspCodeAction => {
                self.execute_lsp_context_action(
                    neoism_ui::panels::context_menu::LspContextAction::CodeAction,
                );
            }
            PaletteAction::LspFormat => {
                self.execute_lsp_context_action(
                    neoism_ui::panels::context_menu::LspContextAction::Format,
                );
            }
            PaletteAction::LspDefinition => {
                self.execute_lsp_context_action(
                    neoism_ui::panels::context_menu::LspContextAction::Definition,
                );
            }
            PaletteAction::LspReferences => {
                self.execute_lsp_context_action(
                    neoism_ui::panels::context_menu::LspContextAction::References,
                );
            }
            PaletteAction::LspRename => {
                self.execute_lsp_context_action(
                    neoism_ui::panels::context_menu::LspContextAction::Rename,
                );
            }
            PaletteAction::LspDocumentSymbols => {
                self.execute_lsp_context_action(
                    neoism_ui::panels::context_menu::LspContextAction::DocumentSymbols,
                );
            }
            PaletteAction::LspWorkspaceSymbols => {
                self.execute_lsp_context_action(
                    neoism_ui::panels::context_menu::LspContextAction::WorkspaceSymbols,
                );
            }
            PaletteAction::ToggleInlayHints => {
                self.execute_lsp_context_action(
                    neoism_ui::panels::context_menu::LspContextAction::ToggleInlayHints,
                );
            }
            PaletteAction::ToggleMinimap => {
                self.toggle_minimap();
            }
            PaletteAction::ClearHistory => {
                let mut terminal = self.context_manager.current_mut().terminal.lock();
                terminal.clear_saved_history();
            }
            PaletteAction::ListFonts => {
                // The palette/router Enter + click paths intercept this
                // and keep the palette open in fonts mode. External
                // callers (the Alt+K command sheet / a context menu)
                // dispatch through here instead, so mirror ListBuffers /
                // ShowWorkplaces and enter fonts mode ourselves — a
                // no-op here is exactly why "List Fonts opens nothing"
                // from those surfaces. `enter_fonts_mode` re-opens the
                // palette, so this works even when we were closed first.
                let fonts = self.sugarloaf.font_family_names();
                self.renderer.command_palette.enter_fonts_mode(fonts);
            }
            PaletteAction::ListBuffers => {
                self.open_workspace_buffers_picker();
            }
            PaletteAction::ShowWorkplaces => {
                self.open_daemon_workspaces_picker();
            }
            PaletteAction::ShowServers => {
                self.request_server_manager();
            }
            PaletteAction::SelectServer { id } => {
                self.request_server_connect(id);
            }
            PaletteAction::EditServer { id } => {
                self.request_server_edit(id);
            }
            PaletteAction::RemoveServer { id } => {
                self.request_server_remove(id);
            }
            PaletteAction::AddServer => {
                self.open_add_server_prompt();
            }
            PaletteAction::CreateServer => {
                self.open_create_server_prompt();
            }
            PaletteAction::CreateWorkspace => {
                self.create_tab(clipboard);
            }
            PaletteAction::ShareCurrentWorkspace => {
                if let Some(workspace_id) = self.current_workspace_id() {
                    self.context_manager.send_workspace_request(
                        neoism_protocol::workspace::WorkspaceClientMessage::ShareWorkspace {
                            workspace_id,
                        },
                    );
                }
            }
            PaletteAction::StopSharingCurrentWorkspace => {
                if let Some(workspace_id) = self.current_workspace_id() {
                    self.context_manager.send_workspace_request(
                        neoism_protocol::workspace::WorkspaceClientMessage::StopSharingWorkspace {
                            workspace_id,
                        },
                    );
                }
            }
            PaletteAction::LeaveWorkspace => {
                // Only meaningful in a JOINED workspace: detach from
                // the host's sessions and close the tab (close_tab's
                // adopted-grid guard keeps the host's shells alive and
                // re-dials home after the last joined workspace).
                if self
                    .context_manager
                    .current_adopted_workspace_id()
                    .is_some()
                {
                    self.close_tab(clipboard);
                } else {
                    tracing::info!(
                        target: "neoism::workspaces",
                        "leave workspace: current workspace is not a joined one; no-op"
                    );
                }
            }
            PaletteAction::SendCurrentWorkspaceToDockerSandbox => {
                if let Some(workspace_id) = self.current_workspace_id() {
                    self.context_manager.send_workspace_request(
                        neoism_protocol::workspace::WorkspaceClientMessage::SendWorkspaceToDockerSandbox {
                            workspace_id,
                        },
                    );
                }
            }
            PaletteAction::SendCurrentWorkspaceToCloud => {
                if let Some(workspace_id) = self.current_workspace_id() {
                    self.context_manager.send_workspace_request(
                        neoism_protocol::workspace::WorkspaceClientMessage::SendWorkspaceToCloud {
                            workspace_id,
                        },
                    );
                }
            }
            PaletteAction::OpenNeoismAgent => {
                self.open_neoism_agent_tab();
            }
            PaletteAction::RunClaude => {
                self.start_agent(crate::neoism::icon::AgentKind::Claude);
            }
            PaletteAction::RunCodex => {
                self.start_agent(crate::neoism::icon::AgentKind::Codex);
            }
            PaletteAction::RunOpenCode => {
                self.start_agent(crate::neoism::icon::AgentKind::OpenCode);
            }
            PaletteAction::MoveWorkspaceToHost {
                workspace_id,
                target_host_id,
                target_daemon_url,
                target_is_local,
            } => {
                // 5D-drag emits this intent when a workspace row is
                // dragged onto a host header in the Workspaces modal.
                // 5D-wire: dispatch the real move route on the local
                // daemon — `target_is_local` → /workspace/demote,
                // otherwise /workspace/promote to the target host.
                // Normally fired directly in `handle_palette_drag_release`;
                // this arm keeps the action dispatchable through the
                // generic `execute_palette_action` path too.
                self.renderer
                    .command_palette
                    .begin_workspace_move(workspace_id.clone(), target_host_id);
                self.context_manager.move_workspace_to_host(
                    workspace_id,
                    target_daemon_url,
                    target_is_local,
                );
            }
            PaletteAction::Quit => {
                tracing::info!(
                    target: "neoism::editor_tabs",
                    current_is_editor = self.context_manager.current().editor.is_some(),
                    active_tab_is_terminal = self.renderer.buffer_tabs.active_is_terminal(),
                    workspace_id = ?self.current_workspace_id(),
                    grid_panes = self.context_manager.current_grid_len(),
                    "Quit palette action invoked"
                );
                self.context_manager.quit();
            }
        }
    }

    pub(crate) fn open_daemon_workspaces_picker(&mut self) {
        self.context_manager.request_daemon_host_workspace_tree();
        // Wave 6A: kick a tailnet peer refresh too (throttled in the
        // daemon link). This open renders the last-known peer list; the
        // refresh keeps it current for the next one.
        self.context_manager.request_tailnet_peers();

        let (entries, peer_hosts) = self.build_workspaces_picker_data();
        self.renderer
            .command_palette
            .enter_workspaces_mode_with_hosts(entries, peer_hosts);
        self.mark_dirty();
    }

    /// Live-refresh the OPEN Workspaces modal when fresh tree data
    /// lands (`HostWorkspaceTree` / `HostWorkspaceList` / `HostList`).
    /// The open used to render only the last-known snapshot — the
    /// async tree request raced the first open, so web/daemon
    /// workspaces looked invisible until a re-open. Mirrors the web
    /// client's refreshWorkspacesModal. Returns true when refreshed.
    pub(crate) fn refresh_open_workspaces_picker(&mut self) -> bool {
        if !self.renderer.command_palette.workspaces_mode_open() {
            return false;
        }
        let (entries, peer_hosts) = self.build_workspaces_picker_data();
        self.renderer
            .command_palette
            .refresh_workspaces_tree(entries, peer_hosts);
        self.mark_dirty();
        true
    }

    fn build_workspaces_picker_data(
        &self,
    ) -> (
        Vec<neoism_ui::panels::command_palette::PaletteWorkspaceEntry>,
        Vec<neoism_ui::panels::command_palette::PaletteHostEntry>,
    ) {
        use neoism_ui::panels::command_palette::{
            HostKind, PaletteHostEntry, PaletteWorkspaceEntry, PaletteWorkspaceTarget,
        };

        // Index the known hosts so each workspace can hang under its
        // owner's header (label / online / daemon_url / kind). Falls
        // back to a synthesized Local group for workspaces whose host
        // hasn't been published in the tree yet.
        let local_host_id = self.context_manager.local_host_id();
        let mut hosts = self.context_manager.daemon_hosts().to_vec();
        let peer_tree = self.context_manager.peer_workspace_tree();
        for host in peer_tree.hosts {
            if let Some(existing) =
                hosts.iter_mut().find(|existing| existing.id == host.id)
            {
                if existing.daemon_url.is_none() {
                    existing.daemon_url = host.daemon_url.clone();
                }
                existing.online |= host.online;
                existing.last_seen = existing.last_seen.max(host.last_seen);
            } else {
                hosts.push(host);
            }
        }
        let host_index: std::collections::HashMap<&str, &neoism_protocol::HostSummary> =
            hosts.iter().map(|h| (h.id.as_str(), h)).collect();

        // Always show the desktop's own open workspaces (immediate, never
        // empty), then merge in any OTHER hosts' workspaces the daemon has
        // published. Dedupe by id so a locally-open workspace that's also in
        // the daemon tree isn't listed twice. This fixes the empty-modal bug:
        // the picker used to read only the daemon tree, which is requested
        // async and is empty on first open / when nothing's published.
        let local_host_label = self.context_manager.local_host_label();
        let mut merged = self.context_manager.local_workspace_summaries();
        let mut seen: std::collections::HashSet<String> =
            merged.iter().map(|w| w.id.clone()).collect();
        for workspace in self.context_manager.daemon_host_workspaces() {
            let joinable_remote = workspace.host_id != local_host_id
                && matches!(
                    workspace.visibility,
                    neoism_protocol::workspace::WorkspaceVisibility::Shared
                        | neoism_protocol::workspace::WorkspaceVisibility::Team
                );
            if joinable_remote && seen.insert(workspace.id.clone()) {
                merged.push(workspace.clone());
            }
        }
        for workspace in peer_tree.workspaces {
            if workspace.host_id != local_host_id && seen.insert(workspace.id.clone()) {
                merged.push(workspace);
            }
        }

        // The workspace this window is currently VIEWING — the adopted id
        // when the current grid came from a daemon, otherwise the local
        // grid's own workspace id. Drives the left accent stripe.
        let current_workspace_marker: Option<String> = self
            .context_manager
            .current_adopted_workspace_id()
            .or_else(|| self.current_workspace_id());

        let entries: Vec<PaletteWorkspaceEntry> = merged
            .iter()
            .map(|workspace| {
                let detail = workspace
                    .root_dir
                    .as_ref()
                    .map(|root| root.display().to_string())
                    .filter(|root| !root.is_empty())
                    .unwrap_or_else(|| "daemon workspace".to_string());

                // Host metadata for grouping. Kind inference: the local
                // window's host is `Local`; any other host with a
                // dialable `daemon_url` is `Remote`. `Cloud` is reserved
                // for ephemeral burst daemons (Wave 6) — there is no
                // cloud flag in `HostSummary` yet, so nothing maps to it
                // today. 5D-data seam: when `/tailnet-peers` carries a
                // richer kind, switch on it here.
                let is_local = workspace.host_id == local_host_id;
                let host = host_index.get(workspace.host_id.as_str());
                let host_label = host.map(|h| h.label.clone()).unwrap_or_else(|| {
                    if is_local {
                        local_host_label.clone()
                    } else {
                        workspace.host_id.clone()
                    }
                });
                let host_online = host.map(|h| h.online).unwrap_or(is_local);
                let daemon_url = host.and_then(|h| h.daemon_url.clone());
                let host_kind = if is_local {
                    HostKind::Local
                } else {
                    HostKind::Remote
                };

                PaletteWorkspaceEntry {
                    title: workspace.title.clone(),
                    detail,
                    target: PaletteWorkspaceTarget {
                        workspace_id: workspace.id.clone(),
                    },
                    host_id: workspace.host_id.clone(),
                    host_label,
                    host_kind,
                    workspace_host_kind: match workspace.host_kind {
                        neoism_protocol::workspace::WorkspaceHostKind::Tailscale => {
                            neoism_ui::panels::command_palette::WorkspaceHostKind::Tailscale
                        }
                        neoism_protocol::workspace::WorkspaceHostKind::DockerSandbox => {
                            neoism_ui::panels::command_palette::WorkspaceHostKind::DockerSandbox
                        }
                        neoism_protocol::workspace::WorkspaceHostKind::CloudSandbox => {
                            neoism_ui::panels::command_palette::WorkspaceHostKind::CloudSandbox
                        }
                        neoism_protocol::workspace::WorkspaceHostKind::Local => {
                            neoism_ui::panels::command_palette::WorkspaceHostKind::Local
                        }
                    },
                    workspace_visibility: match workspace.visibility {
                        neoism_protocol::workspace::WorkspaceVisibility::Shared => {
                            neoism_ui::panels::command_palette::WorkspaceVisibility::Shared
                        }
                        neoism_protocol::workspace::WorkspaceVisibility::Team => {
                            neoism_ui::panels::command_palette::WorkspaceVisibility::Team
                        }
                        neoism_protocol::workspace::WorkspaceVisibility::Private => {
                            neoism_ui::panels::command_palette::WorkspaceVisibility::Private
                        }
                    },
                    daemon_url,
                    host_online,
                    current: current_workspace_marker.as_deref()
                        == Some(workspace.id.as_str()),
                }
            })
            .collect();

        // Wave 6A: discovered tailnet peers join the tree as header-only
        // drop targets, so a workspace can be dragged onto a machine that
        // doesn't own any workspaces yet (drop → promote to the peer's
        // candidate daemon URL). Dedupe against every host the tree
        // already shows — by label (the daemon registers hosts under
        // their machine name, which is also the tailnet hostname) and by
        // the host part of any advertised daemon_url (catches a host
        // whose label was customised but whose URL dials the peer's IP).
        use crate::daemon_client::tailnet_peers::{
            daemon_url_host, tailnet_peer_palette_hosts,
        };
        let mut existing_labels: std::collections::HashSet<String> = entries
            .iter()
            .map(|e| e.host_label.to_lowercase())
            .collect();
        existing_labels.insert(local_host_label.to_lowercase());
        let mut existing_url_hosts: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for host in hosts.iter() {
            existing_labels.insert(host.label.to_lowercase());
            if let Some(url_host) = host.daemon_url.as_deref().and_then(daemon_url_host) {
                existing_url_hosts.insert(url_host);
            }
        }
        for entry in entries.iter() {
            if let Some(url_host) = entry.daemon_url.as_deref().and_then(daemon_url_host)
            {
                existing_url_hosts.insert(url_host);
            }
        }
        // Daemon-tree hosts that own no workspaces still get a
        // header-only row (parity with the web modal's peer_hosts):
        // without this the attached daemon's own host (default id
        // "local" — where web Alt+W-created workspaces land) is
        // invisible here until its first workspace exists, so desktop
        // and web appear to disagree about who's in the tree.
        let populated: std::collections::HashSet<&str> =
            merged.iter().map(|w| w.host_id.as_str()).collect();
        let mut peer_hosts: Vec<PaletteHostEntry> = hosts
            .iter()
            .filter(|host| {
                host.id != local_host_id && !populated.contains(host.id.as_str())
            })
            .map(|host| PaletteHostEntry {
                host_id: host.id.clone(),
                label: host.label.clone(),
                kind: HostKind::Remote,
                daemon_url: host.daemon_url.clone(),
                online: host.online,
            })
            .collect();

        let peers = self.context_manager.tailnet_peers();
        peer_hosts.extend(tailnet_peer_palette_hosts(
            &peers,
            &existing_labels,
            &existing_url_hosts,
        ));

        (entries, peer_hosts)
    }
}
