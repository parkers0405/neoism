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

        match self
            .renderer
            .modal
            .hit_test(mouse_x, mouse_y, window_width, scale_factor)
        {
            Ok(Some(index)) => {
                self.renderer.modal.set_selected_index(index);
                if let Some(action) = self.renderer.modal.selected_action() {
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
            ModalAction::InstallTreesitter { lang } => {
                self.treesitter_installing.remove(&lang);
                self.start_treesitter_install(lang, String::new());
            }
            ModalAction::ApplyTheme { name } => {
                self.apply_unified_theme(&name);
            }
            ModalAction::ApplyShaderOverlay { path } => {
                self.apply_shader_overlay(path);
            }
            ModalAction::RunEditorCommand { command } => {
                self.renderer.modal.close();
                self.send_editor_command(command);
            }
            ModalAction::RunEditorCommandWithInput { command, value } => {
                let value = value.trim();
                if value.is_empty() {
                    self.renderer.notifications.push(
                        "Input required",
                        neoism_ui::panels::notifications::NotificationLevel::Warn,
                    );
                    return;
                }
                self.renderer.modal.close();
                if command == "Rename" {
                    if let Some(editor) = self.context_manager.current().editor.as_ref() {
                        editor.lsp_action_with_text(
                            neoism_protocol::editor::EditorLspAction::Rename,
                            Some(value.to_string()),
                        );
                    }
                    return;
                }
                if command == "WorkspaceSymbols" {
                    if let Some(editor) = self.context_manager.current().editor.as_ref() {
                        editor.lsp_action_with_text(
                            neoism_protocol::editor::EditorLspAction::WorkspaceSymbols,
                            Some(value.to_string()),
                        );
                    }
                    return;
                }
                // Pure dispatch table — see `chrome_policy::modal_input_editor_command`.
                // The host still owns `lua_string_literal` (it lives on the
                // host's nvim performer crate) so the policy hands back a
                // wrapper + raw value the host quote-escapes here.
                let cmd = match neoism_ui::chrome_policy::modal_input_editor_command(
                    command.as_str(),
                    value,
                ) {
                    neoism_ui::chrome_policy::ModalEditorCommand::LuaCall {
                        lua_call_prefix,
                        value,
                    } => format!(
                        "{lua_call_prefix}{})",
                        neoism_backend::performer::nvim::lua_string_literal(&value),
                    ),
                    neoism_ui::chrome_policy::ModalEditorCommand::Raw(raw) => raw,
                };
                self.send_editor_command(cmd);
            }
            ModalAction::OpenLspLocation {
                uri,
                line,
                character,
            } => {
                self.renderer.modal.close();
                if let Some(editor) = self.context_manager.current().editor.as_ref() {
                    editor.open_buffer_at_location(uri, line, character);
                }
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
        // Mason resolves the install plan (download URL, version,
        // package layout). Skipping this lookup is fatal — without a
        // manifest we have no idea what binary to fetch.
        let Some(manifest) = self.resolve_mason_manifest_for_server(&server) else {
            self.renderer.modal.open_message(
                "No Installer Available",
                format!(
                    "Neoism does not have a Mason entry for `{server}` yet. Install the binary manually and reopen the buffer."
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

    pub(crate) fn start_treesitter_install(&mut self, lang: String, _filetype: String) {
        if crate::neoism::ide_tools::treesitter_install_spec(&lang).is_none() {
            self.renderer.notifications.push(
                format!("No Treesitter installer for {lang}"),
                neoism_ui::panels::notifications::NotificationLevel::Warn,
            );
            return;
        }

        self.dispatch_treesitter_parser_install(lang);
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
        if let Some(rest) = tool.strip_prefix("treesitter:") {
            let lang = rest.split(':').next().unwrap_or(rest);
            self.treesitter_installing.remove(lang);
            if success {
                self.renderer.notifications.push(
                    format!("Installed {lang} Treesitter syntax"),
                    neoism_ui::panels::notifications::NotificationLevel::Info,
                );
                self.send_editor_command(
                    neoism_backend::performer::nvim::vim_treesitter_retry_command(),
                );
            } else {
                self.renderer
                    .modal
                    .open(neoism_ui::widgets::modal::ModalSpec {
                        title: format!("Could Not Install {lang} Syntax"),
                        body: message,
                        meta: "Install git/tree-sitter/cc if missing, then retry."
                            .to_string(),
                        input: None,
                        buttons: vec![
                        neoism_ui::widgets::modal::ModalButton::new(
                            "Retry Install",
                            "Enter",
                            neoism_ui::widgets::modal::ModalAction::InstallTreesitter {
                                lang: lang.to_string(),
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

    pub(crate) fn maybe_open_lsp_missing_modal(
        &mut self,
        status: neoism_backend::performer::nvim::LspStatusNotification,
    ) {
        // Resolve display strings + dedupe key via shared policy so
        // both desktop and a future web host agree on which
        // (server, filetype) pairs collapse into a single prompt.
        let descriptor = neoism_ui::chrome_policy::lsp_missing_modal_descriptor(
            neoism_ui::chrome_policy::LspMissingNotificationInput {
                name: status.name.clone(),
                binary: status.binary.clone(),
                filetype: status.filetype.clone(),
            },
        );
        if !self
            .lsp_missing_prompts
            .insert(descriptor.dedupe_key.clone())
        {
            return;
        }

        let server = descriptor.server;
        let binary = descriptor.binary;
        let filetype = descriptor.filetype_label;
        // Resolve via Mason first by server name, then by binary name
        // (e.g. `vscode-json-language-server` -> `json-lsp`). The result
        // drives whether the modal shows an "Install" button at all.
        let manifest = self
            .resolve_mason_manifest_for_server(&server)
            .or_else(|| self.resolve_mason_manifest_for_server(&binary));

        let mut buttons = Vec::new();
        if let Some(ref m) = manifest {
            buttons.push(neoism_ui::widgets::modal::ModalButton::new(
                format!("Install {}", m.name),
                "mason",
                neoism_ui::widgets::modal::ModalAction::InstallLsp {
                    server: server.clone(),
                },
            ));
        }
        buttons.push(neoism_ui::widgets::modal::ModalButton::new(
            "Ignore For Now",
            "Esc",
            neoism_ui::widgets::modal::ModalAction::Close,
        ));

        // Pure body-copy resolver — see `chrome_policy::lsp_missing_modal_body`.
        // The host still owns button construction (it needs the Mason
        // manifest name) and the `BTreeSet` dedupe; the policy
        // just settles which body string runs so multiple frontends agree.
        let body = neoism_ui::chrome_policy::lsp_missing_modal_body(
            neoism_ui::chrome_policy::LspMissingModalBodyInput {
                binary: &binary,
                filetype_label: &filetype,
                has_installer_spec: manifest.is_some(),
            },
        );

        self.renderer
            .modal
            .open(neoism_ui::widgets::modal::ModalSpec {
                title: "LSP Server Missing".to_string(),
                body,
                meta: "Managed nvim will retry after install.".to_string(),
                input: None,
                buttons,
                busy: false,
                blocking: false,
            });
        self.mark_dirty();
    }

    pub(crate) fn maybe_open_lsp_action_result_modal(&mut self) {
        let current = self.context_manager.current_mut();
        if current.editor_lsp_action_result_modal_seen {
            return;
        }
        let Some(neoism_protocol::editor::EditorServerMessage::LspActionResult {
            action,
            summary,
            hover,
            locations,
            symbol_count,
            symbols,
            ..
        }) = current.editor_lsp_action_result.clone()
        else {
            current.editor_lsp_action_result_modal_seen = true;
            return;
        };
        let should_open = matches!(
            action,
            neoism_protocol::editor::EditorLspAction::References
                | neoism_protocol::editor::EditorLspAction::DocumentSymbols
                | neoism_protocol::editor::EditorLspAction::WorkspaceSymbols
                | neoism_protocol::editor::EditorLspAction::Hover
        );
        if !should_open {
            current.editor_lsp_action_result_modal_seen = true;
            return;
        }
        current.editor_lsp_action_result_modal_seen = true;

        // Cap the picker so a huge symbol table / reference set can't
        // build thousands of modal buttons. The modal windows 8 rows at a
        // time and scrolls, so the cap only bounds allocation, not reach.
        const MAX_PICKER_ITEMS: usize = 300;
        use neoism_ui::widgets::modal::{ModalAction, ModalButton};
        let mut buttons: Vec<ModalButton> = Vec::new();

        let body = if let Some(hover) = hover {
            hover
        } else if matches!(
            action,
            neoism_protocol::editor::EditorLspAction::DocumentSymbols
                | neoism_protocol::editor::EditorLspAction::WorkspaceSymbols
        ) && !symbols.is_empty()
        {
            // Zed-style outline: one selectable row per symbol, indented
            // by nesting depth, jumping to the symbol's selection range.
            for symbol in symbols.iter().take(MAX_PICKER_ITEMS) {
                let indent = "  ".repeat(symbol.depth as usize);
                buttons.push(ModalButton::new(
                    format!("{indent}{}", symbol.name),
                    symbol.kind.clone(),
                    ModalAction::OpenLspLocation {
                        uri: symbol.uri.clone(),
                        line: symbol.line,
                        character: symbol.character,
                    },
                ));
            }
            picker_summary("symbol", symbols.len(), MAX_PICKER_ITEMS)
        } else if !locations.is_empty() {
            // References / any location list: one selectable row per hit.
            for location in locations.iter().take(MAX_PICKER_ITEMS) {
                buttons.push(ModalButton::new(
                    lsp_location_label(&location.uri, location.line),
                    String::new(),
                    ModalAction::OpenLspLocation {
                        uri: location.uri.clone(),
                        line: location.line,
                        character: location.character,
                    },
                ));
            }
            picker_summary("location", locations.len(), MAX_PICKER_ITEMS)
        } else if symbol_count > 0 {
            format!("{symbol_count} symbols returned.")
        } else {
            summary.clone()
        };

        buttons.push(ModalButton::new("Close", "Esc", ModalAction::Close));
        self.renderer
            .modal
            .open(neoism_ui::widgets::modal::ModalSpec {
                title: format!("Rust LSP {action:?}"),
                body,
                meta: summary,
                input: None,
                busy: false,
                blocking: false,
                buttons,
            });
    }
}
