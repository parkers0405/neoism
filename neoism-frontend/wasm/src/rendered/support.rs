use super::*;
use neoism_ui::primitives::IdeTheme;
use neoism_ui::render_policy::{
    block_status_color_token, loader_animation_frame, loader_orbit_position,
    loader_pastel_color, BlockStatusColorToken,
};
use neoism_ui::terminal_blocks::BlockStatusKind;
use sugarloaf::Sugarloaf;

pub(crate) fn intersect_rect(a: [f32; 4], b: [f32; 4]) -> Option<[f32; 4]> {
    let ax2 = a[0] + a[2].max(0.0);
    let ay2 = a[1] + a[3].max(0.0);
    let bx2 = b[0] + b[2].max(0.0);
    let by2 = b[1] + b[3].max(0.0);
    let x1 = a[0].max(b[0]);
    let y1 = a[1].max(b[1]);
    let x2 = ax2.min(bx2);
    let y2 = ay2.min(by2);
    (x2 > x1 && y2 > y1).then_some([x1, y1, x2 - x1, y2 - y1])
}

pub(crate) fn block_status_color(theme: IdeTheme, status: BlockStatusKind) -> [u8; 4] {
    match block_status_color_token(status) {
        BlockStatusColorToken::Yellow => theme.u8(theme.yellow),
        BlockStatusColorToken::Green => theme.u8(theme.green),
        BlockStatusColorToken::Red => theme.u8(theme.red),
    }
}

pub(crate) fn draw_running_block_loader_web(
    sugarloaf: &mut Sugarloaf<'static>,
    left: f32,
    row_top: f32,
    cell_h: f32,
    font_size: f32,
    animation_phase: f32,
    clip_rect: [f32; 4],
) -> f32 {
    let slot_w = (font_size * 1.08).max(12.0);
    let side = (font_size * 0.86).min(cell_h * 0.74).max(10.0);
    let half = side * 0.5;
    let dot = (side * 0.34).clamp(3.5, 5.8);
    let center_x = left + slot_w * 0.5;
    let center_y = row_top + cell_h * 0.5;
    let loader_frame = loader_animation_frame(animation_phase);

    for (trail, alpha) in [1.0, 0.58, 0.32, 0.16].into_iter().enumerate() {
        let (dx, dy) =
            loader_orbit_position(loader_frame.phase - trail as f32 * 0.075, half);
        let x = center_x + dx - dot * 0.5;
        let y = center_y + dy - dot * 0.5;
        let dot_rect = [x, y, dot, dot];
        if intersect_rect(dot_rect, clip_rect).is_none() {
            continue;
        }
        sugarloaf.rounded_rect(
            None,
            x,
            y,
            dot,
            dot,
            loader_pastel_color(loader_frame.tick, trail, alpha),
            0.0,
            dot * 0.5,
            5,
        );
    }

    slot_w
}

/// Stable string name for each `PaletteAction` variant. Matches the
/// Rust enum variant name so TS can `switch` on it directly. Kept
/// out of `actions.rs` because `neoism-ui` (and the desktop
/// frontend) deliberately don't serialize this enum — desktop
/// dispatches via a direct method call.
pub(crate) fn palette_action_name(
    action: neoism_ui::panels::command_palette::PaletteAction,
) -> &'static str {
    use neoism_ui::panels::command_palette::PaletteAction as A;
    match action {
        // Server-manager actions are desktop-only today; the web host
        // has no server picker, so these names are inert labels.
        A::ShowServers => "ShowServers",
        A::SelectServer { .. } => "SelectServer",
        A::EditServer { .. } => "EditServer",
        A::RemoveServer { .. } => "RemoveServer",
        A::AddServer => "AddServer",
        A::CreateServer => "CreateServer",
        A::GoToSymbol => "GoToSymbol",
        A::TabCreate => "TabCreate",
        A::TabClose => "TabClose",
        A::TabCloseUnfocused => "TabCloseUnfocused",
        A::SelectNextTab => "SelectNextTab",
        A::SelectPrevTab => "SelectPrevTab",
        A::SplitRight => "SplitRight",
        A::SplitDown => "SplitDown",
        A::SelectNextSplit => "SelectNextSplit",
        A::SelectPrevSplit => "SelectPrevSplit",
        A::ConfigEditor => "ConfigEditor",
        A::WindowCreateNew => "WindowCreateNew",
        A::IncreaseFontSize => "IncreaseFontSize",
        A::DecreaseFontSize => "DecreaseFontSize",
        A::ResetFontSize => "ResetFontSize",
        A::ToggleViMode => "ToggleViMode",
        A::ToggleFullscreen => "ToggleFullscreen",
        A::ToggleAppearanceTheme => "ToggleAppearanceTheme",
        A::OpenThemePicker => "OpenThemePicker",
        A::OpenShaders => "OpenShaders",
        A::OpenMashupPacks => "OpenMashupPacks",
        A::Copy => "Copy",
        A::Paste => "Paste",
        A::SaveDocument => "SaveDocument",
        A::RunNotebookCell => "RunNotebookCell",
        A::RunNotebookCellAndBelow => "RunNotebookCellAndBelow",
        A::RunAllNotebookCells => "RunAllNotebookCells",
        A::InsertNotebookCodeCellAbove => "InsertNotebookCodeCellAbove",
        A::InsertNotebookCodeCellBelow => "InsertNotebookCodeCellBelow",
        A::InsertNotebookMarkdownCellAbove => "InsertNotebookMarkdownCellAbove",
        A::InsertNotebookMarkdownCellBelow => "InsertNotebookMarkdownCellBelow",
        A::DeleteNotebookCell => "DeleteNotebookCell",
        A::MoveNotebookCellUp => "MoveNotebookCellUp",
        A::MoveNotebookCellDown => "MoveNotebookCellDown",
        A::InterruptNotebookKernel => "InterruptNotebookKernel",
        A::ClearNotebookCellOutput => "ClearNotebookCellOutput",
        A::ClearNotebookOutputs => "ClearNotebookOutputs",
        A::RestartNotebookKernel => "RestartNotebookKernel",
        A::SearchForward => "SearchForward",
        A::SearchBackward => "SearchBackward",
        A::GoToLine => "GoToLine",
        A::SearchFiles => "SearchFiles",
        A::SearchWords => "SearchWords",
        A::SearchGitChanges => "SearchGitChanges",
        A::ToggleGitDiffPanel => "ToggleGitDiffPanel",
        A::CreateNeoismNote => "CreateNeoismNote",
        A::OpenNeoismNotes => "OpenNeoismNotes",
        A::LspHover => "LspHover",
        A::LspCodeAction => "LspCodeAction",
        A::LspFormat => "LspFormat",
        A::LspDefinition => "LspDefinition",
        A::LspReferences => "LspReferences",
        A::LspRename => "LspRename",
        A::LspDocumentSymbols => "LspDocumentSymbols",
        A::LspWorkspaceSymbols => "LspWorkspaceSymbols",
        A::ToggleInlayHints => "ToggleInlayHints",
        A::ToggleMinimap => "ToggleMinimap",
        A::ClearHistory => "ClearHistory",
        A::CloseCurrentSplitOrTab => "CloseCurrentSplitOrTab",
        A::ListFonts => "ListFonts",
        A::ListBuffers => "ListBuffers",
        A::ShowWorkplaces => "ShowWorkplaces",
        A::CreateWorkspace => "CreateWorkspace",
        A::OpenNeoismAgent => "OpenNeoismAgent",
        A::RunClaude => "RunClaude",
        A::RunCodex => "RunCodex",
        A::RunOpenCode => "RunOpenCode",
        A::DrawOnNote => "DrawOnNote",
        A::ShareCurrentWorkspace => "ShareCurrentWorkspace",
        A::StopSharingCurrentWorkspace => "StopSharingCurrentWorkspace",
        A::LeaveWorkspace => "LeaveWorkspace",
        A::SendCurrentWorkspaceToDockerSandbox => "SendCurrentWorkspaceToDockerSandbox",
        A::SendCurrentWorkspaceToCloud => "SendCurrentWorkspaceToCloud",
        A::MoveWorkspaceToHost { .. } => "MoveWorkspaceToHost",
        A::Quit => "Quit",
    }
}

/// Yrs origin id for this browser's markdown edits: random,
/// non-zero, masked into Yjs's safe-integer space — the same
/// constraints as the desktop's client-id generator. Zero is
/// reserved as the snapshot-replay origin sentinel.
pub(crate) fn generate_crdt_client_id() -> u64 {
    loop {
        let id =
            (js_sys::Math::random() * 9_007_199_254_740_992.0) as u64 & ((1 << 53) - 1);
        if id != 0 {
            return id;
        }
    }
}

/// Wrap a local replica update in the `ApplySync` wire envelope —
/// mirrors the desktop's `make_apply_sync` (markdown_crdt.rs).
pub(crate) fn make_crdt_apply_sync(
    buffer_id: &str,
    update: neoism_ui::editor::crdt::CrdtTextUpdate,
) -> neoism_protocol::crdt::CrdtClientMessage {
    neoism_protocol::crdt::CrdtClientMessage::ApplySync {
        envelope: neoism_protocol::crdt::CrdtSyncEnvelope {
            buffer_id: buffer_id.to_string(),
            origin_client_id: update.origin_client_id,
            update_v1: update.update_v1,
            state_vector_v1: update.state_vector_v1,
        },
    }
}

/// Parse the JSON Workspaces-modal payload shared by
/// `open_workspaces_palette` and `refresh_workspaces_palette`.
/// See `open_workspaces_palette`'s doc comment for the shape.
pub(crate) fn parse_workspaces_payload(
    payload_json: &str,
) -> Result<
    (
        Vec<neoism_ui::panels::command_palette::PaletteWorkspaceEntry>,
        Vec<neoism_ui::panels::command_palette::PaletteHostEntry>,
    ),
    JsValue,
> {
    use neoism_ui::panels::command_palette::{
        HostKind, PaletteHostEntry, PaletteWorkspaceEntry, PaletteWorkspaceTarget,
        WorkspaceHostKind, WorkspaceVisibility,
    };

    fn parse_kind(kind: Option<&str>) -> HostKind {
        match kind {
            Some("local") => HostKind::Local,
            Some("cloud") => HostKind::Cloud,
            _ => HostKind::Remote,
        }
    }

    fn parse_workspace_host_kind(kind: Option<&str>) -> WorkspaceHostKind {
        match kind {
            Some("tailscale") => WorkspaceHostKind::Tailscale,
            Some("docker_sandbox") => WorkspaceHostKind::DockerSandbox,
            Some("cloud_sandbox") => WorkspaceHostKind::CloudSandbox,
            _ => WorkspaceHostKind::Local,
        }
    }

    fn parse_workspace_visibility(value: Option<&str>) -> WorkspaceVisibility {
        match value {
            Some("shared") => WorkspaceVisibility::Shared,
            Some("team") => WorkspaceVisibility::Team,
            _ => WorkspaceVisibility::Private,
        }
    }

    #[derive(serde::Deserialize)]
    struct JsWorkspace {
        title: String,
        #[serde(default)]
        detail: String,
        workspace_id: String,
        #[serde(default)]
        host_id: Option<String>,
        #[serde(default)]
        host_label: Option<String>,
        #[serde(default)]
        host_kind: Option<String>,
        #[serde(default)]
        workspace_host_kind: Option<String>,
        #[serde(default)]
        workspace_visibility: Option<String>,
        #[serde(default)]
        daemon_url: Option<String>,
        #[serde(default)]
        host_online: Option<bool>,
    }

    #[derive(serde::Deserialize)]
    struct JsPeerHost {
        host_id: String,
        label: String,
        #[serde(default)]
        kind: Option<String>,
        #[serde(default)]
        daemon_url: Option<String>,
        #[serde(default)]
        online: Option<bool>,
    }

    #[derive(serde::Deserialize)]
    struct JsPayload {
        #[serde(default)]
        workspaces: Vec<JsWorkspace>,
        #[serde(default)]
        peer_hosts: Vec<JsPeerHost>,
    }

    let parsed: JsPayload = serde_json::from_str(payload_json)
        .map_err(|e| JsValue::from_str(&format!("workspaces parse: {e}")))?;

    let entries: Vec<PaletteWorkspaceEntry> = parsed
        .workspaces
        .into_iter()
        .map(|w| {
            let host_kind = parse_kind(w.host_kind.as_deref());
            PaletteWorkspaceEntry {
                title: w.title,
                detail: w.detail,
                target: PaletteWorkspaceTarget {
                    workspace_id: w.workspace_id,
                },
                host_id: w.host_id.unwrap_or_else(|| "local".to_string()),
                host_label: w.host_label.unwrap_or_else(|| "local".to_string()),
                host_kind,
                workspace_host_kind: parse_workspace_host_kind(
                    w.workspace_host_kind.as_deref(),
                ),
                workspace_visibility: parse_workspace_visibility(
                    w.workspace_visibility.as_deref(),
                ),
                daemon_url: w.daemon_url,
                host_online: w.host_online.unwrap_or(true),
                // The web host doesn't track a per-window viewed
                // workspace yet; no row gets the current-place stripe.
                current: false,
            }
        })
        .collect();
    let peer_hosts: Vec<PaletteHostEntry> = parsed
        .peer_hosts
        .into_iter()
        .map(|h| PaletteHostEntry {
            host_id: h.host_id,
            label: h.label,
            kind: parse_kind(h.kind.as_deref()),
            daemon_url: h.daemon_url,
            online: h.online.unwrap_or(false),
        })
        .collect();
    Ok((entries, peer_hosts))
}
