// Auto-split from screen/mod.rs. See sibling mod.rs for the Screen struct and
// the constructor/core methods. This file is part of the impl Screen<'_> block.

use super::super::*;
use std::path::PathBuf;

impl Screen<'_> {
    pub(crate) fn start_acp_server_session(
        &mut self,
        config: crate::neoism::acp::AcpServerConfig,
        initial_prompt: Option<String>,
    ) {
        let event_proxy = self.context_manager.event_proxy();
        let window_id = self.context_manager.window_id();
        let wake = std::sync::Arc::new(move || {
            event_proxy.send_event(
                neoism_backend::event::RioEventType::Rio(
                    neoism_backend::event::RioEvent::AcpWake,
                ),
                window_id,
            );
        });

        match crate::neoism::acp::AcpClientHandle::spawn(
            config,
            self.acp_events_tx.clone(),
            wake,
        ) {
            Ok(handle) => {
                handle.start_default_session(initial_prompt);
                self.acp_handles.push(handle);
            }
            Err(message) => {
                self.renderer.notifications.push(
                    message,
                    neoism_ui::panels::notifications::NotificationLevel::Error,
                );
                self.mark_dirty();
            }
        }
    }

    pub(crate) fn start_opencode_acp_session(&mut self, initial_prompt: Option<String>) {
        let cwd = self
            .active_pane_workspace_root()
            .or_else(|| self.active_workspace_root.clone())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let config = crate::neoism::acp::AcpServerConfig::new(
            "opencode", "OpenCode", "opencode", cwd,
        )
        .args(["acp"]);
        self.start_acp_server_session(config, initial_prompt);
    }

    pub(crate) fn respond_acp_permission(
        &mut self,
        server_id: &str,
        request_id: u64,
        option_id: Option<String>,
    ) -> bool {
        self.acp_handles
            .iter()
            .find(|handle| handle.server_id() == server_id)
            .is_some_and(|handle| handle.respond_permission(request_id, option_id))
    }

    pub fn drain_acp_events(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut events = Vec::new();
            while let Ok(event) = self.acp_events_rx.try_recv() {
                events.push(event);
            }
            for event in events {
                self.handle_acp_event(event);
            }
        }
    }

    pub(crate) fn handle_acp_event(&mut self, event: crate::neoism::acp::AcpUiEvent) {
        use crate::neoism::acp::AcpUiEvent;
        use neoism_ui::panels::notifications::NotificationLevel;

        match event {
            AcpUiEvent::Started {
                server_id: _,
                name,
                pid,
            } => {
                let suffix = pid.map(|pid| format!(" pid {pid}")).unwrap_or_default();
                self.renderer.notifications.push(
                    format!("Started {name} ACP{suffix}"),
                    NotificationLevel::Info,
                );
            }
            AcpUiEvent::Initialized {
                server_id,
                agent_capabilities,
                auth_methods,
            } => {
                tracing::info!(
                    target: "neoism::acp",
                    server_id,
                    capabilities = %agent_capabilities,
                    auth_methods = %auth_methods,
                    "ACP initialized"
                );
            }
            AcpUiEvent::SessionCreated {
                server_id,
                session_id,
                cwd,
                response,
            } => {
                tracing::info!(
                    target: "neoism::acp",
                    server_id,
                    session_id,
                    cwd = %cwd.display(),
                    response = %response,
                    "ACP session created"
                );
                self.renderer.notifications.push(
                    format!("ACP session ready in {}", cwd.display()),
                    NotificationLevel::Info,
                );
            }
            AcpUiEvent::SessionUpdate {
                server_id,
                session_id,
                update,
            } => {
                self.handle_acp_session_update(&server_id, &session_id, update);
            }
            AcpUiEvent::PromptFinished {
                server_id,
                session_id,
                stop_reason,
            } => {
                tracing::info!(
                    target: "neoism::acp",
                    server_id,
                    session_id,
                    stop_reason = ?stop_reason,
                    "ACP prompt finished"
                );
                self.renderer.notifications.push(
                    format!(
                        "ACP prompt finished{}",
                        stop_reason
                            .as_deref()
                            .map(|reason| format!(": {reason}"))
                            .unwrap_or_default()
                    ),
                    NotificationLevel::Info,
                );
            }
            AcpUiEvent::FileRead {
                server_id,
                session_id,
                path,
            } => {
                tracing::debug!(
                    target: "neoism::acp",
                    server_id,
                    session_id = ?session_id,
                    path = %path.display(),
                    "ACP file read"
                );
            }
            AcpUiEvent::FileWritten {
                server_id,
                session_id,
                path,
                bytes,
            } => {
                tracing::info!(
                    target: "neoism::acp",
                    server_id,
                    session_id = ?session_id,
                    path = %path.display(),
                    bytes,
                    "ACP file written"
                );
                self.handle_acp_file_written(path, bytes);
            }
            AcpUiEvent::PermissionRequested {
                server_id,
                session_id,
                request_id,
                tool_call,
                options,
            } => {
                tracing::info!(
                    target: "neoism::acp",
                    server_id,
                    session_id = ?session_id,
                    tool_call = %tool_call,
                    options = %options,
                    "ACP permission request received"
                );
                self.open_acp_permission_modal(server_id, request_id, tool_call, options);
            }
            AcpUiEvent::TerminalCreated {
                server_id,
                session_id,
                terminal_id,
                command,
            } => {
                tracing::info!(
                    target: "neoism::acp",
                    server_id,
                    session_id = ?session_id,
                    terminal_id,
                    command,
                    "ACP terminal created"
                );
            }
            AcpUiEvent::Stderr { server_id, line } => {
                tracing::warn!(
                    target: "neoism::acp",
                    server_id,
                    line,
                    "ACP stderr"
                );
            }
            AcpUiEvent::DebugLine {
                server_id,
                direction,
                line,
            } => {
                if std::env::var_os("NEOISM_ACP_LOG").is_some() {
                    tracing::debug!(
                        target: "neoism::acp",
                        server_id,
                        direction = ?direction,
                        line,
                        "ACP JSON-RPC"
                    );
                }
            }
            AcpUiEvent::Exited { server_id, status } => {
                self.renderer.notifications.push(
                    format!("ACP server {server_id} exited ({status:?})"),
                    NotificationLevel::Warn,
                );
            }
            AcpUiEvent::Error { server_id, message } => {
                self.renderer.notifications.push(
                    format!("ACP {server_id}: {message}"),
                    NotificationLevel::Error,
                );
            }
        }
        self.mark_dirty();
    }

    pub(crate) fn open_acp_permission_modal(
        &mut self,
        server_id: String,
        request_id: u64,
        tool_call: serde_json::Value,
        options: serde_json::Value,
    ) {
        use neoism_ui::widgets::modal::{ModalAction, ModalButton, ModalSpec};

        let title = tool_call
            .get("title")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("ACP Permission Request");
        let body = format!(
            "The agent is asking Neoism for permission.\n\n```json\n{}\n```",
            acp_pretty_json(&tool_call, 4000)
        );

        let mut buttons = Vec::new();
        if let Some(options) = options.as_array() {
            for (index, option) in options.iter().take(8).enumerate() {
                let Some(option_id) = acp_permission_option_id(option) else {
                    continue;
                };
                buttons.push(ModalButton::new(
                    acp_permission_option_label(option),
                    (index + 1).to_string(),
                    ModalAction::AcpPermission {
                        server_id: server_id.clone(),
                        request_id,
                        option_id: Some(option_id),
                    },
                ));
            }
        }
        buttons.push(ModalButton::new(
            "Cancel",
            "Esc",
            ModalAction::AcpPermission {
                server_id,
                request_id,
                option_id: None,
            },
        ));

        self.renderer.modal.open(ModalSpec {
            title: title.to_string(),
            body,
            meta: "Choose an option to continue the agent turn.".to_string(),
            input: None,
            buttons,
            busy: false,
            blocking: true,
        });
    }

    pub(crate) fn handle_acp_session_update(
        &mut self,
        server_id: &str,
        session_id: &str,
        update: serde_json::Value,
    ) {
        let kind = update
            .get("sessionUpdate")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown");
        if kind == "tool_call" || kind == "tool_call_update" {
            if let Some(path) = acp_update_primary_path(&update) {
                self.renderer.file_tree.set_active_path(Some(path));
            }
        }
        tracing::debug!(
            target: "neoism::acp",
            server_id,
            session_id,
            kind,
            update = %update,
            "ACP session update"
        );
    }

    pub(crate) fn handle_acp_file_written(&mut self, path: PathBuf, bytes: usize) {
        self.renderer.notifications.push(
            format!("ACP wrote {} bytes to {}", bytes, path.display()),
            neoism_ui::panels::notifications::NotificationLevel::Info,
        );

        if crate::editor::markdown::state::is_markdown_path(&path) {
            self.open_path_in_markdown(path);
        } else {
            self.open_path_in_editor(path);
            self.send_editor_command_raw("checktime".to_string());
        }
    }
}
