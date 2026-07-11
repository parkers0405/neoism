//! Screen-side pane layout cache and request queue for the workspace daemon.
//!
//! This deliberately does not materialise daemon snapshots into native
//! `ContextGrid` yet. Desktop rendering/input resources stay local until the
//! daemon starts broadcasting authoritative pane snapshots for every mutation.

#![allow(dead_code)]

use std::collections::VecDeque;

use crate::daemon_client::DaemonClientHandle;
use neoism_protocol::editor::EditorServerMessage;
use neoism_protocol::workspace::{
    PaneFocusDir, PaneLayoutOp, PaneLayoutSnapshot, PaneLayoutSnapshotNode,
    PaneSplitAxis, PaneSplitPlacement, WorkspaceClientMessage, WorkspaceServerMessage,
};

use super::Screen;

const DAEMON_RESIZE_RATIO_STEP: f32 = 0.025;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenPaneLayoutSnapshotSource {
    FullSnapshot,
    PaneLayoutChanged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenPaneLeaf {
    pub pane_external_id: u64,
    pub surface_id: String,
    pub session_id: String,
    pub route_id: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScreenPaneLayoutCache {
    snapshot: Option<PaneLayoutSnapshot>,
    snapshot_source: Option<ScreenPaneLayoutSnapshotSource>,
    leaves: Vec<ScreenPaneLeaf>,
    focused_pane_external_id: Option<u64>,
    last_changed_op: Option<(u64, PaneLayoutOp)>,
    pending_requests: VecDeque<WorkspaceClientMessage>,
}

impl Default for ScreenPaneLayoutCache {
    fn default() -> Self {
        Self {
            snapshot: None,
            snapshot_source: None,
            leaves: Vec::new(),
            focused_pane_external_id: None,
            last_changed_op: None,
            pending_requests: VecDeque::new(),
        }
    }
}

impl ScreenPaneLayoutCache {
    pub fn snapshot(&self) -> Option<&PaneLayoutSnapshot> {
        self.snapshot.as_ref()
    }

    pub fn snapshot_source(&self) -> Option<ScreenPaneLayoutSnapshotSource> {
        self.snapshot_source
    }

    pub fn leaves(&self) -> &[ScreenPaneLeaf] {
        &self.leaves
    }

    pub fn focused_pane_external_id(&self) -> Option<u64> {
        self.focused_pane_external_id
    }

    pub fn last_changed_op(&self) -> Option<(u64, PaneLayoutOp)> {
        self.last_changed_op
    }

    pub fn has_authoritative_snapshot(&self) -> bool {
        self.snapshot.is_some()
    }

    pub fn pending_request_count(&self) -> usize {
        self.pending_requests.len()
    }

    pub fn drain_pending_requests(
        &mut self,
    ) -> impl Iterator<Item = WorkspaceClientMessage> + '_ {
        self.pending_requests.drain(..)
    }

    pub fn apply_full_snapshot(&mut self, layout: Option<PaneLayoutSnapshot>) -> bool {
        self.apply_snapshot(layout, ScreenPaneLayoutSnapshotSource::FullSnapshot)
    }

    pub fn apply_pane_layout_changed(
        &mut self,
        pane_external_id: u64,
        op: PaneLayoutOp,
        new_layout_snapshot: Option<&str>,
    ) -> Result<bool, serde_json::Error> {
        self.last_changed_op = Some((pane_external_id, op));
        let Some(new_layout_snapshot) = new_layout_snapshot else {
            return Ok(false);
        };
        let snapshot = serde_json::from_str::<PaneLayoutSnapshot>(new_layout_snapshot)?;
        Ok(self.apply_snapshot(
            Some(snapshot),
            ScreenPaneLayoutSnapshotSource::PaneLayoutChanged,
        ))
    }

    pub fn queue_request(
        &mut self,
        pane_external_id: u64,
        op: PaneLayoutOp,
    ) -> WorkspaceClientMessage {
        let message = WorkspaceClientMessage::PaneLayoutOp {
            pane_external_id,
            op,
        };
        self.pending_requests.push_back(message.clone());
        message
    }

    fn apply_snapshot(
        &mut self,
        layout: Option<PaneLayoutSnapshot>,
        source: ScreenPaneLayoutSnapshotSource,
    ) -> bool {
        let normalized = layout.map(PaneLayoutSnapshot::normalized);
        let leaves = normalized.as_ref().map(collect_leaves).unwrap_or_default();
        let focused = normalized
            .as_ref()
            .map(|snapshot| snapshot.focused_pane_external_id);

        let changed = self.snapshot != normalized
            || self.snapshot_source != Some(source)
            || self.leaves != leaves
            || self.focused_pane_external_id != focused;

        self.snapshot = normalized;
        self.snapshot_source = Some(source);
        self.leaves = leaves;
        self.focused_pane_external_id = focused;
        changed
    }
}

impl Screen<'_> {
    pub fn attach_daemon_client(
        &mut self,
        handle: DaemonClientHandle,
        runtime: tokio::runtime::Handle,
        endpoint: String,
        link_is_home: bool,
    ) {
        self.context_manager.attach_daemon_client_with_runtime(
            handle,
            runtime,
            endpoint,
            link_is_home,
        );
    }

    pub fn apply_daemon_server_message(
        &mut self,
        message: &WorkspaceServerMessage,
    ) -> bool {
        match message {
            WorkspaceServerMessage::FullSnapshot { layout, .. } => {
                self.apply_daemon_full_snapshot(layout.clone())
            }
            WorkspaceServerMessage::PaneLayoutChanged {
                pane_external_id,
                op,
                new_layout_snapshot,
            } => self.apply_daemon_pane_layout_changed(
                *pane_external_id,
                *op,
                new_layout_snapshot.as_deref(),
            ),
            // Fresh tree data while the Workspaces modal is open —
            // refresh it in place. (The manager cache was updated just
            // before this hook runs; see the ordering in
            // app/mod.rs::apply_daemon_workspace_message.) Without
            // this, the first-ever open raced the async tree request
            // and web/daemon workspaces looked invisible until a
            // re-open.
            WorkspaceServerMessage::HostWorkspaceTree { .. }
            | WorkspaceServerMessage::HostWorkspaceList { .. }
            | WorkspaceServerMessage::HostList { .. } => {
                // Tree pushes refresh the open picker only. Tab strips
                // are PERSONAL per screen (model rule 3): the inventory
                // updates everywhere, but nothing is pushed into a
                // strip — you pull via the picker / adopt at entry.
                self.refresh_open_workspaces_picker()
            }
            _ => false,
        }
    }

    pub fn apply_daemon_full_snapshot(
        &mut self,
        layout: Option<PaneLayoutSnapshot>,
    ) -> bool {
        let changed = self.daemon_pane_layout.apply_full_snapshot(layout);
        if changed {
            self.mark_dirty();
        }
        changed
    }

    pub fn apply_daemon_pane_layout_changed(
        &mut self,
        pane_external_id: u64,
        op: PaneLayoutOp,
        new_layout_snapshot: Option<&str>,
    ) -> bool {
        match self.daemon_pane_layout.apply_pane_layout_changed(
            pane_external_id,
            op,
            new_layout_snapshot,
        ) {
            Ok(changed) => {
                if changed {
                    self.mark_dirty();
                }
                changed
            }
            Err(error) => {
                tracing::warn!(
                    target: "neoism::screen::daemon_layout",
                    %pane_external_id,
                    ?op,
                    %error,
                    "ignored invalid daemon pane layout snapshot"
                );
                false
            }
        }
    }

    pub fn apply_daemon_editor_message(&mut self, message: EditorServerMessage) -> bool {
        let target_surface = editor_message_surface_id(&message);
        let mut applied = false;
        let lsp_log_enabled = std::env::var_os("NEOISM_LSP_LOG").is_some()
            && matches!(
                &message,
                EditorServerMessage::PopupMenu { .. }
                    | EditorServerMessage::PopupMenuSelect { .. }
                    | EditorServerMessage::PopupHide { .. }
                    | EditorServerMessage::Diagnostics { .. }
                    | EditorServerMessage::LspStatus { .. }
                    | EditorServerMessage::LspSnapshot { .. }
                    | EditorServerMessage::LspMessage { .. }
            );

        for grid in self.context_manager.all_grids_mut() {
            let current_route_id = grid.current().route_id;
            for item in grid.contexts_mut().values_mut() {
                let context = item.context_mut();
                if context.editor.is_none() {
                    continue;
                }
                let surface_matches = match (target_surface, context.editor_surface_id())
                {
                    (Some(target), Some(surface)) => target == surface,
                    (None, Some(_)) => current_route_id == context.route_id,
                    _ => false,
                };
                if surface_matches {
                    context.enqueue_daemon_editor_message(message.clone());
                    context.renderable_content.pending_update.set_dirty();
                    applied = true;
                    if lsp_log_enabled {
                        tracing::info!(
                            target: "neoism::lsp",
                            target_surface = ?target_surface,
                            route_id = context.route_id,
                            current_route_id,
                            "applied daemon editor message"
                        );
                    }
                }
            }
        }
        if lsp_log_enabled && !applied {
            tracing::info!(
                target: "neoism::lsp",
                target_surface = ?target_surface,
                "dropped daemon editor message"
            );
        }

        applied
    }

    pub fn drain_daemon_pane_layout_requests(
        &mut self,
    ) -> impl Iterator<Item = WorkspaceClientMessage> + '_ {
        self.daemon_pane_layout.drain_pending_requests()
    }

    /// Drain a queued peer-workspace join: `(workspace_id, daemon_url)`
    /// of a Workspaces-modal pick whose workspace lives on a tailnet
    /// peer's daemon. The app layer re-dials the daemon connection to
    /// that host and adopts the workspace once its tree lands.
    pub fn take_peer_workspace_join(&mut self) -> Option<(String, String)> {
        self.pending_peer_workspace_join.take()
    }

    /// Drain the "left the last joined workspace" flag; the app layer
    /// re-dials the daemon connection back home.
    pub fn take_daemon_go_home(&mut self) -> bool {
        std::mem::take(&mut self.pending_daemon_go_home)
    }

    pub fn request_split_pane(
        &mut self,
        axis: PaneSplitAxis,
        placement: PaneSplitPlacement,
    ) -> WorkspaceClientMessage {
        self.request_pane_layout_op(PaneLayoutOp::Split { axis, placement })
    }

    pub fn request_close_pane(&mut self) -> WorkspaceClientMessage {
        self.request_pane_layout_op(PaneLayoutOp::Close)
    }

    pub fn request_focus_pane(&mut self, dir: PaneFocusDir) -> WorkspaceClientMessage {
        self.request_pane_layout_op(PaneLayoutOp::Focus { dir })
    }

    pub fn request_resize_pane(&mut self, delta: f32) -> WorkspaceClientMessage {
        self.request_pane_layout_op(PaneLayoutOp::ResizeRatio {
            delta: delta.clamp(-0.5, 0.5),
        })
    }

    pub fn request_move_tab(
        &mut self,
        pane_external_id: u64,
        from: u32,
        to: u32,
    ) -> WorkspaceClientMessage {
        self.request_pane_layout_op_for(
            pane_external_id,
            PaneLayoutOp::MoveTab { from, to },
        )
    }

    pub(crate) fn request_resize_pane_step(
        &mut self,
        grow: bool,
    ) -> WorkspaceClientMessage {
        self.request_resize_pane(if grow {
            DAEMON_RESIZE_RATIO_STEP
        } else {
            -DAEMON_RESIZE_RATIO_STEP
        })
    }

    pub(crate) fn request_pane_layout_op(
        &mut self,
        op: PaneLayoutOp,
    ) -> WorkspaceClientMessage {
        let pane_external_id = self.current_pane_external_id();
        self.request_pane_layout_op_for(pane_external_id, op)
    }

    fn request_pane_layout_op_for(
        &mut self,
        pane_external_id: u64,
        op: PaneLayoutOp,
    ) -> WorkspaceClientMessage {
        if self.context_manager.daemon_client_attached() {
            return self.daemon_pane_layout.queue_request(pane_external_id, op);
        }
        WorkspaceClientMessage::PaneLayoutOp {
            pane_external_id,
            op,
        }
    }

    fn current_pane_external_id(&self) -> u64 {
        self.context_manager.current_route() as u64
    }
}

fn editor_message_surface_id(message: &EditorServerMessage) -> Option<&str> {
    match message {
        EditorServerMessage::Batch { surface_id, .. }
        | EditorServerMessage::GridUpdate { surface_id, .. }
        | EditorServerMessage::GridResize { surface_id, .. }
        | EditorServerMessage::GridClear { surface_id, .. }
        | EditorServerMessage::GridScroll { surface_id, .. }
        | EditorServerMessage::CursorGoto { surface_id, .. }
        | EditorServerMessage::HighlightDefined { surface_id, .. }
        | EditorServerMessage::WinViewport { surface_id, .. }
        | EditorServerMessage::DefaultColors { surface_id, .. }
        | EditorServerMessage::PopupMenu { surface_id, .. }
        | EditorServerMessage::PopupMenuSelect { surface_id, .. }
        | EditorServerMessage::PopupHide { surface_id, .. }
        | EditorServerMessage::MouseMode { surface_id, .. }
        | EditorServerMessage::Diagnostics { surface_id, .. }
        | EditorServerMessage::LspStatus { surface_id, .. }
        | EditorServerMessage::LspSnapshot { surface_id, .. }
        | EditorServerMessage::LspMessage { surface_id, .. }
        | EditorServerMessage::LspActionResult { surface_id, .. }
        | EditorServerMessage::LspCompletions { surface_id, .. }
        | EditorServerMessage::LspHoverResult { surface_id, .. }
        | EditorServerMessage::ModeChange { surface_id, .. }
        | EditorServerMessage::BufferOpened { surface_id, .. }
        | EditorServerMessage::BufferModified { surface_id, .. }
        | EditorServerMessage::Message { surface_id, .. }
        | EditorServerMessage::Notification { surface_id, .. }
        | EditorServerMessage::YankFlash { surface_id, .. }
        | EditorServerMessage::Closed { surface_id, .. }
        | EditorServerMessage::Error { surface_id, .. } => surface_id.as_deref(),
    }
}

fn collect_leaves(snapshot: &PaneLayoutSnapshot) -> Vec<ScreenPaneLeaf> {
    let mut leaves = Vec::new();
    collect_node_leaves(&snapshot.root, &mut leaves);
    leaves
}

fn collect_node_leaves(node: &PaneLayoutSnapshotNode, leaves: &mut Vec<ScreenPaneLeaf>) {
    match node {
        PaneLayoutSnapshotNode::Leaf {
            pane_external_id,
            surface_id,
            session_id,
            route_id,
            ..
        } => leaves.push(ScreenPaneLeaf {
            pane_external_id: *pane_external_id,
            surface_id: surface_id.clone(),
            session_id: session_id.clone(),
            route_id: *route_id,
        }),
        PaneLayoutSnapshotNode::Split { children, .. }
        | PaneLayoutSnapshotNode::Tabs { children, .. } => {
            for child in children {
                collect_node_leaves(child, leaves);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(id: u64) -> PaneLayoutSnapshotNode {
        PaneLayoutSnapshotNode::Leaf {
            pane_external_id: id,
            surface_id: format!("surface-{id}"),
            session_id: format!("session-{id}"),
            path: None,
            route_id: Some(id),
        }
    }

    fn snapshot() -> PaneLayoutSnapshot {
        PaneLayoutSnapshot {
            schema_version:
                neoism_protocol::workspace::PANE_LAYOUT_SNAPSHOT_SCHEMA_VERSION,
            workspace_id: "workspace".into(),
            focused_pane_external_id: 2,
            root: PaneLayoutSnapshotNode::Tabs {
                active: 1,
                children: vec![leaf(1), leaf(2)],
            },
        }
    }

    #[test]
    fn full_snapshot_refreshes_leaf_index() {
        let mut cache = ScreenPaneLayoutCache::default();

        assert!(cache.apply_full_snapshot(Some(snapshot())));

        assert_eq!(
            cache.snapshot_source(),
            Some(ScreenPaneLayoutSnapshotSource::FullSnapshot)
        );
        assert_eq!(cache.focused_pane_external_id(), Some(2));
        assert_eq!(
            cache
                .leaves()
                .iter()
                .map(|leaf| leaf.pane_external_id)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[test]
    fn pane_layout_changed_parses_snapshot_json() {
        let mut cache = ScreenPaneLayoutCache::default();
        let json = serde_json::to_string(&snapshot()).unwrap();

        assert!(cache
            .apply_pane_layout_changed(
                1,
                PaneLayoutOp::Focus {
                    dir: PaneFocusDir::Right,
                },
                Some(&json),
            )
            .unwrap());

        assert_eq!(
            cache.snapshot_source(),
            Some(ScreenPaneLayoutSnapshotSource::PaneLayoutChanged)
        );
        assert_eq!(
            cache.last_changed_op(),
            Some((
                1,
                PaneLayoutOp::Focus {
                    dir: PaneFocusDir::Right
                }
            ))
        );
    }

    #[test]
    fn pane_layout_changed_without_snapshot_keeps_current_cache() {
        let mut cache = ScreenPaneLayoutCache::default();
        cache.apply_full_snapshot(Some(snapshot()));

        assert!(!cache
            .apply_pane_layout_changed(2, PaneLayoutOp::Close, None)
            .unwrap());

        assert_eq!(
            cache.snapshot_source(),
            Some(ScreenPaneLayoutSnapshotSource::FullSnapshot)
        );
        assert_eq!(cache.last_changed_op(), Some((2, PaneLayoutOp::Close)));
        assert!(cache.has_authoritative_snapshot());
    }

    #[test]
    fn queued_requests_are_drainable_workspace_messages() {
        let mut cache = ScreenPaneLayoutCache::default();

        cache.queue_request(
            42,
            PaneLayoutOp::Split {
                axis: PaneSplitAxis::Horizontal,
                placement: PaneSplitPlacement::After,
            },
        );

        assert_eq!(cache.pending_request_count(), 1);
        let messages = cache.drain_pending_requests().collect::<Vec<_>>();
        assert_eq!(cache.pending_request_count(), 0);
        assert_eq!(
            messages,
            vec![WorkspaceClientMessage::PaneLayoutOp {
                pane_external_id: 42,
                op: PaneLayoutOp::Split {
                    axis: PaneSplitAxis::Horizontal,
                    placement: PaneSplitPlacement::After,
                },
            }]
        );
    }
}
