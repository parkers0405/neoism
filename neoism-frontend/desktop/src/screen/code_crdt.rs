//! Desktop wiring for code-pane CRDT co-editing — the multiplayer port
//! of `markdown_crdt.rs` for the native editor. The shared crate owns
//! the binding (`neoism_ui::editor::code::doc_sync`); this file is the
//! Screen-side plumbing:
//!
//! - `drain_code_crdt_messages` runs once per `pump_daemon` turn:
//!   lazily binds every open code pane to its `file://<path>` document,
//!   services queued undo/redo intents through the binding's
//!   origin-scoped history, flushes buffer mutations as minimal ops,
//!   garbage-collects bindings whose panes closed, and returns the
//!   messages to ship.
//! - `apply_code_crdt_message` routes inbound snapshots/syncs into the
//!   matching pane (incremental splice + caret transform), echo-guarded
//!   by this screen's origin client id.
//! - `save_current_code_via_daemon` is the single-writer save: flush,
//!   queue `SaveBuffer`, let the daemon write the converged doc.

use std::collections::{HashMap, HashSet};

use neoism_protocol::crdt::{CrdtClientMessage, CrdtServerMessage, CrdtSyncEnvelope};
use neoism_ui::editor::code::doc_sync::CodeDocBinding;
use neoism_ui::editor::code::CodePane;
use neoism_ui::editor::crdt::CrdtTextUpdate;

use super::markdown_crdt::{buffer_id_for_markdown_path, generate_client_id};
use super::Screen;

/// Origin id for replaying snapshot bytes through the remote-apply path
/// (client ids are generated non-zero, so this never trips the echo
/// guard).
const SNAPSHOT_ORIGIN: u64 = 0;

pub struct CodeCrdtState {
    client_id: u64,
    bindings: HashMap<String, CodeDocBinding>,
    outbound: Vec<CrdtClientMessage>,
}

impl Default for CodeCrdtState {
    fn default() -> Self {
        Self {
            client_id: generate_client_id(),
            bindings: HashMap::new(),
            outbound: Vec::new(),
        }
    }
}

impl CodeCrdtState {
    /// The live binding for a buffer id, if the pane is doc-bound.
    /// Read-only peek for consumers outside the drain (the LSP pump
    /// anchors diagnostics through it).
    pub(crate) fn binding_for(&self, buffer_id: &str) -> Option<&CodeDocBinding> {
        self.bindings.get(buffer_id)
    }
}

impl Screen<'_> {
    /// Per-pump local-edit choke point for code panes (mirror of
    /// `drain_markdown_crdt_messages`).
    pub fn drain_code_crdt_messages(&mut self) -> (Vec<CrdtClientMessage>, bool) {
        if !self.context_manager.daemon_client_attached() {
            // No daemon: buffers fall back to their own snapshot undo.
            for grid in self.context_manager.contexts_mut() {
                for item in grid.contexts_mut().values_mut() {
                    if let Some(code) = item.context_mut().code.as_mut() {
                        code.buffer.set_doc_history_bound(false);
                    }
                }
            }
            return (Vec::new(), false);
        }

        let mut pane_changed = false;
        let state = &mut self.code_crdt;
        let mut open_buffer_ids: HashSet<String> = HashSet::new();
        for grid in self.context_manager.contexts_mut() {
            for item in grid.contexts_mut().values_mut() {
                let Some(code) = item.context_mut().code.as_mut() else {
                    continue;
                };
                let buffer_id = buffer_id_for_markdown_path(&code.path);
                pane_changed |=
                    drain_code_pane_crdt(state, code, buffer_id, &mut open_buffer_ids);
            }
        }

        state
            .bindings
            .retain(|buffer_id, _| open_buffer_ids.contains(buffer_id));
        (std::mem::take(&mut state.outbound), pane_changed)
    }

    /// Daemon-owned save for the CURRENT code pane. Returns false when
    /// the pane isn't doc-bound yet (caller falls back to the local
    /// write path).
    pub fn save_current_code_via_daemon(&mut self) -> bool {
        if !self.context_manager.daemon_client_attached() {
            return false;
        }
        // Wrong-daemon guard (mirror of the markdown save): a PEER link
        // can only save files of the workspace joined THROUGH it.
        if self.context_manager.daemon_link_is_peer()
            && !self.context_manager.current_workspace_is_remote_joined()
        {
            return false;
        }
        let Some(code) = self.context_manager.current_mut().code.as_mut() else {
            return false;
        };
        let buffer_id = buffer_id_for_markdown_path(&code.path);
        let state = &mut self.code_crdt;
        let Some(binding) = state.bindings.get_mut(&buffer_id) else {
            return false;
        };
        if !binding.is_seeded() {
            return false;
        }
        if let Some(update) = binding.flush_local(&code.buffer) {
            state.outbound.push(make_apply_sync(&buffer_id, update));
        }
        state
            .outbound
            .push(CrdtClientMessage::SaveBuffer { buffer_id });
        true
    }

    /// Route an inbound CRDT server message into code panes. Returns
    /// whether visible state changed. Safe to call alongside the
    /// markdown handler — buffer ids are per-path, so exactly one of
    /// the two owns any given id.
    pub fn apply_code_crdt_message(&mut self, message: &CrdtServerMessage) -> bool {
        match message {
            CrdtServerMessage::Snapshot {
                buffer_id,
                update_v1,
                ..
            }
            | CrdtServerMessage::SnapshotFallback {
                buffer_id,
                update_v1,
                ..
            } => self.apply_code_crdt_snapshot(buffer_id, update_v1),
            CrdtServerMessage::Sync { envelope } => self.apply_code_crdt_sync(
                &envelope.buffer_id,
                envelope.origin_client_id,
                &envelope.update_v1,
            ),
            CrdtServerMessage::Saved { buffer_id, .. } => {
                self.apply_code_crdt_saved(buffer_id)
            }
            _ => false,
        }
    }

    fn apply_code_crdt_saved(&mut self, buffer_id: &str) -> bool {
        let Some(path) = self.find_code_path_for_buffer_id(buffer_id) else {
            return false;
        };
        if let Some(code) = self.context_manager.current_mut().code.as_mut() {
            if code.path == path {
                code.buffer.mark_saved();
            }
        }
        self.sync_markdown_tab_modified(&path, false);
        self.notify_code_lsp_saved(&path);
        self.renderer.notifications.push(
            format!("Wrote {}", path.display()),
            neoism_ui::panels::notifications::NotificationLevel::Info,
        );
        true
    }

    fn apply_code_crdt_snapshot(&mut self, buffer_id: &str, update_v1: &[u8]) -> bool {
        let state = &mut self.code_crdt;
        let Some(binding) = state.bindings.get_mut(buffer_id) else {
            return false;
        };
        let Some(code) = find_code_pane_mut(&mut self.context_manager, buffer_id)
        else {
            return false;
        };
        if binding.is_seeded() {
            match binding.apply_remote(SNAPSHOT_ORIGIN, update_v1, &mut code.buffer) {
                Ok(result) => {
                    if let Some(update) = result.flushed_local {
                        state.outbound.push(make_apply_sync(buffer_id, update));
                    }
                    result.changed
                }
                Err(error) => {
                    tracing::warn!(
                        %error,
                        buffer_id,
                        "code CRDT snapshot apply failed"
                    );
                    false
                }
            }
        } else {
            match binding.seed_from_snapshot(update_v1, &mut code.buffer) {
                Ok(changed) => changed,
                Err(error) => {
                    tracing::warn!(%error, buffer_id, "code CRDT seed failed");
                    false
                }
            }
        }
    }

    fn apply_code_crdt_sync(
        &mut self,
        buffer_id: &str,
        origin_client_id: u64,
        update_v1: &[u8],
    ) -> bool {
        let state = &mut self.code_crdt;
        let Some(binding) = state.bindings.get_mut(buffer_id) else {
            return false;
        };
        let Some(code) = find_code_pane_mut(&mut self.context_manager, buffer_id)
        else {
            return false;
        };
        match binding.apply_remote(origin_client_id, update_v1, &mut code.buffer) {
            Ok(result) => {
                if let Some(update) = result.flushed_local {
                    state.outbound.push(make_apply_sync(buffer_id, update));
                }
                result.changed
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    buffer_id,
                    "code CRDT sync apply failed; requesting fresh snapshot"
                );
                state.outbound.push(CrdtClientMessage::RequestSnapshot {
                    buffer_id: buffer_id.to_string(),
                    state_vector_v1: binding.state_vector_v1(),
                });
                false
            }
        }
    }

    fn find_code_path_for_buffer_id(
        &mut self,
        buffer_id: &str,
    ) -> Option<std::path::PathBuf> {
        for grid in self.context_manager.contexts_mut() {
            for item in grid.contexts_mut().values_mut() {
                if let Some(code) = item.context_mut().code.as_mut() {
                    if buffer_id_for_markdown_path(&code.path) == buffer_id {
                        return Some(code.path.clone());
                    }
                }
            }
        }
        None
    }
}

fn drain_code_pane_crdt(
    state: &mut CodeCrdtState,
    code: &mut CodePane,
    buffer_id: String,
    open_buffer_ids: &mut HashSet<String>,
) -> bool {
    // A pane that failed to load has no authoritative text to seed.
    if code.error.is_some() {
        return false;
    }
    let mut pane_changed = false;
    open_buffer_ids.insert(buffer_id.clone());
    match state.bindings.entry(buffer_id) {
        std::collections::hash_map::Entry::Vacant(entry) => {
            code.buffer.set_doc_history_bound(false);
            state.outbound.push(CrdtClientMessage::OpenBuffer {
                buffer_id: entry.key().clone(),
                initial_text: code.buffer.lines.join("\n"),
            });
            let binding = CodeDocBinding::new(state.client_id, entry.key().clone());
            entry.insert(binding);
        }
        std::collections::hash_map::Entry::Occupied(mut entry) => {
            let buffer_id = entry.key().clone();
            let binding = entry.get_mut();
            code.buffer.set_doc_history_bound(binding.is_seeded());
            for request in code.buffer.take_doc_history_requests() {
                use neoism_ui::editor::code::CodeDocHistoryRequest;
                let result = match request {
                    CodeDocHistoryRequest::Undo => binding.undo(&mut code.buffer),
                    CodeDocHistoryRequest::Redo => binding.redo(&mut code.buffer),
                };
                if let Some(update) = result.flushed_local {
                    state.outbound.push(make_apply_sync(&buffer_id, update));
                }
                if let Some(update) = result.history_update {
                    state.outbound.push(make_apply_sync(&buffer_id, update));
                }
                pane_changed |= result.changed;
            }
            if let Some(update) = binding.flush_local(&code.buffer) {
                state.outbound.push(make_apply_sync(&buffer_id, update));
            }
        }
    }
    pane_changed
}

fn make_apply_sync(buffer_id: &str, update: CrdtTextUpdate) -> CrdtClientMessage {
    CrdtClientMessage::ApplySync {
        envelope: CrdtSyncEnvelope {
            buffer_id: buffer_id.to_string(),
            origin_client_id: update.origin_client_id,
            update_v1: update.update_v1,
            state_vector_v1: update.state_vector_v1,
        },
    }
}

fn find_code_pane_mut<'a>(
    context_manager: &'a mut crate::context::ContextManager<crate::event::EventProxy>,
    buffer_id: &str,
) -> Option<&'a mut CodePane> {
    for grid in context_manager.contexts_mut() {
        for item in grid.contexts_mut().values_mut() {
            let context = item.context_mut();
            let matches = context
                .code
                .as_ref()
                .is_some_and(|code| buffer_id_for_markdown_path(&code.path) == buffer_id);
            if matches {
                return context.code.as_mut();
            }
        }
    }
    None
}
