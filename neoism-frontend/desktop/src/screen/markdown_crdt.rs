//! Wave 7B: desktop wiring for markdown-pane CRDT co-editing.
//!
//! The shared crate owns the binding logic (`neoism_ui::editor::markdown::
//! doc_sync`); this file is the Screen-side plumbing:
//!
//! - `drain_markdown_crdt_messages` runs once per `pump_daemon` turn (the
//!   single local-edit choke point at the host level): it lazily binds
//!   every open markdown pane to its `file://<path>` document, flushes
//!   pane mutations into minimal UTF-16 ops, and hands the outbound
//!   `CrdtClientMessage`s to the app loop for shipping.
//! - `apply_markdown_crdt_message` routes inbound snapshots/syncs into the
//!   matching pane (incremental splice + caret transform), echo-guarded
//!   by this screen's origin client id.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
};

use neoism_protocol::crdt::{CrdtClientMessage, CrdtServerMessage, CrdtSyncEnvelope};
use neoism_ui::editor::crdt::CrdtTextUpdate;
use neoism_ui::editor::markdown::doc_sync::MarkdownDocBinding;
use neoism_ui::editor::markdown::MarkdownPane;
use neoism_ui::editor::notebook::NotebookPane;

use super::Screen;

/// Origin id used when replaying snapshot bytes through the remote-apply
/// path. Client ids are generated non-zero, so this never trips the
/// "own origin" echo guard.
const SNAPSHOT_ORIGIN: u64 = 0;

pub struct MarkdownCrdtState {
    /// Yrs client id every local markdown edit from this screen is
    /// stamped with. Random, non-zero, below 2^53 (Yjs JS interop) and
    /// distinct from the daemon's 9e9 default.
    client_id: u64,
    bindings: HashMap<String, MarkdownDocBinding>,
    outbound: Vec<CrdtClientMessage>,
}

impl Default for MarkdownCrdtState {
    fn default() -> Self {
        Self {
            client_id: generate_client_id(),
            bindings: HashMap::new(),
            outbound: Vec::new(),
        }
    }
}

impl MarkdownCrdtState {
    #[allow(dead_code)]
    pub fn client_id(&self) -> u64 {
        self.client_id
    }
}

pub(crate) fn generate_client_id() -> u64 {
    // Mask into Yjs's safe-integer client-id space; retry the (absurdly
    // unlikely) zero so 0 can act as the snapshot-origin sentinel.
    loop {
        let id = (uuid::Uuid::new_v4().as_u128() as u64) & ((1 << 53) - 1);
        if id != 0 {
            return id;
        }
    }
}

/// Buffer id scheme shared with the daemon (`crdt_buffer_id_for_path`):
/// `file://<absolute-path>`. Canonicalized so the markdown pane and an
/// nvim view of the same file land on the same authoritative document.
pub fn buffer_id_for_markdown_path(path: &std::path::Path) -> String {
    let canonical = canonical_buffer_path(path);
    format!("file://{}", canonical.to_string_lossy())
}

/// Virtual CRDT buffer for a notebook's rendered markdown view.
///
/// The daemon's `file://` save path writes plain text, so notebook panes
/// must not share the raw `.ipynb` file buffer. Live collaboration runs
/// over this virtual text buffer; `NotebookPane::save()` remains the JSON
/// writer.
pub fn buffer_id_for_notebook_render_path(path: &std::path::Path) -> String {
    let canonical = canonical_buffer_path(path);
    format!("notebook-render://{}", canonical.to_string_lossy())
}

fn canonical_buffer_path(path: &std::path::Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

impl Screen<'_> {
    pub(crate) fn reload_open_markdown_files_from_disk(
        &mut self,
    ) -> (Vec<CrdtClientMessage>, bool) {
        let mut changed = false;
        let mut outbound = Vec::new();
        let mut modified_paths = Vec::new();

        {
            let state = &mut self.markdown_crdt;

            for grid in self.context_manager.contexts_mut() {
                for item in grid.contexts_mut().values_mut() {
                    let Some(pane) = item.context_mut().markdown.as_mut() else {
                        continue;
                    };
                    let path = normalize_markdown_reload_path(pane.path.clone());
                    match reload_markdown_pane_from_disk(
                        pane,
                        &path,
                        &mut state.bindings,
                        &mut self.markdown_fs_reload_fingerprints,
                    ) {
                        Ok((message, pane_changed)) => {
                            if let Some(message) = message {
                                outbound.push(message);
                            }
                            if pane_changed {
                                modified_paths.push((pane.path.clone(), pane.is_dirty()));
                            }
                            changed |= pane_changed;
                        }
                        Err(error) => {
                            tracing::warn!(
                                %error,
                                path = %path.display(),
                                "markdown disk reload failed"
                            );
                        }
                    }
                }
            }
        }

        for (path, modified) in modified_paths {
            self.sync_markdown_tab_modified(&path, modified);
        }

        (outbound, changed)
    }

    /// Per-pump local-edit choke point. Ensures a CRDT binding for every
    /// open daemon-backed markdown pane, services queued undo/redo
    /// intents through the binding's origin-scoped history (Wave 7D),
    /// flushes any pane mutations as minimal ops, garbage-collects
    /// bindings whose panes closed, and returns the messages to ship to
    /// the daemon plus whether visible pane state changed (undo/redo
    /// mutate the pane here, so the caller must redraw).
    pub fn drain_markdown_crdt_messages(&mut self) -> (Vec<CrdtClientMessage>, bool) {
        if !self.context_manager.daemon_client_attached() {
            // No daemon: panes fall back to their own snapshot undo.
            for grid in self.context_manager.contexts_mut() {
                for item in grid.contexts_mut().values_mut() {
                    let context = item.context_mut();
                    if let Some(pane) = context.markdown.as_mut() {
                        pane.set_doc_history_bound(false);
                    }
                    if let Some(notebook) = context.notebook.as_mut() {
                        notebook.markdown.set_doc_history_bound(false);
                    }
                }
            }
            return (Vec::new(), false);
        }

        let mut pane_changed = false;
        let state = &mut self.markdown_crdt;
        let mut open_buffer_ids: HashSet<String> = HashSet::new();
        for grid in self.context_manager.contexts_mut() {
            for item in grid.contexts_mut().values_mut() {
                let context = item.context_mut();
                if let Some(pane) = context.markdown.as_mut() {
                    let buffer_id = buffer_id_for_markdown_path(&pane.path);
                    pane_changed |= drain_markdown_pane_crdt(
                        state,
                        pane,
                        buffer_id,
                        &mut open_buffer_ids,
                    );
                }
                if let Some(notebook) = context.notebook.as_mut() {
                    let buffer_id = buffer_id_for_notebook_render_path(&notebook.path);
                    let changed = drain_markdown_pane_crdt(
                        state,
                        &mut notebook.markdown,
                        buffer_id,
                        &mut open_buffer_ids,
                    );
                    if changed {
                        notebook.sync_order_from_rendered_markdown();
                    }
                    pane_changed |= changed;
                }
            }
        }

        state
            .bindings
            .retain(|buffer_id, _| open_buffer_ids.contains(buffer_id));
        (std::mem::take(&mut state.outbound), pane_changed)
    }

    /// Daemon-owned save for the CURRENT markdown pane: flush any
    /// pending local edits through the binding (so the doc includes
    /// them), then queue `SaveBuffer` — the daemon writes the converged
    /// doc to disk and broadcasts `Saved`. Returns false when the pane
    /// isn't doc-bound yet (caller falls back to the local write).
    pub fn save_current_markdown_via_daemon(&mut self) -> bool {
        if !self.context_manager.daemon_client_attached() {
            return false;
        }
        // Wrong-daemon guard: a PEER link (joined server) can only save
        // files of the workspace joined THROUGH it. If the view sits in
        // a local workspace while the link still points at the peer
        // (e.g. a refused switch left them desynced), routing SaveBuffer
        // there writes a host path on the wrong machine — fall back to
        // the local write instead.
        if self.context_manager.daemon_link_is_peer()
            && !self.context_manager.current_workspace_is_remote_joined()
        {
            return false;
        }
        let Some(pane) = self.context_manager.current_mut().markdown.as_mut() else {
            return false;
        };
        let buffer_id = buffer_id_for_markdown_path(&pane.path);
        let state = &mut self.markdown_crdt;
        let Some(binding) = state.bindings.get_mut(&buffer_id) else {
            return false;
        };
        if !binding.is_seeded() {
            return false;
        }
        if let Some(update) = binding.flush_local(pane) {
            state.outbound.push(make_apply_sync(&buffer_id, update));
        }
        state
            .outbound
            .push(CrdtClientMessage::SaveBuffer { buffer_id });
        true
    }

    /// Flush current notebook markdown edits into the virtual notebook CRDT
    /// buffer before a local JSON save. This never queues `SaveBuffer`,
    /// because the daemon text save path would corrupt `.ipynb` files.
    pub fn flush_current_notebook_crdt(&mut self) -> bool {
        if !self.context_manager.daemon_client_attached() {
            return false;
        }
        let Some(notebook) = self.context_manager.current_mut().notebook.as_mut() else {
            return false;
        };
        let buffer_id = buffer_id_for_notebook_render_path(&notebook.path);
        let state = &mut self.markdown_crdt;
        let Some(binding) = state.bindings.get_mut(&buffer_id) else {
            return false;
        };
        if !binding.is_seeded() {
            return false;
        }
        if let Some(update) = binding.flush_local(&mut notebook.markdown) {
            state.outbound.push(make_apply_sync(&buffer_id, update));
        }
        true
    }

    /// Route an inbound CRDT server message into the matching markdown
    /// pane. Returns whether visible pane state changed (caller redraws).
    pub fn apply_markdown_crdt_message(&mut self, message: &CrdtServerMessage) -> bool {
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
            } => self.apply_crdt_snapshot(buffer_id, update_v1),
            CrdtServerMessage::Sync { envelope } => self.apply_crdt_sync(
                &envelope.buffer_id,
                envelope.origin_client_id,
                &envelope.update_v1,
            ),
            CrdtServerMessage::Saved { buffer_id, .. } => {
                self.apply_crdt_saved(buffer_id)
            }
            CrdtServerMessage::Error {
                buffer_id: Some(buffer_id),
                message,
            } if message.starts_with("save failed") => {
                self.renderer.notifications.push(
                    format!("Could not write {buffer_id}: {message}"),
                    neoism_ui::panels::notifications::NotificationLevel::Error,
                );
                true
            }
            // `Update` is the legacy duplicate of `Sync` (the hub
            // broadcasts both); applying both would be redundant.
            CrdtServerMessage::Update { .. }
            | CrdtServerMessage::Presence { .. }
            | CrdtServerMessage::PresenceSnapshot { .. }
            | CrdtServerMessage::CompactionStatus(_)
            | CrdtServerMessage::Error { .. } => false,
        }
    }

    /// The daemon flushed this document to disk (our save OR any
    /// peer's — the dirty bit is doc-level). Clear the pane/tab
    /// modified state and run the same post-save bookkeeping the local
    /// write path runs.
    fn apply_crdt_saved(&mut self, buffer_id: &str) -> bool {
        let Some(pane) = find_markdown_pane_mut(&mut self.context_manager, buffer_id)
        else {
            return false;
        };
        pane.mark_saved();
        let path = pane.path.clone();
        self.sync_markdown_tab_modified(&path, false);
        if let Some(result) = self.apply_generated_neoism_tasks_save(&path) {
            let (message, level) = match result {
                Ok(message) => (
                    message,
                    neoism_ui::panels::notifications::NotificationLevel::Info,
                ),
                Err(err) => (
                    err,
                    neoism_ui::panels::notifications::NotificationLevel::Error,
                ),
            };
            self.renderer.notifications.push(message, level);
            return true;
        }
        self.invalidate_note_index_for_path(&path);
        self.rebuild_note_graph_for_path(&path);
        self.renderer.notifications.push(
            format!("Wrote {}", path.display()),
            neoism_ui::panels::notifications::NotificationLevel::Info,
        );
        true
    }

    fn apply_crdt_snapshot(&mut self, buffer_id: &str, update_v1: &[u8]) -> bool {
        let state = &mut self.markdown_crdt;
        let Some(binding) = state.bindings.get_mut(buffer_id) else {
            return false;
        };
        let Some(mut target) = find_crdt_pane_mut(&mut self.context_manager, buffer_id)
        else {
            return false;
        };
        if binding.is_seeded() {
            // Catch-up snapshot for an already-bound doc: replay it
            // through the remote-apply path (idempotent Yrs merge).
            match binding.apply_remote(SNAPSHOT_ORIGIN, update_v1, target.pane_mut()) {
                Ok(result) => {
                    if let Some(update) = result.flushed_local {
                        state.outbound.push(make_apply_sync(buffer_id, update));
                    }
                    target.sync_notebook_if_changed(result.changed);
                    result.changed
                }
                Err(error) => {
                    tracing::warn!(
                        %error,
                        buffer_id,
                        "markdown CRDT snapshot apply failed"
                    );
                    false
                }
            }
        } else {
            match binding.seed_from_snapshot(update_v1, target.pane_mut()) {
                Ok(changed) => {
                    target.sync_notebook_if_changed(changed);
                    changed
                }
                Err(error) => {
                    tracing::warn!(
                        %error,
                        buffer_id,
                        "markdown CRDT seed failed"
                    );
                    false
                }
            }
        }
    }

    fn apply_crdt_sync(
        &mut self,
        buffer_id: &str,
        origin_client_id: u64,
        update_v1: &[u8],
    ) -> bool {
        let state = &mut self.markdown_crdt;
        let Some(binding) = state.bindings.get_mut(buffer_id) else {
            return false;
        };
        let Some(mut target) = find_crdt_pane_mut(&mut self.context_manager, buffer_id)
        else {
            return false;
        };
        match binding.apply_remote(origin_client_id, update_v1, target.pane_mut()) {
            Ok(result) => {
                if let Some(update) = result.flushed_local {
                    state.outbound.push(make_apply_sync(buffer_id, update));
                }
                target.sync_notebook_if_changed(result.changed);
                result.changed
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    buffer_id,
                    "markdown CRDT sync apply failed; requesting fresh snapshot"
                );
                state.outbound.push(CrdtClientMessage::RequestSnapshot {
                    buffer_id: buffer_id.to_string(),
                    state_vector_v1: binding.state_vector_v1(),
                });
                false
            }
        }
    }
}

fn drain_markdown_pane_crdt(
    state: &mut MarkdownCrdtState,
    pane: &mut MarkdownPane,
    buffer_id: String,
    open_buffer_ids: &mut HashSet<String>,
) -> bool {
    // The pane's real content is still in flight from the host daemon.
    // Binding now would seed the CRDT doc with the empty placeholder —
    // whose snapshot then CLOBBERS the fetched content the moment it
    // paints (content flashes, goes blank, tab reads dirty). Bind on the
    // next drain after `apply_remote_source` lands.
    if pane.remote_content_pending {
        return false;
    }
    let mut pane_changed = false;
    open_buffer_ids.insert(buffer_id.clone());
    match state.bindings.entry(buffer_id) {
        std::collections::hash_map::Entry::Vacant(entry) => {
            pane.set_doc_history_bound(false);
            state.outbound.push(CrdtClientMessage::OpenBuffer {
                buffer_id: entry.key().clone(),
                initial_text: pane.lines.join("\n"),
            });
            let binding = MarkdownDocBinding::new(state.client_id, entry.key().clone());
            entry.insert(binding);
        }
        std::collections::hash_map::Entry::Occupied(mut entry) => {
            let buffer_id = entry.key().clone();
            let binding = entry.get_mut();
            // Route pane Ctrl+Z/redo through the CRDT history once the
            // doc is authoritative.
            pane.set_doc_history_bound(binding.is_seeded());
            for request in pane.take_doc_history_requests() {
                use neoism_ui::editor::markdown::MarkdownDocHistoryRequest;
                let result = match request {
                    MarkdownDocHistoryRequest::Undo => binding.undo(pane),
                    MarkdownDocHistoryRequest::Redo => binding.redo(pane),
                };
                if let Some(update) = result.flushed_local {
                    state.outbound.push(make_apply_sync(&buffer_id, update));
                }
                if let Some(update) = result.history_update {
                    state.outbound.push(make_apply_sync(&buffer_id, update));
                }
                pane_changed |= result.changed;
            }
            if let Some(update) = binding.flush_local(pane) {
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

enum MarkdownCrdtTargetMut<'a> {
    Markdown(&'a mut MarkdownPane),
    Notebook(&'a mut NotebookPane),
}

impl MarkdownCrdtTargetMut<'_> {
    fn pane_mut(&mut self) -> &mut MarkdownPane {
        match self {
            Self::Markdown(pane) => pane,
            Self::Notebook(notebook) => &mut notebook.markdown,
        }
    }

    fn sync_notebook_if_changed(&mut self, changed: bool) {
        if changed {
            if let Self::Notebook(notebook) = self {
                notebook.sync_order_from_rendered_markdown();
            }
        }
    }
}

fn find_crdt_pane_mut<'a>(
    context_manager: &'a mut crate::context::ContextManager<crate::event::EventProxy>,
    buffer_id: &str,
) -> Option<MarkdownCrdtTargetMut<'a>> {
    for grid in context_manager.contexts_mut() {
        for item in grid.contexts_mut().values_mut() {
            let context = item.context_mut();
            if context
                .markdown
                .as_ref()
                .is_some_and(|pane| buffer_id_for_markdown_path(&pane.path) == buffer_id)
            {
                return context
                    .markdown
                    .as_mut()
                    .map(MarkdownCrdtTargetMut::Markdown);
            }
            if context.notebook.as_ref().is_some_and(|notebook| {
                buffer_id_for_notebook_render_path(&notebook.path) == buffer_id
            }) {
                return context
                    .notebook
                    .as_mut()
                    .map(MarkdownCrdtTargetMut::Notebook);
            }
        }
    }
    None
}

fn find_markdown_pane_mut<'a>(
    context_manager: &'a mut crate::context::ContextManager<crate::event::EventProxy>,
    buffer_id: &str,
) -> Option<&'a mut MarkdownPane> {
    for grid in context_manager.contexts_mut() {
        for item in grid.contexts_mut().values_mut() {
            let context = item.context_mut();
            let matches = context
                .markdown
                .as_ref()
                .is_some_and(|pane| buffer_id_for_markdown_path(&pane.path) == buffer_id);
            if matches {
                return context.markdown.as_mut();
            }
        }
    }
    None
}

fn reload_markdown_pane_from_disk(
    pane: &mut MarkdownPane,
    path: &PathBuf,
    bindings: &mut HashMap<String, MarkdownDocBinding>,
    fingerprints: &mut HashMap<PathBuf, (u64, std::time::SystemTime)>,
) -> std::io::Result<(Option<CrdtClientMessage>, bool)> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => metadata,
        _ => {
            fingerprints.remove(path);
            return Ok((None, false));
        }
    };
    let modified = metadata
        .modified()
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    let fingerprint = (metadata.len(), modified);
    if fingerprints.get(path).copied() == Some(fingerprint) {
        return Ok((None, false));
    }

    let text = fs::read_to_string(path)?;
    let next_lines = markdown_text_to_lines(&text);
    if pane.lines == next_lines {
        fingerprints.insert(path.clone(), fingerprint);
        return Ok((None, false));
    }

    pane.set_source_preserving_view(&text);

    let buffer_id = buffer_id_for_markdown_path(&pane.path);
    let message = bindings
        .get_mut(&buffer_id)
        .and_then(|binding| binding.flush_local(pane))
        .map(|update| make_apply_sync(&buffer_id, update));

    fingerprints.insert(path.clone(), fingerprint);
    Ok((message, true))
}

fn markdown_text_to_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn normalize_markdown_reload_path(path: PathBuf) -> PathBuf {
    fs::canonicalize(&path).unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notebook_render_buffer_id_is_virtual_not_file_buffer() {
        let path = std::path::Path::new("/work/demo.ipynb");

        assert_eq!(buffer_id_for_markdown_path(path), "file:///work/demo.ipynb");
        assert_eq!(
            buffer_id_for_notebook_render_path(path),
            "notebook-render:///work/demo.ipynb"
        );
    }

    #[test]
    fn disk_reload_rebuilds_markdown_render_state() {
        let root = std::env::temp_dir().join(format!(
            "neoism-markdown-disk-reload-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("note.md");
        std::fs::write(&path, "plain text").unwrap();

        let mut pane = MarkdownPane::from_source(path.clone(), "plain text");
        std::fs::write(&path, "# Agent edit\n\n- [ ] visible").unwrap();

        let mut bindings = HashMap::new();
        let mut fingerprints = HashMap::new();
        let (_message, changed) = reload_markdown_pane_from_disk(
            &mut pane,
            &path,
            &mut bindings,
            &mut fingerprints,
        )
        .unwrap();

        assert!(changed);
        assert_eq!(pane.lines, vec!["# Agent edit", "", "- [ ] visible"]);
        assert_eq!(
            pane.blocks,
            vec![
                neoism_ui::editor::markdown::MarkdownBlock::Heading {
                    level: 1,
                    text: "Agent edit".to_string(),
                },
                neoism_ui::editor::markdown::MarkdownBlock::Task {
                    checked: false,
                    text: "visible".to_string(),
                },
            ]
        );
        assert!(!pane.is_dirty());

        let _ = std::fs::remove_dir_all(root);
    }
}
