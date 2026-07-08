use super::*;

/// Handle to a running nvim subprocess.
///
/// The redraw receiver is intentionally hoisted out (returned alongside
/// the session by `spawn`) so the websocket task can poll it from a
/// `tokio::select!` arm without partial-borrow gymnastics on this
/// struct. Dropping the `NvimSession` aborts the io future and kills
/// the child (`kill_on_drop` on the Command).
pub struct NvimSession {
    /// nvim-rs client handle. `None` once `close()` has been called.
    pub(crate) nvim: Arc<Mutex<Option<Neovim<NeovimWriter>>>>,
    /// Background task driving nvim-rs io. Aborted on drop.
    io_handle: tokio::task::JoinHandle<()>,
    /// Last width/height we sent over ui_attach / ui_try_resize, so we
    /// can no-op redundant resizes.
    last_size: (u64, u64),
    /// Last workspace root we `:cd`-ed nvim into, so the per-envelope
    /// `set_workspace_root` is a no-op unless the root actually moved.
    /// Without this, EVERY editor message (each keystroke!) ran a
    /// non-fast `:cd` command — which nvim defers whenever a count is
    /// pending, stalling the websocket loop 4s per key and dropping
    /// the very input that would clear the count (the digit freeze).
    last_workspace_root: Option<PathBuf>,
    /// Current web editor surface / pane route id. Single-nvim mode
    /// stamps redraws with the latest addressed surface until true
    /// per-surface grids land.
    active_surface_id: Arc<Mutex<Option<String>>>,
    /// `nvim --headless` still emits UI-grid frames over msgpack after
    /// `ui_attach`. Keep attach/default-screen frames out of the client
    /// until the real buffer is open and we force a repaint.
    redraw_enabled: Arc<Mutex<bool>>,
    /// Tracks the file path each surface last opened. Used to restore nvim
    /// to the correct buffer when focus switches between surfaces sharing
    /// this session. Keyed by surface_id.
    surface_files: HashMap<String, PathBuf>,
    /// Which surface nvim's active buffer currently belongs to.
    current_nvim_surface: Option<String>,
    /// PID / process-group leader of the `nvim --embed` child (`0` if
    /// unknown). nvim is spawned in its own group (`process_group(0)`),
    /// so `kill(-pgid, SIGKILL)` on drop reaps nvim AND every language
    /// server it launched. `kill_on_drop` alone only reaped nvim and
    /// orphaned rust-analyzer/tsserver to init.
    child_pgid: i32,
}

impl Drop for NvimSession {
    fn drop(&mut self) {
        self.io_handle.abort();
        // Reap the whole nvim process group (nvim + its LSP servers).
        // `io_handle` owns the `Child`, whose `kill_on_drop` will SIGKILL
        // nvim itself; this additionally takes down the language servers
        // that would otherwise linger in swap. `kill()` returns
        // immediately — it only delivers the signal.
        #[cfg(unix)]
        if self.child_pgid > 0 {
            // SAFETY: plain libc `kill`; `-pgid` targets the process
            // group led by the nvim child.
            unsafe {
                libc::kill(-self.child_pgid, libc::SIGKILL);
            }
        }
    }
}

/// Error type for spawn / send_keys / open_buffer.
#[derive(Debug, thiserror::Error)]
pub enum NvimError {
    #[error("nvim binary not found on $PATH (spawn unsupported on this host)")]
    NotImplemented,
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("rpc error: {0}")]
    Rpc(String),
    #[error("session already closed")]
    Closed,
}

/// Process-wide registry of daemon-owned nvim sessions.
///
/// The legacy websocket path kept one `NvimSession` per connection.
/// This registry keys sessions by editor surface id so multiple
/// clients connected to the same pane can subscribe to the same nvim
/// redraw stream and send input to the same subprocess.
#[derive(Clone, Default)]
pub struct NvimSessionRegistry {
    inner: Arc<Mutex<HashMap<String, Arc<ManagedNvimSession>>>>,
}

#[derive(Clone)]
pub struct NvimSessionHandle {
    key: String,
    inner: Arc<ManagedNvimSession>,
}

pub(crate) struct ManagedNvimSession {
    session: Mutex<NvimSession>,
    redraw_tx: broadcast::Sender<EditorServerMessage>,
    cursor_overlay_tx: broadcast::Sender<CursorOverlayServerMessage>,
    fanout_tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for ManagedNvimSession {
    fn drop(&mut self) {
        for task in &self.fanout_tasks {
            task.abort();
        }
    }
}

impl NvimSessionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn key_for_message(message: &EditorClientMessage) -> String {
        message
            .surface_id()
            .filter(|id| !id.is_empty())
            .unwrap_or(DEFAULT_SESSION_KEY)
            .to_string()
    }

    pub async fn get_or_spawn(
        &self,
        key: String,
        crdt: &CrdtSyncHub,
    ) -> Result<NvimSessionHandle, NvimError> {
        if let Some(existing) = self.inner.lock().await.get(&key).cloned() {
            return Ok(NvimSessionHandle {
                key,
                inner: existing,
            });
        }

        let (session, mut redraw_rx, mut cursor_overlay_rx, mut buffer_lines_rx) =
            NvimSession::spawn(&[]).await?;
        // The CRDT→nvim applier only needs the rpc client, not the whole
        // session — cloning the inner handle keeps the bridge task free
        // of an `Arc<ManagedNvimSession>` cycle (Drop must still abort
        // these tasks).
        let nvim_for_apply = session.nvim.clone();
        let (redraw_tx, _) = broadcast::channel(1024);
        let (cursor_overlay_tx, _) = broadcast::channel(1024);
        let redraw_forward_tx = redraw_tx.clone();
        let cursor_forward_tx = cursor_overlay_tx.clone();
        let mut fanout_tasks = vec![
            tokio::spawn(async move {
                while let Some(message) = redraw_rx.recv().await {
                    let _ = redraw_forward_tx.send(message);
                }
            }),
            tokio::spawn(async move {
                while let Some(message) = cursor_overlay_rx.recv().await {
                    let _ = cursor_forward_tx.send(message);
                }
            }),
        ];

        // Wave 6C bidirectional CRDT cutover, direction nvim→CRDT:
        // fold incremental `on_lines` changes into the authoritative
        // replica. Changes triggered by our own CRDT→nvim applies never
        // reach this channel — the lua `on_lines` callback drops them
        // while `vim.g.neoism_crdt_applying` is set (echo guard #1).
        // Per-SESSION echo-guard identity. Every screen runs its own
        // embedded nvim against the same shared docs; stamping all of
        // them with the one daemon id made each session's applier
        // (which skips its own origin) swallow every other session's
        // edits too — nvim↔nvim across two screens was silent. A
        // unique random origin per session keeps the self-echo guard
        // while letting peers' edits through.
        let session_origin = generate_session_origin(crdt.daemon_client_id());
        let nvim_to_crdt_hub = crdt.clone();
        fanout_tasks.push(tokio::spawn(async move {
            while let Some(event) = buffer_lines_rx.recv().await {
                let change = match event {
                    NvimBufferEvent::Lines(change) => change,
                    NvimBufferEvent::WriteRequested { path } => {
                        // `:w` intercepted by BufWriteCmd — daemon-owned
                        // save. The preceding on_lines notifies arrived
                        // on this same channel, so the hub text already
                        // includes the keystrokes being written.
                        let buffer_id =
                            crate::crdt::crdt_buffer_id_for_path(&path);
                        if !nvim_to_crdt_hub.buffers().has_buffer(&buffer_id) {
                            tracing::warn!(
                                path = %path.display(),
                                "[crdt-trace] write intercepted for untracked                                  buffer; nothing flushed"
                            );
                            continue;
                        }
                        if let neoism_protocol::crdt::CrdtServerMessage::Error {
                            message,
                            ..
                        } = nvim_to_crdt_hub.save_buffer(&buffer_id)
                        {
                            tracing::warn!(
                                buffer_id = %buffer_id,
                                error = %message,
                                "[crdt-trace] daemon-owned save failed"
                            );
                        }
                        continue;
                    }
                };
                let buffer_id = crate::crdt::crdt_buffer_id_for_path(&change.path);
                if !nvim_to_crdt_hub.buffers().has_buffer(&buffer_id) {
                    // No tracked doc under this EXACT id — the edit is
                    // silently lost. Loud, because a path-form mismatch
                    // between the seed and nvim_buf_get_name lands here.
                    tracing::warn!(
                        target: "neoism::crdt_fold",
                        buffer_id = %buffer_id,
                        "[crdt-fold] on_lines change DROPPED: no tracked doc under this id"
                    );
                    continue;
                }
                match nvim_to_crdt_hub.apply_nvim_lines_change(
                    &buffer_id,
                    change.firstline as usize,
                    change.lastline as usize,
                    change.new_line_count as usize,
                    &change.new_text,
                    session_origin,
                ) {
                    Some(neoism_protocol::crdt::CrdtServerMessage::Error {
                        message,
                        ..
                    }) => {
                        tracing::warn!(
                            buffer_id = %buffer_id,
                            error = %message,
                            "[crdt-trace] nvim on_lines change rejected by CRDT hub"
                        );
                    }
                    Some(_) => {
                        tracing::info!(
                            target: "neoism::crdt_fold",
                            buffer_id = %buffer_id,
                            session_origin,
                            "[crdt-fold] nvim edit folded into doc + broadcast"
                        );
                    }
                    None => {
                        tracing::info!(
                            target: "neoism::crdt_fold",
                            buffer_id = %buffer_id,
                            "[crdt-fold] on_lines was a no-op (text already matched)"
                        );
                    }
                }
            }
        }));

        // Wave 6C, direction CRDT→nvim: replay remote (non-daemon-origin)
        // Sync updates into the live nvim buffer. Daemon-origin updates
        // are the ones we just produced FROM nvim above; skipping them is
        // echo guard #2.
        let crdt_to_nvim_hub = crdt.clone();
        fanout_tasks.push(tokio::spawn(async move {
            let mut rx = crdt_to_nvim_hub.subscribe();
            'outer: loop {
                // Wait for the first relevant Sync...
                let first_buffer = loop {
                    match rx.recv().await {
                        Ok(neoism_protocol::crdt::CrdtServerMessage::Sync {
                            envelope,
                        }) => {
                            if remote_sync_targets_nvim(&envelope, session_origin)
                                .is_some()
                            {
                                break envelope.buffer_id;
                            }
                        }
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Lagged(skipped)) => {
                            tracing::warn!(
                                skipped,
                                "[crdt-trace] CRDT→nvim applier lagged behind hub broadcast"
                            );
                        }
                        Err(broadcast::error::RecvError::Closed) => break 'outer,
                    }
                };

                // ...then COALESCE the burst: letter-by-letter co-editing
                // broadcasts one Sync per keystroke, and replaying each
                // one ships the FULL buffer text into an exec_lua diff —
                // megabytes per second on a large file, multiplied by
                // every live session. That overload wedged the embedded
                // daemon (and with it the whole desktop UI: stale grids,
                // dead scrolling). A short quiet-window turns a burst
                // into ONE apply of the latest hub text per buffer.
                let mut pending: std::collections::BTreeSet<String> =
                    std::collections::BTreeSet::new();
                pending.insert(first_buffer);
                loop {
                    match tokio::time::timeout(
                        std::time::Duration::from_millis(40),
                        rx.recv(),
                    )
                    .await
                    {
                        Ok(Ok(neoism_protocol::crdt::CrdtServerMessage::Sync {
                            envelope,
                        })) => {
                            if remote_sync_targets_nvim(&envelope, session_origin)
                                .is_some()
                            {
                                pending.insert(envelope.buffer_id);
                            }
                        }
                        Ok(Ok(_)) => {}
                        Ok(Err(broadcast::error::RecvError::Lagged(skipped))) => {
                            tracing::warn!(
                                skipped,
                                "[crdt-trace] CRDT→nvim applier lagged behind hub broadcast"
                            );
                        }
                        Ok(Err(broadcast::error::RecvError::Closed)) => break 'outer,
                        Err(_) => break, // quiet window elapsed
                    }
                }

                for buffer_id in pending {
                    let Some(path) = crate::crdt::crdt_path_for_buffer_id(&buffer_id)
                    else {
                        continue;
                    };
                    let Ok(text) = crdt_to_nvim_hub.buffers().text(&buffer_id) else {
                        continue;
                    };
                    match apply_authoritative_text_to_nvim(
                        &nvim_for_apply,
                        path,
                        &text,
                    )
                    .await
                    {
                        Ok(applied) => {
                            tracing::info!(
                                target: "neoism::crdt_fold",
                                buffer_id = %buffer_id,
                                session_origin,
                                applied,
                                "[crdt-fold] coalesced remote update replayed into this nvim session"
                            );
                        }
                        Err(NvimError::Closed) => break 'outer,
                        Err(err) => {
                            tracing::warn!(
                                buffer_id = %buffer_id,
                                error = %err,
                                "[crdt-trace] failed to replay remote CRDT update into nvim"
                            );
                        }
                    }
                }
            }
        }));
        let managed = Arc::new(ManagedNvimSession {
            session: Mutex::new(session),
            redraw_tx,
            cursor_overlay_tx,
            fanout_tasks,
        });

        let mut guard = self.inner.lock().await;
        let entry = guard.entry(key.clone()).or_insert_with(|| managed.clone());
        Ok(NvimSessionHandle {
            key,
            inner: entry.clone(),
        })
    }

    pub async fn remove(&self, key: &str) {
        self.inner.lock().await.remove(key);
    }

    /// Remove every session whose key starts with `prefix` — the
    /// connection-namespace reaper. Sessions are keyed
    /// `{socket_namespace}:{surface}`; when a websocket dies abruptly
    /// its sessions used to linger forever (nvim child + CRDT applier
    /// tasks each), so every reconnect cycle multiplied the per-edit
    /// replay fan-out. Returns how many were reaped.
    pub async fn remove_prefix(&self, prefix: &str) -> usize {
        let mut guard = self.inner.lock().await;
        let keys: Vec<String> = guard
            .keys()
            .filter(|key| key.starts_with(prefix))
            .cloned()
            .collect();
        for key in &keys {
            guard.remove(key);
        }
        keys.len()
    }

    #[cfg(test)]
    pub(crate) async fn len(&self) -> usize {
        self.inner.lock().await.len()
    }
}

impl NvimSessionHandle {
    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn subscribe_redraw(&self) -> broadcast::Receiver<EditorServerMessage> {
        self.inner.redraw_tx.subscribe()
    }

    pub fn subscribe_cursor_overlay(
        &self,
    ) -> broadcast::Receiver<CursorOverlayServerMessage> {
        self.inner.cursor_overlay_tx.subscribe()
    }

    pub async fn handle(&self, msg: EditorClientMessage) -> Result<(), NvimError> {
        // A non-fast COMMAND must never pin the session Mutex across its
        // (possibly deferred) rpc await. nvim defers every non-fast RPC
        // while normal mode has a pending count/operator (a bare digit
        // left open); holding the session lock for the whole rpc-timeout
        // window would block the concurrent `SendKeys` that clears the
        // count — the digit-key freeze. Do the surface bookkeeping under a
        // brief lock, clone the rpc handle out, then issue the command
        // lock-free (mirrors the diagnostics poll's clone-out gate).
        if let EditorClientMessage::Command {
            ref command,
            ref surface_id,
            ..
        } = msg
        {
            let command = command.clone();
            let surface_id = surface_id.clone();
            let nvim = {
                let mut guard = self.inner.session.lock().await;
                if let Some(sid) = surface_id.as_deref() {
                    guard.set_active_surface_id(Some(sid.to_string())).await;
                }
                guard.ensure_nvim_on_surface(surface_id.as_deref()).await?;
                guard.nvim.clone()
            };
            // Fast pending-count gate (nvim_get_mode is FUNC_API_FAST, so
            // it answers even while blocked): while normal mode has a count/
            // operator open, nvim DEFERS every non-fast command until input
            // clears it, so issuing one now would hang this await for the
            // full rpc-timeout window and stall the serial editor-message
            // loop — blocking the very SendKeys that clears the count (the
            // digit-key freeze). Skip instead; the client re-sends the
            // best-effort commands (`/`-search preview/query) on the next
            // reply/keystroke. Mirrors the diagnostics poll's block gate.
            if let Some(client) = nvim.lock().await.clone() {
                if nvim_is_blocked(&client).await {
                    tracing::trace!(
                        command = %command,
                        "skipping client command while nvim is blocked (pending count)"
                    );
                    return Ok(());
                }
            }
            return command_rpc(&nvim, &command).await;
        }
        self.inner.session.lock().await.handle(msg).await
    }

    pub async fn set_workspace_root(&self, root: &Path) -> Result<(), NvimError> {
        self.inner
            .session
            .lock()
            .await
            .set_workspace_root(root)
            .await
    }

    pub async fn snapshot_diagnostics(&self) -> Option<Vec<ProtoDiagnosticItem>> {
        // Hold the outer session lock only long enough to clone the rpc
        // handle: `handle()` (SendKeys!) waits on this same Mutex, so
        // holding it across a deferred exec_lua froze the user's keys.
        let nvim = self.inner.session.lock().await.nvim.clone();
        snapshot_diagnostics_rpc(&nvim).await
    }

    pub async fn snapshot_lsp_states(&self) -> Option<Vec<(String, LspState)>> {
        let nvim = self.inner.session.lock().await.nvim.clone();
        snapshot_lsp_states_rpc(&nvim).await
    }

    /// Read the current buffer's full text plus its absolute path.
    ///
    /// Used by the CRDT cutover (Wave 5 item 5A) to seed the
    /// daemon-authoritative replica from nvim's view of the buffer after
    /// an `OpenBuffer`. Returns `None` when the active buffer is unnamed
    /// (scratch / no file backing) so we never seed a CRDT replica with a
    /// bogus key. This does NOT touch the redraw path.
    pub async fn read_active_buffer(&self) -> Result<Option<BufferText>, NvimError> {
        self.inner.session.lock().await.read_active_buffer().await
    }

    /// Attach the Wave 6C `on_lines` → CRDT bridge to the CURRENT nvim
    /// buffer (idempotent; safe to call on every `OpenBuffer`). Returns
    /// `true` when a new attach happened, `false` when the buffer was
    /// already streaming changes.
    pub async fn attach_buffer_change_events(&self) -> Result<bool, NvimError> {
        self.inner
            .session
            .lock()
            .await
            .attach_buffer_change_events()
            .await
    }

    /// Reconcile a loaded nvim buffer to the daemon-authoritative CRDT
    /// text (CRDT→nvim direction). The replace runs under the
    /// `neoism_crdt_applying` suppression flag so the resulting
    /// `on_lines` events never re-enter the CRDT (echo-loop guard).
    /// Returns `Ok(false)` when no loaded buffer matches `path`.
    pub async fn apply_authoritative_text(
        &self,
        path: &Path,
        text: &str,
    ) -> Result<bool, NvimError> {
        let nvim = self.inner.session.lock().await.nvim.clone();
        apply_authoritative_text_to_nvim(&nvim, &path.to_string_lossy(), text).await
    }
}

/// Snapshot of an nvim buffer's text + identity, used to seed the
/// daemon-authoritative CRDT replica.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferText {
    /// Absolute path of the file backing the buffer.
    pub path: PathBuf,
    /// Joined buffer lines (newline-separated, no trailing newline added).
    pub text: String,
    /// Cursor line in LSP coordinates (0-based).
    pub cursor_line: u32,
    /// Cursor character in LSP coordinates (0-based; nvim byte column today).
    pub cursor_col: u32,
}

/// One incremental nvim buffer change, as reported by the lua
/// `nvim_buf_attach` `on_lines` callback: lines `[firstline, lastline)`
/// were replaced by `new_line_count` lines whose joined text is
/// `new_text`. Changes made while the daemon itself is applying a
/// remote CRDT update are suppressed at the lua layer and never appear
/// on this channel.
/// Out-of-band buffer events fired by the injected lua bridges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NvimBufferEvent {
    /// `on_lines`: an nvim-side edit to fold into the CRDT hub.
    Lines(NvimBufferLinesChange),
    /// `BufWriteCmd`: nvim's `:w` was intercepted — the daemon (single
    /// writer) must flush the authoritative doc text to disk instead of
    /// letting nvim write its own buffer over a peer's fresh edits.
    WriteRequested { path: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NvimBufferLinesChange {
    /// Absolute path of the file backing the changed buffer.
    pub path: PathBuf,
    /// First replaced line (0-based, inclusive).
    pub firstline: u64,
    /// One past the last replaced line of the OLD text (exclusive).
    pub lastline: u64,
    /// Number of lines in the NEW region (0 == pure deletion).
    pub new_line_count: u64,
    /// The new region's lines joined with `\n` (empty for deletions).
    pub new_text: String,
}

/// Random non-zero per-session origin id for the nvim echo guard:
/// below 2^53 (Yjs-safe space, same constraint every client id in this
/// codebase honors) and never colliding with the shared daemon id.
pub(crate) fn generate_session_origin(daemon_client_id: u64) -> u64 {
    loop {
        let id = (uuid::Uuid::new_v4().as_u128() as u64) & ((1 << 53) - 1);
        if id != 0 && id != daemon_client_id {
            return id;
        }
    }
}

/// Decide whether a hub Sync envelope must be replayed into THIS nvim
/// session. Returns the target file path, or `None` for updates this
/// session itself produced (`own_origin_id` — replaying them would be
/// the echo loop this guard exists to prevent) and for non-`file://`
/// buffer ids. Other nvim sessions' edits carry THEIR origin ids and
/// pass through — that's how two screens' nvims converge.
pub fn remote_sync_targets_nvim(
    envelope: &CrdtSyncEnvelope,
    own_origin_id: u64,
) -> Option<&str> {
    if envelope.origin_client_id == own_origin_id {
        return None;
    }
    crate::crdt::crdt_path_for_buffer_id(&envelope.buffer_id)
}

/// Apply the authoritative CRDT text to the nvim buffer backing `path`
/// in ONE atomic `exec_lua` round trip: the lua finds the buffer, diffs
/// the line arrays (common prefix/suffix), and `nvim_buf_set_lines`s
/// only the changed middle slice while `vim.g.neoism_crdt_applying`
/// suppresses the `on_lines` → CRDT bridge. Returns `Ok(false)` when no
/// loaded buffer matches `path` (nothing to do).
pub(crate) async fn apply_authoritative_text_to_nvim(
    nvim_slot: &Arc<Mutex<Option<Neovim<NeovimWriter>>>>,
    path: &str,
    text: &str,
) -> Result<bool, NvimError> {
    // Clone the rpc handle out instead of holding the `nvim` Mutex across
    // the exec_lua await: this applier runs as a detached task and its
    // non-fast exec_lua is deferred whenever normal mode has a pending
    // count/operator. Holding the Mutex across that deferral would block
    // `send_keys` on the same slot — freezing input on every pane sharing
    // this session (the digit-key freeze).
    let nvim = nvim_slot.lock().await.clone().ok_or(NvimError::Closed)?;
    let lua = r#"
        local path, text = ...
        local target = nil
        for _, b in ipairs(vim.api.nvim_list_bufs()) do
            if vim.api.nvim_buf_is_loaded(b) and vim.api.nvim_buf_get_name(b) == path then
                target = b
                break
            end
        end
        if target == nil then return false end
        local new_lines = vim.split(text, "\n", { plain = true })
        local old_lines = vim.api.nvim_buf_get_lines(target, 0, -1, false)
        local n_old, n_new = #old_lines, #new_lines
        local prefix = 0
        while prefix < n_old and prefix < n_new
            and old_lines[prefix + 1] == new_lines[prefix + 1] do
            prefix = prefix + 1
        end
        local suffix = 0
        while suffix < (n_old - prefix) and suffix < (n_new - prefix)
            and old_lines[n_old - suffix] == new_lines[n_new - suffix] do
            suffix = suffix + 1
        end
        if prefix == n_old and n_old == n_new then return true end
        local replacement = {}
        for i = prefix + 1, n_new - suffix do
            table.insert(replacement, new_lines[i])
        end
        vim.g.neoism_crdt_applying = true
        local ok, err = pcall(
            vim.api.nvim_buf_set_lines, target, prefix, n_old - suffix, false, replacement
        )
        vim.g.neoism_crdt_applying = false
        if not ok then error(err) end
        return true
    "#;
    let value = nvim_rpc_timeout(
        "apply authoritative text",
        nvim.exec_lua(lua, vec![Value::from(path), Value::from(text)]),
    )
    .await?;
    Ok(value.as_bool().unwrap_or(false))
}

impl NvimSession {
    /// Spawn `nvim --embed` and complete the ui_attach handshake.
    ///
    /// `args` is passed verbatim to the child (in addition to
    /// `--embed`). Pass `&[]` for a vanilla session; pass
    /// `&[OsString::from("path/to/file")]` to open a buffer at start.
    ///
    /// Returns `NvimError::NotImplemented` if no `nvim` binary is
    /// available on `$PATH`. Callers (the websocket task) translate
    /// this into an `EditorServerMessage::Error` so the client can
    /// surface a "nvim not installed" toast without the daemon
    /// crashing.
    ///
    /// On success, returns `(session, redraw_rx)`. The receiver is
    /// hoisted out of the session struct so the caller can poll it
    /// from a `tokio::select!` arm without holding a `&mut`
    /// borrow of the session across awaits.
    /// Spawn the embedded nvim and return the session alongside the
    /// editor redraw + cursor overlay receivers. Both channels are
    /// hoisted out so the websocket task can poll them from a
    /// `tokio::select!` arm without holding a `&mut` borrow on the
    /// session across awaits.
    pub async fn spawn(
        args: &[OsString],
    ) -> Result<
        (
            Self,
            mpsc::UnboundedReceiver<EditorServerMessage>,
            mpsc::UnboundedReceiver<CursorOverlayServerMessage>,
            mpsc::UnboundedReceiver<NvimBufferEvent>,
        ),
        NvimError,
    > {
        // Probe `$PATH` for `nvim`. We could let `Command::spawn`
        // fail with `ErrorKind::NotFound` and map that to
        // `NotImplemented`, but a direct `which`-style probe is
        // cheaper than spinning up a child only to discover it.
        if which_nvim().is_none() {
            tracing::error!(
                "[nvim-trace] spawn aborted: no `nvim` binary on $PATH; \
                 install neovim on the daemon host or set PATH"
            );
            return Err(NvimError::NotImplemented);
        }
        tracing::info!(?args, "[nvim-trace] spawning embedded nvim subprocess");

        let mut cmd = TokioCommand::new("nvim");
        // Anchor the embedded nvim's cwd to the workspace root so
        // `:edit <workspace-relative-path>` resolves to the same file
        // the `files` service handler reads. Without this, the child
        // inherits the daemon's cwd which may differ from
        // `NEOISM_WORKSPACE_ROOT` (e.g. systemd-managed daemons).
        let workspace_root = crate::files::workspace_root();
        tracing::info!(
            cwd = %workspace_root.display(),
            "[nvim-trace] anchoring embedded nvim cwd to workspace root"
        );
        cmd.current_dir(&workspace_root)
            .arg("--embed")
            .arg("--headless")
            // `--clean` skips user config; the daemon doesn't want
            // the user's local init.lua to interfere with the
            // headless server.
            .arg("--clean")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Own process group so a single `kill(-pgid)` on drop reaps nvim
        // plus the LSP servers it spawns (rust-analyzer, tsserver, …)
        // instead of orphaning multi-GB indexers to init.
        #[cfg(unix)]
        cmd.process_group(0);
        let mut child = cmd.spawn().map_err(|e| {
            tracing::error!(
                error = %e,
                "[nvim-trace] failed to spawn `nvim --embed` child process"
            );
            NvimError::Io(e)
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "child stdin missing"))?;
        let stdout = child.stdout.take().ok_or_else(|| {
            io::Error::new(io::ErrorKind::Other, "child stdout missing")
        })?;
        let reader = stdout.compat();
        let writer: NeovimWriter = Box::new(stdin.compat_write());

        let (redraw_tx, redraw_rx) = mpsc::unbounded_channel::<EditorServerMessage>();
        let (cursor_overlay_tx, cursor_overlay_rx) =
            mpsc::unbounded_channel::<CursorOverlayServerMessage>();
        let (buffer_lines_tx, buffer_lines_rx) =
            mpsc::unbounded_channel::<NvimBufferEvent>();
        let hl_table = Arc::new(Mutex::new(HighlightTable::default()));
        let active_surface_id = Arc::new(Mutex::new(None));
        let redraw_enabled = Arc::new(Mutex::new(false));
        let redraw_batch = Arc::new(Mutex::new(None));
        let handler = RedrawHandler {
            redraw_tx: redraw_tx.clone(),
            cursor_overlay_tx: cursor_overlay_tx.clone(),
            buffer_lines_tx,
            hl_table: hl_table.clone(),
            default_fg: Arc::new(Mutex::new(0x00FF_FFFF)),
            default_bg: Arc::new(Mutex::new(0x0000_0000)),
            grid_sizes: Arc::new(Mutex::new(HashMap::from([(
                1,
                (DEFAULT_WIDTH as u32, DEFAULT_HEIGHT as u32),
            )]))),
            last_cursor: Arc::new(Mutex::new(LastCursor::default())),
            active_surface_id: active_surface_id.clone(),
            redraw_enabled: redraw_enabled.clone(),
            redraw_batch,
            textoff: Arc::new(Mutex::new(0)),
        };
        let (neovim, io_future) = Neovim::<NeovimWriter>::new(reader, writer, handler);

        // Drive nvim-rs io on a dedicated task. When the io future
        // resolves, the session has ended — emit a `Closed` and
        // stop.
        // Capture the pid (== pgid, via process_group(0)) before the
        // child moves into the io task, so Drop can reap the group.
        let child_pgid = child.id().map(|p| p as i32).unwrap_or(0);
        let close_tx = redraw_tx.clone();
        let close_surface_id = active_surface_id.clone();
        let io_handle = tokio::spawn(async move {
            let result = io_future.await;
            let reason = match result {
                Ok(()) => None,
                Err(err) => Some(format!("nvim io error: {err}")),
            };
            let surface_id = close_surface_id.lock().await.clone();
            let _ = close_tx.send(EditorServerMessage::Closed { surface_id, reason });
            // Reap the child explicitly so we don't leave a zombie.
            let _ = child.wait().await;
        });

        apply_ide_defaults(&neovim).await?;

        // ui_attach: subscribe to the ext_* surface the chrome
        // consumes. We use the linegrid format (per-cell deltas) since
        // it's the cheapest to translate into `GridCell`s.
        let mut opts = UiAttachOptions::new();
        // Note: nvim-rs 0.9.2 has an upstream typo `set_messages_externa`
        // (missing trailing `l`). We use the real symbol so the build
        // doesn't fail; if a future nvim-rs version fixes the name,
        // this site flips with no behavioural change.
        opts.set_linegrid_external(true);
        opts.set_popupmenu_external(true);
        opts.set_cmdline_external(true);
        opts.set_messages_externa(true);
        opts.set_rgb(true);
        neovim
            .ui_attach(DEFAULT_WIDTH as i64, DEFAULT_HEIGHT as i64, &opts)
            .await
            .map_err(|e| {
                tracing::error!(
                    error = %e,
                    "[nvim-trace] ui_attach handshake failed; child is alive but \
                     not driving a UI"
                );
                NvimError::Rpc(format!("ui_attach failed: {e}"))
            })?;
        tracing::info!(
            "[nvim-trace] ui_attach OK ({}x{}); embedded session ready for OpenBuffer",
            DEFAULT_WIDTH,
            DEFAULT_HEIGHT
        );

        Ok((
            NvimSession {
                nvim: Arc::new(Mutex::new(Some(neovim))),
                io_handle,
                last_size: (DEFAULT_WIDTH, DEFAULT_HEIGHT),
                last_workspace_root: None,
                active_surface_id,
                redraw_enabled,
                surface_files: HashMap::new(),
                current_nvim_surface: None,
                child_pgid,
            },
            redraw_rx,
            cursor_overlay_rx,
            buffer_lines_rx,
        ))
    }

    /// Forward an `EditorClientMessage` to the embedded nvim.
    pub async fn handle(&mut self, msg: EditorClientMessage) -> Result<(), NvimError> {
        if let Some(surface_id) = msg.surface_id() {
            self.set_active_surface_id(Some(surface_id.to_string()))
                .await;
        }
        match msg {
            EditorClientMessage::OpenBuffer {
                path,
                line,
                character,
                ref surface_id,
                ..
            } => {
                if let Some(sid) = surface_id.as_deref() {
                    self.surface_files.insert(sid.to_string(), path.clone());
                    self.current_nvim_surface = Some(sid.to_string());
                }
                self.open_buffer(&path).await?;
                if let Some(line) = line {
                    self.command(&format!(
                        "call cursor({}, {})",
                        line.saturating_add(1),
                        character.unwrap_or(0).saturating_add(1)
                    ))
                    .await?;
                }
                Ok(())
            }
            EditorClientMessage::SendKeys {
                ref bytes,
                ref surface_id,
                ..
            } => {
                self.ensure_nvim_on_surface(surface_id.as_deref()).await?;
                self.send_keys(bytes).await
            }
            EditorClientMessage::Command {
                ref command,
                ref surface_id,
                ..
            } => {
                self.ensure_nvim_on_surface(surface_id.as_deref()).await?;
                self.command(command).await
            }
            EditorClientMessage::MouseInput {
                ref button,
                ref action,
                ref modifier,
                grid,
                row,
                col,
                count,
                ref surface_id,
                ..
            } => {
                self.ensure_nvim_on_surface(surface_id.as_deref()).await?;
                self.mouse_input(button, action, modifier, grid, row, col, count)
                    .await
            }
            EditorClientMessage::Resize {
                width,
                height,
                ref surface_id,
                ..
            } => {
                self.ensure_nvim_on_surface(surface_id.as_deref()).await?;
                self.resize(width as u64, height as u64).await
            }
            EditorClientMessage::LspAction { .. } => Ok(()),
            // These LSP requests are intercepted in the socket loop (they call
            // the Rust engine directly, never nvim); no-op if one reaches here.
            EditorClientMessage::LspComplete { .. }
            | EditorClientMessage::LspHoverAt { .. } => Ok(()),
            EditorClientMessage::Close => self.close().await,
        }
    }

    /// If nvim's current buffer belongs to a different surface than `surface_id`,
    /// switch it to that surface's last-opened file. No-ops when no file is
    /// tracked for the surface yet (bootstrap not yet complete) or when already
    /// on the right surface.
    async fn ensure_nvim_on_surface(
        &mut self,
        surface_id: Option<&str>,
    ) -> Result<(), NvimError> {
        let Some(target) = surface_id else {
            return Ok(());
        };
        if self.current_nvim_surface.as_deref() == Some(target) {
            return Ok(());
        }
        let Some(path) = self.surface_files.get(target).cloned() else {
            // Surface hasn't sent OpenBuffer yet; nothing to switch to.
            return Ok(());
        };
        tracing::info!(
            surface_id = target,
            path = %path.display(),
            from_surface = ?self.current_nvim_surface,
            "[nvim-trace] surface switch: restoring buffer for surface"
        );
        self.current_nvim_surface = Some(target.to_string());
        self.open_buffer(&path).await
    }

    async fn set_active_surface_id(&self, surface_id: Option<String>) {
        *self.active_surface_id.lock().await = surface_id;
    }

    async fn open_buffer(&self, path: &PathBuf) -> Result<(), NvimError> {
        // The buffer switch itself is the important redraw. Keeping the
        // gate closed here drops BufEnter/grid_line traffic and can leave
        // clients looking at the previous tab's stale grid until some later
        // edit happens to repaint the whole screen.
        *self.redraw_enabled.lock().await = true;
        let guard = self.nvim.lock().await;
        let nvim = guard.as_ref().ok_or(NvimError::Closed)?;
        let path = path.to_string_lossy();
        let cmd = neoism_backend::performer::nvim::vim_select_file_command(&path);
        tracing::info!(
            path = %path,
            cmd = %cmd,
            "[nvim-trace] OpenBuffer → select-file dispatched to embedded nvim"
        );
        nvim_rpc_timeout("select file", nvim.command(&cmd))
            .await
            .map_err(|e| {
                tracing::error!(
                    error = %e,
                    path = %path,
                    "[nvim-trace] select-file failed; nvim rejected the open"
                );
                e
            })?;
        nvim_rpc_timeout("redraw after edit", nvim.command("redraw!")).await?;
        Ok(())
    }

    /// Read the active buffer's text + path in a single nvim round trip.
    ///
    /// Returns `Ok(None)` for an unnamed/scratch buffer. The text is the
    /// buffer's lines joined with `\n` (matching nvim's internal model),
    /// which is exactly what the daemon CRDT replica wants to seed from.
    async fn read_active_buffer(&self) -> Result<Option<BufferText>, NvimError> {
        let guard = self.nvim.lock().await;
        let nvim = guard.as_ref().ok_or(NvimError::Closed)?;
        // One lua call returns `{ path, lines }`. `nvim_buf_get_lines`
        // with `false` strict_indexing on a fresh buffer yields the full
        // line range; we join with "\n" to mirror nvim's text model.
        let lua = r#"
            local path = vim.api.nvim_buf_get_name(0)
            local lines = vim.api.nvim_buf_get_lines(0, 0, -1, false)
            local cursor = vim.api.nvim_win_get_cursor(0)
            return {
                path = path,
                text = table.concat(lines, "\n"),
                cursor_line = math.max((cursor[1] or 1) - 1, 0),
                cursor_col = cursor[2] or 0,
            }
        "#;
        let value = nvim_rpc_timeout("read active buffer", nvim.exec_lua(lua, vec![]))
            .await
            .map_err(|e| {
                tracing::warn!(
                    error = %e,
                    "[nvim-trace] read_active_buffer lua failed"
                );
                e
            })?;

        let path = value
            .as_map()
            .and_then(|map| map_get_str(map, "path"))
            .unwrap_or_default();
        if path.is_empty() {
            // Unnamed buffer (scratch/no file) — nothing authoritative to seed.
            return Ok(None);
        }
        let text = value
            .as_map()
            .and_then(|map| map_get_str(map, "text"))
            .unwrap_or_default();
        let cursor_line = value
            .as_map()
            .and_then(|map| map_get_u64(map, "cursor_line"))
            .unwrap_or(0) as u32;
        let cursor_col = value
            .as_map()
            .and_then(|map| map_get_u64(map, "cursor_col"))
            .unwrap_or(0) as u32;
        Ok(Some(BufferText {
            path: PathBuf::from(path),
            text,
            cursor_line,
            cursor_col,
        }))
    }

    /// Attach the `on_lines` → `neoism_crdt_lines` rpc bridge to the
    /// CURRENT buffer (Wave 6C nvim→CRDT direction). Idempotent per
    /// buffer via `b:neoism_crdt_attached`; the flag is cleared again in
    /// `on_detach` so an unload/reload re-attaches cleanly.
    ///
    /// The callback drops events fired while `vim.g.neoism_crdt_applying`
    /// is set — that flag is only ever raised by
    /// `apply_authoritative_text_to_nvim`, which is how a CRDT-applied
    /// remote update avoids re-entering the CRDT as a fresh nvim edit.
    async fn attach_buffer_change_events(&self) -> Result<bool, NvimError> {
        let guard = self.nvim.lock().await;
        let nvim = guard.as_ref().ok_or(NvimError::Closed)?;
        let lua = r#"
            local buf = vim.api.nvim_get_current_buf()
            if vim.b[buf].neoism_crdt_attached then return false end
            local ok = vim.api.nvim_buf_attach(buf, false, {
                on_lines = function(_, b, _tick, first, last, new_last)
                    if vim.g.neoism_crdt_applying then return end
                    local name = vim.api.nvim_buf_get_name(b)
                    if name == nil or name == "" then return end
                    local lines = vim.api.nvim_buf_get_lines(b, first, new_last, false)
                    pcall(vim.rpcnotify, 1, "neoism_crdt_lines",
                        name, first, last, new_last - first, table.concat(lines, "\n"))
                end,
                on_detach = function(_, b)
                    pcall(function() vim.b[b].neoism_crdt_attached = nil end)
                end,
            })
            if ok then
                vim.b[buf].neoism_crdt_attached = true
                -- Daemon-owned save: intercept every write of this
                -- CRDT-tracked buffer. The daemon flushes the
                -- AUTHORITATIVE doc (which includes every peer's
                -- accepted edits) instead of nvim writing its own
                -- buffer over them. The augroup is per-buffer and
                -- cleared so a detach/re-attach never stacks autocmds.
                local group = vim.api.nvim_create_augroup(
                    "NeoismCrdtWrite_" .. buf, { clear = true })
                vim.api.nvim_create_autocmd("BufWriteCmd", {
                    group = group,
                    buffer = buf,
                    callback = function()
                        local name = vim.api.nvim_buf_get_name(buf)
                        if name == nil or name == "" then return end
                        pcall(vim.rpcnotify, 1, "neoism_crdt_write", name)
                        vim.bo[buf].modified = false
                    end,
                })
            end
            return ok
        "#;
        let value =
            nvim_rpc_timeout("attach buffer change events", nvim.exec_lua(lua, vec![]))
                .await?;
        Ok(value.as_bool().unwrap_or(false))
    }

    pub async fn set_workspace_root(&mut self, root: &Path) -> Result<(), NvimError> {
        // No-op unless the root moved: this runs on EVERY editor
        // envelope, and `:cd` is a non-fast command nvim defers while
        // a count/operator is pending — re-running it per keystroke
        // wedged the input path (the digit freeze).
        if self.last_workspace_root.as_deref() == Some(root) {
            return Ok(());
        }
        let command =
            neoism_backend::performer::nvim::vim_cd_command(&root.to_string_lossy());
        self.command(&command).await?;
        self.last_workspace_root = Some(root.to_path_buf());
        Ok(())
    }

    async fn command(&self, command: &str) -> Result<(), NvimError> {
        command_rpc(&self.nvim, command).await
    }

    async fn send_keys(&self, bytes: &[u8]) -> Result<(), NvimError> {
        // Clone the rpc handle out rather than holding the `nvim` Mutex
        // across the await. `nvim_input` is FUNC_API_FAST so it answers
        // even while a count is pending, but any concurrent non-fast rpc
        // that (before their own clone-out fix) held this Mutex could
        // still stall input; cloning out keeps the input lane wait-free.
        let nvim = self.nvim.lock().await.clone().ok_or(NvimError::Closed)?;
        // nvim_input takes a string; web clients send the literal
        // keysequence bytes (e.g. `"i"`, `"<Esc>"`, etc.) UTF-8 encoded.
        let s = std::str::from_utf8(bytes)
            .map_err(|e| NvimError::Rpc(format!("non-utf8 input bytes: {e}")))?;
        nvim_rpc_timeout("input", nvim.input(s)).await.map(|_| ())
    }

    async fn mouse_input(
        &self,
        button: &str,
        action: &str,
        modifier: &str,
        grid: i64,
        row: i64,
        col: i64,
        count: u32,
    ) -> Result<(), NvimError> {
        // Clone the rpc handle out (see `send_keys`) so the mouse-input
        // lane never waits on the `nvim` Mutex held by a deferred non-fast
        // rpc.
        let nvim = self.nvim.lock().await.clone().ok_or(NvimError::Closed)?;
        for _ in 0..count.max(1) {
            nvim_rpc_timeout(
                "mouse input",
                nvim.input_mouse(button, action, modifier, grid, row, col),
            )
            .await?;
        }
        Ok(())
    }

    async fn resize(&mut self, width: u64, height: u64) -> Result<(), NvimError> {
        if (width, height) == self.last_size {
            return Ok(());
        }
        let guard = self.nvim.lock().await;
        let nvim = guard.as_ref().ok_or(NvimError::Closed)?;
        nvim_rpc_timeout(
            "ui_try_resize",
            nvim.ui_try_resize(width as i64, height as i64),
        )
        .await?;
        let page_rows = ((height as f32) * 0.66).round().max(1.0) as u64;
        let scroll_command = format!("set scroll={page_rows}");
        let _ = nvim_rpc_timeout("set scroll", nvim.command(&scroll_command)).await;
        drop(guard);
        self.last_size = (width, height);
        Ok(())
    }

    async fn close(&self) -> Result<(), NvimError> {
        let mut guard = self.nvim.lock().await;
        if let Some(nvim) = guard.take() {
            // Best-effort `:qa!` — even if it fails, dropping the
            // session will abort the io future and `kill_on_drop`
            // will reap the child.
            let _ = nvim_rpc_timeout("close", nvim.command(":qa!")).await;
        }
        Ok(())
    }
}

/// Dispatch a non-fast `nvim_command` WITHOUT holding the `nvim` Mutex
/// across the rpc await. The client handle is cloned out first (a cheap
/// clone that shares the underlying rpc writer), so a command nvim defers
/// (pending count/operator/prompt) can never hold the Mutex hostage and
/// wedge a concurrent `send_keys` — the digit-key freeze.
pub(crate) async fn command_rpc(
    handle: &Arc<Mutex<Option<Neovim<NeovimWriter>>>>,
    command: &str,
) -> Result<(), NvimError> {
    let nvim = handle.lock().await.clone().ok_or(NvimError::Closed)?;
    let file_open_command =
        command.contains("vim.cmd.edit") || command.contains("File Open Failed");
    let cwd_command = command.contains("vim.cmd.cd");
    if file_open_command || cwd_command {
        tracing::warn!(
            target: "neoism::nvim_command",
            command = %command,
            file_open_command,
            cwd_command,
            "nvim command dispatched"
        );
    } else {
        tracing::trace!(
            command = %command,
            "[nvim-trace] Command dispatched to embedded nvim"
        );
    }
    nvim_rpc_timeout("command", nvim.command(command))
        .await
        .map_err(|e| {
            tracing::error!(
                target: "neoism::nvim_command",
                error = %e,
                command = %command,
                file_open_command,
                cwd_command,
                "nvim command failed"
            );
            e
        })
}

pub(crate) async fn nvim_rpc_timeout<T, E, F>(label: &'static str, fut: F) -> Result<T, NvimError>
where
    E: std::fmt::Display,
    F: std::future::Future<Output = Result<T, E>>,
{
    match timeout(NVIM_RPC_TIMEOUT, fut).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(NvimError::Rpc(format!("{label} failed: {error}"))),
        Err(_) => Err(NvimError::Rpc(format!(
            "{label} timed out after {}s",
            NVIM_RPC_TIMEOUT.as_secs()
        ))),
    }
}

/// Look up a string-keyed entry in an rmpv map (the shape `nvim_exec_lua`
/// returns for a lua table). nvim returns map entries as `(Value, Value)`
/// pairs with string keys.
pub(crate) fn map_get_str(map: &[(Value, Value)], key: &str) -> Option<String> {
    map.iter()
        .find(|(k, _)| k.as_str() == Some(key))
        .and_then(|(_, v)| v.as_str().map(str::to_owned))
}

pub(crate) fn map_get_u64(map: &[(Value, Value)], key: &str) -> Option<u64> {
    map.iter()
        .find(|(k, _)| k.as_str() == Some(key))
        .and_then(|(_, v)| v.as_u64())
}

/// Locate `nvim` on `$PATH`. Returns `Some(path)` on hit, `None`
/// otherwise. Pure stdlib so we don't pull in `which`.
pub(crate) fn which_nvim() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join("nvim");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

pub(crate) async fn apply_ide_defaults(nvim: &Neovim<NeovimWriter>) -> Result<(), NvimError> {
    for command in neoism_backend::performer::nvim::ide_init_commands() {
        nvim.command(&command)
            .await
            .map_err(|e| NvimError::Rpc(format!("ide init command failed: {e}")))?;
    }
    nvim.command(r#"lua vim.opt.mouse = 'a'"#)
        .await
        .map_err(|e| NvimError::Rpc(format!("ide mouse default failed: {e}")))?;
    install_buf_modified_autocmd(nvim).await?;
    tracing::info!("[nvim-trace] applied Rio embedded editor runtime");
    Ok(())
}

pub(crate) async fn install_buf_modified_autocmd(
    nvim: &Neovim<NeovimWriter>,
) -> Result<(), NvimError> {
    let lua = r#"
        vim.api.nvim_create_augroup("NeoismBufModified", { clear = true })
        local function notify_modified()
            local path = vim.api.nvim_buf_get_name(0)
            if path == nil or path == "" then return end
            local modified = vim.bo.modified and true or false
            pcall(vim.rpcnotify, 1, "rio_buf_modified", path, modified)
        end
        vim.api.nvim_create_autocmd({ "BufModifiedSet", "BufWritePost", "BufEnter" }, {
            group = "NeoismBufModified",
            callback = notify_modified,
        })

        -- 7C-2: the gutter width (textoff) in cells — remote carets
        -- arrive in BUFFER columns; renderers add this to land in the
        -- text area instead of inside the line numbers.
        vim.api.nvim_create_augroup("NeoismTextoff", { clear = true })
        local function notify_textoff()
            local ok, info = pcall(vim.fn.getwininfo, vim.api.nvim_get_current_win())
            if ok and info and info[1] and info[1].textoff then
                pcall(vim.rpcnotify, 1, "neoism_textoff", info[1].textoff)
            end
        end
        vim.api.nvim_create_autocmd(
            { "BufEnter", "WinEnter", "VimResized", "OptionSet" },
            { group = "NeoismTextoff", callback = notify_textoff }
        )
        notify_textoff()
    "#;
    nvim.exec_lua(lua, vec![]).await.map(|_| ()).map_err(|e| {
        NvimError::Rpc(format!("buf-modified autocmd install failed: {e}"))
    })?;
    tracing::info!("[nvim-trace] installed BufModifiedSet rpc bridge");
    Ok(())
}

