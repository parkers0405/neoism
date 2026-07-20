// Copyright (c) 2023-present, Raphael Amorim.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! Palette actions and surface-aware visibility filtering.

use crate::widgets::modal::{ModalAction, ModalButton, ModalSpec};

/// Actions that can be triggered from the command palette.
///
/// Almost every variant is a fieldless one-shot, but `MoveWorkspaceToHost`
/// carries an owned `String` payload (it's produced at runtime by the
/// Workspaces drag gesture, never stored in the static `COMMANDS`
/// catalog). That payload makes the enum `Clone` rather than `Copy`;
/// the few sites that previously moved a `PaletteAction` out of a shared
/// reference now `.clone()` it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteAction {
    TabCreate,
    TabClose,
    TabCloseUnfocused,
    SelectNextTab,
    SelectPrevTab,
    SplitRight,
    SplitDown,
    SelectNextSplit,
    SelectPrevSplit,
    ConfigEditor,
    WindowCreateNew,
    IncreaseFontSize,
    DecreaseFontSize,
    ResetFontSize,
    ToggleViMode,
    ToggleFullscreen,
    ToggleAppearanceTheme,
    OpenThemePicker,
    OpenShaders,
    OpenMashupPacks,
    Copy,
    Paste,
    SaveDocument,
    RunNotebookCell,
    RunNotebookCellAndBelow,
    RunAllNotebookCells,
    InsertNotebookCodeCellAbove,
    InsertNotebookCodeCellBelow,
    InsertNotebookMarkdownCellAbove,
    InsertNotebookMarkdownCellBelow,
    DeleteNotebookCell,
    MoveNotebookCellUp,
    MoveNotebookCellDown,
    InterruptNotebookKernel,
    ClearNotebookCellOutput,
    ClearNotebookOutputs,
    RestartNotebookKernel,
    SearchForward,
    SearchBackward,
    /// Switch the palette into `:` ex mode so the user can type a bare
    /// line number — the existing `:N` dispatch jumps there.
    GoToLine,
    /// Open the finder in document-symbols quick-jump mode — the same
    /// mode typing `@` as the first char of the Ctrl+P query enters.
    GoToSymbol,
    SearchFiles,
    SearchWords,
    SearchGitChanges,
    ToggleGitDiffPanel,
    CreateNeoismNote,
    DrawOnNote,
    OpenNeoismNotes,
    LspHover,
    LspCodeAction,
    LspFormat,
    LspDefinition,
    LspReferences,
    LspRename,
    LspDocumentSymbols,
    LspWorkspaceSymbols,
    ToggleInlayHints,
    ToggleMinimap,
    ClearHistory,
    CloseCurrentSplitOrTab,
    /// Browse the family names of every registered font. Does NOT
    /// execute a one-shot action — the palette stays open with the
    /// font list as its contents. Handled by `router`, not
    /// `Screen::execute_palette_action`.
    ListFonts,
    /// Browse open buffers/tabs in the current workspace. Handled by
    /// router/screen because it needs live workspace state.
    ListBuffers,
    /// Open the web workplace switcher. Desktop has native window/workspace
    /// chrome; web owns this action at the host layer.
    ShowWorkplaces,
    ShowServers,
    SelectServer {
        id: String,
    },
    EditServer {
        id: String,
    },
    RemoveServer {
        id: String,
    },
    AddServer,
    /// Host a server from THIS machine for a chosen folder and
    /// auto-join it — the create half of the Create/Join pair.
    CreateServer,
    CreateWorkspace,
    ShareCurrentWorkspace,
    StopSharingCurrentWorkspace,
    /// Leave a JOINED (adopted-from-another-host) workspace: detach
    /// from the host's sessions — never killing them — close the tab,
    /// and re-dial the home daemon when it was the last joined one.
    LeaveWorkspace,
    SendCurrentWorkspaceToDockerSandbox,
    SendCurrentWorkspaceToCloud,
    /// Open a fresh terminal tab and launch one of the agent CLIs in
    /// it. If the binary isn't on PATH the dispatcher first opens the
    /// install modal and runs the install in the background.
    OpenNeoismAgent,
    RunClaude,
    RunCodex,
    RunOpenCode,
    /// Move a workspace to a different host. Emitted by the Workspaces
    /// modal's drag gesture (5D-drag) when a workspace row is dropped
    /// onto a host header. This is a pure *intent* — it carries the
    /// source workspace id and the chosen target host's identity, but
    /// does NOT itself talk to any daemon.
    ///
    /// 5D-wire (parent-owned) maps this to the real move route:
    ///   - `target_is_local == false` → POST `/workspace/promote` on the
    ///     local daemon with the target host's HTTP base as `target_url`,
    ///     moving the workspace's home onto that remote host.
    ///   - `target_is_local == true`  → POST `/workspace/demote`,
    ///     pulling the workspace's home back to this local machine.
    /// The promote/demote split is owned by 5D-wire; the gesture layer
    /// only fills in the fields below.
    MoveWorkspaceToHost {
        /// Source workspace being moved (the dragged row's id).
        workspace_id: String,
        /// Stable id of the host the workspace is being dropped onto.
        target_host_id: String,
        /// The target host's client-dialable daemon endpoint, if known.
        /// `None` for the local host (and any host whose address hasn't
        /// been published yet). 5D-wire derives the promote `target_url`
        /// from this.
        target_daemon_url: Option<String>,
        /// `true` when the drop target is the Local host (`HostKind::Local`)
        /// → 5D-wire treats it as a demote. `false` → promote to remote.
        target_is_local: bool,
    },
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteBufferTarget {
    Workspace(usize),
    Pane { route_id: usize, tab_index: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteSurface {
    Terminal,
    Editor,
    Markdown,
    Notebook,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteBufferEntry {
    pub title: String,
    pub detail: String,
    pub target: PaletteBufferTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteWorkspaceTarget {
    pub workspace_id: String,
}

/// What sort of machine a host is, for the host header row's icon +
/// affordances. `Local` is this machine (no `daemon_url` shown);
/// `Remote` is a peer on the tailnet/LAN reachable by `daemon_url`;
/// `Cloud` is an ephemeral burst daemon. The kind only drives the
/// header glyph + whether the `daemon_url` is surfaced — selection and
/// switching are identical across kinds.
/// One server shown by the shared command palette's Servers mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteServerEntry {
    pub id: String,
    pub name: String,
    pub address: String,
    pub local: bool,
    pub status: crate::panels::ServerIndicatorStatus,
    pub active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKind {
    Local,
    Remote,
    Cloud,
}

impl HostKind {
    /// Header glyph by kind. Local `⌂` (house = this machine), Remote
    /// `💻` (a peer laptop), Cloud `☁`. Drawn as plain text in the
    /// header row's icon slot, mirroring how file_tree paints its
    /// folder glyph.
    pub fn icon(self) -> &'static str {
        match self {
            HostKind::Local => "\u{2302}",   // ⌂
            HostKind::Remote => "\u{1f4bb}", // 💻
            HostKind::Cloud => "\u{2601}",   // ☁
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceHostKind {
    Local,
    Tailscale,
    DockerSandbox,
    CloudSandbox,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceVisibility {
    Private,
    Shared,
    Team,
}

impl WorkspaceHostKind {
    pub fn icon_override(self, visibility: WorkspaceVisibility) -> Option<&'static str> {
        match self {
            WorkspaceHostKind::CloudSandbox => Some("☁"),
            WorkspaceHostKind::DockerSandbox => Some("⬢"),
            WorkspaceHostKind::Tailscale => Some("◌"),
            WorkspaceHostKind::Local => match visibility {
                WorkspaceVisibility::Private => None,
                WorkspaceVisibility::Shared | WorkspaceVisibility::Team => Some("󰒗"),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteWorkspaceEntry {
    pub title: String,
    pub detail: String,
    pub target: PaletteWorkspaceTarget,
    /// Stable id of the host that owns this workspace. Workspaces with
    /// the same `host_id` group under one header, in first-seen order.
    pub host_id: String,
    /// Human label for the host header (e.g. `framework`, `mac`).
    pub host_label: String,
    /// Host machine kind — drives the header glyph + whether the
    /// `daemon_url` is surfaced.
    pub host_kind: HostKind,
    pub workspace_host_kind: WorkspaceHostKind,
    pub workspace_visibility: WorkspaceVisibility,
    /// Client-dialable daemon endpoint for the host. Shown in the
    /// header for non-local hosts; `None` for the local host (and any
    /// host whose address isn't known yet).
    pub daemon_url: Option<String>,
    /// Whether the owning host is currently reachable. Drives the
    /// online dot (`●` vs `○`) in the header.
    pub host_online: bool,
    /// Whether this is the workspace the window is CURRENTLY viewing —
    /// drawn as a left accent stripe, mirroring the active-server marker.
    pub current: bool,
}

impl PaletteWorkspaceEntry {
    /// Build an entry that lives under a single implicit Local host.
    /// Used by callers that don't yet have a real host tree (the flat
    /// "old dropdown" behavior) so existing call sites degrade
    /// gracefully to one `⌂ local` group.
    ///
    /// 5D-data seam: once `enter_workspaces_mode` is fed a real
    /// `HostWorkspaceTree` / `/tailnet-peers` payload, callers should
    /// populate the host fields directly instead of going through this.
    pub fn local(title: String, detail: String, workspace_id: String) -> Self {
        Self {
            title,
            detail,
            target: PaletteWorkspaceTarget { workspace_id },
            host_id: "local".to_string(),
            host_label: "local".to_string(),
            host_kind: HostKind::Local,
            workspace_host_kind: WorkspaceHostKind::Local,
            workspace_visibility: WorkspaceVisibility::Private,
            daemon_url: None,
            host_online: true,
            current: false,
        }
    }
}

/// A drop-target-only host row for the Workspaces tree (Wave 6A).
///
/// Hosts in the tree are normally synthesized from the workspaces they
/// own (`PaletteWorkspaceEntry`'s host fields), so a machine with zero
/// workspaces never gets a header — and can't be a drag target. This
/// entry represents exactly such a machine: a discovered tailnet peer
/// (or any future workspace-less host) that should still render as a
/// host header so a workspace can be dragged onto it to promote it
/// there. Mirrors the host fields of [`PaletteWorkspaceEntry`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteHostEntry {
    /// Stable id for the header row (drag drop-target identity). Peer
    /// hosts use a `tailnet:`-prefixed id so they can never collide
    /// with daemon host ids.
    pub host_id: String,
    /// Human label for the header (e.g. the peer's tailnet hostname).
    pub label: String,
    /// Header glyph kind. Tailnet peers are `Remote`.
    pub kind: HostKind,
    /// Client-dialable daemon endpoint (`ws://<ip>:7878/session` for a
    /// tailnet peer). Dropping a workspace on this header promotes it
    /// to this URL; `None` makes the header informational-only.
    pub daemon_url: Option<String>,
    /// Reachability — offline hosts render dimmed (`○`) and are not
    /// valid drop targets.
    pub online: bool,
}

pub const WORKSPACE_ROOT_DETAIL_PREFIX: &str = "workspace root · ";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PaletteShaderEntry {
    pub title: String,
    pub detail: String,
    pub filter: Option<String>,
}

pub fn theme_picker_modal_spec() -> ModalSpec {
    let themes = [
        ("pastel_dark", "Pastel Dark", "Neoism default"),
        ("nvchad_one", "NvChad One", "Base46-style"),
        ("tokyo_night", "Tokyo Night", "Blue/purple"),
        ("catppuccin_mocha", "Catppuccin Mocha", "Warm pastel"),
    ];
    let mut buttons: Vec<_> = themes
        .iter()
        .map(|(name, label, hint)| {
            ModalButton::new(
                *label,
                *hint,
                ModalAction::ApplyTheme {
                    name: (*name).to_string(),
                },
            )
        })
        .collect();
    // Runtime themes (ide-themes/*.toml + Mash Up Pack themes) after
    // the builtin four, same action.
    for (name, description) in crate::primitives::ide_theme::custom_ide_theme_entries() {
        let hint = if description.is_empty() {
            "Custom theme".to_string()
        } else {
            description
        };
        buttons.push(ModalButton::new(
            name.clone(),
            hint,
            ModalAction::ApplyTheme { name },
        ));
    }
    buttons.push(ModalButton::new("Close", "Esc", ModalAction::Close));

    ModalSpec {
        title: "Theme Picker".to_string(),
        body: "Pick a unified IDE theme. This applies live to Neoism chrome, terminal defaults, and the editor syntax palette.".to_string(),
        meta: "Enter applies, Esc closes.".to_string(),
        input: None,
        buttons,
        busy: false,
        blocking: true,
    }
}

pub fn shaders_modal_spec<I, S>(configured_shaders: I) -> ModalSpec
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let shaders: Vec<String> = configured_shaders
        .into_iter()
        .map(|shader| shader.as_ref().to_string())
        .collect();
    let body = if shaders.is_empty() {
        "No shader choices are configured. Add paths under [renderer] shader-overlays to make shaders selectable here.".to_string()
    } else {
        "Pick a shader overlay. This applies live to the rendered Neoism frame."
            .to_string()
    };
    let mut buttons = Vec::with_capacity(shaders.len() + 2);
    buttons.push(ModalButton::new(
        "None",
        "Disable shader overlay",
        ModalAction::ApplyShaderOverlay { path: None },
    ));
    buttons.extend(shaders.iter().map(|shader| {
        let label = shader
            .rsplit(['/', '\\'])
            .next()
            .and_then(|name| name.strip_prefix("builtin:").or(Some(name)))
            .map(|name| name.strip_suffix(".glsl").unwrap_or(name))
            .filter(|name| !name.is_empty())
            .unwrap_or(shader.as_str());
        ModalButton::new(
            label,
            "Apply shader overlay",
            ModalAction::ApplyShaderOverlay {
                path: Some(shader.clone()),
            },
        )
    }));
    buttons.push(ModalButton::new("Close", "Esc", ModalAction::Close));

    ModalSpec {
        title: "Shaders".to_string(),
        body,
        meta: "Enter applies the selected overlay, None disables it.".to_string(),
        input: None,
        buttons,
        busy: false,
        blocking: true,
    }
}

/// One installed Mash Up Pack, as the host lists it for the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteMashupEntry {
    /// Stable id (directory name under `packs/`).
    pub id: String,
    /// Display name from the manifest.
    pub name: String,
    /// Manifest description plus the slots the pack ships.
    pub detail: String,
}

pub fn mashup_packs_modal_spec(packs: Vec<PaletteMashupEntry>) -> ModalSpec {
    let body = if packs.is_empty() {
        "No Mash Up Packs installed. Drop a pack folder (pack.toml + theme/shader/fonts) under the config dir's packs/ to see it here.".to_string()
    } else {
        "Pick a Mash Up Pack — theme, shader, and fonts applied together as one look. Each slot stays individually changeable afterwards (Theme Picker, Shaders, [fonts]).".to_string()
    };
    let mut buttons = Vec::with_capacity(packs.len() + 2);
    buttons.push(ModalButton::new(
        "None",
        "Deactivate pack (keeps current theme, clears its shader)",
        ModalAction::ApplyMashupPack { id: None },
    ));
    buttons.extend(packs.into_iter().map(|pack| {
        ModalButton::new(
            pack.name,
            pack.detail,
            ModalAction::ApplyMashupPack { id: Some(pack.id) },
        )
    }));
    buttons.push(ModalButton::new("Close", "Esc", ModalAction::Close));

    ModalSpec {
        title: "Mash Up Packs".to_string(),
        body,
        meta: "Enter applies the whole pack, None deactivates.".to_string(),
        input: None,
        buttons,
        busy: false,
        blocking: true,
    }
}

pub(crate) fn command_visible_for_surface(
    action: &PaletteAction,
    surface: PaletteSurface,
) -> bool {
    match action {
        // Vi mode toggles the code pane's vim input when a code buffer
        // owns focus, and the terminal's scrollback vi mode otherwise.
        PaletteAction::ToggleViMode => {
            matches!(surface, PaletteSurface::Terminal | PaletteSurface::Editor)
        }
        PaletteAction::ClearHistory => surface == PaletteSurface::Terminal,
        PaletteAction::SaveDocument => {
            matches!(
                surface,
                PaletteSurface::Editor | PaletteSurface::Markdown | PaletteSurface::Notebook
            )
        }
        PaletteAction::RunNotebookCell
        | PaletteAction::RunNotebookCellAndBelow
        | PaletteAction::RunAllNotebookCells
        | PaletteAction::InsertNotebookCodeCellAbove
        | PaletteAction::InsertNotebookCodeCellBelow
        | PaletteAction::InsertNotebookMarkdownCellAbove
        | PaletteAction::InsertNotebookMarkdownCellBelow
        | PaletteAction::DeleteNotebookCell
        | PaletteAction::MoveNotebookCellUp
        | PaletteAction::MoveNotebookCellDown
        | PaletteAction::InterruptNotebookKernel
        | PaletteAction::ClearNotebookCellOutput
        | PaletteAction::ClearNotebookOutputs
        | PaletteAction::RestartNotebookKernel => {
            surface == PaletteSurface::Notebook
        }
        PaletteAction::SearchForward | PaletteAction::SearchBackward => {
            !matches!(surface, PaletteSurface::Markdown | PaletteSurface::Notebook)
        }
        // Line/symbol jumps only make sense when the focused pane
        // hosts a code buffer.
        PaletteAction::GoToLine | PaletteAction::GoToSymbol => {
            surface == PaletteSurface::Editor
        }
        PaletteAction::LspHover
        | PaletteAction::LspCodeAction
        | PaletteAction::LspFormat
        | PaletteAction::LspDefinition
        | PaletteAction::LspReferences
        | PaletteAction::LspRename
        | PaletteAction::LspDocumentSymbols
        | PaletteAction::LspWorkspaceSymbols
        | PaletteAction::ToggleInlayHints
        | PaletteAction::ToggleMinimap => surface == PaletteSurface::Editor,
        PaletteAction::TabCreate
        | PaletteAction::TabClose
        | PaletteAction::TabCloseUnfocused
        | PaletteAction::SelectNextTab
        | PaletteAction::SelectPrevTab
        | PaletteAction::SplitRight
        | PaletteAction::SplitDown
        | PaletteAction::SelectNextSplit
        | PaletteAction::SelectPrevSplit
        | PaletteAction::ConfigEditor
        | PaletteAction::WindowCreateNew
        | PaletteAction::IncreaseFontSize
        | PaletteAction::DecreaseFontSize
        | PaletteAction::ResetFontSize
        | PaletteAction::ToggleFullscreen
        | PaletteAction::ToggleAppearanceTheme
        | PaletteAction::OpenThemePicker
        | PaletteAction::OpenShaders
        | PaletteAction::OpenMashupPacks
        | PaletteAction::Copy
        | PaletteAction::Paste
        | PaletteAction::SearchFiles
        | PaletteAction::SearchWords
        | PaletteAction::SearchGitChanges
        | PaletteAction::ToggleGitDiffPanel
        | PaletteAction::CreateNeoismNote
        | PaletteAction::DrawOnNote
        | PaletteAction::OpenNeoismNotes
        | PaletteAction::CloseCurrentSplitOrTab
        | PaletteAction::ListFonts
        | PaletteAction::ListBuffers
        | PaletteAction::ShowWorkplaces
        | PaletteAction::ShowServers
        | PaletteAction::SelectServer { .. }
        | PaletteAction::EditServer { .. }
        | PaletteAction::RemoveServer { .. }
        | PaletteAction::AddServer
        | PaletteAction::CreateServer
        | PaletteAction::CreateWorkspace
        | PaletteAction::ShareCurrentWorkspace
        | PaletteAction::StopSharingCurrentWorkspace
        | PaletteAction::LeaveWorkspace
        | PaletteAction::SendCurrentWorkspaceToDockerSandbox
        | PaletteAction::SendCurrentWorkspaceToCloud
        | PaletteAction::OpenNeoismAgent
        | PaletteAction::RunClaude
        | PaletteAction::RunCodex
        | PaletteAction::RunOpenCode
        // Runtime-only action — never lives in the static COMMANDS
        // catalog, so its surface visibility is moot, but the match
        // must stay exhaustive.
        | PaletteAction::MoveWorkspaceToHost { .. }
        | PaletteAction::Quit => true,
    }
}
