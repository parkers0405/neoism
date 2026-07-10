use super::*;

pub(crate) struct ExternalMessage {
    pub(crate) kind: String,
    pub(crate) text: String,
}

/// nvim-rs Handler that forwards `redraw` notifications to Rio.
/// Other notify/request kinds are logged at trace level for now —
/// Phase 2c may grow handlers for clipboard, custom rpc, etc.
#[derive(Clone)]
struct BridgeHandler<T: EventListener> {
    redraw_tx: std_mpsc::Sender<RedrawNotification>,
    buf_mod_tx: std_mpsc::Sender<BufModifiedNotification>,
    buf_enter_tx: std_mpsc::Sender<BufEnterNotification>,
    cwd_tx: std_mpsc::Sender<CwdNotification>,
    notify_tx: std_mpsc::Sender<RioNotify>,
    winbar_tx: std_mpsc::Sender<WinbarNotification>,
    showcmd_tx: std_mpsc::Sender<String>,
    lsp_status_tx: std_mpsc::Sender<LspStatusNotification>,
    lsp_snapshot_tx: std_mpsc::Sender<LspSnapshotNotification>,
    lsp_message_tx: std_mpsc::Sender<LspMessageNotification>,
    diagnostics_tx: std_mpsc::Sender<DiagnosticsNotification>,
    yank_flash_tx: std_mpsc::Sender<YankFlashNotification>,
    search_matches_tx: std_mpsc::Sender<SearchMatchesNotification>,
    minimap_tx: std_mpsc::Sender<MinimapNotification>,
    modal_tx: std_mpsc::Sender<ModalNotification>,
    treesitter_missing_tx: std_mpsc::Sender<TreesitterMissingNotification>,
    redraw_wake_in_flight: Arc<AtomicBool>,
    event_proxy: T,
    window_id: WindowId,
    route_id: usize,
}

#[async_trait]
impl<T> Handler for BridgeHandler<T>
where
    T: EventListener + Clone + Send + Sync + 'static,
{
    type Writer = NeovimWriter;

    async fn handle_notify(
        &self,
        name: String,
        args: Vec<Value>,
        _neovim: Neovim<Self::Writer>,
    ) {
        if name == "redraw" {
            let mut queued_any = false;
            for raw in args {
                for message in parse_external_messages(&raw) {
                    self.forward_external_message(message);
                }
                if let Some(text) = parse_showcmd(&raw) {
                    let _ = self.showcmd_tx.send(text);
                }
                if self.redraw_tx.send(RedrawNotification { raw }).is_err() {
                    // Receiver dropped — the pane is shutting down.
                    // Stop forwarding to avoid log spam.
                    return;
                }
                queued_any = true;
            }
            if queued_any {
                self.wake_ui();
            }
        } else if name == "rio_buf_modified" {
            // args = [path: String, modified: bool]
            if let (Some(path), Some(modified)) = (
                args.first().and_then(|v| v.as_str()).map(PathBuf::from),
                args.get(1).and_then(|v| v.as_bool()),
            ) {
                let _ = self
                    .buf_mod_tx
                    .send(BufModifiedNotification { path, modified });
                self.wake_ui();
            }
        } else if name == "rio_buf_enter" {
            // args = [path: String] — empty paths (scratch buffers,
            // unnamed buffers) are filtered upstream in lua.
            if let Some(path) = args.first().and_then(|v| v.as_str()).map(PathBuf::from) {
                let _ = self.buf_enter_tx.send(BufEnterNotification { path });
                self.wake_ui();
            }
        } else if name == "rio_cwd" {
            // args = [cwd: String]
            if let Some(path) = args.first().and_then(|v| v.as_str()).map(PathBuf::from) {
                let _ = self.cwd_tx.send(CwdNotification { path });
                self.wake_ui();
            }
        } else if name == "rio_winbar" {
            // args = [line: u64, col: u64, symbol: String?, total_lines: u64?]
            // CursorMoved fires hot — the per-frame drain on the
            // renderer side handles backpressure (we keep latest).
            let line = args.first().and_then(|v| v.as_u64()).unwrap_or(0);
            let col = args.get(1).and_then(|v| v.as_u64()).unwrap_or(0);
            let symbol = args
                .get(2)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let total_lines = args.get(3).and_then(|v| v.as_u64()).unwrap_or(0);
            let _ = self.winbar_tx.send(WinbarNotification {
                line,
                col,
                symbol,
                total_lines,
            });
            self.wake_ui();
        } else if name == "rio_lsp_status" {
            // args = [state, name?, binary?, filetype?] where state is
            // initializing|active|missing.
            if let Some(state) = args.first().and_then(|v| v.as_str()).map(String::from) {
                let name = args.get(1).and_then(|v| v.as_str()).map(String::from);
                let binary = args.get(2).and_then(|v| v.as_str()).map(String::from);
                let filetype = args.get(3).and_then(|v| v.as_str()).map(String::from);
                let _ = self.lsp_status_tx.send(LspStatusNotification {
                    state,
                    name,
                    binary,
                    filetype,
                });
                self.wake_ui();
            }
        } else if name == "rio_lsp_snapshot" {
            // args = [filetype: String, servers: Array<Map>] —
            // comprehensive per-buffer snapshot of every LSP server
            // that's attached OR registered as a candidate for the
            // filetype. See `LspSnapshotNotification`.
            let filetype = args
                .first()
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let servers: Vec<LspSnapshotServer> = args
                .get(1)
                .and_then(|v| v.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| {
                            let entries = item.as_map()?;
                            let mut name = String::new();
                            let mut binary = String::new();
                            let mut ft = String::new();
                            let mut state = String::new();
                            let mut source = None;
                            let mut message = None;
                            let mut level = None;
                            for (k, v) in entries {
                                let key = k.as_str().unwrap_or("");
                                match key {
                                    "name" => name = v.as_str().unwrap_or("").to_string(),
                                    "binary" => {
                                        binary = v.as_str().unwrap_or("").to_string()
                                    }
                                    "filetype" => {
                                        ft = v.as_str().unwrap_or("").to_string()
                                    }
                                    "state" => {
                                        state = v.as_str().unwrap_or("").to_string()
                                    }
                                    "source" => {
                                        source = v.as_str().map(|s| s.to_string())
                                    }
                                    "message" => {
                                        message = v.as_str().map(|s| s.to_string())
                                    }
                                    "level" => level = v.as_str().map(|s| s.to_string()),
                                    _ => {}
                                }
                            }
                            if name.is_empty() {
                                return None;
                            }
                            Some(LspSnapshotServer {
                                name,
                                binary,
                                filetype: ft,
                                state,
                                source,
                                message,
                                level,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            if std::env::var_os("NEOISM_LSP_LOG").is_some() {
                let states = servers
                    .iter()
                    .map(|server| {
                        format!(
                            "{}:{}:{}",
                            server.name,
                            server.state,
                            server.source.as_deref().unwrap_or("")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                tracing::info!(
                    target: "neoism::lsp",
                    route_id = self.route_id,
                    window_id = ?self.window_id,
                    filetype = %filetype,
                    server_count = servers.len(),
                    states = %states,
                    "received rio_lsp_snapshot"
                );
            }
            let _ = self
                .lsp_snapshot_tx
                .send(LspSnapshotNotification { filetype, servers });
            self.wake_ui();
        } else if name == "rio_lsp_message" {
            // args = [server: String, text: String, level: String]
            if let (Some(server), Some(text)) = (
                args.first().and_then(|v| v.as_str()).map(String::from),
                args.get(1).and_then(|v| v.as_str()).map(String::from),
            ) {
                let level = args
                    .get(2)
                    .and_then(|v| v.as_str())
                    .unwrap_or("info")
                    .to_string();
                let _ = self.lsp_message_tx.send(LspMessageNotification {
                    server,
                    text,
                    level,
                });
                self.wake_ui();
            }
        } else if name == "rio_diagnostics" {
            // args = [error_count, warn_count, info_count, hint_count,
            //         file_path: String, items: Vec<[lnum, severity, message, source]>]
            let error = args.first().and_then(|v| v.as_u64()).unwrap_or(0);
            let warn = args.get(1).and_then(|v| v.as_u64()).unwrap_or(0);
            let info = args.get(2).and_then(|v| v.as_u64()).unwrap_or(0);
            let hint = args.get(3).and_then(|v| v.as_u64()).unwrap_or(0);
            let file_path = args
                .get(4)
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(PathBuf::from);
            let mut items: Vec<DiagnosticItem> = Vec::new();
            if let Some(arr) = args.get(5).and_then(|v| v.as_array()) {
                for entry in arr {
                    let Some(tuple) = entry.as_array() else {
                        continue;
                    };
                    let lnum = tuple.first().and_then(|v| v.as_u64()).unwrap_or(1);
                    let severity =
                        tuple.get(1).and_then(|v| v.as_u64()).unwrap_or(1) as u8;
                    let message = tuple
                        .get(2)
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let source = tuple
                        .get(3)
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(ToOwned::to_owned);
                    items.push(DiagnosticItem {
                        lnum,
                        severity,
                        message,
                        source,
                    });
                }
            }
            if std::env::var_os("NEOISM_LSP_LOG").is_some() {
                let source_count =
                    items.iter().filter(|item| item.source.is_some()).count();
                tracing::info!(
                    target: "neoism::lsp",
                    route_id = self.route_id,
                    window_id = ?self.window_id,
                    error,
                    warn,
                    info,
                    hint,
                    items = items.len(),
                    source_count,
                    file_path = ?file_path,
                    "received rio_diagnostics"
                );
            }
            let _ = self.diagnostics_tx.send(DiagnosticsNotification {
                error,
                warn,
                info,
                hint,
                file_path,
                items,
            });
            self.wake_ui();
        } else if name == "rio_search_matches" {
            // args = [matches: Vec<[lnum, col, text]>]
            let mut matches: Vec<SearchMatch> = Vec::new();
            if let Some(arr) = args.first().and_then(|v| v.as_array()) {
                for entry in arr {
                    let Some(tuple) = entry.as_array() else {
                        continue;
                    };
                    let lnum = tuple.first().and_then(|v| v.as_u64()).unwrap_or(0);
                    let (col, text_index) = match tuple.get(1).and_then(|v| v.as_u64()) {
                        Some(col) => (col, 2),
                        None => (1, 1),
                    };
                    let text = tuple
                        .get(text_index)
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if lnum > 0 {
                        matches.push(SearchMatch { lnum, col, text });
                    }
                }
            }
            let _ = self
                .search_matches_tx
                .send(SearchMatchesNotification { matches });
            self.wake_ui();
        } else if name == "rio_minimap_snapshot" {
            // args = [path, changedtick, total_lines, top, bottom,
            //         cursor, sample_stride, lines?, git_changes?]
            let path = args
                .first()
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(PathBuf::from);
            let changedtick = args.get(1).and_then(|v| v.as_u64()).unwrap_or(0);
            let total_lines = args.get(2).and_then(|v| v.as_u64()).unwrap_or(0);
            let top_line = args.get(3).and_then(|v| v.as_u64()).unwrap_or(1);
            let bottom_line = args.get(4).and_then(|v| v.as_u64()).unwrap_or(top_line);
            let cursor_line = args.get(5).and_then(|v| v.as_u64()).unwrap_or(top_line);
            let sample_stride = args.get(6).and_then(|v| v.as_u64()).unwrap_or(1);
            let lines = args.get(7).and_then(|v| v.as_array()).map(|arr| {
                arr.iter()
                    .map(|value| value.as_str().unwrap_or("").to_string())
                    .collect::<Vec<_>>()
            });
            let git_changes = args
                .get(8)
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|entry| {
                            let tuple = entry.as_array()?;
                            let line =
                                tuple.first().and_then(|v| v.as_u64()).unwrap_or(0);
                            let kind = tuple
                                .get(1)
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            (line > 0 && !kind.is_empty())
                                .then_some(MinimapGitChange { line, kind })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let _ = self.minimap_tx.send(MinimapNotification {
                path,
                changedtick,
                total_lines,
                top_line,
                bottom_line,
                cursor_line,
                sample_stride,
                lines,
                git_changes,
            });
            self.wake_ui();
        } else if name == "rio_yank_flash" {
            // args = [row_top, row_bot, col_left?, col_right?] —
            // 0-based screen rows/cols already clamped by lua.
            let row_top = args.first().and_then(|v| v.as_u64()).unwrap_or(0);
            let row_bot = args.get(1).and_then(|v| v.as_u64()).unwrap_or(row_top);
            let col_left = args.get(2).and_then(|v| v.as_u64()).map(|v| v as u32);
            let col_right = args.get(3).and_then(|v| v.as_u64()).map(|v| v as u32);
            let _ = self.yank_flash_tx.send(YankFlashNotification {
                row_top: row_top as u32,
                row_bot: row_bot as u32,
                col_left,
                col_right,
            });
            self.wake_ui();
        } else if name == "rio_modal" || name == "rio_modal_actions" {
            // args = [title: String, body: String, level?: info|warn|error]
            if let (Some(title), Some(body)) = (
                args.first().and_then(|v| v.as_str()).map(String::from),
                args.get(1).and_then(|v| v.as_str()).map(String::from),
            ) {
                let level = args
                    .get(2)
                    .and_then(|v| v.as_str())
                    .map(NotifyLevel::from_str)
                    .unwrap_or(NotifyLevel::Info);
                let actions = args.get(3).map(parse_modal_actions).unwrap_or_default();
                let _ = self.modal_tx.send(ModalNotification {
                    title,
                    body,
                    level,
                    actions,
                });
                self.wake_ui();
            }
        } else if name == "rio_treesitter_missing" {
            // args = [lang: String, filetype?: String]
            if let Some(lang) = args.first().and_then(|v| v.as_str()).map(String::from) {
                let filetype = args
                    .get(1)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let _ = self
                    .treesitter_missing_tx
                    .send(TreesitterMissingNotification { lang, filetype });
                self.wake_ui();
            }
        } else if name == "rio_lsp_log" {
            if std::env::var_os("NEOISM_LSP_LOG").is_some() {
                let event = args.first().and_then(|v| v.as_str()).unwrap_or("unknown");
                let fields_json = args.get(1).and_then(|v| v.as_str()).unwrap_or("{}");
                tracing::info!(
                    target: "neoism::lsp",
                    route_id = self.route_id,
                    window_id = ?self.window_id,
                    event = %event,
                    fields = %fields_json,
                    "nvim lsp log"
                );
            }
        } else if name == "rio_notify" {
            // args = [message: String, level: String? (info|warn|error)]
            if let Some(message) = args.first().and_then(|v| v.as_str()).map(String::from)
            {
                let level = args
                    .get(1)
                    .and_then(|v| v.as_str())
                    .map(NotifyLevel::from_str)
                    .unwrap_or(NotifyLevel::Info);
                let _ = self.notify_tx.send(RioNotify { message, level });
                self.wake_ui();
            }
        } else if name == "rio_clipboard_store" {
            // args = [text: String, ty: String? (clipboard|selection), message: String?]
            if let Some(text) = args.first().and_then(|v| v.as_str()).map(String::from) {
                let ty = match args.get(1).and_then(|v| v.as_str()) {
                    Some("selection") => ClipboardType::Selection,
                    _ => ClipboardType::Clipboard,
                };
                self.event_proxy
                    .send_event(RioEvent::ClipboardStore(ty, text), self.window_id);

                if let Some(message) =
                    args.get(2).and_then(|v| v.as_str()).map(String::from)
                {
                    let level = args
                        .get(3)
                        .and_then(|v| v.as_str())
                        .map(NotifyLevel::from_str)
                        .unwrap_or(NotifyLevel::Info);
                    let _ = self.notify_tx.send(RioNotify { message, level });
                    self.wake_ui();
                }
            }
        } else {
            tracing::trace!(target: "neoism_backend::nvim", "ignoring notify {name}");
        }
    }

    async fn handle_request(
        &self,
        name: String,
        _args: Vec<Value>,
        _neovim: Neovim<Self::Writer>,
    ) -> Result<Value, Value> {
        tracing::trace!(target: "neoism_backend::nvim", "ignoring request {name}");
        Ok(Value::Nil)
    }
}

impl<T> BridgeHandler<T>
where
    T: EventListener + Clone + Send + Sync + 'static,
{
    fn forward_external_message(&self, message: ExternalMessage) {
        let text = message.text.trim().to_string();
        if text.is_empty() {
            return;
        }

        let level = match message.kind.as_str() {
            "emsg" | "lua_error" => NotifyLevel::Error,
            "wmsg" => NotifyLevel::Warn,
            _ => NotifyLevel::Info,
        };

        let long = text.len() > 160 || text.lines().count() > 3;
        if level == NotifyLevel::Error || long {
            let title = match level {
                NotifyLevel::Error => "Nvim Error",
                NotifyLevel::Warn => "Nvim Warning",
                NotifyLevel::Info => "Nvim Message",
            };
            let _ = self.modal_tx.send(ModalNotification {
                title: title.to_string(),
                body: text,
                level,
                actions: Vec::new(),
            });
        } else {
            let _ = self.notify_tx.send(RioNotify {
                message: text,
                level,
            });
        }

        self.wake_ui();
    }

    fn wake_ui(&self) {
        if !self.redraw_wake_in_flight.swap(true, Ordering::AcqRel) {
            self.event_proxy
                .send_event(RioEvent::TerminalDamaged(self.route_id), self.window_id);
        }
    }
}

/// Extract the latest `msg_showcmd` state from one raw redraw event.
/// `Some("")` means nvim cleared the pending command; `None` means the
/// event wasn't a showcmd update at all. Content shape matches
/// `msg_show`: `[[attr_id, text], ...]`.
pub(crate) fn parse_showcmd(raw: &Value) -> Option<String> {
    let items = raw.as_array()?;
    if items.first().and_then(Value::as_str)? != "msg_showcmd" {
        return None;
    }
    let mut latest = None;
    for params in items.iter().skip(1) {
        let Some(params) = params.as_array() else {
            continue;
        };
        let Some(content) = params.first() else {
            continue;
        };
        latest = Some(message_content_text(content));
    }
    latest
}

pub(crate) fn parse_external_messages(raw: &Value) -> Vec<ExternalMessage> {
    let Some(items) = raw.as_array() else {
        return Vec::new();
    };
    let Some(event_name) = items.first().and_then(Value::as_str) else {
        return Vec::new();
    };
    if event_name != "msg_show" {
        return Vec::new();
    }

    let mut out = Vec::new();
    for params in items.iter().skip(1) {
        let Some(params) = params.as_array() else {
            continue;
        };
        let Some(kind) = params.first().and_then(Value::as_str) else {
            continue;
        };
        let Some(content) = params.get(1) else {
            continue;
        };
        let history = params.get(3).and_then(Value::as_bool).unwrap_or(true);
        if kind == "bufwrite" && !history {
            // `:write` with ext_messages sends a transient path-only
            // update first, then the final history entry containing
            // the byte/line count. Toasting both looks like Rio ran
            // the save twice, so keep only the final persisted message.
            continue;
        }
        let text = message_content_text(content);
        if text.trim().is_empty() {
            continue;
        }
        out.push(ExternalMessage {
            kind: kind.to_string(),
            text,
        });
    }
    out
}

fn message_content_text(content: &Value) -> String {
    let Some(chunks) = content.as_array() else {
        return String::new();
    };

    let mut out = String::new();
    for chunk in chunks {
        let Some(chunk) = chunk.as_array() else {
            continue;
        };
        if let Some(text) = chunk.get(1).and_then(Value::as_str) {
            out.push_str(text);
        }
    }
    out
}

fn parse_modal_actions(value: &Value) -> Vec<ModalActionNotification> {
    let Some(items) = value.as_array() else {
        return Vec::new();
    };

    items
        .iter()
        .filter_map(|item| {
            let item = item.as_array()?;
            let label = item.first().and_then(Value::as_str)?.to_string();
            let hint = item
                .get(1)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let command = item.get(2).and_then(Value::as_str)?.to_string();
            Some(ModalActionNotification {
                label,
                hint,
                command,
            })
        })
        .collect()
}
/// Body of the dedicated runtime thread. Builds a current-thread
/// tokio runtime, spawns nvim, completes handshake + ui_attach, and
/// then services commands until shutdown.
pub(crate) fn run_nvim_runtime(
    config: NvimSpawnConfig,
    mut cmd_rx: tokio_mpsc::UnboundedReceiver<NvimCommand>,
    mut resize_rx: tokio_watch::Receiver<(u64, u64)>,
    redraw_tx: std_mpsc::Sender<RedrawNotification>,
    buf_mod_tx: std_mpsc::Sender<BufModifiedNotification>,
    buf_enter_tx: std_mpsc::Sender<BufEnterNotification>,
    cwd_tx: std_mpsc::Sender<CwdNotification>,
    notify_tx: std_mpsc::Sender<RioNotify>,
    winbar_tx: std_mpsc::Sender<WinbarNotification>,
    showcmd_tx: std_mpsc::Sender<String>,
    lsp_status_tx: std_mpsc::Sender<LspStatusNotification>,
    lsp_snapshot_tx: std_mpsc::Sender<LspSnapshotNotification>,
    lsp_message_tx: std_mpsc::Sender<LspMessageNotification>,
    diagnostics_tx: std_mpsc::Sender<DiagnosticsNotification>,
    yank_flash_tx: std_mpsc::Sender<YankFlashNotification>,
    search_matches_tx: std_mpsc::Sender<SearchMatchesNotification>,
    minimap_tx: std_mpsc::Sender<MinimapNotification>,
    modal_tx: std_mpsc::Sender<ModalNotification>,
    treesitter_missing_tx: std_mpsc::Sender<TreesitterMissingNotification>,
    redraw_wake_in_flight: Arc<AtomicBool>,
    child_pgid: Arc<AtomicI32>,
    ready_tx: std_mpsc::Sender<Result<()>>,
    event_proxy: impl EventListener + Clone + Send + Sync + 'static,
    window_id: WindowId,
    route_id: usize,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            let _ = ready_tx.send(Err(anyhow!("failed to build tokio runtime: {e}")));
            return;
        }
    };

    runtime.block_on(async move {
        // 1. Spawn nvim --embed with piped stdio.
        let mut cmd = build_nvim_command(&config);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = ready_tx.send(Err(anyhow!("failed to spawn nvim: {e}")));
                return;
            }
        };

        // Publish the child's PID so `Drop` can reap the whole process
        // group (nvim + every LSP server it spawns) with one signal. On
        // unix `build_nvim_command` made nvim a group leader, so its pgid
        // equals this pid. Store `0` if the platform doesn't report a pid.
        if let Some(pid) = child.id() {
            child_pgid.store(pid as i32, Ordering::SeqCst);
        }

        let stdout = match child.stdout.take() {
            Some(s) => s,
            None => {
                let _ = ready_tx.send(Err(anyhow!("nvim child missing stdout")));
                return;
            }
        };
        let stdin = match child.stdin.take() {
            Some(s) => s,
            None => {
                let _ = ready_tx.send(Err(anyhow!("nvim child missing stdin")));
                return;
            }
        };
        // stderr drained off-thread so it doesn't block nvim. We don't
        // care about contents in Phase 2b — Phase 2c may surface them.
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                use tokio::io::{AsyncBufReadExt, BufReader};
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::warn!(target: "neoism_backend::nvim", "nvim stderr: {line}");
                }
            });
        }

        // 2. Complete the rpc handshake.
        let handler = BridgeHandler {
            redraw_tx: redraw_tx.clone(),
            buf_mod_tx: buf_mod_tx.clone(),
            buf_enter_tx: buf_enter_tx.clone(),
            cwd_tx: cwd_tx.clone(),
            notify_tx: notify_tx.clone(),
            winbar_tx: winbar_tx.clone(),
            showcmd_tx: showcmd_tx.clone(),
            lsp_status_tx: lsp_status_tx.clone(),
            lsp_snapshot_tx: lsp_snapshot_tx.clone(),
            lsp_message_tx: lsp_message_tx.clone(),
            diagnostics_tx: diagnostics_tx.clone(),
            yank_flash_tx: yank_flash_tx.clone(),
            search_matches_tx: search_matches_tx.clone(),
            minimap_tx: minimap_tx.clone(),
            modal_tx: modal_tx.clone(),
            treesitter_missing_tx: treesitter_missing_tx.clone(),
            redraw_wake_in_flight,
            event_proxy,
            window_id,
            route_id,
        };
        let writer: NeovimWriter = Box::new(stdin.compat_write());
        let reader = stdout.compat();
        let handshake = Neovim::<NeovimWriter>::handshake(
            reader,
            writer,
            handler,
            "RioNvimHandshake",
        )
        .await;
        let (neovim, io_future) = match handshake {
            Ok(pair) => pair,
            Err(e) => {
                let _ = ready_tx.send(Err(anyhow!("nvim handshake failed: {e}")));
                return;
            }
        };

        // 3. Drive the io future on the runtime — it owns the rpc
        //    read loop. Errors here mean nvim disconnected.
        tokio::spawn(async move {
            if let Err(e) = io_future.await {
                tracing::warn!(target: "neoism_backend::nvim", "nvim io loop ended: {e}");
            }
        });

        // 4. Run any user-supplied --cmd init commands.
        for cmd in &config.init_commands {
            if let Err(e) = neovim.command(cmd).await {
                tracing::warn!(target: "neoism_backend::nvim", "init command `{cmd}` failed: {e}");
            }
        }

        // 5. ui_attach with initial geometry. Defaults are reasonable
        //    if the caller forgot to set them.
        let cols = if config.initial_cols == 0 { 80 } else { config.initial_cols };
        let rows = if config.initial_rows == 0 { 24 } else { config.initial_rows };
        let mut ui_options = UiAttachOptions::new();
        ui_options
            .set_rgb(true)
            .set_linegrid_external(true)
            .set_popupmenu_external(true)
            .set_messages_externa(true);
        if let Err(e) = neovim.ui_attach(cols as i64, rows as i64, &ui_options).await {
            let _ = ready_tx.send(Err(anyhow!("ui_attach failed: {e}")));
            return;
        }

        // Handshake + ui_attach complete — release the caller. Initial
        // file loading continues below on the nvim runtime thread so
        // window/editor construction is not gated on disk IO, filetype
        // autocmds, LSP startup, or first-buffer redraw volume.
        let _ = ready_tx.send(Ok(()));

        // Optionally :edit the initial file. Anchor cwd FIRST so any
        // FileType autocmd / LSP that fires off `:edit` sees the
        // project root, not nvim's startup pwd.
        if let Some(cwd) = &config.cwd {
            let cd_cmd = format!(
                r#"lua pcall(vim.cmd.cd, {})"#,
                lua_string_literal(&cwd.display().to_string())
            );
            if let Err(e) = neovim.command(&cd_cmd).await {
                tracing::warn!(target: "neoism_backend::nvim", "initial :cd failed: {e}");
            }
        }
        if let Some(path) = &config.initial_file {
            let edit_cmd = vim_edit_command(&path.display().to_string());
            tracing::debug!(target: "neoism_backend::nvim", "initial open: {edit_cmd}");
            if let Err(e) = neovim.command(&edit_cmd).await {
                tracing::warn!(target: "neoism_backend::nvim", "initial :edit failed: {e}");
            }
        }

        // 6. Service commands until Shutdown / sender drop.
        //
        // EVERY rpc await below is bounded by a timeout. An unbounded await
        // here was the year-old freeze: if a single response is lost (reader
        // hiccup) or nvim blocks inside a prompt that needs INPUT to clear
        // (vim.fn.input(), confirm(), :! reading stdin — ext_messages does
        // not cover them all), the loop hung on that one await forever, so
        // the very input that could unstick nvim could never be sent —
        // mutual deadlock with both processes idle in epoll. Timing out
        // keeps the pump alive: the user's next Esc/Enter still reaches
        // nvim and the stall self-recovers instead of freezing the editor.
        const INPUT_RPC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
        const COMMAND_RPC_TIMEOUT: std::time::Duration =
            std::time::Duration::from_secs(30);
        // After the first timeout the bridge is degraded: keep draining the
        // queue with a short bound so mashed keys don't each wait the full
        // window; the first successful reply clears the flag.
        const DEGRADED_RPC_TIMEOUT: std::time::Duration =
            std::time::Duration::from_millis(250);

        // Non-fast RPC (nvim_command / nvim_ui_try_resize) rides a
        // SEPARATE sequential lane. nvim defers every non-fast request
        // while normal mode has a pending count or operator (a bare "1"
        // left open), a prompt, or a hit-enter page — only FUNC_API_FAST
        // calls (nvim_input / nvim_input_mouse) are still answered in
        // that state. With a single lane, one deferred :bnext or
        // :checktime queued the very <Esc> that would clear the pending
        // state behind the 30s timeout: press a digit, editor dead.
        // Split lanes keep input flowing no matter what a command is
        // waiting on; the deferred command completes naturally once
        // input unblocks nvim.
        let (slow_tx, mut slow_rx) = tokio_mpsc::unbounded_channel::<String>();
        let slow_neovim = neovim.clone();
        tokio::spawn(async move {
            enum SlowRequest {
                Command(String),
                Resize { cols: u64, rows: u64 },
            }

            let mut lane_degraded = false;
            let mut command_lane_open = true;
            let mut resize_lane_open = true;
            while command_lane_open || resize_lane_open {
                // Commands stay ordered, while resize notifications are
                // latest-wins. If 200 sizes arrive while one RPC is pending,
                // `borrow_and_update` returns only size 200; sizes 1..199 never
                // reach Neovim and therefore never generate stale redraws.
                let request = tokio::select! {
                    command = slow_rx.recv(), if command_lane_open => {
                        match command {
                            Some(command) => Some(SlowRequest::Command(command)),
                            None => {
                                command_lane_open = false;
                                None
                            }
                        }
                    }
                    changed = resize_rx.changed(), if resize_lane_open => {
                        match changed {
                            Ok(()) => {
                                let (cols, rows) = *resize_rx.borrow_and_update();
                                Some(SlowRequest::Resize { cols, rows })
                            }
                            Err(_) => {
                                resize_lane_open = false;
                                None
                            }
                        }
                    }
                };
                let Some(request) = request else {
                    continue;
                };

                match request {
                    SlowRequest::Command(cmd) => {
                        let command_timeout = if lane_degraded {
                            DEGRADED_RPC_TIMEOUT
                        } else {
                            COMMAND_RPC_TIMEOUT
                        };
                        match tokio::time::timeout(
                            command_timeout,
                            slow_neovim.command(&cmd),
                        )
                        .await
                        {
                            Err(_) => {
                                lane_degraded = true;
                                tracing::error!(
                                    target: "neoism_backend::nvim",
                                    command = %cmd,
                                    "nvim command rpc timed out after {command_timeout:?}; \
                                     command lane continues (command may still be running in nvim)"
                                );
                            }
                            Ok(Err(e)) => tracing::warn!(target: "neoism_backend::nvim", "command `{cmd}` failed: {e}"),
                            Ok(Ok(_)) => lane_degraded = false,
                        }
                    }
                    SlowRequest::Resize { cols, rows } => {
                        let resize_timeout = if lane_degraded {
                            DEGRADED_RPC_TIMEOUT
                        } else {
                            INPUT_RPC_TIMEOUT
                        };
                        tracing::trace!(target: "neoism_backend::nvim", cols, rows, "sending nvim resize over rpc");
                        match tokio::time::timeout(
                            resize_timeout,
                            slow_neovim.ui_try_resize(cols as i64, rows as i64),
                        )
                        .await
                        {
                            Err(_) => {
                                lane_degraded = true;
                                tracing::error!(
                                    target: "neoism_backend::nvim",
                                    "nvim resize rpc timed out after {resize_timeout:?}; command lane continues"
                                );
                            }
                            Ok(Err(e)) => tracing::warn!(target: "neoism_backend::nvim", "resize failed: {e}"),
                            Ok(Ok(_)) => {
                                lane_degraded = false;
                                tracing::trace!(target: "neoism_backend::nvim", cols, rows, "nvim resize rpc completed");
                            }
                        }
                    }
                }
            }
        });

        let mut rpc_degraded = false;
        while let Some(cmd) = cmd_rx.recv().await {
            let input_timeout = if rpc_degraded {
                DEGRADED_RPC_TIMEOUT
            } else {
                INPUT_RPC_TIMEOUT
            };
            match cmd {
                NvimCommand::Input(keys) => {
                    tracing::trace!(
                        target: "neoism_backend::nvim",
                        keys = %keys.escape_debug(),
                        "sending nvim input over rpc"
                    );
                    match tokio::time::timeout(input_timeout, neovim.input(&keys)).await
                    {
                        Err(_) => {
                            rpc_degraded = true;
                            tracing::error!(
                                target: "neoism_backend::nvim",
                                keys = %keys.escape_debug(),
                                "nvim input rpc timed out after {input_timeout:?}; \
                                 pump continues (nvim may be in a blocking prompt)"
                            );
                        }
                        Ok(Err(e)) => tracing::warn!(target: "neoism_backend::nvim", "input failed: {e}"),
                        Ok(Ok(_)) => {
                            rpc_degraded = false;
                            tracing::trace!(target: "neoism_backend::nvim", "nvim input rpc completed");
                        }
                    }
                }
                NvimCommand::Mouse {
                    button,
                    action,
                    modifier,
                    grid,
                    row,
                    col,
                } => {
                    tracing::trace!(target: "neoism_backend::nvim", button = %button, action = %action, modifier = %modifier, grid, row, col, "sending nvim mouse input over rpc");
                    match tokio::time::timeout(
                        input_timeout,
                        neovim.input_mouse(&button, &action, &modifier, grid, row, col),
                    )
                    .await
                    {
                        Err(_) => {
                            rpc_degraded = true;
                            tracing::error!(
                                target: "neoism_backend::nvim",
                                "nvim mouse rpc timed out after {input_timeout:?}; pump continues"
                            );
                        }
                        Ok(Err(e)) => tracing::warn!(target: "neoism_backend::nvim", "mouse input failed: {e}"),
                        Ok(Ok(_)) => rpc_degraded = false,
                    }
                }
                NvimCommand::MouseMany {
                    button,
                    action,
                    modifier,
                    grid,
                    row,
                    col,
                    count,
                } => {
                    tracing::trace!(target: "neoism_backend::nvim", button = %button, action = %action, modifier = %modifier, grid, row, col, count, "sending nvim mouse input batch over rpc");
                    let batch = futures::future::join_all(
                        (0..count).map(|_| {
                            neovim.input_mouse(&button, &action, &modifier, grid, row, col)
                        }),
                    );
                    match tokio::time::timeout(input_timeout, batch).await {
                        Err(_) => {
                            rpc_degraded = true;
                            tracing::error!(
                                target: "neoism_backend::nvim",
                                "nvim mouse batch rpc timed out after {input_timeout:?}; pump continues"
                            );
                        }
                        Ok(results) => {
                            rpc_degraded = false;
                            for result in results {
                                if let Err(e) = result {
                                    tracing::warn!(target: "neoism_backend::nvim", "mouse input batch item failed: {e}");
                                    break;
                                }
                            }
                        }
                    }
                }
                NvimCommand::Command(cmd) => {
                    // Non-fast RPC: hand off to the command lane so a
                    // deferred command can never block the next input.
                    let _ = slow_tx.send(cmd);
                }
                NvimCommand::Shutdown => {
                    tracing::trace!(target: "neoism_backend::nvim", "nvim runtime received shutdown");
                    break;
                }
            }
        }

        // 7. Terminate immediately. Close paths already detach this
        //    runtime thread, so waiting on `qa!` only keeps nvim/LSP
        //    processes alive longer without improving UI safety.
        let _ = child.start_kill();
    });
}
