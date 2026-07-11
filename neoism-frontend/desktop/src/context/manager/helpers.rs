use crate::context::tab::Context;
use crate::layout::ContextGrid;
use neoism_backend::event::EventListener;
use neoism_backend::event::WindowId;
use neoism_protocol::workspace::PaneLayoutSnapshot;
use neoism_ui::session_layout::{
    session_layout_close_focused_route_pair, session_leaf_external_id, SessionLayout,
    SessionLeafId, SessionLeafKind, SessionLeafSpec, SplitAxis, SplitPlacement,
};
use std::path::PathBuf;

pub(crate) fn session_layout_for_grid<T: EventListener>(
    grid: &ContextGrid<T>,
) -> Option<SessionLayout> {
    if grid.panel_count() <= 1 {
        return None;
    }

    session_layout_mirror_for_grid(grid)
}

#[allow(dead_code)]
pub(crate) fn first_focused_snapshot_session(
    snapshot: &PaneLayoutSnapshot,
) -> Option<String> {
    snapshot_session_for_external_id(&snapshot.root, snapshot.focused_pane_external_id)
        .or_else(|| first_snapshot_session(&snapshot.root))
}

#[allow(dead_code)]
pub(crate) fn snapshot_session_for_external_id(
    node: &neoism_protocol::workspace::PaneLayoutSnapshotNode,
    pane_external_id: u64,
) -> Option<String> {
    use neoism_protocol::workspace::PaneLayoutSnapshotNode;

    match node {
        PaneLayoutSnapshotNode::Leaf {
            pane_external_id: id,
            session_id,
            ..
        } if *id == pane_external_id => Some(session_id.clone()),
        PaneLayoutSnapshotNode::Leaf { .. } => None,
        PaneLayoutSnapshotNode::Split { children, .. }
        | PaneLayoutSnapshotNode::Tabs { children, .. } => children
            .iter()
            .find_map(|child| snapshot_session_for_external_id(child, pane_external_id)),
    }
}

#[allow(dead_code)]
pub(crate) fn first_snapshot_session(
    node: &neoism_protocol::workspace::PaneLayoutSnapshotNode,
) -> Option<String> {
    use neoism_protocol::workspace::PaneLayoutSnapshotNode;

    match node {
        PaneLayoutSnapshotNode::Leaf { session_id, .. } => Some(session_id.clone()),
        PaneLayoutSnapshotNode::Split { children, .. }
        | PaneLayoutSnapshotNode::Tabs { children, .. } => {
            children.iter().find_map(first_snapshot_session)
        }
    }
}

pub(crate) fn session_layout_mirror_for_grid<T: EventListener>(
    grid: &ContextGrid<T>,
) -> Option<SessionLayout> {
    if grid.panel_count() == 0 {
        return None;
    }

    let ordered_nodes = grid.get_ordered_keys();
    let mut ordered_specs = ordered_nodes.iter().filter_map(|node| {
        let item = grid.contexts().get(node)?;
        Some((*node, session_leaf_spec_for_context(item.context())))
    });

    let (_, first_spec) = ordered_specs.next()?;
    let mut layout = SessionLayout::new(first_spec);
    let mut leaves_by_route = Vec::new();
    if let Some(route) = layout
        .leaf(layout.focused_leaf())
        .and_then(|leaf| leaf.external_id)
    {
        leaves_by_route.push((route as usize, layout.focused_leaf()));
    }

    for (_, spec) in ordered_specs {
        let route = spec.external_id?;
        let leaf = layout
            .split_focused(SplitAxis::Vertical, SplitPlacement::After, spec)
            .ok()?;
        leaves_by_route.push((route as usize, leaf));
    }

    let current_route = grid.contexts().get(&grid.current)?.context().route_id;
    let current_leaf = leaves_by_route
        .iter()
        .find_map(|(route, leaf)| (*route == current_route).then_some(*leaf))?;
    layout.focus_leaf(current_leaf).ok()?;
    layout.validate().ok()?;
    Some(layout)
}

pub(crate) fn session_layout_close_current_grid_route<T: EventListener>(
    grid: &ContextGrid<T>,
) -> Option<(usize, usize)> {
    let layout = session_layout_for_grid(grid)?;
    let (closing, focus) = session_layout_close_focused_route_pair(&layout)?;
    Some((closing as usize, focus as usize))
}

pub(crate) fn unix_timestamp_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

/// One host = one MACHINE: same resolution as the daemon's
/// `machine_host_id` (env override, else the real hostname), so a
/// desktop and the daemon on the same machine publish into ONE host
/// group instead of an artificial desktop/local split. Two machines
/// (a Mac attaching over the tailnet) differ naturally by hostname.
pub(crate) fn desktop_host_id(_window_id: WindowId) -> String {
    std::env::var("NEOISM_HOST_ID")
        .ok()
        .filter(|id| !id.trim().is_empty())
        .unwrap_or_else(desktop_host_label)
}

pub(crate) fn desktop_host_label() -> String {
    std::env::var("NEOISM_HOST_LABEL")
        .ok()
        .filter(|label| !label.trim().is_empty())
        .unwrap_or_else(|| {
            let host = gethostname::gethostname().to_string_lossy().into_owned();
            if host.trim().is_empty() {
                "local".to_string()
            } else {
                host
            }
        })
}

pub(crate) fn desktop_workspace_id<T: EventListener>(
    window_id: WindowId,
    grid: &ContextGrid<T>,
    fallback_index: usize,
) -> String {
    let stable = grid.workspace_route_id().unwrap_or(fallback_index);
    format!(
        "{}-window-{}-workspace-{stable}",
        desktop_host_id(window_id),
        u64::from(window_id)
    )
}

pub(crate) fn desktop_tab_id(window_id: WindowId, route_id: usize) -> String {
    format!(
        "{}-window-{}-route-{route_id}",
        desktop_host_id(window_id),
        u64::from(window_id)
    )
}

pub(crate) fn desktop_daemon_url() -> Option<String> {
    std::env::var("NEOISM_HOST_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| tailscale_daemon_url(7878))
}

pub(crate) fn tailscale_daemon_url(port: u16) -> Option<String> {
    // Non-blocking cached probe: the tailscale CLI stalls ~2s when
    // tailscaled is down, and this runs on the main thread inside
    // every workspace sync (file open / tab switch / workspace
    // switch). See crate::tailscale.
    let ip = crate::tailscale::cached_ipv4()?;
    Some(format!("ws://{ip}:{port}/session"))
}

pub(crate) fn context_title_or_fallback<T: EventListener>(
    context: &Context<T>,
    route_id: usize,
) -> String {
    context
        .terminal
        .try_lock_unfair()
        .map(|terminal| terminal.title.trim().to_string())
        .filter(|title| !title.is_empty())
        .or_else(|| {
            context
                .editor_path
                .as_ref()
                .and_then(|path| path.file_name())
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| format!("Route {route_id}"))
}

pub(crate) fn context_workspace_tab_kind_and_path<T: EventListener>(
    context: &Context<T>,
    fallback_root: Option<PathBuf>,
) -> (String, Option<PathBuf>) {
    if let Some(path) = context.editor_path.clone() {
        return ("editor".to_string(), Some(path));
    }
    if let Some(markdown) = context.markdown.as_ref() {
        return ("markdown".to_string(), Some(markdown.path.clone()));
    }
    if let Some(draw) = context.draw.as_ref() {
        return ("drawing".to_string(), Some(draw.path.clone()));
    }
    if let Some(notebook) = context.notebook.as_ref() {
        return ("notebook".to_string(), Some(notebook.path.clone()));
    }
    ("terminal".to_string(), fallback_root)
}

pub(crate) fn session_leaf_spec_for_context<T: EventListener>(
    context: &Context<T>,
) -> SessionLeafSpec {
    let kind = if context.editor.is_some() {
        SessionLeafKind::Editor
    } else if context.neoism_agent.is_some() {
        SessionLeafKind::Agent
    } else if context.markdown.is_some() {
        SessionLeafKind::Custom("markdown".to_string())
    } else if context.notebook.is_some() {
        SessionLeafKind::Custom("notebook".to_string())
    } else if context.draw.is_some() {
        SessionLeafKind::Custom("draw".to_string())
    } else if context.neoism_tags.is_some() {
        SessionLeafKind::Custom("tags".to_string())
    } else if context.neoism_extensions.is_some() {
        SessionLeafKind::Custom("extensions".to_string())
    } else {
        SessionLeafKind::Terminal
    };

    SessionLeafSpec::new(kind).with_external_id(context.route_id as u64)
}

pub(crate) fn session_leaf_route(
    layout: &SessionLayout,
    leaf: SessionLeafId,
) -> Option<usize> {
    session_leaf_external_id(layout, leaf).map(|route| route as usize)
}
