use super::*;

impl EditorBackend {
    pub fn daemon(
        surface_id: String,
        handle: DaemonClientHandle,
        runtime: Option<tokio::runtime::Handle>,
        config: NvimSpawnConfig,
    ) -> Self {
        let (send_tx, resize_tx) = DaemonEditorBackend::spawn_send_worker(
            handle.clone(),
            runtime.clone(),
            config.cwd.clone(),
            surface_id.clone(),
        );
        Self::Daemon(DaemonEditorBackend {
            surface_id,
            handle,
            runtime,
            config,
            send_tx,
            resize_tx,
        })
    }

    pub fn surface_id(&self) -> Option<&str> {
        match self {
            Self::Local(_) => None,
            Self::Daemon(editor) => Some(&editor.surface_id),
        }
    }

    pub fn daemon_bootstrap(&self) -> Option<(String, NvimSpawnConfig)> {
        match self {
            Self::Local(_) => None,
            Self::Daemon(editor) => {
                Some((editor.surface_id.clone(), editor.config.clone()))
            }
        }
    }

    pub fn clear_redraw_wake(&self) {
        if let Self::Local(editor) = self {
            editor.clear_redraw_wake();
        }
    }

    pub fn input(&self, keys: impl Into<String>) {
        match self {
            Self::Local(editor) => editor.input(keys),
            Self::Daemon(editor) => editor.send(EditorClientMessage::SendKeys {
                bytes: keys.into().into_bytes(),
                surface_id: Some(editor.surface_id.clone()),
            }),
        }
    }

    pub fn command(&self, cmd: impl Into<String>) {
        match self {
            Self::Local(editor) => editor.command(cmd),
            Self::Daemon(editor) => editor.send(EditorClientMessage::Command {
                command: cmd.into(),
                surface_id: Some(editor.surface_id.clone()),
            }),
        }
    }

    pub fn lsp_action(&self, action: neoism_protocol::editor::EditorLspAction) {
        self.lsp_action_with_text(action, None);
    }

    pub fn lsp_action_with_text(
        &self,
        action: neoism_protocol::editor::EditorLspAction,
        text: Option<String>,
    ) {
        match self {
            Self::Local(_) => {}
            Self::Daemon(editor) => editor.send(EditorClientMessage::LspAction {
                action,
                text,
                surface_id: Some(editor.surface_id.clone()),
            }),
        }
    }

    pub fn apply_lsp_code_action(
        &self,
        action: neoism_protocol::editor::EditorLspCodeAction,
    ) {
        match self {
            Self::Local(_) => {}
            Self::Daemon(editor) => {
                editor.send(EditorClientMessage::ApplyLspCodeAction {
                    action,
                    surface_id: Some(editor.surface_id.clone()),
                })
            }
        }
    }

    /// Request engine completion at the current cursor. `seq` is echoed back
    /// so the reply handler can drop a response a newer keystroke superseded.
    pub fn lsp_complete(&self, seq: u64, trigger_character: Option<String>) {
        match self {
            Self::Local(_) => {}
            Self::Daemon(editor) => editor.send(EditorClientMessage::LspComplete {
                seq,
                trigger_character,
                surface_id: Some(editor.surface_id.clone()),
            }),
        }
    }

    /// Resolve and accept a daemon-owned completion. The full item returns to
    /// the daemon so textEdit ranges, additionalTextEdits and the originating
    /// server identity are not flattened into simulated backspaces.
    pub fn apply_lsp_completion(
        &self,
        item: neoism_protocol::editor::EditorLspCompletionItem,
        replace_prefix: String,
    ) {
        match self {
            Self::Local(_) => {}
            Self::Daemon(editor) => {
                editor.send(EditorClientMessage::ApplyLspCompletion {
                    item,
                    replace_prefix,
                    surface_id: Some(editor.surface_id.clone()),
                })
            }
        }
    }

    pub fn cancel_lsp_completion(&self) {
        match self {
            Self::Local(_) => {}
            Self::Daemon(editor) => {
                editor.send(EditorClientMessage::CancelLspCompletion {
                    surface_id: Some(editor.surface_id.clone()),
                })
            }
        }
    }

    /// Request hover docs at an explicit Neovim UI grid cell. The daemon uses
    /// Neovim's own hit testing to resolve the buffer position without moving
    /// the cursor.
    pub fn lsp_hover_at(&self, seq: u64, grid: i64, row: i64, col: i64) {
        match self {
            Self::Local(_) => {}
            Self::Daemon(editor) => editor.send(EditorClientMessage::LspHoverAt {
                seq,
                grid,
                row,
                col,
                surface_id: Some(editor.surface_id.clone()),
            }),
        }
    }

    pub fn open_buffer_at_location(&self, uri: String, line: u32, character: u32) {
        match self {
            Self::Local(_) => {}
            Self::Daemon(editor) => editor.send(EditorClientMessage::OpenBuffer {
                path: std::path::PathBuf::from(uri),
                line: Some(line),
                character: Some(character),
                surface_id: Some(editor.surface_id.clone()),
            }),
        }
    }

    pub fn resize(&self, cols: u64, rows: u64) {
        match self {
            Self::Local(editor) => editor.resize(cols, rows),
            Self::Daemon(editor) => editor.resize(
                cols.min(u64::from(u32::MAX)) as u32,
                rows.min(u64::from(u32::MAX)) as u32,
            ),
        }
    }

    pub fn mouse_input(
        &self,
        button: impl Into<String>,
        action: impl Into<String>,
        modifier: impl Into<String>,
        grid: i64,
        row: i64,
        col: i64,
    ) {
        match self {
            Self::Local(editor) => {
                editor.mouse_input(button, action, modifier, grid, row, col);
            }
            Self::Daemon(editor) => editor.send(EditorClientMessage::MouseInput {
                button: button.into(),
                action: action.into(),
                modifier: modifier.into(),
                grid,
                row,
                col,
                count: 1,
                surface_id: Some(editor.surface_id.clone()),
            }),
        }
    }

    pub fn mouse_input_many(
        &self,
        button: impl Into<String>,
        action: impl Into<String>,
        modifier: impl Into<String>,
        grid: i64,
        row: i64,
        col: i64,
        count: u32,
    ) {
        match self {
            Self::Local(editor) => {
                editor.mouse_input_many(button, action, modifier, grid, row, col, count);
            }
            Self::Daemon(editor) => editor.send(EditorClientMessage::MouseInput {
                button: button.into(),
                action: action.into(),
                modifier: modifier.into(),
                grid,
                row,
                col,
                count,
                surface_id: Some(editor.surface_id.clone()),
            }),
        }
    }

    pub fn drain_buf_modified(&self) -> Vec<BufModifiedNotification> {
        match self {
            Self::Local(editor) => editor.drain_buf_modified(),
            Self::Daemon(_) => Vec::new(),
        }
    }

    pub fn drain_buf_enter(&self) -> Vec<BufEnterNotification> {
        match self {
            Self::Local(editor) => editor.drain_buf_enter(),
            Self::Daemon(_) => Vec::new(),
        }
    }

    pub fn drain_cwd(&self) -> Vec<CwdNotification> {
        match self {
            Self::Local(editor) => editor.drain_cwd(),
            Self::Daemon(_) => Vec::new(),
        }
    }

    pub fn drain_notifications(&self) -> Vec<RioNotify> {
        match self {
            Self::Local(editor) => editor.drain_notifications(),
            Self::Daemon(_) => Vec::new(),
        }
    }

    pub fn drain_winbar(&self) -> Option<WinbarNotification> {
        match self {
            Self::Local(editor) => editor.drain_winbar(),
            Self::Daemon(_) => None,
        }
    }

    pub fn drain_showcmd(&self) -> Option<String> {
        match self {
            Self::Local(editor) => editor.drain_showcmd(),
            Self::Daemon(_) => None,
        }
    }

    #[allow(dead_code)]
    pub fn drain_lsp_status(&self) -> Option<LspStatusNotification> {
        match self {
            Self::Local(editor) => editor.drain_lsp_status(),
            Self::Daemon(_) => None,
        }
    }

    pub fn drain_all_lsp_statuses(&self) -> Vec<LspStatusNotification> {
        match self {
            Self::Local(editor) => editor.drain_all_lsp_statuses(),
            Self::Daemon(_) => Vec::new(),
        }
    }

    pub fn drain_lsp_snapshot(
        &self,
    ) -> Option<neoism_backend::performer::nvim::LspSnapshotNotification> {
        match self {
            Self::Local(editor) => editor.drain_lsp_snapshot(),
            Self::Daemon(_) => None,
        }
    }

    pub fn drain_lsp_messages(
        &self,
    ) -> Vec<neoism_backend::performer::nvim::LspMessageNotification> {
        match self {
            Self::Local(editor) => editor.drain_lsp_messages(),
            Self::Daemon(_) => Vec::new(),
        }
    }

    pub fn drain_diagnostics(&self) -> Option<DiagnosticsNotification> {
        match self {
            Self::Local(editor) => editor.drain_diagnostics(),
            Self::Daemon(_) => None,
        }
    }

    pub fn drain_yank_flashes(&self) -> Vec<YankFlashNotification> {
        match self {
            Self::Local(editor) => editor.drain_yank_flashes(),
            Self::Daemon(_) => Vec::new(),
        }
    }

    pub fn drain_search_matches(&self) -> Option<SearchMatchesNotification> {
        match self {
            Self::Local(editor) => editor.drain_search_matches(),
            Self::Daemon(_) => None,
        }
    }

    pub fn drain_minimap(&self) -> Option<MinimapNotification> {
        match self {
            Self::Local(editor) => editor.drain_minimap(),
            Self::Daemon(_) => None,
        }
    }

    pub fn drain_modals(&self) -> Vec<ModalNotification> {
        match self {
            Self::Local(editor) => editor.drain_modals(),
            Self::Daemon(_) => Vec::new(),
        }
    }

    pub fn drain_treesitter_missing(&self) -> Vec<TreesitterMissingNotification> {
        match self {
            Self::Local(editor) => editor.drain_treesitter_missing(),
            Self::Daemon(_) => Vec::new(),
        }
    }

    pub fn config(&self) -> &NvimSpawnConfig {
        match self {
            Self::Local(editor) => editor.config(),
            Self::Daemon(editor) => &editor.config,
        }
    }
}

impl DaemonEditorBackend {
    fn spawn_send_worker(
        handle: DaemonClientHandle,
        runtime: Option<tokio::runtime::Handle>,
        workspace_root: Option<std::path::PathBuf>,
        surface_id: String,
    ) -> (
        Option<tokio_mpsc::UnboundedSender<EditorClientMessage>>,
        Option<tokio_watch::Sender<(u32, u32)>>,
    ) {
        let (send_tx, mut recv_rx) =
            tokio_mpsc::unbounded_channel::<EditorClientMessage>();
        let (resize_tx, mut resize_rx) = tokio_watch::channel::<(u32, u32)>((0, 0));
        let worker = async move {
            enum Outbound {
                Message(EditorClientMessage),
                Resize(u32, u32),
            }

            let mut messages_open = true;
            let mut resize_open = true;
            while messages_open || resize_open {
                let outbound = tokio::select! {
                    message = recv_rx.recv(), if messages_open => {
                        match message {
                            Some(message) => Some(Outbound::Message(message)),
                            None => {
                                messages_open = false;
                                None
                            }
                        }
                    }
                    changed = resize_rx.changed(), if resize_open => {
                        match changed {
                            Ok(()) => {
                                // The watch channel is already a latest-wins
                                // queue while the websocket send is pending.
                                // Do not add another frame of latency here:
                                // the daemon is the single resize-debounce
                                // boundary, and it lets the first resize
                                // through immediately.
                                let (width, height) = *resize_rx.borrow_and_update();
                                Some(Outbound::Resize(width, height))
                            }
                            Err(_) => {
                                resize_open = false;
                                None
                            }
                        }
                    }
                };
                let Some(outbound) = outbound else {
                    continue;
                };
                let message = match outbound {
                    Outbound::Message(message) => message,
                    Outbound::Resize(width, height) => EditorClientMessage::Resize {
                        width,
                        height,
                        surface_id: Some(surface_id.clone()),
                    },
                };
                if let Err(error) = handle
                    .send_editor_with_workspace_root(message, workspace_root.clone())
                    .await
                {
                    tracing::warn!(
                        target: "neoism::daemon_editor",
                        %error,
                        "daemon editor request failed"
                    );
                }
            }
        };
        if let Some(runtime) = runtime {
            runtime.spawn(worker);
            return (Some(send_tx), Some(resize_tx));
        }
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(worker);
            return (Some(send_tx), Some(resize_tx));
        }
        (None, None)
    }

    fn resize(&self, width: u32, height: u32) {
        let next = (width.max(1), height.max(1));
        if let Some(tx) = &self.resize_tx {
            if *tx.borrow() != next {
                let _ = tx.send(next);
            }
            return;
        }
        self.send(EditorClientMessage::Resize {
            width: next.0,
            height: next.1,
            surface_id: Some(self.surface_id.clone()),
        });
    }

    fn send(&self, message: EditorClientMessage) {
        if let Some(send_tx) = &self.send_tx {
            if let Err(error) = send_tx.send(message) {
                tracing::warn!(
                    target: "neoism::daemon_editor",
                    %error,
                    "daemon editor send queue closed"
                );
            }
            return;
        }

        let handle = self.handle.clone();
        let workspace_root = self.config.cwd.clone();
        if let Some(runtime) = self.runtime.clone() {
            runtime.spawn(async move {
                if let Err(error) = handle
                    .send_editor_with_workspace_root(message, workspace_root)
                    .await
                {
                    tracing::warn!(
                        target: "neoism::daemon_editor",
                        %error,
                        "daemon editor request failed"
                    );
                }
            });
            return;
        }
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                if let Err(error) = handle
                    .send_editor_with_workspace_root(message, workspace_root)
                    .await
                {
                    tracing::warn!(
                        target: "neoism::daemon_editor",
                        %error,
                        "daemon editor request failed"
                    );
                }
            });
        }
    }
}
