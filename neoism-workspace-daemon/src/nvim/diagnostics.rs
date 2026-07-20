use super::*;

// -----------------------------------------------------------------------
// LSP diagnostics push (wave-7 web parity)
// -----------------------------------------------------------------------

impl NvimSession {
    /// Best-effort snapshot of the active buffer's `vim.diagnostic.get`
    /// items. Returns an empty vec when nvim is closed, no LSP is
    /// attached, or the rpc call fails — callers fan this into a
    /// `DiagnosticsPush` for every subscribed route.
    ///
    /// We use the buffer-scoped lua call (`vim.diagnostic.get(0)`) so
    /// the cost stays bounded; web sessions only ever drive one
    /// visible buffer per route.
    pub async fn snapshot_diagnostics(&self) -> Option<Vec<ProtoDiagnosticItem>> {
        snapshot_diagnostics_rpc(&self.nvim).await
    }

    /// Best-effort probe of the attached LSP servers. Returns the
    /// list of `(server_name, state)` pairs nvim currently reports.
    /// `None` when nvim is closed/blocked or the call fails — callers
    /// must skip publishing rather than treat it as "no servers".
    pub async fn snapshot_lsp_states(&self) -> Option<Vec<(String, LspState)>> {
        snapshot_lsp_states_rpc(&self.nvim).await
    }
}

/// True when nvim is not at a safe state — pending count/operator in
/// normal mode, a prompt, a hit-enter page. In that state nvim DEFERS
/// every non-fast RPC (`nvim_exec_lua`, `nvim_command`) until more
/// input arrives; issuing one anyway would hold our locks hostage for
/// as long as the user leaves the state open. `nvim_get_mode` is
/// FUNC_API_FAST, so it answers even then.
pub(crate) async fn nvim_is_blocked(nvim: &Neovim<NeovimWriter>) -> bool {
    match nvim_rpc_timeout("get_mode", nvim.get_mode()).await {
        Ok(pairs) => pairs
            .iter()
            .any(|(k, v)| k.as_str() == Some("blocking") && v.as_bool() == Some(true)),
        // On timeout/error assume blocked — safer to skip a poll tick
        // than to stack a deferred exec_lua behind it.
        Err(_) => true,
    }
}

/// Clone the rpc client out of `nvim` WITHOUT holding the Mutex across
/// any RPC await — `send_keys` locks the same Mutex, so holding it
/// across a deferred exec_lua froze the user's keys (the digit-key
/// freeze). Returns `None` when the session is closed.
pub(crate) async fn clone_nvim_client(
    nvim: &Arc<Mutex<Option<Neovim<NeovimWriter>>>>,
) -> Option<Neovim<NeovimWriter>> {
    nvim.lock().await.clone()
}

/// Best-effort snapshot of the active buffer's `vim.diagnostic.get`
/// items. `None` when nvim is closed, blocked (mid-count/prompt), or
/// the rpc call fails — callers skip publishing on `None` so a skipped
/// poll never masquerades as "diagnostics cleared".
///
/// The buffer-scoped lua call (`vim.diagnostic.get(0)`) keeps the cost
/// bounded; web sessions only ever drive one visible buffer per route.
pub(crate) async fn snapshot_diagnostics_rpc(
    handle: &Arc<Mutex<Option<Neovim<NeovimWriter>>>>,
) -> Option<Vec<ProtoDiagnosticItem>> {
    let nvim = clone_nvim_client(handle).await?;
    if nvim_is_blocked(&nvim).await {
        return None;
    }
    // Encode the diagnostics list as JSON inside lua so we can
    // round-trip it through `exec_lua`'s value return cleanly.
    // Each entry is { lnum, col, severity, message, source }.
    let lua = r#"
        local diags = vim.diagnostic.get(0) or {}
        local out = {}
        for _, d in ipairs(diags) do
            table.insert(out, {
                lnum = d.lnum or 0,
                col = d.col or 0,
                severity = d.severity or 1,
                message = d.message or "",
                source = d.source or "",
            })
        end
        return out
    "#;
    let value = match nvim_rpc_timeout("diagnostics exec_lua", nvim.exec_lua(lua, vec![]))
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(error = %e, "diagnostics exec_lua failed");
            return None;
        }
    };
    Some(decode_diagnostics(value))
}

pub(crate) async fn snapshot_lsp_states_rpc(
    handle: &Arc<Mutex<Option<Neovim<NeovimWriter>>>>,
) -> Option<Vec<(String, LspState)>> {
    let nvim = clone_nvim_client(handle).await?;
    if nvim_is_blocked(&nvim).await {
        return None;
    }
    let lua = r#"
        local clients = (vim.lsp.get_clients and vim.lsp.get_clients({ bufnr = 0 }))
            or (vim.lsp.get_active_clients and vim.lsp.get_active_clients({ bufnr = 0 }))
            or {}
        local out = {}
        for _, c in ipairs(clients) do
            local state = "ready"
            if c.is_stopped and c.is_stopped() then state = "stopped" end
            table.insert(out, { name = c.name or "", state = state })
        end
        return out
    "#;
    let value = nvim_rpc_timeout("lsp states exec_lua", nvim.exec_lua(lua, vec![]))
        .await
        .ok()?;
    let Value::Array(items) = value else {
        return None;
    };
    let mut out = Vec::new();
    for item in items {
        let Value::Map(fields) = item else { continue };
        let mut name = String::new();
        let mut state_str = String::from("ready");
        for (k, v) in fields {
            let Value::String(key) = k else { continue };
            match key.as_str().unwrap_or("") {
                "name" => {
                    if let Some(s) = v.as_str() {
                        name = s.to_string();
                    }
                }
                "state" => {
                    if let Some(s) = v.as_str() {
                        state_str = s.to_string();
                    }
                }
                _ => {}
            }
        }
        if name.is_empty() {
            continue;
        }
        let state = match state_str.as_str() {
            "stopped" => LspState::Stopped,
            "starting" => LspState::Starting,
            "indexing" => LspState::Indexing,
            _ => LspState::Ready,
        };
        out.push((name, state));
    }
    Some(out)
}

pub(crate) fn decode_diagnostics(value: Value) -> Vec<ProtoDiagnosticItem> {
    let Value::Array(items) = value else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items {
        let Value::Map(fields) = item else { continue };
        let mut line = 0u32;
        let mut col = 0u32;
        let mut severity = 1u8;
        let mut message = String::new();
        let mut source: Option<String> = None;
        for (k, v) in fields {
            let Value::String(key) = k else { continue };
            match key.as_str().unwrap_or("") {
                "lnum" => {
                    if let Some(n) = value_as_u64(&v) {
                        line = n as u32;
                    }
                }
                "col" => {
                    if let Some(n) = value_as_u64(&v) {
                        col = n as u32;
                    }
                }
                "severity" => {
                    if let Some(n) = value_as_u64(&v) {
                        severity = n.min(4).max(1) as u8;
                    }
                }
                "message" => {
                    if let Some(s) = v.as_str() {
                        message = s.to_string();
                    }
                }
                "source" => {
                    if let Some(s) = v.as_str() {
                        if !s.is_empty() {
                            source = Some(s.to_string());
                        }
                    }
                }
                _ => {}
            }
        }
        out.push(ProtoDiagnosticItem {
            line,
            col,
            end_line: line,
            end_col: col,
            severity,
            message,
            source,
            code: None,
            code_description: None,
            tags: Vec::new(),
            related_information: Vec::new(),
        });
    }
    out
}

/// Connection-scoped subscription table for diagnostics push. Maps
/// `RouteId -> bool` (presence == subscribed); we keep it simple
/// rather than tracking the route -> file mapping because the
/// daemon's nvim only drives one active buffer at a time today.
#[derive(Default)]
pub struct DiagnosticsSubscriptions {
    routes: HashMap<RouteId, ()>,
    /// Last-published per-route snapshot. We hash-compare to avoid
    /// re-pushing identical diagnostic sets on every poll tick.
    last_push: HashMap<RouteId, u64>,
    /// Last LSP states we pushed, keyed by server name. Used to
    /// suppress duplicate `LspStatusUpdate` events.
    last_lsp: HashMap<String, LspState>,
}

impl DiagnosticsSubscriptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn subscribe(&mut self, route_id: RouteId) {
        self.routes.insert(route_id, ());
    }

    pub fn unsubscribe(&mut self, route_id: RouteId) {
        self.routes.remove(&route_id);
        self.last_push.remove(&route_id);
    }

    pub fn routes(&self) -> Vec<RouteId> {
        self.routes.keys().copied().collect()
    }

    /// Pull one poll's worth of diagnostics + lsp state from `session`.
    /// Free of `&mut self` on purpose: the server spawns this as a
    /// detached task so a blocked/deferred nvim RPC can never stall the
    /// websocket loop that dispatches SendKeys (the digit-key freeze).
    /// Either field is `None` when the snapshot was skipped (nvim
    /// closed, mid-count, or rpc failure) — `apply` must not publish
    /// for a skipped snapshot.
    pub async fn fetch(session: &NvimSessionHandle) -> DiagnosticsFetch {
        DiagnosticsFetch {
            items: session.snapshot_diagnostics().await,
            lsp: session.snapshot_lsp_states().await,
        }
    }

    /// Fold a completed fetch into the subscription state and emit any
    /// frames that changed since the last poll onto `tx`. Sync so the
    /// websocket loop can run it inline without awaiting nvim.
    pub fn apply(
        &mut self,
        fetch: DiagnosticsFetch,
        tx: &mpsc::UnboundedSender<DiagnosticsServerMessage>,
    ) {
        if self.routes.is_empty() {
            return;
        }
        if let Some(items) = fetch.items {
            let hash = hash_items(&items);
            let route_ids: Vec<RouteId> = self.routes.keys().copied().collect();
            for route_id in route_ids {
                let prev = self.last_push.get(&route_id).copied();
                if prev == Some(hash) {
                    continue;
                }
                self.last_push.insert(route_id, hash);
                let msg = if items.is_empty() {
                    DiagnosticsServerMessage::DiagnosticsCleared { route_id }
                } else {
                    DiagnosticsServerMessage::DiagnosticsPush {
                        route_id,
                        items: items.clone(),
                    }
                };
                let _ = tx.send(msg);
            }
        }
        // LSP lifecycle deltas.
        if let Some(lsp) = fetch.lsp {
            let mut seen: HashMap<String, LspState> = HashMap::new();
            for (server, state) in lsp {
                if self.last_lsp.get(&server) != Some(&state) {
                    let _ = tx.send(DiagnosticsServerMessage::LspStatusUpdate {
                        server: server.clone(),
                        state: state.clone(),
                    });
                }
                seen.insert(server, state);
            }
            // Any server that went away counts as Stopped.
            for (server, _) in self.last_lsp.iter() {
                if !seen.contains_key(server) {
                    let _ = tx.send(DiagnosticsServerMessage::LspStatusUpdate {
                        server: server.clone(),
                        state: LspState::Stopped,
                    });
                }
            }
            self.last_lsp = seen;
        }
    }
}

/// One poll's worth of snapshot data, produced by
/// `DiagnosticsSubscriptions::fetch` and folded in by `apply`.
pub struct DiagnosticsFetch {
    pub items: Option<Vec<ProtoDiagnosticItem>>,
    pub lsp: Option<Vec<(String, LspState)>>,
}

impl DiagnosticsFetch {
    pub fn from_parts(
        items: Option<Vec<ProtoDiagnosticItem>>,
        lsp: Option<Vec<(String, LspState)>>,
    ) -> Self {
        Self { items, lsp }
    }
}

pub(crate) fn hash_items(items: &[ProtoDiagnosticItem]) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    items.len().hash(&mut hasher);
    for it in items {
        it.line.hash(&mut hasher);
        it.col.hash(&mut hasher);
        it.end_line.hash(&mut hasher);
        it.end_col.hash(&mut hasher);
        it.severity.hash(&mut hasher);
        it.message.hash(&mut hasher);
        it.source.hash(&mut hasher);
        it.code.hash(&mut hasher);
        it.code_description.hash(&mut hasher);
        it.tags.hash(&mut hasher);
        it.related_information.len().hash(&mut hasher);
        for related in &it.related_information {
            related.path.hash(&mut hasher);
            related.line.hash(&mut hasher);
            related.col.hash(&mut hasher);
            related.end_line.hash(&mut hasher);
            related.end_col.hash(&mut hasher);
            related.message.hash(&mut hasher);
        }
    }
    hasher.finish()
}
