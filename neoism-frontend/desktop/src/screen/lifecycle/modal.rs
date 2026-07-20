use super::*;

impl Screen<'_> {
    pub fn handle_modal_click(&mut self) -> bool {
        if !self.renderer.modal.is_active() {
            return false;
        }
        let blocking = self.renderer.modal.is_blocking();

        let window_width = self.sugarloaf.window_size().width;
        let scale_factor = self.sugarloaf.scale_factor();
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();

        // Mode slider (today only the server modal's Join ↔ Create):
        // switching reopens the other form, keeping the modal up.
        if let Some(tab) = self.renderer.modal.form_tab_hit(mouse_x, mouse_y) {
            if tab == 0 {
                self.open_add_server_prompt();
            } else {
                self.open_create_server_prompt();
            }
            self.mark_dirty();
            return true;
        }

        match self
            .renderer
            .modal
            .hit_test(mouse_x, mouse_y, window_width, scale_factor)
        {
            Ok(Some(index)) => {
                if self.renderer.modal.focus_form_hit(index) {
                    self.mark_dirty();
                    return true;
                }
                self.renderer.modal.set_selected_index(index);
                let selected = self.renderer.modal.selected_action();
                let action = if matches!(
                    selected,
                    Some(neoism_ui::widgets::modal::ModalAction::ServerFormSubmit)
                ) {
                    self.renderer.modal.submit_form()
                } else {
                    selected
                };
                if let Some(action) = action {
                    self.execute_modal_action(action);
                }
                self.mark_dirty();
                true
            }
            Ok(None) => blocking,
            Err(()) => {
                if !blocking {
                    return false;
                }
                if let Some(action) = self.renderer.modal.escape_action() {
                    self.execute_modal_action(action);
                } else {
                    self.renderer.modal.close();
                }
                self.mark_dirty();
                true
            }
        }
    }

    pub fn execute_modal_action(
        &mut self,
        action: neoism_ui::widgets::modal::ModalAction,
    ) {
        use neoism_ui::widgets::modal::ModalAction;

        // Mirror each `ModalAction` to the shared policy's
        // [`ModalActionTag`] so the dispatch contract is checkable.
        // The actual side effects (installer threads, file-tree ops,
        // nvim commands) stay on the host because they need access to
        // `self`, but the policy lets a non-desktop frontend agree on
        // which arms close before / after / not at all.
        let policy_tag = modal_action_policy_tag(&action);
        let _ = neoism_ui::chrome_policy::modal_action_dispatch(policy_tag);

        match action {
            ModalAction::Close => {
                self.renderer.modal.close();
            }
            ModalAction::InstallLsp { server } => {
                self.start_lsp_install(server);
            }
            ModalAction::InstallPythonKernel => {
                self.start_python_kernel_install_modal();
            }
            ModalAction::ApplyTheme { name } => {
                self.apply_unified_theme(&name);
            }
            ModalAction::ApplyShaderOverlay { path } => {
                self.apply_shader_overlay(path);
            }
            ModalAction::ApplyMashupPack { id } => {
                self.apply_mashup_pack(id);
            }
            ModalAction::RunEditorCommand { command: _ } => {
                // nvim removed; native editor equivalent TBD.
                self.renderer.modal.close();
            }
            ModalAction::RunEditorCommandWithInput {
                command: _,
                value: _,
            } => {
                // nvim removed; native editor equivalent TBD.
                self.renderer.modal.close();
            }
            ModalAction::OpenLspLocation {
                uri,
                line,
                character,
            } => {
                // Jump-to-line/column comes back with the native editor's
                // LSP integration; for now just open the file.
                let _ = (line, character);
                self.renderer.modal.close();
                self.open_path_in_code(std::path::PathBuf::from(
                    uri.strip_prefix("file://").unwrap_or(&uri).to_string(),
                ));
            }
            ModalAction::ApplyLspCodeAction { action: _ } => {
                // nvim removed; native editor code actions TBD.
                self.renderer.modal.close();
            }
            ModalAction::InstallAgent { kind } => {
                if let Some(ak) = crate::neoism::icon::AgentKind::from_id(&kind) {
                    self.start_agent_install(ak);
                }
            }
            ModalAction::RunAgent { kind } => {
                if let Some(ak) = crate::neoism::icon::AgentKind::from_id(&kind) {
                    self.renderer.modal.close();
                    self.start_agent(ak);
                }
            }
            ModalAction::AcpPermission {
                server_id,
                request_id,
                option_id,
            } => {
                self.renderer.modal.close();
                #[cfg(not(target_arch = "wasm32"))]
                if !self.respond_acp_permission(&server_id, request_id, option_id) {
                    self.renderer.notifications.push(
                        "ACP permission request was no longer pending",
                        neoism_ui::panels::notifications::NotificationLevel::Warn,
                    );
                }
            }
            ModalAction::FileTreeEdit { path } => {
                self.renderer.modal.close();
                self.activate_file_tree_path(PathBuf::from(path));
            }
            ModalAction::FileTreeCopy { path } => {
                self.renderer.modal.close();
                self.copy_file_tree_path(PathBuf::from(path));
            }
            ModalAction::FileTreePaste { dest_dir } => {
                self.renderer.modal.close();
                self.paste_file_tree_clipboard(PathBuf::from(dest_dir));
            }
            ModalAction::FileTreePromptDelete { path } => {
                self.confirm_delete_file_tree_path(PathBuf::from(path));
            }
            ModalAction::FileTreeDelete { path } => {
                self.delete_file_tree_path(PathBuf::from(path));
                self.renderer.notes_sidebar.refresh_notes();
            }
            ModalAction::FileTreePromptNewFile { dir } => {
                self.open_file_tree_new_file_prompt(PathBuf::from(dir));
            }
            ModalAction::NotesPromptNewFile { dir } => {
                self.open_notes_new_file_prompt(PathBuf::from(dir));
            }
            ModalAction::NotesOpenGraph => {
                self.open_neoism_graph_view();
            }
            ModalAction::NotesOpenCreateMenu => {
                self.open_notes_create_menu_at_button();
            }
            ModalAction::FileTreePromptNewFolder { dir } => {
                self.open_file_tree_new_folder_prompt(PathBuf::from(dir));
            }
            ModalAction::FileTreePromptRename { path } => {
                self.open_file_tree_rename_prompt(PathBuf::from(path));
            }
            ModalAction::FileTreeNewFile { dir, name } => {
                self.create_file_tree_file(PathBuf::from(dir), name);
                self.renderer.notes_sidebar.refresh_notes();
            }
            ModalAction::NotesNewFile { dir, name } => {
                self.create_notes_file(PathBuf::from(dir), name);
            }
            ModalAction::NotesNewDrawing { dir } => {
                self.create_neoism_drawing_in(PathBuf::from(dir));
            }
            ModalAction::NotesPromptIcon { path } => {
                self.open_notes_icon_prompt(PathBuf::from(path));
            }
            ModalAction::NotesSetIcon { path, icon } => {
                self.set_notes_entry_icon(PathBuf::from(path), icon);
            }
            ModalAction::FileTreeNewFolder { dir, name } => {
                self.create_file_tree_folder(PathBuf::from(dir), name);
                self.renderer.notes_sidebar.refresh_notes();
            }
            ModalAction::FileTreeRename { path, name } => {
                self.rename_file_tree_path(PathBuf::from(path), name);
                self.renderer.notes_sidebar.refresh_notes();
            }
            ModalAction::RenameTab {
                index,
                agent_session_id,
                name,
            } => {
                let name = name.trim();
                if name.is_empty() {
                    self.renderer.notifications.push(
                        "Name required",
                        neoism_ui::panels::notifications::NotificationLevel::Warn,
                    );
                    return;
                }
                let name = name.to_string();
                self.renderer.modal.close();
                // Local label first — always applies, every tab kind.
                self.renderer.buffer_tabs.set_title(index, name.clone());
                // Agent tabs additionally publish the title at the daemon
                // level so it survives reloads / shows in the session list
                // / syncs cross-device. We resolve the live agent pane on
                // the CURRENT workspace (where the right-click happened)
                // and queue an `OutboundAgentCommand::SetTitle`.
                if agent_session_id.is_some() {
                    if let Some(agent) =
                        self.context_manager.current_mut().neoism_agent.as_mut()
                    {
                        if !agent.publish_session_title(name) {
                            tracing::debug!(
                                target: "neoism::neoism_agent",
                                index,
                                "rename: agent pane had no live session yet; \
                                 kept local tab label only"
                            );
                        }
                    }
                }
            }
            ModalAction::NotesVaultPromptAdd => {
                self.open_notes_vault_add_prompt();
            }
            ModalAction::ServerFormSubmit => {
                let values = self
                    .renderer
                    .modal
                    .take_submitted_form()
                    .unwrap_or_default();
                let value = |id: &str| {
                    values
                        .iter()
                        .find(|(field_id, _)| field_id == id)
                        .map(|(_, value)| value.trim().to_string())
                        .filter(|value| !value.is_empty())
                };
                // Code rename form: the `code_rename_to` field id is
                // the discriminator (present even when left empty).
                if values.iter().any(|(id, _)| id == "code_rename_to") {
                    self.renderer.modal.close();
                    match value("code_rename_to") {
                        Some(new_name) => self.submit_code_rename(new_name),
                        None => {
                            self.renderer.code_lsp.pending_rename = None;
                            self.renderer.notifications.push(
                                "Rename needs a non-empty name",
                                neoism_ui::panels::notifications::NotificationLevel::Warn,
                            );
                        }
                    }
                    return;
                }
                // Create-server form: `create_dir` is the discriminator
                // (no address — we spawn the server ourselves and join).
                if let Some(dir) = value("create_dir") {
                    self.renderer.modal.close();
                    self.create_and_join_local_server(dir, value("name"), value("token"));
                    // Land back on the Servers list so the new entry
                    // (and its connect status) is visible, instead of
                    // dumping the user to whatever was behind.
                    self.request_server_manager();
                    return;
                }
                let Some(address) = value("address") else {
                    self.renderer.notifications.push(
                        "Server address is required",
                        neoism_ui::panels::notifications::NotificationLevel::Warn,
                    );
                    return;
                };
                self.renderer.modal.close();
                if let Some(server_id) = value("server_id") {
                    self.request_server_edit_submit(
                        server_id,
                        address,
                        value("name"),
                        value("token"),
                    );
                } else {
                    self.request_server_add(address, value("name"), value("token"));
                }
                // Back to the Servers list rather than closing the
                // whole flow — join status shows right there.
                self.request_server_manager();
            }
            ModalAction::ServerRemoveConfirm { id } => {
                self.renderer.modal.close();
                self.request_server_remove(id);
            }
            ModalAction::NotesVaultAdd { name } => {
                self.add_notes_vault(name);
            }
            ModalAction::NotesVaultPromptRename => {
                self.open_notes_vault_rename_prompt();
            }
            ModalAction::NotesVaultRename { name } => {
                self.rename_notes_vault(name);
            }
            ModalAction::NotesVaultSwitch { name } => {
                self.switch_notes_vault(name);
            }
            ModalAction::NotesVaultOpenVaultsRoot => {
                self.open_notes_vaults_root();
            }
            ModalAction::NotesVaultLinkCurrentWorkspace => {
                self.link_current_workspace_to_notes_vault();
            }
            ModalAction::NotesVaultPromptLinkProject { vault } => {
                self.open_notes_vault_link_project_prompt(vault);
            }
            ModalAction::NotesVaultLinkProject { vault, path } => {
                self.link_project_dir_to_notes_vault(vault, path);
            }
            ModalAction::NotesVaultShareWithRemarkable { vault } => {
                self.share_vault_with_remarkable(vault);
            }
        }
        self.mark_dirty();
    }

    pub(crate) fn start_lsp_install(&mut self, server: String) {
        // The package catalog resolves the install plan (download URL,
        // version, package layout). Skipping this lookup is fatal —
        // without a manifest we have no idea what binary to fetch.
        let Some(manifest) = self.resolve_catalog_manifest_for_server(&server) else {
            self.renderer.modal.open_message(
                "No Installer Available",
                format!(
                    "Neoism does not have a managed installer for `{server}` yet. Install the binary manually and reopen the buffer."
                ),
            );
            return;
        };

        let display = manifest.name.clone();
        self.renderer.modal.open(neoism_ui::widgets::modal::ModalSpec {
            title: format!("Installing {}", display),
            body: format!(
                "Neoism is downloading and installing `{}`. The binary will be placed at ~/.local/share/neoism/extensions/bin/.",
                manifest.id
            ),
            meta: "This can take a moment.".to_string(),
            input: None,
            buttons: vec![neoism_ui::widgets::modal::ModalButton::new(
                "Dismiss",
                "Esc",
                neoism_ui::widgets::modal::ModalAction::Close,
            )],
            busy: true,
            blocking: false,
        });

        // Hand off to the shared install pipeline. Completion (close
        // busy modal, open success/failure modal, refresh managed bin
        // map, broadcast LSP retry) is handled by `pump_install_progress`
        // in `bridges/extensions.rs` keyed off the `MissingLspModal`
        // source tag — no event round-trip required.
        self.dispatch_install_via_runner(manifest, server, display);
    }

    pub fn handle_ide_tool_install_finished(
        &mut self,
        tool: String,
        success: bool,
        message: String,
    ) {
        if let Some(rest) = tool.strip_prefix("agent:") {
            let kind_id = rest.to_string();
            let display = crate::neoism::icon::AgentKind::from_id(&kind_id)
                .map(|k| k.display_name())
                .unwrap_or(rest);
            if success {
                self.renderer
                    .modal
                    .open(neoism_ui::widgets::modal::ModalSpec {
                        title: format!("Installed {display}"),
                        body: message,
                        meta: "Open a tab and launch it now?".to_string(),
                        input: None,
                        buttons: vec![
                            neoism_ui::widgets::modal::ModalButton::new(
                                "Launch",
                                "Enter",
                                neoism_ui::widgets::modal::ModalAction::RunAgent {
                                    kind: kind_id.clone(),
                                },
                            ),
                            neoism_ui::widgets::modal::ModalButton::new(
                                "Close",
                                "Esc",
                                neoism_ui::widgets::modal::ModalAction::Close,
                            ),
                        ],
                        busy: false,
                        blocking: false,
                    });
            } else {
                self.renderer
                    .modal
                    .open(neoism_ui::widgets::modal::ModalSpec {
                        title: format!("Could Not Install {display}"),
                        body: message,
                        meta:
                            "Install the missing toolchain (npm / curl / bash) and retry."
                                .to_string(),
                        input: None,
                        buttons: vec![
                            neoism_ui::widgets::modal::ModalButton::new(
                                "Retry",
                                "Enter",
                                neoism_ui::widgets::modal::ModalAction::InstallAgent {
                                    kind: kind_id.clone(),
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
            }
            self.mark_dirty();
            return;
        }
        // Catch-all for unknown tool prefixes. LSP installs no longer
        // flow through this event — they go through the install_runner
        // tracker in bridges/extensions.rs. Anything reaching here is
        // either a future install kind that hasn't grown its own modal
        // yet or a stale event; surface it as a notification so it
        // isn't silently dropped.
        let level = if success {
            neoism_ui::panels::notifications::NotificationLevel::Info
        } else {
            neoism_ui::panels::notifications::NotificationLevel::Error
        };
        self.renderer
            .notifications
            .push(format!("{tool}: {message}"), level);

        self.mark_dirty();
    }
}
