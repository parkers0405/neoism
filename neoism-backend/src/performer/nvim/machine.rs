use super::*;

/// Handle for a running editor-pane backend. Owned by `Context` in
/// place of the PTY-backed `_io_thread: Machine`. Drop sends a
/// shutdown signal and joins the runtime thread.
pub struct NvimEmbedMachine {
    config: NvimSpawnConfig,
    /// Sender into the runtime thread — clone-safe via `Mutex` so
    /// `&mut self` methods can fire commands without taking ownership.
    cmd_tx: Mutex<Option<tokio_mpsc::UnboundedSender<NvimCommand>>>,
    /// Latest requested UI size. A watch channel is deliberately used
    /// instead of the command FIFO: window managers can emit hundreds of
    /// intermediate sizes during one drag, and only the newest geometry is
    /// useful. Neovim redraws every accepted `ui_try_resize`, so queueing
    /// obsolete sizes creates a redraw storm that can leave the visible grid
    /// minutes behind the actual window.
    resize_tx: tokio_watch::Sender<(u64, u64)>,
    /// Receiver for redraw events — Phase 2c will pull from this in
    /// the mio loop and apply to `Crosswords`.
    redraw_rx: Mutex<Option<std_mpsc::Receiver<RedrawNotification>>>,
    /// Receiver for `rio_buf_modified` events — drained per-frame by
    /// the renderer to update the buffer-tab dirty dot.
    buf_mod_rx: Mutex<Option<std_mpsc::Receiver<BufModifiedNotification>>>,
    /// Receiver for `rio_buf_enter` events — drained per-frame by the
    /// renderer to keep `buffer_tabs` and `file_tree` highlight in
    /// sync with nvim's actual current buffer.
    buf_enter_rx: Mutex<Option<std_mpsc::Receiver<BufEnterNotification>>>,
    /// Receiver for `rio_cwd` events — drained per-frame to keep the
    /// Rust workspace root in sync with nvim `:cd` / `:lcd` changes.
    cwd_rx: Mutex<Option<std_mpsc::Receiver<CwdNotification>>>,
    /// Receiver for `rio_notify` toasts — drained per-frame by the
    /// renderer and pushed into the chrome notifications surface.
    notify_rx: Mutex<Option<std_mpsc::Receiver<RioNotify>>>,
    /// Receiver for `rio_winbar` cursor-context updates — drained
    /// per-frame, only the latest is kept (older ones are stale).
    winbar_rx: Mutex<Option<std_mpsc::Receiver<WinbarNotification>>>,
    /// Receiver for `msg_showcmd` pending-key updates — drained per
    /// frame like winbar; the status line shows the half-typed
    /// normal-mode command next to the mode label.
    showcmd_rx: Mutex<Option<std_mpsc::Receiver<String>>>,
    /// Receiver for editor LSP lifecycle state updates.
    lsp_status_rx: Mutex<Option<std_mpsc::Receiver<LspStatusNotification>>>,
    /// Receiver for comprehensive per-buffer LSP server snapshots.
    /// Emitted by lua on BufEnter / LspAttach / LspDetach so the
    /// status-line popup can show every server that's attached OR
    /// registered as a candidate for the current filetype with its
    /// state (active / initializing / missing / errored).
    lsp_snapshot_rx: Mutex<Option<std_mpsc::Receiver<LspSnapshotNotification>>>,
    /// Receiver for per-server `vim.notify` messages (string-matched
    /// to registered LSP server names on the lua side) so the popup
    /// can show the most recent startup / stderr error.
    lsp_message_rx: Mutex<Option<std_mpsc::Receiver<LspMessageNotification>>>,
    /// Receiver for `rio_diagnostics` snapshots — drained per-frame by
    /// the renderer to update the status line error/warn pills and
    /// (when open) the diagnostics popup.
    diagnostics_rx: Mutex<Option<std_mpsc::Receiver<DiagnosticsNotification>>>,
    /// Receiver for `rio_yank_flash` notifications — drained per-frame
    /// to spawn a fading highlight on the yanked rows.
    yank_flash_rx: Mutex<Option<std_mpsc::Receiver<YankFlashNotification>>>,
    /// Receiver for `rio_search_matches` updates — drained per-frame
    /// when the command palette is in `/` Search mode so the dropdown
    /// shows live buffer matches.
    search_matches_rx: Mutex<Option<std_mpsc::Receiver<SearchMatchesNotification>>>,
    /// Receiver for Rust minimap snapshots from managed nvim lua.
    minimap_rx: Mutex<Option<std_mpsc::Receiver<MinimapNotification>>>,
    /// Receiver for Rust-owned modal requests from managed nvim lua.
    modal_rx: Mutex<Option<std_mpsc::Receiver<ModalNotification>>>,
    /// Receiver for missing Treesitter parser notices.
    treesitter_missing_rx:
        Mutex<Option<std_mpsc::Receiver<TreesitterMissingNotification>>>,
    /// Coalesces nvim redraw wakeups the same way PTY damage does.
    /// The UI thread clears it when it starts draining redraws.
    redraw_wake_in_flight: Arc<AtomicBool>,
    /// PID of the `nvim --embed` child, which is also the leader of its
    /// own process group (see `build_nvim_command`'s `process_group(0)`).
    /// Published by the runtime thread once the child spawns; `0` until
    /// then (or on non-unix). `Drop` reads it to `kill(-pgid, SIGKILL)`
    /// the whole subtree — nvim *and* every LSP server it launched
    /// (rust-analyzer, tsserver, …) — so closing an editor doesn't orphan
    /// multi-GB language servers into swap. `start_kill()` alone only
    /// reaped nvim, leaving its LSP children reparented to init.
    child_pgid: Arc<AtomicI32>,
    /// Runtime thread handle. Dropped on shutdown so cleanup stays off
    /// the UI path.
    runtime_thread: Option<thread::JoinHandle<()>>,
}

impl NvimEmbedMachine {
    /// Construct + spawn the embedded nvim. Returns once the RPC
    /// handshake and `ui_attach` complete successfully — or with an
    /// error if the child failed to start or the handshake errored.
    ///
    /// Blocks the caller for the duration of the handshake (typically
    /// tens of milliseconds). The caller already runs on a worker
    /// thread (see `ContextManager::create_context`), so this is
    /// fine; the renderer is unaffected.
    pub fn spawn<T>(
        config: NvimSpawnConfig,
        event_proxy: T,
        window_id: WindowId,
        route_id: usize,
    ) -> Result<Self>
    where
        T: EventListener + Clone + Send + Sync + 'static,
    {
        let (cmd_tx, cmd_rx) = tokio_mpsc::unbounded_channel::<NvimCommand>();
        let initial_cols = if config.initial_cols == 0 {
            80
        } else {
            config.initial_cols
        };
        let initial_rows = if config.initial_rows == 0 {
            24
        } else {
            config.initial_rows
        };
        let (resize_tx, resize_rx) =
            tokio_watch::channel::<(u64, u64)>((initial_cols, initial_rows));
        let (redraw_tx, redraw_rx) = std_mpsc::channel::<RedrawNotification>();
        let (buf_mod_tx, buf_mod_rx) = std_mpsc::channel::<BufModifiedNotification>();
        let (buf_enter_tx, buf_enter_rx) = std_mpsc::channel::<BufEnterNotification>();
        let (cwd_tx, cwd_rx) = std_mpsc::channel::<CwdNotification>();
        let (notify_tx, notify_rx) = std_mpsc::channel::<RioNotify>();
        let (winbar_tx, winbar_rx) = std_mpsc::channel::<WinbarNotification>();
        let (showcmd_tx, showcmd_rx) = std_mpsc::channel::<String>();
        let (lsp_status_tx, lsp_status_rx) = std_mpsc::channel::<LspStatusNotification>();
        let (lsp_snapshot_tx, lsp_snapshot_rx) =
            std_mpsc::channel::<LspSnapshotNotification>();
        let (lsp_message_tx, lsp_message_rx) =
            std_mpsc::channel::<LspMessageNotification>();
        let (diagnostics_tx, diagnostics_rx) =
            std_mpsc::channel::<DiagnosticsNotification>();
        let (yank_flash_tx, yank_flash_rx) = std_mpsc::channel::<YankFlashNotification>();
        let (search_matches_tx, search_matches_rx) =
            std_mpsc::channel::<SearchMatchesNotification>();
        let (minimap_tx, minimap_rx) = std_mpsc::channel::<MinimapNotification>();
        let (modal_tx, modal_rx) = std_mpsc::channel::<ModalNotification>();
        let (treesitter_missing_tx, treesitter_missing_rx) =
            std_mpsc::channel::<TreesitterMissingNotification>();
        let (ready_tx, ready_rx) = std_mpsc::channel::<Result<()>>();
        let redraw_wake_in_flight = Arc::new(AtomicBool::new(false));

        let cfg = config.clone();
        let runtime_redraw_wake = Arc::clone(&redraw_wake_in_flight);
        let child_pgid = Arc::new(AtomicI32::new(0));
        let runtime_child_pgid = Arc::clone(&child_pgid);
        let runtime_thread = thread::Builder::new()
            .name("rio-nvim-embed".into())
            .spawn(move || {
                run_nvim_runtime(
                    cfg,
                    cmd_rx,
                    resize_rx,
                    redraw_tx,
                    buf_mod_tx,
                    buf_enter_tx,
                    cwd_tx,
                    notify_tx,
                    winbar_tx,
                    showcmd_tx,
                    lsp_status_tx,
                    lsp_snapshot_tx,
                    lsp_message_tx,
                    diagnostics_tx,
                    yank_flash_tx,
                    search_matches_tx,
                    minimap_tx,
                    modal_tx,
                    treesitter_missing_tx,
                    runtime_redraw_wake,
                    runtime_child_pgid,
                    ready_tx,
                    event_proxy,
                    window_id,
                    route_id,
                );
            })
            .context("failed to spawn nvim runtime thread")?;

        // Block until the runtime reports the handshake outcome. Any
        // error here means nvim failed to start — surface it instead
        // of leaving a dangling thread.
        let handshake = ready_rx
            .recv()
            .map_err(|_| anyhow!("nvim runtime thread exited before handshake"))?;
        handshake?;

        Ok(Self {
            config,
            cmd_tx: Mutex::new(Some(cmd_tx)),
            resize_tx,
            redraw_rx: Mutex::new(Some(redraw_rx)),
            buf_mod_rx: Mutex::new(Some(buf_mod_rx)),
            buf_enter_rx: Mutex::new(Some(buf_enter_rx)),
            cwd_rx: Mutex::new(Some(cwd_rx)),
            notify_rx: Mutex::new(Some(notify_rx)),
            winbar_rx: Mutex::new(Some(winbar_rx)),
            showcmd_rx: Mutex::new(Some(showcmd_rx)),
            lsp_status_rx: Mutex::new(Some(lsp_status_rx)),
            lsp_snapshot_rx: Mutex::new(Some(lsp_snapshot_rx)),
            lsp_message_rx: Mutex::new(Some(lsp_message_rx)),
            diagnostics_rx: Mutex::new(Some(diagnostics_rx)),
            yank_flash_rx: Mutex::new(Some(yank_flash_rx)),
            search_matches_rx: Mutex::new(Some(search_matches_rx)),
            minimap_rx: Mutex::new(Some(minimap_rx)),
            modal_rx: Mutex::new(Some(modal_rx)),
            treesitter_missing_rx: Mutex::new(Some(treesitter_missing_rx)),
            redraw_wake_in_flight,
            child_pgid,
            runtime_thread: Some(runtime_thread),
        })
    }

    /// Push raw input text to nvim via `nvim_input`. The string uses
    /// nvim key notation: literal characters plus `<C-x>` / `<Esc>` /
    /// `<CR>` / `<Up>` etc. Phase 2c translates winit key events into
    /// this notation; for now callers pass strings directly.
    pub fn input(&self, keys: impl Into<String>) {
        let keys = keys.into();
        tracing::trace!(
            target: "neoism_backend::nvim",
            keys = %keys.escape_debug(),
            "queueing nvim input"
        );
        if let Some(tx) = self.cmd_tx.lock().unwrap().as_ref() {
            if let Err(err) = tx.send(NvimCommand::Input(keys)) {
                tracing::warn!(target: "neoism_backend::nvim", "failed to queue nvim input: {err:?}");
            }
        } else {
            tracing::trace!(target: "neoism_backend::nvim", "dropped nvim input: command channel closed");
        }
    }

    /// Push a GUI mouse event to nvim. Coordinates are 0-based grid
    /// cells, matching `nvim_input_mouse(button, action, modifier, grid, row, col)`.
    pub fn mouse_input(
        &self,
        button: impl Into<String>,
        action: impl Into<String>,
        modifier: impl Into<String>,
        grid: i64,
        row: i64,
        col: i64,
    ) {
        if let Some(tx) = self.cmd_tx.lock().unwrap().as_ref() {
            if let Err(err) = tx.send(NvimCommand::Mouse {
                button: button.into(),
                action: action.into(),
                modifier: modifier.into(),
                grid,
                row,
                col,
            }) {
                tracing::warn!(target: "neoism_backend::nvim", "failed to queue nvim mouse input: {err:?}");
            }
        } else {
            tracing::trace!(target: "neoism_backend::nvim", "dropped nvim mouse input: command channel closed");
        }
    }

    /// Push several identical GUI mouse events as one renderer-thread
    /// command. Useful for wheel bursts where each row is still a
    /// distinct nvim mouse unit, but queueing/awaiting them one-by-one
    /// adds avoidable latency.
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
        if count == 0 {
            return;
        }
        if let Some(tx) = self.cmd_tx.lock().unwrap().as_ref() {
            if let Err(err) = tx.send(NvimCommand::MouseMany {
                button: button.into(),
                action: action.into(),
                modifier: modifier.into(),
                grid,
                row,
                col,
                count,
            }) {
                tracing::warn!(target: "neoism_backend::nvim", "failed to queue nvim mouse input batch: {err:?}");
            }
        } else {
            tracing::trace!(target: "neoism_backend::nvim", "dropped nvim mouse input batch: command channel closed");
        }
    }

    /// Run an Ex command in the embedded nvim. Used by chrome to swap
    /// the editor's current buffer (`:edit <path>`) without spawning a
    /// new pane.
    pub fn command(&self, cmd: impl Into<String>) {
        let cmd = cmd.into();
        if let Some(tx) = self.cmd_tx.lock().unwrap().as_ref() {
            if let Err(err) = tx.send(NvimCommand::Command(cmd)) {
                tracing::warn!(target: "neoism_backend::nvim", "failed to queue nvim command: {err:?}");
            }
        }
    }

    /// Inform nvim of a viewport resize.
    pub fn resize(&self, cols: u64, rows: u64) {
        tracing::trace!(target: "neoism_backend::nvim", cols, rows, "queueing nvim resize");
        let next = (cols.max(1), rows.max(1));
        // `resize_all_grids` and the terminal reconciliation pass can both
        // observe the same final geometry. Avoid waking the runtime for an
        // identical value, and let `watch` collapse every genuinely distinct
        // intermediate value to the newest one while an RPC is in flight.
        if *self.resize_tx.borrow() == next {
            return;
        }
        if let Err(err) = self.resize_tx.send(next) {
            tracing::trace!(
                target: "neoism_backend::nvim",
                cols = next.0,
                rows = next.1,
                "dropped nvim resize: runtime channel closed ({err})"
            );
        }
    }

    /// Take ownership of the redraw receiver. Intended to be called
    /// once by the consumer (Phase 2c — the mio loop) so it can poll
    /// for events. Subsequent calls return `None`.
    pub fn take_redraw_rx(&self) -> Option<std_mpsc::Receiver<RedrawNotification>> {
        self.redraw_rx.lock().unwrap().take()
    }

    /// Drain any pending `rio_buf_modified` notifications. Non-blocking
    /// — returns whatever's queued at the moment of the call. Renderer
    /// calls this once per frame to keep dirty dots in sync.
    pub fn drain_buf_modified(&self) -> Vec<BufModifiedNotification> {
        let mut out = Vec::new();
        if let Some(rx) = self.buf_mod_rx.lock().unwrap().as_ref() {
            while let Ok(n) = rx.try_recv() {
                out.push(n);
            }
        }
        out
    }

    /// Drain any pending `rio_buf_enter` notifications. Mirrors
    /// `drain_buf_modified` — non-blocking, called once per frame from
    /// the renderer to update buffer-tab activation + tree highlight.
    pub fn drain_buf_enter(&self) -> Vec<BufEnterNotification> {
        let mut out = Vec::new();
        if let Some(rx) = self.buf_enter_rx.lock().unwrap().as_ref() {
            while let Ok(n) = rx.try_recv() {
                out.push(n);
            }
        }
        out
    }

    /// Drain cwd changes emitted by embedded nvim. Non-blocking;
    /// renderer calls this once per frame to keep workspace chrome and
    /// subsequent editor spawns rooted in the same directory as nvim.
    pub fn drain_cwd(&self) -> Vec<CwdNotification> {
        let mut out = Vec::new();
        if let Some(rx) = self.cwd_rx.lock().unwrap().as_ref() {
            while let Ok(n) = rx.try_recv() {
                out.push(n);
            }
        }
        out
    }

    /// Drain any pending `rio_notify` toasts. Mirrors
    /// `drain_buf_modified` — non-blocking, called once per frame from
    /// the renderer.
    pub fn drain_notifications(&self) -> Vec<RioNotify> {
        let mut out = Vec::new();
        let mut dropped = 0usize;
        if let Some(rx) = self.notify_rx.lock().unwrap().as_ref() {
            while let Ok(n) = rx.try_recv() {
                if out.len() < MAX_NVIM_NOTIFICATIONS_PER_DRAIN {
                    out.push(n);
                } else {
                    dropped = dropped.saturating_add(1);
                }
            }
        }
        if dropped > 0 {
            out.push(RioNotify {
                message: format!("Suppressed {dropped} additional nvim messages"),
                level: NotifyLevel::Warn,
            });
        }
        out
    }

    /// Drain any pending `rio_winbar` updates. Returns only the latest
    /// — older ones are stale by definition since the cursor's already
    /// moved past them, and the breadcrumbs strip only renders one
    /// trailing context segment.
    pub fn drain_winbar(&self) -> Option<WinbarNotification> {
        let mut latest = None;
        if let Some(rx) = self.winbar_rx.lock().unwrap().as_ref() {
            while let Ok(n) = rx.try_recv() {
                latest = Some(n);
            }
        }
        latest
    }

    /// Drain any pending `msg_showcmd` updates. Latest wins; an empty
    /// string means nvim cleared the pending command (count consumed
    /// or cancelled).
    pub fn drain_showcmd(&self) -> Option<String> {
        let mut latest = None;
        if let Some(rx) = self.showcmd_rx.lock().unwrap().as_ref() {
            while let Ok(n) = rx.try_recv() {
                latest = Some(n);
            }
        }
        latest
    }

    pub fn drain_lsp_status(&self) -> Option<LspStatusNotification> {
        let mut latest = None;
        if let Some(rx) = self.lsp_status_rx.lock().unwrap().as_ref() {
            while let Ok(n) = rx.try_recv() {
                latest = Some(n);
            }
        }
        latest
    }

    /// Drain ALL queued LSP-status notifications since the last poll.
    /// Multi-client buffers (e.g. ruff + pyright on Python) emit one
    /// status event per attached client; the single-return drain above
    /// collapses them to one. This variant preserves the full sequence
    /// so the per-buffer attached-LSP list can be rebuilt accurately.
    pub fn drain_all_lsp_statuses(&self) -> Vec<LspStatusNotification> {
        let mut out = Vec::new();
        if let Some(rx) = self.lsp_status_rx.lock().unwrap().as_ref() {
            while let Ok(n) = rx.try_recv() {
                out.push(n);
            }
        }
        out
    }

    /// Drain the latest per-buffer LSP snapshot (if any). Only the most
    /// recent is kept — older snapshots reflect a stale buffer.
    pub fn drain_lsp_snapshot(&self) -> Option<LspSnapshotNotification> {
        let mut latest = None;
        if let Some(rx) = self.lsp_snapshot_rx.lock().unwrap().as_ref() {
            while let Ok(n) = rx.try_recv() {
                latest = Some(n);
            }
        }
        latest
    }

    /// Drain all queued per-server LSP messages since the last poll.
    pub fn drain_lsp_messages(&self) -> Vec<LspMessageNotification> {
        let mut out = Vec::new();
        if let Some(rx) = self.lsp_message_rx.lock().unwrap().as_ref() {
            while let Ok(n) = rx.try_recv() {
                out.push(n);
            }
        }
        out
    }

    /// Drain pending `rio_diagnostics` snapshots, returning only the
    /// most recent — older entries are stale by definition since they
    /// reflect a previous state of the buffer's diagnostic list.
    pub fn drain_diagnostics(&self) -> Option<DiagnosticsNotification> {
        let mut latest = None;
        if let Some(rx) = self.diagnostics_rx.lock().unwrap().as_ref() {
            while let Ok(n) = rx.try_recv() {
                latest = Some(n);
            }
        }
        latest
    }

    /// Drain pending yank-flash notifications. All of them — unlike
    /// the others, every flash matters because each represents a
    /// distinct yank action by the user, and dropping older ones
    /// would silently swallow rapid `yy` sequences.
    pub fn drain_yank_flashes(&self) -> Vec<YankFlashNotification> {
        let mut out = Vec::new();
        if let Some(rx) = self.yank_flash_rx.lock().unwrap().as_ref() {
            while let Ok(n) = rx.try_recv() {
                out.push(n);
            }
        }
        out
    }

    /// Drain `/`-search match snapshots, returning only the latest —
    /// older ones are stale by definition since they reflect a
    /// previous query.
    pub fn drain_search_matches(&self) -> Option<SearchMatchesNotification> {
        let mut latest = None;
        if let Some(rx) = self.search_matches_rx.lock().unwrap().as_ref() {
            while let Ok(n) = rx.try_recv() {
                latest = Some(n);
            }
        }
        latest
    }

    /// Drain minimap snapshots, returning only the newest payload. Full
    /// snapshots can contain thousands of short lines, so keeping stale
    /// entries would waste UI-thread work without improving freshness.
    pub fn drain_minimap(&self) -> Option<MinimapNotification> {
        let mut latest = None;
        if let Some(rx) = self.minimap_rx.lock().unwrap().as_ref() {
            while let Ok(n) = rx.try_recv() {
                latest = Some(n);
            }
        }
        latest
    }

    pub fn drain_modals(&self) -> Vec<ModalNotification> {
        let mut out = Vec::new();
        let mut dropped = 0usize;
        if let Some(rx) = self.modal_rx.lock().unwrap().as_ref() {
            while let Ok(n) = rx.try_recv() {
                if out.len() < MAX_NVIM_MODALS_PER_DRAIN {
                    out.push(n);
                } else {
                    dropped = dropped.saturating_add(1);
                }
            }
        }
        if dropped > 0 {
            out.push(ModalNotification {
                title: "Nvim Messages".to_string(),
                body: format!("Suppressed {dropped} additional nvim modal messages"),
                level: NotifyLevel::Warn,
                actions: Vec::new(),
            });
        }
        out
    }

    pub fn drain_treesitter_missing(&self) -> Vec<TreesitterMissingNotification> {
        let mut out = Vec::new();
        if let Some(rx) = self.treesitter_missing_rx.lock().unwrap().as_ref() {
            while let Ok(n) = rx.try_recv() {
                out.push(n);
            }
        }
        out
    }

    pub fn clear_redraw_wake(&self) {
        self.redraw_wake_in_flight.store(false, Ordering::Release);
    }

    /// Borrow the spawn config (useful for diagnostics / restart).
    pub fn config(&self) -> &NvimSpawnConfig {
        &self.config
    }
}

impl Drop for NvimEmbedMachine {
    fn drop(&mut self) {
        // Send shutdown then drop the sender so the runtime's command
        // loop sees both the explicit signal and the channel close.
        if let Some(tx) = self.cmd_tx.lock().unwrap().take() {
            let _ = tx.send(NvimCommand::Shutdown);
        }

        // Reap the ENTIRE nvim process group right here, synchronously.
        // `kill()` only *delivers* SIGKILL and returns immediately — it
        // does not wait for the processes to die — so this stays off the
        // critical path for Cmd+W (the very reason the runtime thread was
        // detached below). Targeting the negative pgid hits nvim AND all
        // of its language servers in one syscall; the old `start_kill()`
        // path signalled only nvim, orphaning rust-analyzer/tsserver to
        // init where they lingered (multiple GB each) and forced the
        // machine into swap. nvim's atomic swap/undo writes already cover
        // data safety under a hard kill.
        #[cfg(unix)]
        {
            let pgid = self.child_pgid.load(Ordering::SeqCst);
            if pgid > 0 {
                // SAFETY: plain libc `kill`; `-pgid` addresses the whole
                // process group led by the nvim child.
                unsafe {
                    libc::kill(-pgid, libc::SIGKILL);
                }
            }
        }

        // Detach instead of join: the runtime thread still wakes on the
        // Shutdown/channel close above and tears down its tokio runtime
        // off the UI path. nvim's own atomic-write guarantees cover
        // swap/undo files.
        let _ = self.runtime_thread.take();
    }
}
