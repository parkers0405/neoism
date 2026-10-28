use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use web_time::{Duration, Instant};

use sugarloaf::text::DrawOpts;
use sugarloaf::{GraphicOverlay, Sugarloaf};

use crate::panels::agent_pane::state::NeoismAgentImage;
use crate::panels::agent_pane::view::image_preview::register_image;

use crate::editor::markdown::MarkdownPane;
use crate::editor::neodraw::{render_scene, Camera, Vec2};
use crate::primitives::{draw_icon_centered_with_occlusion, IdeTheme};
use crate::syntax::{highlight_line, syn_color, Lang};
use crate::widgets::mermaid::{mermaid_scene, parse_mermaid_diagram, MermaidDiagram};
use crate::widgets::scrollbar;

mod body;

const MODAL_WIDTH: f32 = 560.0;
const MODAL_CORNER_RADIUS: f32 = 13.0;
const MODAL_MARGIN_TOP: f32 = 92.0;
const MODAL_PADDING: f32 = 18.0;
const TITLE_FONT_SIZE: f32 = 15.0;
const BODY_FONT_SIZE: f32 = 12.0;
const META_FONT_SIZE: f32 = 11.0;
const ACTION_FONT_SIZE: f32 = 13.0;
const ACTION_ROW_HEIGHT: f32 = 32.0;
const INPUT_ROW_HEIGHT: f32 = 34.0;
const FORM_SWITCH_ROW_HEIGHT: f32 = 36.0;
const FORM_SWITCH_FOCUS: usize = usize::MAX;
const MARKDOWN_INPUT_HEIGHT: f32 = 180.0;
const INPUT_FONT_SIZE: f32 = 13.0;
const BODY_LINE_HEIGHT: f32 = 19.0;
const TITLE_HEIGHT: f32 = 24.0;
const SEPARATOR_HEIGHT: f32 = 1.0;
const BUSY_BAR_HEIGHT: f32 = 4.0;
const BUSY_BAR_WIDTH: f32 = 120.0;
const MAX_BODY_LINES: usize = 14;
const MAX_VISIBLE_ACTIONS: usize = 8;
const HOVER_ANIMATION_DURATION: Duration = Duration::from_millis(150);

#[allow(dead_code)]
const DEPTH_BACKDROP: f32 = 0.0;
const DEPTH_BG: f32 = 0.1;
const DEPTH_ELEMENT: f32 = 0.2;
const ORDER: u8 = 24;
// Placements belong to the modal, not the thumbnail strip. Clear this owner on
// every render, including the first inactive frame after dismissal.
const IMAGE_OVERLAY_OWNER: usize = 0x4D4F_4449;

/// Center the complete image inside a spacious logical-pixel viewport. Invalid
/// dimensions yield no placement rather than NaNs in the graphics pipeline.
fn image_aspect_fit(viewport: [f32; 2], aspect: f32, scale: f32) -> Option<[f32; 4]> {
    let [w, h] = viewport;
    if !w.is_finite() || !h.is_finite() || w <= 0.0 || h <= 0.0
        || !aspect.is_finite() || aspect <= 0.0 || !scale.is_finite() || scale <= 0.0
    {
        return None;
    }
    let mx = (48.0 * scale).min(w * 0.12);
    let my = (72.0 * scale).min(h * 0.22);
    let width = (w - 2.0 * mx).min((h - 2.0 * my) * aspect);
    let height = width / aspect;
    (width > 0.0 && height > 0.0).then_some([
        (w - width) * 0.5, (h - height) * 0.5, width, height,
    ])
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModalAction {
    Close,
    OpenUrl { url: String },
    CopyInvite { url: String },
    UpdateNeoism {
        version: String,
    },
    InstallLsp {
        server: String,
    },
    InstallPythonKernel,
    ApplyTheme {
        name: String,
    },
    ApplyShaderOverlay {
        path: Option<String>,
    },
    /// Apply a Mash Up Pack (theme + shader + fonts as one look).
    /// `None` deactivates the current pack, keeping the theme.
    ApplyMashupPack {
        id: Option<String>,
    },
    RunEditorCommand {
        command: String,
    },
    RunEditorCommandWithInput {
        command: String,
        value: String,
    },
    LuaPromptReply {
        request_id: String,
        value: String,
        cancelled: bool,
    },
    MarkdownFileLink {
        document: std::path::PathBuf,
        value: String,
    },
    DocumentationNotebook {
        kind: crate::editor::documentation_notebook::NotebookInput,
        notebook: Option<std::path::PathBuf>,
        value: String,
    },
    /// Add a durable EPUB highlight carrying the note entered in the modal.
    EpubAddNote {
        value: String,
    },
    EpubGoTo {
        href: String,
    },
    EpubOpenContents,
    EpubOpenChapterPages {
        href: String,
    },
    EpubGoToPage {
        href: String,
        page: usize,
    },
    EpubOpenAnnotation {
        id: String,
    },
    EpubGoToAnnotation {
        id: String,
    },
    EpubEditAnnotation {
        id: String,
    },
    EpubUpdateAnnotation {
        id: String,
        value: String,
    },
    EpubDeleteAnnotation {
        id: String,
    },
    EpubOpenAnnotationCollections {
        id: String,
    },
    EpubToggleAnnotationCollection {
        annotation_id: String,
        collection_id: String,
    },
    EpubPromptNewAnnotationCollection {
        annotation_id: String,
    },
    EpubCreateAnnotationCollection {
        annotation_id: String,
        value: String,
    },
    OpenLspLocation {
        uri: String,
        line: u32,
        character: u32,
    },
    ApplyLspCodeAction {
        action: neoism_protocol::editor::EditorLspCodeAction,
    },
    /// Retry an agent CLI install (claude / codex / opencode).
    /// `kind` matches `AgentKind::id()`.
    InstallAgent {
        kind: String,
    },
    /// Once an agent install succeeds, the success modal offers a
    /// "Launch" button that fires this action — opens a new tab and
    /// runs the binary.
    RunAgent {
        kind: String,
    },
    AcpPermission {
        server_id: String,
        request_id: u64,
        option_id: Option<String>,
    },
    FileTreeEdit {
        path: String,
    },
    FileTreeCopy {
        path: String,
    },
    FileTreePaste {
        dest_dir: String,
    },
    FileTreePromptDelete {
        path: String,
        notes: bool,
    },
    GitRestore { root: std::path::PathBuf, path: String },
    GitRestoreBlock {
        root: std::path::PathBuf,
        path: String,
        block: neoism_protocol::git::GitChangeBlock,
    },
    FileTreeDelete {
        path: String,
        notes: bool,
    },
    FileTreePromptNewFile {
        dir: String,
    },
    NotesNewNotebook {
        dir: String,
    },
    NotesPromptNewFile {
        dir: String,
    },
    FileTreePromptNewFolder {
        dir: String,
    },
    FileTreePromptRename {
        path: String,
        notes: bool,
    },
    FileTreeNewFile {
        dir: String,
        name: String,
    },
    NotesNewFile {
        dir: String,
        name: String,
    },
    /// Create a fresh `.neodraw` in `dir` (the vault the notes sidebar is
    /// viewing) and open it — dispatched from the sidebar's ⋮ create menu.
    NotesNewDrawing {
        dir: String,
    },
    /// Prompt for a custom icon glyph for the notes entry at `path`.
    NotesPromptIcon {
        path: String,
    },
    /// Persist `icon` (emoji/any glyph; empty = reset to default) for the
    /// notes entry at `path` in the vault's `.neoism-icons.json`.
    NotesSetIcon {
        path: String,
        icon: String,
    },
    FileTreeNewFolder {
        dir: String,
        name: String,
    },
    FileTreeRename {
        path: String,
        name: String,
        notes: bool,
    },
    /// Rename the buffer tab at `index` to `name` (filled from the modal input).
    /// This does not rename the persisted conversation.
    AgentRenameSession {
        session_id: String,
        server: String,
        directory: Option<String>,
        name: String,
    },
    AgentDeleteSession {
        session_id: String,
        server: String,
        directory: Option<String>,
    },
    RenameTab {
        index: usize,
        agent_session_id: Option<String>,
        name: String,
    },
    NotesVaultPromptAdd,
    SystemFormSubmit,
    /// Confirm deactivation without losing the durable enablement intent.
    SystemDeactivateConfirm { id: String },
    SystemDisconnectConfirm {
        id: String,
        remove: bool,
    },
    ServerFormSubmit,
    ServerReconnectRetry {
        endpoint: String,
    },
    ServerReconnectDiscard {
        endpoint: String,
    },
    ServerReconnectLeave {
        endpoint: String,
    },
    ServerRemoveConfirm {
        id: String,
    },
    NotesVaultAdd {
        name: String,
    },
    NotesVaultPromptRename {
        vault: Option<String>,
    },
    NotesVaultRename {
        old_path: Option<String>,
        name: String,
    },
    NotesVaultSwitch {
        name: String,
    },
    /// Switch the notes panel to the HOST's shared vault for the current
    /// joined workspace (the one linked-vault section of the selector).
    NotesVaultSwitchShared,
    NotesVaultOpenVaultsRoot,
    NotesVaultLinkCurrentWorkspace {
        /// Optional absolute folder inside a vault. `None` uses the notes
        /// sidebar's current root (the whole vault for legacy callers).
        notes_dir: Option<String>,
    },
    NotesVaultPromptLinkProject {
        vault: String,
        notes_dir: Option<String>,
    },
    NotesVaultLinkProject {
        vault: String,
        path: String,
        notes_dir: Option<String>,
    },
    NotesVaultConvert {
        source: String,
        dest_dir: String,
    },
    /// Render every note in `vault` to a reMarkable document bundle and
    /// push it to the tablet (a "Neoism" folder of writable pages).
    NotesVaultShareWithRemarkable {
        vault: String,
    },
}

impl ModalAction {
    fn with_input(self, value: String) -> Self {
        match self {
            ModalAction::FileTreeNewFile { dir, .. } => {
                ModalAction::FileTreeNewFile { dir, name: value }
            }
            ModalAction::NotesNewFile { dir, .. } => {
                ModalAction::NotesNewFile { dir, name: value }
            }
            ModalAction::NotesSetIcon { path, .. } => {
                ModalAction::NotesSetIcon { path, icon: value }
            }
            ModalAction::FileTreeNewFolder { dir, .. } => {
                ModalAction::FileTreeNewFolder { dir, name: value }
            }
            ModalAction::FileTreeRename { path, notes, .. } => {
                ModalAction::FileTreeRename {
                    path,
                    name: value,
                    notes,
                }
            }
            ModalAction::AgentRenameSession {
                session_id,
                server,
                directory,
                ..
            } => ModalAction::AgentRenameSession {
                session_id,
                server,
                directory,
                name: value,
            },
            ModalAction::RenameTab {
                index,
                agent_session_id,
                ..
            } => ModalAction::RenameTab {
                index,
                agent_session_id,
                name: value,
            },
            ModalAction::NotesVaultAdd { .. } => {
                ModalAction::NotesVaultAdd { name: value }
            }
            ModalAction::NotesVaultRename { old_path, .. } => {
                ModalAction::NotesVaultRename {
                    old_path,
                    name: value,
                }
            }
            ModalAction::NotesVaultLinkProject {
                vault, notes_dir, ..
            } => ModalAction::NotesVaultLinkProject {
                vault,
                path: value,
                notes_dir,
            },
            ModalAction::RunEditorCommandWithInput { command, .. } => {
                ModalAction::RunEditorCommandWithInput { command, value }
            }
            ModalAction::LuaPromptReply {
                request_id,
                cancelled,
                ..
            } => ModalAction::LuaPromptReply {
                request_id,
                value,
                cancelled,
            },
            ModalAction::MarkdownFileLink { document, .. } => {
                ModalAction::MarkdownFileLink { document, value }
            }
            ModalAction::DocumentationNotebook { kind, notebook, .. } => {
                ModalAction::DocumentationNotebook {
                    kind,
                    notebook,
                    value,
                }
            }
            ModalAction::EpubAddNote { .. } => ModalAction::EpubAddNote { value },
            ModalAction::EpubUpdateAnnotation { id, .. } => {
                ModalAction::EpubUpdateAnnotation { id, value }
            }
            ModalAction::EpubCreateAnnotationCollection { annotation_id, .. } => {
                ModalAction::EpubCreateAnnotationCollection {
                    annotation_id,
                    value,
                }
            }
            action => action,
        }
    }

    fn is_destructive(&self) -> bool {
        matches!(
            self,
            ModalAction::FileTreePromptDelete { .. }
                | ModalAction::FileTreeDelete { .. }
                | ModalAction::GitRestore { .. }
                | ModalAction::GitRestoreBlock { .. }
                | ModalAction::EpubDeleteAnnotation { .. }
                | ModalAction::ServerReconnectDiscard { .. }
        )
    }
}

#[derive(Clone, Debug)]
pub struct ModalButton {
    pub label: String,
    pub hint: String,
    pub action: ModalAction,
}

impl ModalButton {
    pub fn new(
        label: impl Into<String>,
        hint: impl Into<String>,
        action: ModalAction,
    ) -> Self {
        Self {
            label: label.into(),
            hint: hint.into(),
            action,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ModalSpec {
    pub title: String,
    pub body: String,
    pub meta: String,
    pub input: Option<ModalInputSpec>,
    pub buttons: Vec<ModalButton>,
    pub busy: bool,
    pub blocking: bool,
}

#[derive(Clone, Debug)]
pub struct ModalInputSpec {
    pub value: String,
    pub placeholder: String,
}

#[derive(Clone, Debug)]
pub struct ModalFormField {
    pub id: String,
    pub label: String,
    pub value: String,
    pub placeholder: String,
    pub secret: bool,
}

#[derive(Clone, Debug)]
pub struct ModalFormSpec {
    pub title: String,
    pub fields: Vec<ModalFormField>,
    pub submit_label: String,
}

/// Optional modal-level boolean control; text fields remain unchanged.
#[derive(Clone, Debug)]
struct ModalFormSwitch {
    label: String,
    checked: bool,
    enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BodyLineKind {
    Text,
    Heading,
    Bullet,
    Code(Lang),
    Mermaid,
}

#[derive(Clone, Debug)]
struct BodyLine {
    text: String,
    kind: BodyLineKind,
    mermaid: Option<MermaidDiagram>,
    row_span: usize,
    links: Vec<BodyLink>,
    continuation: bool,
}

#[derive(Clone, Debug)]
struct BodyLink {
    range: std::ops::Range<usize>,
    url: String,
}

#[derive(Clone, Debug)]
struct BodyLayout {
    source: String,
    width: f32,
    scale: f32,
    font_probe: (f32, f32),
    lines: Vec<BodyLine>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ModalHoverTarget {
    Close,
    Invite,
    Link(usize),
    Tab(usize),
    Switch,
    Input,
    Field(usize),
    Action(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModalPointerKind {
    Default,
    Pointer,
    Text,
}

impl BodyLine {
    fn new(text: String, kind: BodyLineKind) -> Self {
        Self {
            text,
            kind,
            mermaid: None,
            row_span: 1,
            links: Vec::new(),
            continuation: false,
        }
    }
}

/// One form layout shared by paint, hit testing, scrolling and focus reveal.
#[derive(Clone, Copy, Debug)]
struct FormGeometry {
    panel: [f32; 4],
    body: [f32; 4],
    actions: [f32; 4],
    action_row_height: f32,
    switch: Option<[f32; 4]>,
    switch_in_body: bool,
    compact: bool,
    scale: f32,
    pad: f32,
    gap: f32,
    content_height: f32,
    fields_start: f32,
}

impl FormGeometry {
    fn max_scroll(self) -> f32 {
        (self.content_height - self.body[3]).max(0.0)
    }
    fn field_row(self, ordinal: usize, scroll: f32) -> [f32; 4] {
        [
            self.body[0],
            self.body[1] + self.fields_start + ordinal as f32 * 64.0 * self.scale
                - scroll,
            self.body[2],
            54.0 * self.scale,
        ]
    }
    fn field_input(self, ordinal: usize, scroll: f32) -> [f32; 4] {
        let mut row = self.field_row(ordinal, scroll);
        row[1] += 20.0 * self.scale;
        row[3] = INPUT_ROW_HEIGHT * self.scale;
        row
    }
}

/// Identity and cancellation shared by a modal and its background operation.
#[derive(Clone, Debug, Default)]
pub struct ModalActivity(Arc<AtomicBool>);

impl ModalActivity {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
}

impl PartialEq for ModalActivity {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for ModalActivity {}

#[derive(Debug)]
pub struct UniversalModal {
    active: bool,
    image: Option<NeoismAgentImage>,
    image_rect: Option<[f32; 4]>,
    title: String,
    body: String,
    body_layout: Option<BodyLayout>,
    body_link: Option<(String, String)>,
    body_link_hit_rects: Vec<(String, [f32; 4])>,
    invite_url: Option<String>,
    invite_hit_rects: Vec<[f32; 4]>,
    meta: String,
    buttons: Vec<ModalButton>,
    input: Option<ModalInputSpec>,
    form: Option<ModalFormSpec>,
    form_focus: usize,
    /// Segmented mode slider labels (empty = none) + active segment;
    /// rects recorded at render for click hit-testing.
    form_tabs: Vec<String>,
    form_active_tab: usize,
    form_tab_rects: Vec<[f32; 4]>,
    form_tabs_enabled: bool,
    form_switch: Option<ModalFormSwitch>,
    form_switch_rect: Option<[f32; 4]>,
    form_scroll: f32,
    form_compact: bool,
    form_reveal_pending: bool,
    /// Caret position in CHARS within the focused input (single input or
    /// focused form field). Reset to end-of-value on open and focus change.
    input_caret: usize,
    markdown_input: bool,
    markdown_pane: Option<MarkdownPane>,
    markdown_input_rect: Option<[f32; 4]>,
    submitted_form: Option<Vec<(String, String)>>,
    selected_index: usize,
    scroll_offset: usize,
    body_scroll_offset: usize,
    body_wheel_accumulator: f32,
    scale: f32,
    busy: bool,
    activity: Option<ModalActivity>,
    progress: Option<u8>,
    blocking: bool,
    dismissible: bool,
    opened_at: Instant,
    top_anchor: f32,
    /// Confirmed modal outcomes awaiting a host that cannot execute
    /// side effects inline (the web chrome). Queued by the chrome's
    /// modal dispatch (`chrome/pages.rs`) and drained by the host
    /// bridge; survives `close()` so a "confirm then close" flow
    /// hands the action over even though the modal is already gone.
    pending_host_actions: Vec<ModalHostAction>,
    viewport_height: f32,
    input_hit_rects: Vec<[f32; 4]>,
    form_field_hit_rects: Vec<(usize, [f32; 4])>,
    close_rect: Option<[f32; 4]>,
    hovered_target: Option<ModalHoverTarget>,
    previous_hover_target: Option<ModalHoverTarget>,
    hover_started: Option<Instant>,
}

/// A modal outcome the CHROME could validate but not execute — the
/// host owns the side effect (daemon files op, LSP rename request).
/// This is the web twin of the arms in desktop
/// `Screen::execute_modal_action` that touch the filesystem / LSP:
/// the chrome validates input exactly like desktop, closes the modal
/// per the shared dispatch policy, and queues one of these for the
/// host bridge to drain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModalHostAction {
    /// Create `name` under `dir` (validated: relative, non-escaping).
    /// Mirrors `ModalAction::FileTreeNewFile`.
    FileTreeNewFile { dir: String, name: String },
    /// Create folder `name` under `dir`. Mirrors
    /// `ModalAction::FileTreeNewFolder`.
    FileTreeNewFolder { dir: String, name: String },
    /// Rename the entry at `path` to `name` (same folder; relative
    /// sub-paths allowed, desktop parity). Mirrors
    /// `ModalAction::FileTreeRename`.
    FileTreeRename { path: String, name: String },
    /// Delete the entry at `path` (confirmed through the destructive
    /// modal). Mirrors `ModalAction::FileTreeDelete`.
    FileTreeDelete { path: String },
    /// LSP symbol rename confirmed with a non-empty `name` — the twin
    /// of desktop's `submit_code_rename` entered through the
    /// `code_rename_to` form.
    LspRename { name: String },
    /// Spec-driven generic outcome: a host-defined `id` plus the
    /// modal input's value (empty for plain confirm buttons). Lets
    /// hosts drive arbitrary confirm/input flows (vault + workspace
    /// ops as their wire paths land) without growing this enum.
    Generic { id: String, value: String },
}

fn draw_form_rect(
    sugarloaf: &mut (impl crate::primitives::paint::Paint + ?Sized),
    rect: [f32; 4],
    clip: [f32; 4],
    color: [f32; 4],
    radius: f32,
    order: u8,
) {
    if let Some(r) = crate::primitives::geom::intersect_rect(rect, clip) {
        sugarloaf.rounded_rect(
            None,
            r[0],
            r[1],
            r[2],
            r[3],
            color,
            DEPTH_ELEMENT,
            radius.min(r[2] * 0.5).min(r[3] * 0.5),
            order,
        );
    }
}

impl Default for UniversalModal {
    fn default() -> Self {
        Self {
            active: false,
            image: None,
            image_rect: None,
            title: String::new(),
            body: String::new(),
            body_layout: None,
            body_link: None,
            body_link_hit_rects: Vec::new(),
            invite_url: None,
            invite_hit_rects: Vec::new(),
            meta: String::new(),
            buttons: Vec::new(),
            input: None,
            form: None,
            form_focus: 0,
            form_tabs: Vec::new(),
            form_active_tab: 0,
            form_tab_rects: Vec::new(),
            form_tabs_enabled: true,
            form_switch: None,
            form_switch_rect: None,
            form_scroll: 0.0,
            form_compact: false,
            form_reveal_pending: true,
            input_caret: 0,
            markdown_input: false,
            markdown_pane: None,
            markdown_input_rect: None,
            submitted_form: None,
            selected_index: 0,
            scroll_offset: 0,
            body_scroll_offset: 0,
            body_wheel_accumulator: 0.0,
            scale: 1.0,
            busy: false,
            activity: None,
            progress: None,
            blocking: true,
            dismissible: true,
            opened_at: Instant::now(),
            top_anchor: MODAL_MARGIN_TOP,
            pending_host_actions: Vec::new(),
            viewport_height: 720.0,
            input_hit_rects: Vec::new(),
            form_field_hit_rects: Vec::new(),
            close_rect: None,
            hovered_target: None,
            previous_hover_target: None,
            hover_started: None,
        }
    }
}

impl Drop for UniversalModal {
    fn drop(&mut self) {
        if let Some(activity) = &self.activity {
            activity.cancel();
        }
    }
}

impl UniversalModal {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn set_scale(&mut self, scale: f32) {
        let scale = scale.clamp(0.5, 3.0);
        if self.scale != scale {
            self.form_reveal_pending = true;
            self.body_layout = None;
            self.body_link_hit_rects.clear();
        }
        self.scale = scale;
    }

    pub fn set_top_anchor(&mut self, top: f32) {
        self.top_anchor = top.max(0.0);
    }

    pub fn close(&mut self) {
        if let Some(activity) = self.activity.take() {
            activity.cancel();
        }
        self.active = false;
        self.image = None;
        self.image_rect = None;
        self.body_link = None;
        self.body_layout = None;
        self.body_link_hit_rects.clear();
        self.buttons.clear();
        self.input = None;
        self.markdown_input = false;
        self.markdown_pane = None;
        self.markdown_input_rect = None;
        self.form = None;
        self.form_tabs.clear();
        self.form_tab_rects.clear();
        self.form_active_tab = 0;
        self.form_tabs_enabled = true;
        self.form_switch = None;
        self.form_switch_rect = None;
        self.form_scroll = 0.0;
        self.form_compact = false;
        self.form_reveal_pending = true;
        self.submitted_form = None;
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.body_scroll_offset = 0;
        self.body_wheel_accumulator = 0.0;
        self.busy = false;
        self.progress = None;
        self.blocking = true;
        self.dismissible = true;
        self.input_hit_rects.clear();
        self.form_field_hit_rects.clear();
        self.close_rect = None;
        self.hovered_target = None;
        self.previous_hover_target = None;
        self.hover_started = None;
    }

    pub fn open(&mut self, spec: ModalSpec) {
        if let Some(activity) = self.activity.take() {
            activity.cancel();
        }
        self.active = true;
        self.image = None;
        self.image_rect = None;
        self.invite_url = None;
        self.invite_hit_rects.clear();
        self.title = spec.title;
        self.body = spec.body;
        self.body_link = None;
        self.body_layout = None;
        self.body_link_hit_rects.clear();
        self.meta = spec.meta;
        self.input = spec.input;
        self.form = None;
        self.form_tabs.clear();
        self.form_tab_rects.clear();
        self.form_active_tab = 0;
        self.form_tabs_enabled = true;
        self.form_switch = None;
        self.form_switch_rect = None;
        self.form_scroll = 0.0;
        self.form_compact = false;
        self.form_reveal_pending = true;
        self.submitted_form = None;
        self.buttons = if spec.buttons.is_empty() {
            vec![ModalButton::new("Close", "Esc", ModalAction::Close)]
        } else {
            spec.buttons
        };
        self.markdown_input = matches!(
            self.buttons.first().map(|button| &button.action),
            Some(
                ModalAction::EpubUpdateAnnotation { .. }
                    | ModalAction::MarkdownFileLink { .. }
                    | ModalAction::DocumentationNotebook { .. }
                    | ModalAction::EpubAddNote { .. }
            )
        );
        self.markdown_pane = if self.markdown_input {
            let source = self
                .input
                .as_ref()
                .map(|input| input.value.as_str())
                .unwrap_or_default();
            let mut pane = MarkdownPane::from_source(std::path::PathBuf::new(), source);
            pane.embedded = true;
            pane.enter_insert();
            pane.cursor_line = pane.lines.len().saturating_sub(1);
            pane.cursor_col = pane.lines.last().map(String::len).unwrap_or(0);
            Some(pane)
        } else {
            None
        };
        self.markdown_input_rect = None;
        self.busy = spec.busy;
        self.progress = None;
        self.blocking = spec.blocking;
        self.dismissible = true;
        self.opened_at = Instant::now();
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.body_scroll_offset = 0;
        self.body_wheel_accumulator = 0.0;
        self.input_caret = self.focused_input_chars();
        self.input_hit_rects.clear();
        self.form_field_hit_rects.clear();
        self.close_rect = None;
        self.hovered_target = None;
        self.previous_hover_target = None;
        self.hover_started = None;
    }

    /// Full-resolution inspection. Hosts route thumbnail clicks here and keep
    /// calling `render` after close so retained GPU placements are cleared.
    pub fn open_image(&mut self, image: NeoismAgentImage) {
        self.open(ModalSpec {
            title: image.filename.to_string(),
            body: String::new(),
            meta: String::new(),
            input: None,
            buttons: vec![ModalButton::new("Close", "Esc", ModalAction::Close)],
            busy: false,
            blocking: true,
        });
        self.image = Some(image);
    }

    pub fn clear_image_overlay(&self, sugarloaf: &mut Sugarloaf) {
        sugarloaf.clear_image_overlays_for(IMAGE_OVERLAY_OWNER);
    }

    pub fn is_image_open(&self) -> bool {
        self.active && self.image.is_some()
    }

    pub fn open_form(&mut self, spec: ModalFormSpec) {
        self.open_form_with_action(spec, ModalAction::ServerFormSubmit);
    }

    pub fn open_form_with_action(&mut self, spec: ModalFormSpec, action: ModalAction) {
        self.open(ModalSpec {
            title: spec.title.clone(),
            body: String::new(),
            meta: String::new(),
            buttons: vec![
                ModalButton::new(spec.submit_label.clone(), "Enter", action),
                ModalButton::new("Cancel", "Esc", ModalAction::Close),
            ],
            input: None,
            busy: false,
            blocking: true,
        });
        self.form = Some(spec);
        self.form_tabs.clear();
        self.form_tab_rects.clear();
        self.form_active_tab = 0;
        self.form_tabs_enabled = true;
        self.form_switch = None;
        self.form_switch_rect = None;
        self.form_scroll = 0.0;
        self.form_compact = false;
        self.form_reveal_pending = true;
        self.form_focus = self
            .form
            .as_ref()
            .and_then(|form| form.fields.iter().position(|field| !field.label.is_empty()))
            .unwrap_or(0);
        self.submitted_form = None;
        self.input_caret = self.focused_input_chars();
    }

    pub fn take_submitted_form(&mut self) -> Option<Vec<(String, String)>> {
        self.submitted_form.take()
    }

    /// Queue a confirmed outcome for the host bridge. Used by hosts
    /// whose side effects live across an FFI boundary (web/wasm) —
    /// desktop executes its `ModalAction`s inline instead.
    pub fn queue_host_action(&mut self, action: ModalHostAction) {
        self.pending_host_actions.push(action);
    }

    /// Drain every queued [`ModalHostAction`] (oldest first). Safe to
    /// call after `close()` — confirm-then-close flows queue before
    /// closing and the queue survives the close.
    pub fn take_host_actions(&mut self) -> Vec<ModalHostAction> {
        std::mem::take(&mut self.pending_host_actions)
    }

    /// True when at least one host action is waiting to be drained.
    pub fn has_host_actions(&self) -> bool {
        !self.pending_host_actions.is_empty()
    }

    /// Dedicated ready dialog: copy the capability URL rather than opening it.
    pub fn open_fast_server_invite(&mut self, url: String) {
        self.open(ModalSpec {
            title: "Fast Server ready".into(),
            body: format!("Invite a collaborator with this link and your password:\n\n{url}\n\nStop hosting ends this temporary session; project files are kept."),
            meta: "Click the link to copy the invite".into(),
            input: None,
            buttons: vec![ModalButton::new("Copy invite", "Enter", ModalAction::CopyInvite { url: url.clone() }),
                ModalButton::new("Close", "Esc", ModalAction::Close)],
            busy: false,
            blocking: true,
        });
        self.invite_url = Some(url);
    }

    pub fn invite_action_at(&self, x: f32, y: f32) -> Option<ModalAction> {
        if !self.active || !self.invite_hit_rects.iter().any(|r|
            x >= r[0] && x <= r[0] + r[2] && y >= r[1] && y <= r[1] + r[3]) {
            return None;
        }
        self.invite_url.clone().map(|url| ModalAction::CopyInvite { url })
    }

    pub fn mark_invite_copied(&mut self) {
        self.meta = "Invite copied".into();
    }

    pub(crate) fn take_invite_copy(&mut self) -> Option<String> {
        let index = self.pending_host_actions.iter().position(|action|
            matches!(action, ModalHostAction::Generic { id, .. } if id == "copy-invite"))?;
        match self.pending_host_actions.remove(index) {
            ModalHostAction::Generic { value, .. } => Some(value),
            _ => None,
        }
    }

    pub fn open_message(&mut self, title: impl Into<String>, body: impl Into<String>) {
        self.open(ModalSpec {
            title: title.into(),
            body: body.into(),
            meta: String::new(),
            input: None,
            buttons: vec![ModalButton::new("Close", "Esc", ModalAction::Close)],
            busy: false,
            blocking: true,
        });
    }

    /// Keep authorization targets out of Markdown syntax: OAuth state may
    /// contain parentheses or punctuation that must reach the browser intact.
    pub fn open_link_message(
        &mut self,
        title: impl Into<String>,
        body: impl Into<String>,
        label: impl Into<String>,
        url: impl Into<String>,
    ) {
        self.open_message(title, body);
        self.body_link = Some((label.into(), url.into()));
    }

    pub fn open_activity_message(
        &mut self,
        activity: ModalActivity,
        title: impl Into<String>,
        body: impl Into<String>,
        status: impl Into<String>,
    ) {
        if activity.is_cancelled() {
            return;
        }
        self.open_message(title, body);
        self.activity = Some(activity);
        self.meta = status.into();
        self.busy = true;
    }

    /// Late worker results may update their own modal, never reopen a closed
    /// modal or replace a different operation's dialog.
    pub fn update_activity_message(
        &mut self,
        activity: &ModalActivity,
        body: impl Into<String>,
        status: impl Into<String>,
        busy: bool,
        link: Option<(String, String)>,
    ) -> bool {
        if !self.active || activity.is_cancelled() || self.activity.as_ref() != Some(activity) {
            return false;
        }
        self.body_link = link;
        self.update_progress(body, status, None, busy)
    }

    pub fn is_blocking(&self) -> bool {
        self.active && self.blocking
    }

    pub fn set_dismissible(&mut self, dismissible: bool) {
        self.dismissible = dismissible;
        if !dismissible {
            self.close_rect = None;
        }
    }

    pub fn is_dismissible(&self) -> bool {
        self.dismissible
    }

    pub fn owns_editor_focus(&self) -> bool {
        self.is_blocking() || self.has_input()
    }

    pub fn close_if_non_blocking(&mut self) -> bool {
        if self.active && !self.blocking {
            self.close();
            true
        } else {
            false
        }
    }

    pub fn active_server_recovery_endpoint(&self) -> Option<&str> {
        if !self.active {
            return None;
        }
        self.buttons.iter().find_map(|button| match &button.action {
            ModalAction::ServerReconnectRetry { endpoint }
            | ModalAction::ServerReconnectLeave { endpoint } => Some(endpoint.as_str()),
            _ => None,
        })
    }

    pub fn active_title(&self) -> Option<&str> {
        self.active.then_some(self.title.as_str())
    }

    pub fn has_input(&self) -> bool {
        self.active && (self.input.is_some() || self.form.is_some())
    }

    pub fn text_entry_at(&self, x: f32, y: f32) -> bool {
        self.active
            && self.input_hit_rects.iter().any(|[rx, ry, rw, rh]| {
                x >= *rx && x <= *rx + *rw && y >= *ry && y <= *ry + *rh
            })
    }

    pub fn close_button_hit(&self, x: f32, y: f32) -> bool {
        self.close_rect.is_some_and(|[rx, ry, rw, rh]| {
            x >= rx && x <= rx + rw && y >= ry && y <= ry + rh
        })
    }

    pub fn has_markdown_input(&self) -> bool {
        self.active && self.markdown_input && self.input.is_some()
    }

    pub fn markdown_input_source(&self) -> Option<String> {
        self.markdown_pane
            .as_ref()
            .map(|pane| pane.lines.join("\n"))
    }

    pub fn click_markdown_input(&mut self, x: f32, y: f32) -> bool {
        let Some([rx, ry, rw, rh]) = self.markdown_input_rect else {
            return false;
        };
        if x < rx || x > rx + rw || y < ry || y > ry + rh {
            return false;
        }
        if let Some(pane) = self.markdown_pane.as_mut() {
            let _ = pane.click_at(x, y);
            pane.enter_insert();
        }
        true
    }

    pub fn scroll_markdown_input(&mut self, x: f32, y: f32, delta_pixels: f32) -> bool {
        let Some([rx, ry, rw, rh]) = self.markdown_input_rect else {
            return false;
        };
        if x < rx || x > rx + rw || y < ry || y > ry + rh {
            return false;
        }
        if let Some(pane) = self.markdown_pane.as_mut() {
            pane.scroll_by_content_pixels(-delta_pixels, rh);
            // Modal wheel handling is event-driven rather than part of the
            // workspace's continuous Markdown scroll tick, so apply the
            // target immediately. The virtual surface consumes it next draw.
            pane.snap_scroll_to_target();
        }
        true
    }

    pub fn insert_input_newline(&mut self) {
        if !self.has_markdown_input() {
            return;
        }
        if let Some(pane) = self.markdown_pane.as_mut() {
            pane.insert_newline();
            return;
        }
        let caret = self.input_caret.min(self.focused_input_chars());
        let Some(value) = self.focused_input_value_mut() else {
            return;
        };
        let byte = Self::byte_index_at(value, caret);
        value.insert(byte, '\n');
        self.input_caret = caret + 1;
    }

    pub fn move_input_up(&mut self) {
        if !self.has_markdown_input() {
            return;
        }
        if let Some(pane) = self.markdown_pane.as_mut() {
            pane.move_up();
            return;
        }
        let caret = self.input_caret.min(self.focused_input_chars());
        let Some(value) = self.focused_input_value() else {
            return;
        };
        let before: String = value.chars().take(caret).collect();
        let column = before
            .rsplit('\n')
            .next()
            .map(str::chars)
            .map(Iterator::count)
            .unwrap_or(0);
        let current_start = before.chars().count() - column;
        if current_start == 0 {
            return;
        }
        let previous_end = current_start - 1;
        let previous_start = value
            .chars()
            .take(previous_end)
            .collect::<String>()
            .rfind('\n')
            .map(|index| index + 1)
            .unwrap_or(0);
        self.input_caret = previous_start + column.min(previous_end - previous_start);
    }

    pub fn move_input_down(&mut self) {
        if !self.has_markdown_input() {
            return;
        }
        if let Some(pane) = self.markdown_pane.as_mut() {
            pane.move_down();
            return;
        }
        let caret = self.input_caret.min(self.focused_input_chars());
        let Some(value) = self.focused_input_value() else {
            return;
        };
        let chars = value.chars().collect::<Vec<_>>();
        let current_start = chars[..caret]
            .iter()
            .rposition(|ch| *ch == '\n')
            .map(|index| index + 1)
            .unwrap_or(0);
        let column = caret - current_start;
        let Some(next_start) = chars[caret..]
            .iter()
            .position(|ch| *ch == '\n')
            .map(|offset| caret + offset + 1)
        else {
            return;
        };
        let next_len = chars[next_start..]
            .iter()
            .position(|ch| *ch == '\n')
            .unwrap_or(chars.len() - next_start);
        self.input_caret = next_start + column.min(next_len);
    }

    fn focused_input_value(&self) -> Option<&str> {
        if let Some(form) = self.form.as_ref() {
            return form
                .fields
                .get(self.form_focus)
                .map(|field| field.value.as_str());
        }
        self.input.as_ref().map(|input| input.value.as_str())
    }

    fn focused_input_value_mut(&mut self) -> Option<&mut String> {
        if let Some(form) = self.form.as_mut() {
            return form
                .fields
                .get_mut(self.form_focus)
                .map(|field| &mut field.value);
        }
        self.input.as_mut().map(|input| &mut input.value)
    }

    fn focused_input_chars(&self) -> usize {
        self.focused_input_value()
            .map(|value| value.chars().count())
            .unwrap_or(0)
    }

    fn byte_index_at(value: &str, caret_chars: usize) -> usize {
        value
            .char_indices()
            .nth(caret_chars)
            .map(|(index, _)| index)
            .unwrap_or(value.len())
    }

    pub fn input_caret_chars(&self) -> usize {
        self.input_caret
    }

    pub fn push_input(&mut self, text: &str) {
        if let Some(pane) = self.markdown_pane.as_mut() {
            pane.insert_text(text);
            return;
        }
        let caret = self.input_caret.min(self.focused_input_chars());
        let inserted = text.chars().count();
        let Some(value) = self.focused_input_value_mut() else {
            return;
        };
        let at = Self::byte_index_at(value, caret);
        value.insert_str(at, text);
        self.input_caret = caret + inserted;
    }

    pub fn pop_input(&mut self) {
        if let Some(pane) = self.markdown_pane.as_mut() {
            pane.backspace();
            return;
        }
        let caret = self.input_caret.min(self.focused_input_chars());
        if caret == 0 {
            return;
        }
        let Some(value) = self.focused_input_value_mut() else {
            return;
        };
        let start = Self::byte_index_at(value, caret - 1);
        let end = Self::byte_index_at(value, caret);
        value.replace_range(start..end, "");
        self.input_caret = caret - 1;
    }

    pub fn delete_input(&mut self) {
        if let Some(pane) = self.markdown_pane.as_mut() {
            pane.delete_forward();
            return;
        }
        let caret = self.input_caret.min(self.focused_input_chars());
        let Some(value) = self.focused_input_value_mut() else {
            return;
        };
        let start = Self::byte_index_at(value, caret);
        let end = Self::byte_index_at(value, caret + 1);
        if start < end {
            value.replace_range(start..end, "");
        }
        self.input_caret = caret;
    }

    pub fn move_input_caret_left(&mut self) {
        if let Some(pane) = self.markdown_pane.as_mut() {
            pane.move_left();
            return;
        }
        let caret = self.input_caret.min(self.focused_input_chars());
        self.input_caret = caret.saturating_sub(1);
    }

    pub fn move_input_caret_right(&mut self) {
        if let Some(pane) = self.markdown_pane.as_mut() {
            pane.move_right();
            return;
        }
        self.input_caret = (self.input_caret + 1).min(self.focused_input_chars());
    }

    pub fn input_caret_to_start(&mut self) {
        if let Some(pane) = self.markdown_pane.as_mut() {
            pane.move_line_start();
            return;
        }
        self.input_caret = 0;
    }

    pub fn input_caret_to_end(&mut self) {
        if let Some(pane) = self.markdown_pane.as_mut() {
            pane.move_line_end();
            return;
        }
        self.input_caret = self.focused_input_chars();
    }

    pub fn needs_redraw(&self) -> bool {
        self.active
            && ((self.busy && self.progress.is_none())
                || self
                    .hover_started
                    .is_some_and(|started| started.elapsed() < HOVER_ANIMATION_DURATION))
    }

    pub fn body_link_action_at(&self, x: f32, y: f32) -> Option<ModalAction> {
        if !self.active {
            return None;
        }
        self.body_link_hit_rects.iter().find_map(|(url, rect)| {
            (x >= rect[0] && x < rect[0] + rect[2]
                && y >= rect[1] && y < rect[1] + rect[3])
                .then(|| ModalAction::OpenUrl { url: url.clone() })
        })
    }

    fn record_body_link(&mut self, url: &str, rect: [f32; 4], clip: [f32; 4]) -> Option<usize> {
        let hit = crate::primitives::geom::intersect_rect(rect, clip)?;
        let index = self.body_link_hit_rects.len();
        self.body_link_hit_rects.push((url.to_string(), hit));
        Some(index)
    }

    pub fn pointer_move(
        &mut self,
        x: f32,
        y: f32,
        window_width: f32,
        scale_factor: f32,
    ) -> bool {
        let target = self.hover_target_at(x, y, window_width, scale_factor);
        if target == self.hovered_target {
            return false;
        }
        self.previous_hover_target = self.hovered_target;
        self.hovered_target = target;
        self.hover_started = Some(Instant::now());
        true
    }

    pub fn pointer_leave(&mut self) -> bool {
        if self.hovered_target.is_none() {
            return false;
        }
        self.previous_hover_target = self.hovered_target;
        self.hovered_target = None;
        self.hover_started = Some(Instant::now());
        true
    }

    pub fn pointer_kind(&self) -> ModalPointerKind {
        match self.hovered_target {
            Some(ModalHoverTarget::Tab(_)) if !self.form_tabs_enabled => {
                ModalPointerKind::Default
            }
            Some(ModalHoverTarget::Switch)
                if !self
                    .form_switch
                    .as_ref()
                    .is_some_and(|switch| switch.enabled) =>
            {
                ModalPointerKind::Default
            }
            Some(ModalHoverTarget::Input | ModalHoverTarget::Field(_)) => {
                ModalPointerKind::Text
            }
            Some(
                ModalHoverTarget::Close
                | ModalHoverTarget::Invite
                | ModalHoverTarget::Link(_)
                | ModalHoverTarget::Tab(_)
                | ModalHoverTarget::Switch
                | ModalHoverTarget::Action(_),
            ) => ModalPointerKind::Pointer,
            None => ModalPointerKind::Default,
        }
    }

    fn hover_target_at(
        &self,
        x: f32,
        y: f32,
        window_width: f32,
        scale_factor: f32,
    ) -> Option<ModalHoverTarget> {
        let contains = |rect: [f32; 4]| {
            x >= rect[0]
                && x <= rect[0] + rect[2]
                && y >= rect[1]
                && y <= rect[1] + rect[3]
        };
        if let Some(index) = self.body_link_hit_rects.iter().position(|(_, rect)| contains(*rect)) {
            return Some(ModalHoverTarget::Link(index));
        }
        if self.invite_action_at(x, y).is_some() {
            return Some(ModalHoverTarget::Invite);
        }
        if self.close_rect.is_some_and(contains) {
            return Some(ModalHoverTarget::Close);
        }
        if let Some(index) = self.form_tab_hit(x, y) {
            return Some(ModalHoverTarget::Tab(index));
        }
        if self.form_switch_hit(x, y) {
            return Some(ModalHoverTarget::Switch);
        }
        if let Some((index, _)) = self
            .form_field_hit_rects
            .iter()
            .find(|(_, rect)| contains(*rect))
        {
            return Some(ModalHoverTarget::Field(*index));
        }
        if self.form.is_none() && self.input_hit_rects.iter().any(|rect| contains(*rect))
        {
            return Some(ModalHoverTarget::Input);
        }
        match self.hit_test(x, y, window_width, scale_factor) {
            Ok(Some(index)) if self.form.is_some() && index >= self.buttons.len() => {
                Some(ModalHoverTarget::Field(index - self.buttons.len()))
            }
            Ok(Some(index)) if index < self.buttons.len() => {
                Some(ModalHoverTarget::Action(index))
            }
            _ => None,
        }
    }

    fn hover_intensity(&self, target: ModalHoverTarget) -> f32 {
        let Some(started) = self.hover_started else {
            return if self.hovered_target == Some(target) {
                1.0
            } else {
                0.0
            };
        };
        let progress = (started.elapsed().as_secs_f32()
            / HOVER_ANIMATION_DURATION.as_secs_f32())
        .clamp(0.0, 1.0);
        let eased = crate::animation::ease_out_cubic(progress);
        let from = if self.previous_hover_target == Some(target) {
            1.0
        } else {
            0.0
        };
        let to = if self.hovered_target == Some(target) {
            1.0
        } else {
            0.0
        };
        from + (to - from) * eased
    }

    pub fn update_progress(
        &mut self,
        body: impl Into<String>,
        meta: impl Into<String>,
        progress: Option<u8>,
        busy: bool,
    ) -> bool {
        if !self.active {
            return false;
        }
        self.body = body.into();
        self.body_layout = None;
        self.body_link_hit_rects.clear();
        self.meta = meta.into();
        self.progress = progress.map(|value| value.min(100));
        self.busy = busy;
        true
    }

    /// Non-consuming, unmasked draft snapshot. May contain secrets: never log it.
    pub fn form_values(&self) -> Vec<(String, String)> {
        self.form
            .as_ref()
            .map(|form| {
                form.fields
                    .iter()
                    .map(|field| (field.id.clone(), field.value.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn set_form_switch(
        &mut self,
        label: impl Into<String>,
        checked: bool,
        enabled: bool,
    ) {
        if self.form.is_none() {
            return;
        }
        self.form_switch = Some(ModalFormSwitch {
            label: label.into(),
            checked,
            enabled,
        });
        if !enabled && self.form_focus == FORM_SWITCH_FOCUS {
            self.form_focus = self
                .form
                .as_ref()
                .and_then(|form| {
                    form.fields.iter().position(|field| !field.label.is_empty())
                })
                .unwrap_or_else(|| {
                    self.form.as_ref().map_or(0, |form| form.fields.len())
                });
            self.input_caret = self.focused_input_chars();
            self.request_form_reveal();
        }
    }

    pub fn form_switch_checked(&self) -> Option<bool> {
        self.form_switch.as_ref().map(|switch| switch.checked)
    }

    /// Logical coordinates, like `form_tab_hit`; disabled rows are not interactive.
    pub fn form_switch_hit(&self, x: f32, y: f32) -> bool {
        self.active
            && self
                .form_switch
                .as_ref()
                .is_some_and(|switch| switch.enabled)
            && self.form_switch_rect.is_some_and(|rect| {
                x >= rect[0]
                    && x <= rect[0] + rect[2]
                    && y >= rect[1]
                    && y <= rect[1] + rect[3]
            })
    }

    pub fn form_switch_focused(&self) -> bool {
        self.active
            && self.form_focus == FORM_SWITCH_FOCUS
            && self
                .form_switch
                .as_ref()
                .is_some_and(|switch| switch.enabled)
    }

    /// Focus and toggle the control, returning the NEW value for host dispatch.
    pub fn activate_form_switch(&mut self) -> Option<bool> {
        if !self.active {
            return None;
        }
        let switch = self.form_switch.as_mut()?;
        if !switch.enabled {
            return None;
        }
        switch.checked = !switch.checked;
        let checked = switch.checked;
        self.form_focus = FORM_SWITCH_FOCUS;
        self.input_caret = 0;
        self.request_form_reveal();
        Some(checked)
    }

    pub fn set_form_tabs_enabled(&mut self, enabled: bool) {
        self.form_tabs_enabled = enabled;
    }

    pub fn form_tabs_enabled(&self) -> bool {
        self.form_tabs_enabled
    }

    pub fn set_form_description(&mut self, description: impl Into<String>) {
        if self.form.is_some() {
            self.body = description.into();
            self.body_layout = None;
            self.body_link_hit_rects.clear();
        }
    }

    /// Attach a segmented mode slider to the OPEN form (call right
    /// after `open_form`). `open_form` clears it, so plain forms never
    /// inherit another modal's tabs.
    pub fn set_form_tabs(&mut self, tabs: Vec<String>, active: usize) {
        self.form_active_tab = active.min(tabs.len().saturating_sub(1));
        self.form_tabs = tabs;
    }

    /// Change the selected segment without rebuilding the form; obeys the lock.
    pub fn set_form_active_tab(&mut self, index: usize) -> bool {
        if !self.active || !self.form_tabs_enabled || index >= self.form_tabs.len() {
            return false;
        }
        self.form_active_tab = index;
        true
    }

    pub fn form_tab_count(&self) -> usize {
        self.form_tabs.len()
    }

    pub fn form_active_tab(&self) -> usize {
        self.form_active_tab
    }

    /// Which slider segment (if any) sits under the pointer — rects are
    /// recorded at render time in the same logical space the modal's
    /// `hit_test` uses.
    pub fn form_tab_hit(&self, x: f32, y: f32) -> Option<usize> {
        self.form_tab_rects
            .iter()
            .position(|rect| {
                rect[2] > 0.0
                    && rect[3] > 0.0
                    && x >= rect[0]
                    && x <= rect[0] + rect[2]
                    && y >= rect[1]
                    && y <= rect[1] + rect[3]
            })
            .filter(|_| self.active && self.form_tabs_enabled)
    }

    pub fn focus_next_form_field(&mut self) -> bool {
        self.focus_form_control(false)
    }

    pub fn focus_previous_form_field(&mut self) -> bool {
        self.focus_form_control(true)
    }

    fn focus_form_control(&mut self, backwards: bool) -> bool {
        let Some(form) = self.form.as_ref() else {
            return false;
        };
        if form.fields.is_empty() && self.form_switch.is_none() {
            return false;
        }
        let mut controls = Vec::new();
        controls.extend(
            form.fields
                .iter()
                .enumerate()
                .filter(|(_, field)| !field.label.is_empty())
                .map(|(index, _)| index),
        );
        if self
            .form_switch
            .as_ref()
            .is_some_and(|switch| switch.enabled)
        {
            controls.push(FORM_SWITCH_FOCUS);
        }
        // Preserve plain text forms' historical field-only Tab cycle.
        if self.form_switch.is_some() {
            controls.push(form.fields.len());
        }
        if controls.is_empty() {
            return true;
        }
        let next = match controls.iter().position(|index| *index == self.form_focus) {
            Some(index) if backwards => (index + controls.len() - 1) % controls.len(),
            Some(index) => (index + 1) % controls.len(),
            None if backwards => controls.len() - 1,
            None => 0,
        };
        self.form_focus = controls[next];
        self.input_caret = self.focused_input_chars();
        self.request_form_reveal();
        true
    }

    /// Vertical navigation for FORM modals: Down walks the fields top
    /// to bottom, then the enabled switch, then the button row; Down
    /// moves within the buttons and Up reverses that visible order.
    /// `form_focus == fields.len()` is the buttons zone — every
    /// focused-input accessor already returns None there, so typing
    /// simply goes nowhere until focus returns to a field. Returns
    /// false when no form is open (caller keeps plain button
    /// navigation). Hidden fields (empty labels) are skipped, matching
    /// Tab's `focus_next_form_field`.
    pub fn form_focus_move(&mut self, down: bool) -> bool {
        let moved = self.form_focus_move_inner(down);
        if moved {
            self.request_form_reveal();
        }
        moved
    }

    fn form_focus_move_inner(&mut self, down: bool) -> bool {
        let Some(form) = self.form.as_ref() else {
            return false;
        };
        let count = form.fields.len();
        let fields: Vec<usize> = form
            .fields
            .iter()
            .enumerate()
            .filter(|(_, field)| !field.label.is_empty())
            .map(|(index, _)| index)
            .collect();
        let switch_enabled = self
            .form_switch
            .as_ref()
            .is_some_and(|switch| switch.enabled);
        if count == 0 && self.form_switch.is_none() {
            return false;
        }
        if self.form_switch_focused() {
            if down {
                self.form_focus = count;
            } else if let Some(last) = fields.last() {
                self.form_focus = *last;
            }
        } else if self.form_focus >= count {
            if down {
                self.move_selection_down();
            } else if self.selected_index > 0 {
                self.move_selection_up();
            } else if switch_enabled {
                self.form_focus = FORM_SWITCH_FOCUS;
            } else if let Some(last) = fields.last() {
                self.form_focus = *last;
            }
        } else if down {
            self.form_focus = fields
                .iter()
                .copied()
                .find(|index| *index > self.form_focus)
                .unwrap_or(if switch_enabled {
                    FORM_SWITCH_FOCUS
                } else {
                    count
                });
        } else if let Some(previous) = fields
            .iter()
            .rev()
            .copied()
            .find(|index| *index < self.form_focus)
        {
            self.form_focus = previous;
        }
        self.input_caret = self.focused_input_chars();
        true
    }

    /// True while a form's focus sits on the button row — the renderer
    /// highlights the selected button instead of a field caret.
    pub fn form_buttons_focused(&self) -> bool {
        self.form.as_ref().is_some_and(|form| {
            self.form_focus >= form.fields.len() && self.form_focus != FORM_SWITCH_FOCUS
        })
    }

    pub fn move_selection_up(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
            if self.selected_index < self.scroll_offset {
                self.scroll_offset = self.selected_index;
            }
        }
    }

    pub fn move_selection_down(&mut self) {
        if self.selected_index + 1 < self.buttons.len() {
            self.selected_index += 1;
            let visible = self.visible_action_count().max(1);
            if self.selected_index >= self.scroll_offset + visible {
                self.scroll_offset = self.selected_index - visible + 1;
            }
        }
    }

    pub fn selected_action(&self) -> Option<ModalAction> {
        let action = self
            .buttons
            .get(self.selected_index)
            .map(|button| button.action.clone())?;
        Some(self.apply_input(action))
    }

    pub fn has_action(&self, matches: impl Fn(&ModalAction) -> bool) -> bool {
        self.buttons.iter().any(|button| matches(&button.action))
    }

    pub fn submit_form(&mut self) -> Option<ModalAction> {
        let form = self.form.as_ref()?;
        self.submitted_form = Some(
            form.fields
                .iter()
                .map(|field| (field.id.clone(), field.value.clone()))
                .collect(),
        );
        self.buttons.first().map(|button| button.action.clone())
    }

    pub fn action_for_hint(&self, hint: &str) -> Option<ModalAction> {
        if hint.len() != 1 {
            return None;
        }
        self.buttons
            .iter()
            .find(|button| button.hint.eq_ignore_ascii_case(hint))
            .map(|button| self.apply_input(button.action.clone()))
    }

    pub fn escape_action(&self) -> Option<ModalAction> {
        self.buttons
            .iter()
            .find(|button| button.hint.eq_ignore_ascii_case("Esc"))
            .map(|button| self.apply_input(button.action.clone()))
    }

    fn apply_input(&self, action: ModalAction) -> ModalAction {
        if let Some(pane) = self.markdown_pane.as_ref() {
            action.with_input(pane.lines.join("\n"))
        } else if let Some(input) = self.input.as_ref() {
            action.with_input(input.value.clone())
        } else {
            action
        }
    }

    pub fn set_selected_index(&mut self, index: usize) {
        if index < self.buttons.len() {
            self.selected_index = index;
            if self.selected_index < self.scroll_offset {
                self.scroll_offset = self.selected_index;
            } else if self.selected_index
                >= self.scroll_offset + self.visible_action_count().max(1)
            {
                self.scroll_offset =
                    self.selected_index - self.visible_action_count().max(1) + 1;
            }
        }
    }

    fn visible_action_count(&self) -> usize {
        self.buttons.len().min(self.action_row_budget())
    }

    fn body_row_budget(&self) -> usize {
        ((self.viewport_height * 0.32 / (BODY_LINE_HEIGHT * self.scale).max(1.0)).floor()
            as usize)
            .clamp(1, MAX_BODY_LINES)
    }

    fn action_row_budget(&self) -> usize {
        ((self.viewport_height * 0.30 / (ACTION_ROW_HEIGHT * self.scale).max(1.0)).floor()
            as usize)
            .clamp(1, MAX_VISIBLE_ACTIONS)
    }

    fn body_lines(&self) -> Vec<BodyLine> {
        if let Some(layout) = &self.body_layout {
            if layout.source == self.body {
                return layout.lines.clone();
            }
        }
        // Geometry can be queried before the first paint. The render pass
        // replaces this estimate with the actual font measurements.
        self.layout_body(
            (MODAL_WIDTH - MODAL_PADDING * 2.0 - 8.0) * self.scale,
            self.scale,
            |text, _| text.chars().count() as f32 * BODY_FONT_SIZE * self.scale * 0.6,
        )
    }

    fn layout_body(
        &self,
        width: f32,
        scale: f32,
        mut measure: impl FnMut(&str, bool) -> f32,
    ) -> Vec<BodyLine> {
        let mut lines = body::layout_lines(
            &self.body, width, scale, self.invite_url.as_deref(), &mut measure,
        );
        if let Some((label, url)) = &self.body_link {
            if !lines.is_empty() {
                lines.push(BodyLine::new(String::new(), BodyLineKind::Text));
            }
            lines.extend(body::link_lines(label, url, width, scale, &mut measure));
        }
        lines
    }

    fn prepare_body_layout(&mut self, sugarloaf: &mut (impl crate::primitives::paint::Paint + ?Sized), width: f32, scale: f32) {
        let opts = DrawOpts { font_size: BODY_FONT_SIZE * scale, ..DrawOpts::default() };
        let bold_opts = DrawOpts { bold: true, ..opts.clone() };
        let probe = "Wim 0123456789";
        let font_probe = (
            sugarloaf.text_mut().measure(probe, &opts),
            sugarloaf.text_mut().measure(probe, &bold_opts),
        );
        if self.body_layout.as_ref().is_some_and(|layout| {
            layout.source == self.body && layout.width == width
                && layout.scale == scale && layout.font_probe == font_probe
        }) {
            return;
        }
        let lines = self.layout_body(
            width, scale,
            |text, bold| sugarloaf.text_mut().measure(text, if bold { &bold_opts } else { &opts }),
        );
        self.body_layout = Some(BodyLayout {
            source: self.body.clone(), width, scale, font_probe, lines,
        });
        self.body_link_hit_rects.clear();
    }

    fn visible_body_lines(&self) -> Vec<BodyLine> {
        let mut rows = 0usize;
        let mut visible = Vec::new();
        for line in self.body_lines().into_iter().skip(self.body_scroll_offset) {
            if rows > 0 && rows + line.row_span > self.body_row_budget() {
                break;
            }
            rows += line.row_span;
            visible.push(line);
            if rows >= self.body_row_budget() {
                break;
            }
        }
        visible
    }

    fn visible_body_line_count(&self) -> usize {
        self.visible_body_lines()
            .iter()
            .map(|line| line.row_span)
            .sum::<usize>()
            .min(self.body_row_budget())
            .max(1)
    }

    fn clamp_body_scroll(&mut self) {
        let max_offset = self
            .body_lines()
            .len()
            .saturating_sub(self.body_row_budget());
        self.body_scroll_offset = self.body_scroll_offset.min(max_offset);
    }

    fn scroll_body_rows(&mut self, rows: i32) {
        if rows == 0 {
            return;
        }
        self.body_link_hit_rects.clear();
        let max_offset = self
            .body_lines()
            .len()
            .saturating_sub(self.body_row_budget());
        self.body_scroll_offset = if rows < 0 {
            self.body_scroll_offset
                .saturating_sub(rows.unsigned_abs() as usize)
        } else {
            self.body_scroll_offset
                .saturating_add(rows as usize)
                .min(max_offset)
        };
    }

    pub fn scroll_body_page(&mut self, down: bool) {
        if let Some(geometry) = self.form_geometry(MODAL_WIDTH * self.scale + 100.0, 1.0)
        {
            self.scroll_form_by(
                geometry,
                if down {
                    geometry.body[3]
                } else {
                    -geometry.body[3]
                },
            );
            return;
        }
        let rows = if down {
            self.body_row_budget() as i32
        } else {
            -(self.body_row_budget() as i32)
        };
        self.scroll_body_rows(rows);
    }

    fn scroll_actions_rows(&mut self, rows: i32) {
        let visible = self.visible_action_count().max(1);
        if rows == 0 || self.buttons.len() <= visible {
            return;
        }
        let max_offset = self.buttons.len().saturating_sub(visible);
        self.scroll_offset = if rows < 0 {
            self.scroll_offset
                .saturating_sub(rows.unsigned_abs() as usize)
        } else {
            self.scroll_offset
                .saturating_add(rows as usize)
                .min(max_offset)
        };
        if self.selected_index < self.scroll_offset {
            self.selected_index = self.scroll_offset;
        } else if self.selected_index >= self.scroll_offset + visible {
            self.selected_index = self.scroll_offset + visible - 1;
        }
    }

    pub fn scroll_at(
        &mut self,
        mouse_x: f32,
        mouse_y: f32,
        window_width: f32,
        scale_factor: f32,
        delta_pixels: f32,
    ) -> bool {
        if !self.active {
            return false;
        }
        // Consume wheel on the backdrop too; never scroll obscured content.
        if self.is_image_open() {
            return true;
        }
        let (x, y, w, h) = self.modal_rect(window_width, scale_factor);
        if self.form.is_none()
            && (mouse_x < x || mouse_x > x + w || mouse_y < y || mouse_y > y + h)
        {
            return false;
        }
        if self.form.is_some() {
            let geometry = self.form_geometry(window_width, scale_factor).unwrap();
            // Consume outside wheel too: the blocking form owns input.
            if mouse_x >= geometry.body[0]
                && mouse_x <= geometry.body[0] + geometry.body[2]
                && mouse_y >= geometry.body[1]
                && mouse_y <= geometry.body[1] + geometry.body[3]
            {
                self.scroll_form_by(geometry, -delta_pixels);
            }
            return true;
        }
        if delta_pixels == 0.0 {
            return true;
        }

        let row_h = (BODY_LINE_HEIGHT * self.scale).max(1.0);
        self.body_wheel_accumulator += delta_pixels;
        let mut rows = 0i32;
        while self.body_wheel_accumulator.abs() >= row_h {
            let sign = self.body_wheel_accumulator.signum();
            self.body_wheel_accumulator -= sign * row_h;
            rows += if sign > 0.0 { -1 } else { 1 };
        }

        let actions_y = self.actions_y(y);
        if mouse_y >= actions_y {
            self.scroll_actions_rows(rows);
        } else {
            self.scroll_body_rows(rows);
        }
        true
    }

    fn body_scroll_normalized(&self) -> f32 {
        let max_offset = self
            .body_lines()
            .len()
            .saturating_sub(self.body_row_budget());
        if max_offset == 0 {
            0.0
        } else {
            self.body_scroll_offset as f32 / max_offset as f32
        }
    }

    fn form_geometry(
        &self,
        window_width: f32,
        scale_factor: f32,
    ) -> Option<FormGeometry> {
        let form = self.form.as_ref()?;
        let viewport_w = (window_width / scale_factor).max(1.0);
        let viewport_h = self.viewport_height.max(1.0);
        let count = form
            .fields
            .iter()
            .filter(|field| !field.label.is_empty())
            .count();
        let description_rows = self
            .form_description_lines()
            .iter()
            .map(|line| line.row_span)
            .sum::<usize>();
        let description = description_rows as f32 * BODY_LINE_HEIGHT
            + if self.meta.is_empty() { 0.0 } else { 20.0 };
        let switch_height = if self.form_switch.is_some() {
            FORM_SWITCH_ROW_HEIGHT
        } else {
            0.0
        };
        let actions = self.buttons.len() as f32 * ACTION_ROW_HEIGHT;
        let natural = (MODAL_PADDING * 2.0
            + TITLE_HEIGHT
            + 12.0
            + switch_height
            + description
            + count as f32 * 64.0
            + 19.0
            + actions)
            * self.scale;
        let compact = self.form_compact || natural > viewport_h - 24.0 * self.scale;
        // Compact spacing first, then bounded scale adaptation (never tiny text).
        let s = if compact {
            self.scale.min(((viewport_h - 8.0) / 207.0).max(0.75))
        } else {
            self.scale
        };
        let minimal = viewport_h < 120.0;
        let pad = if minimal {
            2.0
        } else if compact {
            6.0
        } else {
            MODAL_PADDING
        } * s;
        let gap = if minimal {
            1.0
        } else if compact {
            4.0
        } else {
            12.0
        } * s;
        // Preserve every button row, shortening padding rather than its text.
        let action_row_height = if minimal { 24.0 } else { ACTION_ROW_HEIGHT } * s;
        let actions_height = self.buttons.len() as f32 * action_row_height;
        let margin = (if compact { 4.0 } else { 12.0 } * s)
            .min(viewport_h * 0.02)
            .min(viewport_w * 0.05);
        let width = (MODAL_WIDTH * s).min((viewport_w - 2.0 * margin).max(1.0));
        let max_height = (viewport_h - 2.0 * margin).max(1.0);
        let footer_gap = if minimal {
            2.0 * s
        } else if compact {
            5.0 * s
        } else {
            18.0 * s + SEPARATOR_HEIGHT
        };
        let footer_height = pad + footer_gap + actions_height;
        let header_height = pad + TITLE_HEIGHT * s + gap;
        // Extremely short windows keep title/actions pinned and put the switch
        // AFTER the last field in the scrollable body, never in its prefix.
        let switch_in_body =
            header_height + switch_height * s + footer_height + 54.0 * s > max_height;
        let pinned_switch = if switch_in_body {
            0.0
        } else {
            switch_height * s
        };
        let fields_start = description * s;
        let fields_end = fields_start + count as f32 * 64.0 * s;
        let content_height = fields_end
            + if switch_in_body {
                switch_height * s
            } else {
                0.0
            };
        let height = (header_height + pinned_switch + content_height + footer_height)
            .min(max_height);
        let x = (viewport_w - width) * 0.5;
        let y = self
            .top_anchor
            .min((viewport_h - height - margin).max(margin));
        // On pathological sub-title-sized viewports every area is still bounded.
        let footer_h = footer_height.min((height - header_height.min(height)).max(0.0));
        let actions_y = y + height - footer_h + footer_gap;
        let action_h = actions_height.min((y + height - pad - actions_y).max(0.0));
        let body_y = (y + header_height).min(y + height - footer_h);
        let body_h = (y + height - footer_h - pinned_switch - body_y).max(0.0);
        let body = [x + pad, body_y, (width - 2.0 * pad).max(0.0), body_h];
        let switch = self.form_switch.as_ref().map(|_| {
            [
                body[0],
                if switch_in_body {
                    body_y + fields_end
                } else {
                    body_y + body_h
                },
                body[2],
                28.0 * s,
            ]
        });
        Some(FormGeometry {
            panel: [x, y, width, height],
            body,
            actions: [body[0], actions_y.min(y + height), body[2], action_h],
            action_row_height,
            switch,
            switch_in_body,
            compact,
            scale: s,
            pad,
            gap,
            content_height,
            fields_start,
        })
    }

    fn scroll_form_by(&mut self, geometry: FormGeometry, pixels: f32) {
        self.body_link_hit_rects.clear();
        self.form_scroll = (self.form_scroll + pixels).clamp(0.0, geometry.max_scroll());
        self.form_reveal_pending = false;
        self.form_field_hit_rects.clear();
        self.input_hit_rects.clear();
        if geometry.switch_in_body {
            self.form_switch_rect = None;
        }
    }

    fn request_form_reveal(&mut self) {
        if let Some(geometry) = self.form_geometry(MODAL_WIDTH * self.scale + 100.0, 1.0)
        {
            self.reveal_form_focus(geometry);
            if geometry.switch_in_body {
                self.form_switch_rect = None;
            }
        }
        self.form_reveal_pending = true;
        self.form_field_hit_rects.clear();
        self.input_hit_rects.clear();
    }

    fn reveal_form_focus(&mut self, geometry: FormGeometry) {
        let target = if self.form_switch_focused() && geometry.switch_in_body {
            geometry
                .switch
                .map(|rect| (rect[1] - geometry.body[1], rect[3]))
        } else {
            self.form.as_ref().and_then(|form| {
                let ordinal = form
                    .fields
                    .iter()
                    .take(self.form_focus.min(form.fields.len()))
                    .filter(|field| !field.label.is_empty())
                    .count();
                form.fields
                    .get(self.form_focus)
                    .filter(|field| !field.label.is_empty())
                    .map(|_| {
                        (
                            geometry.fields_start
                                + ordinal as f32 * 64.0 * geometry.scale,
                            54.0 * geometry.scale,
                        )
                    })
            })
        };
        if let Some((mut top, mut height)) = target {
            if height > geometry.body[3] && !self.form_switch_focused() {
                // If the label+input cannot fit, prioritize the editable input.
                top += 20.0 * geometry.scale;
                height = INPUT_ROW_HEIGHT * geometry.scale;
            }
            if height > geometry.body[3] {
                self.form_scroll = top + height - geometry.body[3];
            } else if top < self.form_scroll {
                self.form_scroll = top;
            } else if top + height > self.form_scroll + geometry.body[3] {
                self.form_scroll = top + height - geometry.body[3];
            }
        }
        self.form_scroll = self.form_scroll.clamp(0.0, geometry.max_scroll());
        self.form_reveal_pending = false;
    }

    fn modal_rect(&self, window_width: f32, scale_factor: f32) -> (f32, f32, f32, f32) {
        if let Some(geometry) = self.form_geometry(window_width, scale_factor) {
            let [x, y, w, h] = geometry.panel;
            return (x, y, w, h);
        }
        let s = self.scale;
        let viewport_w = (window_width / scale_factor).max(1.0);
        let margin = (12.0 * s).min(viewport_w * 0.05);
        let width = (MODAL_WIDTH * s).min((viewport_w - margin * 2.0).max(1.0));
        let body_lines = self.visible_body_line_count() as f32;
        let meta_h = if self.meta.is_empty() { 0.0 } else { 20.0 * s };
        let busy_h = if self.activity.is_none() && self.busy {
            BUSY_BAR_HEIGHT * s + 12.0 * s
        } else {
            0.0
        };
        let input_h = if self.input.is_some() {
            if self.markdown_input {
                MARKDOWN_INPUT_HEIGHT * s + 12.0 * s
            } else {
                INPUT_ROW_HEIGHT * s + 12.0 * s
            }
        } else {
            0.0
        };
        let actions_h = self.visible_action_count() as f32 * ACTION_ROW_HEIGHT * s;
        let height = MODAL_PADDING * s
            + TITLE_HEIGHT * s
            + 12.0 * s
            + body_lines * BODY_LINE_HEIGHT * s
            + meta_h
            + busy_h
            + input_h
            + 14.0 * s
            + SEPARATOR_HEIGHT
            + 4.0 * s
            + actions_h
            + MODAL_PADDING * s;
        let max_h = (self.viewport_height - margin * 2.0).max(1.0);
        let height = height.min(max_h);
        let x = (viewport_w - width) / 2.0;
        let y = self
            .top_anchor
            .min((self.viewport_height - height - margin).max(margin));
        (x, y, width, height)
    }

    fn actions_y(&self, modal_y: f32) -> f32 {
        if let Some(geometry) = self.form_geometry(MODAL_WIDTH * self.scale + 100.0, 1.0)
        {
            return modal_y + geometry.actions[1] - geometry.panel[1];
        }
        let s = self.scale;
        let body_lines = self.visible_body_line_count() as f32;
        let meta_h = if self.meta.is_empty() { 0.0 } else { 20.0 * s };
        let busy_h = if self.activity.is_none() && self.busy {
            BUSY_BAR_HEIGHT * s + 12.0 * s
        } else {
            0.0
        };
        let input_h = if self.input.is_some() {
            if self.markdown_input {
                MARKDOWN_INPUT_HEIGHT * s + 12.0 * s
            } else {
                INPUT_ROW_HEIGHT * s + 12.0 * s
            }
        } else {
            0.0
        };
        modal_y
            + MODAL_PADDING * s
            + TITLE_HEIGHT * s
            + 12.0 * s
            + body_lines * BODY_LINE_HEIGHT * s
            + meta_h
            + busy_h
            + input_h
            + 14.0 * s
            + SEPARATOR_HEIGHT
            + 4.0 * s
    }

    pub fn hit_test(
        &self,
        mouse_x: f32,
        mouse_y: f32,
        window_width: f32,
        scale_factor: f32,
    ) -> Result<Option<usize>, ()> {
        if self.is_image_open() {
            if self.close_button_hit(mouse_x, mouse_y) {
                return Ok(Some(0));
            }
            // The image itself is inert. Outside it is the dismissing backdrop,
            // using the same Err convention as ordinary modal outside clicks.
            return if self.image_rect.is_some_and(|[x, y, w, h]| {
                mouse_x >= x && mouse_x <= x + w && mouse_y >= y && mouse_y <= y + h
            }) {
                Ok(None)
            } else {
                Err(())
            };
        }
        let (x, y, w, h) = self.modal_rect(window_width, scale_factor);
        if mouse_x < x || mouse_x > x + w || mouse_y < y || mouse_y > y + h {
            return Err(());
        }
        if let Some(geometry) = self.form_geometry(window_width, scale_factor) {
            let contains = |rect: [f32; 4]| {
                mouse_x >= rect[0]
                    && mouse_x <= rect[0] + rect[2]
                    && mouse_y >= rect[1]
                    && mouse_y <= rect[1] + rect[3]
                    && rect[2] > 0.0
                    && rect[3] > 0.0
            };
            if contains(geometry.body) {
                let mut ordinal = 0;
                for (index, field) in
                    self.form.as_ref().unwrap().fields.iter().enumerate()
                {
                    if field.label.is_empty() {
                        continue;
                    }
                    if let Some(rect) = crate::primitives::geom::intersect_rect(
                        geometry.field_input(ordinal, self.form_scroll),
                        geometry.body,
                    ) {
                        if contains(rect) {
                            return Ok(Some(self.buttons.len() + index));
                        }
                    }
                    ordinal += 1;
                }
            }
            if contains(geometry.actions) {
                let row = ((mouse_y - geometry.actions[1]) / geometry.action_row_height)
                    as usize;
                if row < self.buttons.len() {
                    return Ok(Some(row));
                }
            }
            return Ok(None);
        }
        let actions_y = self.actions_y(y);
        if mouse_y < actions_y {
            return Ok(None);
        }

        let row = ((mouse_y - actions_y) / (ACTION_ROW_HEIGHT * self.scale)) as usize;
        let actual = self.scroll_offset + row;
        if row < self.visible_action_count() && actual < self.buttons.len() {
            Ok(Some(actual))
        } else {
            Ok(None)
        }
    }

    pub fn active_rect(&self, window_width: f32, scale_factor: f32) -> Option<[f32; 4]> {
        self.active.then(|| {
            if self.is_image_open() {
                return [0.0, 0.0, (window_width / scale_factor).max(1.0), self.viewport_height];
            }
            let (x, y, w, h) = self.modal_rect(window_width, scale_factor);
            [x, y, w, h]
        })
    }

    pub fn focus_form_hit(&mut self, hit: usize) -> bool {
        let Some(form) = self.form.as_ref() else {
            return false;
        };
        let Some(index) = hit.checked_sub(self.buttons.len()) else {
            return false;
        };
        if form
            .fields
            .get(index)
            .is_some_and(|field| !field.label.is_empty())
        {
            self.form_focus = index;
            self.input_caret = self.focused_input_chars();
            self.request_form_reveal();
            true
        } else {
            false
        }
    }

    /// Whitespace-only form descriptions are absent, not placeholder rows.
    fn form_description_lines(&self) -> Vec<BodyLine> {
        if self.body.trim().is_empty() {
            Vec::new()
        } else {
            self.body_lines()
        }
    }

    fn draw_form_body(
        &mut self,
        sugarloaf: &mut (impl crate::primitives::paint::Paint + ?Sized),
        theme: &IdeTheme,
        geometry: FormGeometry,
    ) {
        let s = geometry.scale;
        let clip = geometry.body;
        let x = clip[0];
        let mut y = clip[1] - self.form_scroll;
        let lines = self.form_description_lines();
        let opts = DrawOpts {
            font_size: BODY_FONT_SIZE * s,
            color: theme.u8(theme.dim),
            clip_rect: Some(clip),
            ..DrawOpts::default()
        };
        for line in lines {
            self.draw_body_line(sugarloaf, theme, &line, x, y, clip[2], clip, s);
            y += BODY_LINE_HEIGHT * s * line.row_span as f32;
        }
        if !self.meta.is_empty() {
            sugarloaf.text_mut().draw(
                x,
                y,
                &self.meta,
                &DrawOpts {
                    color: theme.u8(theme.muted),
                    ..opts
                },
            );
        }
        let Some(form) = self.form.as_ref() else {
            return;
        };
        let mut ordinal = 0;
        for (index, field) in form.fields.iter().enumerate() {
            if field.label.is_empty() {
                continue;
            }
            let row = geometry.field_row(ordinal, self.form_scroll);
            let input = geometry.field_input(ordinal, self.form_scroll);
            ordinal += 1;
            let Some(hit) = crate::primitives::geom::intersect_rect(input, clip) else {
                continue;
            };
            sugarloaf.text_mut().draw(
                x,
                row[1],
                &field.label,
                &DrawOpts {
                    font_size: 12.0 * s,
                    color: theme.u8(theme.muted),
                    clip_rect: Some(clip),
                    ..DrawOpts::default()
                },
            );
            draw_form_rect(
                sugarloaf,
                input,
                clip,
                theme.f32(theme.surface),
                5.0 * s,
                ORDER,
            );
            let hover = self.hover_intensity(ModalHoverTarget::Field(index));
            if hover > 0.0 && index != self.form_focus {
                draw_form_rect(
                    sugarloaf,
                    input,
                    clip,
                    theme.f32_alpha(theme.accent, hover * 0.08),
                    5.0 * s,
                    ORDER + 1,
                );
            }
            self.form_field_hit_rects.push((index, hit));
            self.input_hit_rects.push(hit);
            let value = if field.value.is_empty() {
                field.placeholder.clone()
            } else if field.secret {
                "•".repeat(field.value.chars().count())
            } else {
                field.value.clone()
            };
            let opts = DrawOpts {
                font_size: INPUT_FONT_SIZE * s,
                color: theme.u8(if field.value.is_empty() {
                    theme.muted
                } else {
                    theme.fg
                }),
                clip_rect: Some(hit),
                ..DrawOpts::default()
            };
            sugarloaf.text_mut().draw(
                x + 12.0 * s,
                input[1] + (input[3] - INPUT_FONT_SIZE * s) * 0.5,
                &value,
                &opts,
            );
            if index == self.form_focus {
                draw_form_rect(
                    sugarloaf,
                    [x, input[1], 2.0 * s, input[3]],
                    clip,
                    theme.f32(theme.accent),
                    0.0,
                    ORDER + 1,
                );
                let chars = self.input_caret.min(field.value.chars().count());
                let caret_text = if field.secret {
                    "•".repeat(chars)
                } else {
                    field.value.chars().take(chars).collect::<String>()
                };
                let caret_x =
                    x + 12.0 * s + sugarloaf.text_mut().measure(&caret_text, &opts);
                let caret_h = (INPUT_FONT_SIZE + 6.0) * s;
                draw_form_rect(
                    sugarloaf,
                    [
                        caret_x,
                        input[1] + (input[3] - caret_h) * 0.5,
                        INPUT_FONT_SIZE * 0.55 * s,
                        caret_h,
                    ],
                    hit,
                    theme.f32_alpha(theme.accent, 0.55),
                    0.0,
                    ORDER + 2,
                );
            }
        }
        if geometry.max_scroll() > 0.0 && clip[3] > 0.0 {
            let thumb_h = (clip[3] * clip[3] / geometry.content_height)
                .max(12.0 * s)
                .min(clip[3]);
            let thumb_y =
                clip[1] + (clip[3] - thumb_h) * self.form_scroll / geometry.max_scroll();
            draw_form_rect(
                sugarloaf,
                [clip[0] + clip[2] - 3.0 * s, thumb_y, 3.0 * s, thumb_h],
                clip,
                theme.f32_alpha(theme.muted, 0.55),
                1.5 * s,
                ORDER + 3,
            );
        }
    }

    fn render_image(&mut self, sugarloaf: &mut Sugarloaf, viewport: [f32; 2]) {
        let [w, h] = viewport;
        let s = self.scale;
        sugarloaf.overlay_rect(0.0, 0.0, w, h, [0.025, 0.028, 0.035, 0.98], DEPTH_BACKDROP, ORDER);
        self.image_rect = self.image.as_ref()
            .and_then(|image| register_image(sugarloaf, image))
            .and_then(|(id, aspect)| {
                let rect = image_aspect_fit(viewport, aspect, s)?;
                let scale = sugarloaf.scale_factor();
                sugarloaf.push_image_overlay(IMAGE_OVERLAY_OWNER, GraphicOverlay {
                    image_id: id,
                    x: rect[0] * scale,
                    y: rect[1] * scale,
                    width: rect[2] * scale,
                    height: rect[3] * scale,
                    z_index: GraphicOverlay::LATE_OVERLAY_Z_INDEX,
                    source_rect: GraphicOverlay::FULL_SOURCE_RECT,
                });
                Some(rect)
            });
        if self.image_rect.is_none() {
            let opts = DrawOpts {
                font_size: 13.0 * s,
                color: [180, 183, 190, 255],
                clip_rect: Some([0.0, 0.0, w, h]),
                ..DrawOpts::default()
            };
            let label = "Image unavailable";
            let width = sugarloaf.overlay_text_mut().measure(label, &opts);
            sugarloaf.overlay_text_mut().draw((w - width) * 0.5, h * 0.5, label, &opts);
        }
        // Reserve the top band for a quiet floating control, outside the image.
        // Overlay labels flush AFTER late graphics, so the close glyph never
        // disappears underneath a full-resolution image placement.
        let size = (32.0 * s).min(w * 0.18).min(h * 0.18);
        let inset = (20.0 * s).min(w * 0.05).min(h * 0.04);
        let close = [w - inset - size, inset, size, size];
        self.close_rect = self.dismissible.then_some(close);
        if self.dismissible {
            let hover = self.hover_intensity(ModalHoverTarget::Close);
            sugarloaf.rounded_rect(None, close[0], close[1], size, size,
                [0.16, 0.17, 0.19, 0.7 + hover * 0.2], DEPTH_ELEMENT, size * 0.5, ORDER + 1);
            let opts = DrawOpts {
                font_size: size * 0.6,
                color: [235, 237, 242, 255],
                clip_rect: Some(close),
                ..DrawOpts::default()
            };
            let width = sugarloaf.overlay_text_mut().measure("×", &opts);
            sugarloaf.overlay_text_mut().draw(
                close[0] + (size - width) * 0.5, close[1] + size * 0.18, "×", &opts,
            );
        }
    }

    pub fn render(&mut self, sugarloaf: &mut Sugarloaf, dimensions: (f32, f32, f32), theme: &IdeTheme) {
        sugarloaf.clear_image_overlays_for(IMAGE_OVERLAY_OWNER);
        if self.active && self.is_image_open() {
            self.render_image(sugarloaf, [dimensions.0 / dimensions.2, dimensions.1 / dimensions.2]);
            return;
        }
        self.render_portable(sugarloaf, dimensions, theme);
    }

    /// Original ordinary-modal painter, shared with native GPU hosts.
    /// Image overlays remain owned by the desktop wrapper above.
    pub fn render_portable(&mut self, sugarloaf: &mut (impl crate::primitives::paint::Paint + ?Sized), dimensions: (f32, f32, f32), theme: &IdeTheme) {
        if !self.active {
            return;
        }
        let (window_width, window_height, scale_factor) = dimensions;
        let viewport_height = (window_height / scale_factor).max(1.0);
        if self.viewport_height != viewport_height {
            self.form_reveal_pending = true;
        }
        self.viewport_height = viewport_height;
        self.form_compact = false;
        let viewport_width = (window_width / scale_factor).max(1.0);
        let normal_margin = if self.form.is_some() {
            (12.0 * self.scale).min(viewport_height * 0.02).min(viewport_width * 0.05)
        } else {
            (12.0 * self.scale).min(viewport_width * 0.05)
        };
        let normal_width = (MODAL_WIDTH * self.scale)
            .min((viewport_width - 2.0 * normal_margin).max(1.0));
        let normal_pad = if self.form.is_some() && viewport_height < 120.0 { 2.0 } else { MODAL_PADDING } * self.scale;
        // Decide compact mode from NORMAL-width wrapping, then hold that
        // decision while measuring the wider compact viewport. Otherwise a
        // two-row description can shrink to one row and undo compact mode.
        self.prepare_body_layout(
            sugarloaf, (normal_width - 2.0 * normal_pad - 8.0 * self.scale).max(1.0), self.scale,
        );
        if let Some(geometry) = self.form_geometry(window_width, scale_factor) {
            self.form_compact = geometry.compact;
            if geometry.compact {
                self.prepare_body_layout(
                    sugarloaf, (geometry.body[2] - 8.0 * geometry.scale).max(1.0), geometry.scale,
                );
            }
        }
        self.clamp_body_scroll();
        self.body_link_hit_rects.clear();
        self.input_hit_rects.clear();
        self.form_field_hit_rects.clear();
        let form_geometry = self.form_geometry(window_width, scale_factor);
        if let Some(geometry) = form_geometry {
            self.form_scroll = self.form_scroll.clamp(0.0, geometry.max_scroll());
            if self.form_reveal_pending {
                self.reveal_form_focus(geometry);
            }
        }
        let s = form_geometry.map_or(self.scale, |geometry| geometry.scale);
        let pad = form_geometry.map_or(MODAL_PADDING * s, |geometry| geometry.pad);
        let radius = MODAL_CORNER_RADIUS * s;
        let (x, y, w, h) = self.modal_rect(window_width, scale_factor);
        let body_lines = self.visible_body_lines();
        self.invite_hit_rects.clear();
        let text_clip = [
            x + pad,
            y + pad,
            (w - pad * 2.0).max(0.0),
            (h - pad * 2.0).max(0.0),
        ];

        // Backdrop intentionally elided — the modal body itself is
        // solid, so the user can still read what's behind the popup
        // (matches the `:` command palette pattern the user asked us
        // to match across overlays).
        let _ = (window_width, window_height, scale_factor);

        sugarloaf.rounded_rect(
            None,
            x,
            y,
            w,
            h,
            theme.f32(theme.panel_bg()),
            DEPTH_BG,
            radius,
            ORDER,
        );

        let title_opts = DrawOpts {
            font_size: TITLE_FONT_SIZE * s,
            color: theme.u8(theme.fg),
            bold: true,
            clip_rect: Some(text_clip),
            ..DrawOpts::default()
        };
        let body_opts = DrawOpts {
            font_size: BODY_FONT_SIZE * s,
            color: theme.u8(theme.dim),
            clip_rect: Some(text_clip),
            ..DrawOpts::default()
        };
        let meta_opts = DrawOpts {
            font_size: META_FONT_SIZE * s,
            color: theme.u8(theme.muted),
            clip_rect: Some(text_clip),
            ..DrawOpts::default()
        };

        let text_x = x + pad;
        let mut text_y = y + pad;
        let text_w = (w - pad * 2.0).max(0.0);
        let title = truncate_to_fit(self.title.as_str(), (text_w - 28.0 * s).max(0.0), sugarloaf, &title_opts);
        sugarloaf
            .text_mut()
            .draw(text_x, text_y, title.as_str(), &title_opts);
        let close_size = if form_geometry.is_some() {
            TITLE_HEIGHT
        } else {
            36.0
        } * s;
        let close = [
            x + w - close_size - 6.0 * s,
            y + if form_geometry.is_some() {
                pad
            } else {
                6.0 * s
            },
            close_size,
            close_size,
        ];
        let close = crate::primitives::geom::intersect_rect(close, [x, y, w, h])
            .unwrap_or([x, y, 0.0, 0.0]);
        self.close_rect = self.dismissible.then_some(close);
        let close_opts = DrawOpts {
            clip_rect: Some(close),
            font_size: 18.0 * s,
            color: if self.hovered_target == Some(ModalHoverTarget::Close) {
                theme.u8(theme.fg)
            } else {
                theme.u8(theme.dim)
            },
            ..DrawOpts::default()
        };
        if self.dismissible {
            let hover = self.hover_intensity(ModalHoverTarget::Close);
            if hover > 0.0 {
                sugarloaf.rounded_rect(
                    None,
                    close[0],
                    close[1],
                    close[2],
                    close[3],
                    theme.f32_alpha(theme.hover, hover * 0.9),
                    DEPTH_ELEMENT,
                    6.0 * s,
                    ORDER + 1,
                );
            }
            sugarloaf.text_mut().draw(
                close[0]
                    + if form_geometry.is_some() {
                        3.0 * s
                    } else {
                        11.0 * s
                    },
                close[1]
                    + if form_geometry.is_some() {
                        3.0 * s
                    } else {
                        8.0 * s
                    },
                "×",
                &close_opts,
            );
        }

        // Segmented mode slider, centered in the title row (the server
        // modal's Join ↔ Create switch). Draws inside the row the title
        // already reserves, so no layout below shifts.
        self.form_tab_rects.clear();
        if self.form.is_some() && !self.form_tabs.is_empty() {
            let tab_opts = DrawOpts {
                font_size: BODY_FONT_SIZE * s,
                color: theme.u8(theme.fg),
                clip_rect: Some(text_clip),
                ..DrawOpts::default()
            };
            let pad_x = 10.0 * s;
            let gap = 6.0 * s;
            let tab_h = (TITLE_HEIGHT * s - 4.0 * s).max(14.0 * s);
            let widths: Vec<f32> = self
                .form_tabs
                .iter()
                .map(|label| sugarloaf.text_mut().measure(label, &tab_opts) + pad_x * 2.0)
                .collect();
            let total: f32 = widths.iter().sum::<f32>() + gap * (widths.len() - 1) as f32;
            let mut tab_x = x + (w - total) * 0.5;
            let tab_y = y + pad - 2.0 * s;
            for (index, label) in self.form_tabs.iter().enumerate() {
                let tab_w = widths[index];
                let active = index == self.form_active_tab;
                let hover = self.hover_intensity(ModalHoverTarget::Tab(index));
                let Some(tab_rect) = crate::primitives::geom::intersect_rect(
                    [tab_x, tab_y, tab_w, tab_h],
                    [x, y, w, h],
                ) else {
                    self.form_tab_rects.push([0.0; 4]);
                    tab_x += tab_w + gap;
                    continue;
                };
                sugarloaf.rounded_rect(
                    None,
                    tab_rect[0],
                    tab_rect[1],
                    tab_rect[2],
                    tab_rect[3],
                    if !self.form_tabs_enabled {
                        theme.f32_alpha(
                            if active { theme.accent } else { theme.hover },
                            0.12,
                        )
                    } else if active {
                        theme.f32_alpha(theme.accent, 0.30)
                    } else {
                        theme.f32_alpha(theme.hover, 0.35 + hover * 0.55)
                    },
                    DEPTH_ELEMENT,
                    tab_h * 0.5,
                    ORDER + 2,
                );
                let label_opts = DrawOpts {
                    color: if !self.form_tabs_enabled {
                        theme.u8(theme.dim)
                    } else if active {
                        theme.u8(theme.fg)
                    } else {
                        theme.u8(theme.muted)
                    },
                    ..tab_opts
                };
                sugarloaf.text_mut().draw(
                    tab_x + pad_x,
                    tab_y + (tab_h - BODY_FONT_SIZE * s) * 0.5,
                    label,
                    &label_opts,
                );
                if let Some(rect) = crate::primitives::geom::intersect_rect(
                    [tab_x, tab_y, tab_w, tab_h],
                    [x, y, w, h],
                ) {
                    self.form_tab_rects.push(rect);
                }
                tab_x += tab_w + gap;
            }
        }

        text_y +=
            TITLE_HEIGHT * s + form_geometry.map_or(12.0 * s, |geometry| geometry.gap);
        self.form_switch_rect = None;
        if let Some(switch) = self.form_switch.as_ref() {
            let text_clip = form_geometry
                .filter(|geometry| geometry.switch_in_body)
                .map_or(text_clip, |geometry| geometry.body);
            if let Some(geometry) = form_geometry {
                text_y = geometry.switch.map_or(text_y, |rect| rect[1])
                    - if geometry.switch_in_body {
                        self.form_scroll
                    } else {
                        0.0
                    };
            }
            let row = [text_x, text_y, text_w, 28.0 * s];
            self.form_switch_rect =
                crate::primitives::geom::intersect_rect(row, text_clip);
            let focused = self.form_switch_focused();
            let hover = if switch.enabled {
                self.hover_intensity(ModalHoverTarget::Switch)
            } else {
                0.0
            };
            // SettingsPage's 40×22 pill and 16px round knob, clipped to the modal.
            let mut draw = |rect: [f32; 4], color: [f32; 4], radius: f32, order: u8| {
                if let Some(r) = crate::primitives::geom::intersect_rect(rect, text_clip)
                {
                    sugarloaf.rounded_rect(
                        None,
                        r[0],
                        r[1],
                        r[2],
                        r[3],
                        color,
                        DEPTH_ELEMENT,
                        radius.min(r[2] * 0.5).min(r[3] * 0.5),
                        order,
                    );
                }
            };
            if focused || hover > 0.0 {
                draw(
                    row,
                    theme.f32_alpha(
                        theme.accent,
                        if focused { 0.18 } else { hover * 0.08 },
                    ),
                    6.0 * s,
                    ORDER + 1,
                );
            }
            let tx = text_x + text_w - 40.0 * s;
            let ty = text_y + 3.0 * s;
            if focused {
                draw(
                    [tx - 2.0 * s, ty - 2.0 * s, 44.0 * s, 26.0 * s],
                    theme.f32(theme.accent),
                    13.0 * s,
                    ORDER + 2,
                );
            }
            draw(
                [tx, ty, 40.0 * s, 22.0 * s],
                theme.f32(if !switch.enabled {
                    theme.surface
                } else if switch.checked {
                    theme.accent
                } else {
                    theme.border
                }),
                11.0 * s,
                ORDER + 3,
            );
            let kx = tx + if switch.checked { 21.0 * s } else { 3.0 * s };
            draw(
                [kx, ty + 3.0 * s, 16.0 * s, 16.0 * s],
                theme.f32(if !switch.enabled {
                    theme.muted
                } else if switch.checked {
                    theme.fg
                } else {
                    theme.dim
                }),
                8.0 * s,
                ORDER + 4,
            );
            let opts = DrawOpts {
                font_size: INPUT_FONT_SIZE * s,
                color: theme.u8(if switch.enabled {
                    theme.fg
                } else {
                    theme.muted
                }),
                clip_rect: crate::primitives::geom::intersect_rect(
                    [text_x, text_y, (text_w - 52.0 * s).max(0.0), 28.0 * s],
                    text_clip,
                ),
                ..DrawOpts::default()
            };
            if opts.clip_rect.is_some() {
                let label = truncate_to_fit(
                    &switch.label,
                    (text_w - 52.0 * s).max(0.0),
                    sugarloaf,
                    &opts,
                );
                sugarloaf.text_mut().draw(
                    text_x + 6.0 * s,
                    text_y + 7.0 * s,
                    &label,
                    &opts,
                );
            }
        }
        if let Some(geometry) = form_geometry {
            self.draw_form_body(sugarloaf, theme, geometry);
            self.draw_modal_actions(
                sugarloaf,
                theme,
                [x, y, w, h],
                (s, pad),
                form_geometry,
            );
            return;
        }
        let body_view_y = text_y;
        let body_view_h = self.visible_body_line_count() as f32 * BODY_LINE_HEIGHT * s;
        let body_clip = crate::primitives::geom::intersect_rect(
            [text_x, body_view_y, text_w, body_view_h], text_clip,
        ).unwrap_or([text_x, body_view_y, 0.0, 0.0]);
        if body_lines.is_empty() {
            sugarloaf.text_mut().draw(
                text_x,
                text_y,
                "No details available.",
                &body_opts,
            );
            text_y += BODY_LINE_HEIGHT * s;
        } else {
            for line in body_lines {
                if self.invite_url.as_deref() == Some(line.text.as_str()) {
                    let rect = [text_x, text_y, text_w, BODY_LINE_HEIGHT * s];
                    self.invite_hit_rects.push(rect);
                    let hover = self.hover_intensity(ModalHoverTarget::Invite);
                    if hover > 0.0 {
                        sugarloaf.rounded_rect(None, rect[0], rect[1], rect[2], rect[3],
                            theme.f32_alpha(theme.hover, hover), DEPTH_ELEMENT, 4.0 * s, ORDER);
                    }
                    let opts = DrawOpts { color: theme.u8(theme.accent), ..body_opts.clone() };
                    let label = truncate_to_fit(&line.text, text_w, sugarloaf, &opts);
                    sugarloaf.text_mut().draw(text_x, text_y, &label, &opts);
                    let width = sugarloaf.text_mut().measure(&label, &opts);
                    sugarloaf.rect(None, text_x, text_y + BODY_FONT_SIZE * s + 2.0 * s,
                        width, s, theme.f32_alpha(theme.accent, 0.5), DEPTH_ELEMENT, ORDER);
                    text_y += BODY_LINE_HEIGHT * s;
                    continue;
                }
                self.draw_body_line(
                    sugarloaf, theme, &line, text_x, text_y, text_w, body_clip, s,
                );
                text_y += BODY_LINE_HEIGHT * s * line.row_span as f32;
            }
        }

        let total_body_lines = self.body_lines().len();
        let body_budget = self.body_row_budget();
        if total_body_lines > body_budget {
            if let Some((thumb_y, thumb_h)) = scrollbar::compute_thumb(
                body_budget,
                total_body_lines,
                body_view_y,
                body_view_h,
                self.body_scroll_normalized(),
            ) {
                let bar_x = text_x + text_w - scrollbar::width();
                scrollbar::draw_track(
                    sugarloaf,
                    bar_x,
                    body_view_y,
                    body_view_h,
                    0.95,
                    DEPTH_ELEMENT + 0.05,
                    ORDER + 2,
                );
                scrollbar::draw_thumb(
                    sugarloaf,
                    bar_x,
                    thumb_y,
                    thumb_h,
                    0.95,
                    false,
                    DEPTH_ELEMENT + 0.05,
                    ORDER + 2,
                );
            }
        }

        if !self.meta.is_empty() {
            let loading = self.activity.is_some() && self.busy;
            let inset = if loading { 18.0 * s } else { 0.0 };
            if loading {
                let now = web_time::SystemTime::now()
                    .duration_since(web_time::UNIX_EPOCH)
                    .unwrap_or_default();
                let phase = crate::render_policy::animation_phase_from_unix_secs(
                    now.as_secs(), now.subsec_nanos(),
                );
                for (trail, alpha) in crate::render_policy::RUNNING_SQUARE_ALPHAS.into_iter().enumerate() {
                    let (dx, dy) = crate::render_policy::running_square_position(phase, trail, s);
                    draw_form_rect(
                        sugarloaf,
                        [text_x + 5.0 * s + dx - s, text_y + META_FONT_SIZE * s * 0.5 + dy - s, 2.0 * s, 2.0 * s],
                        text_clip, theme.f32_alpha(theme.fg, alpha), 0.0, ORDER + 1,
                    );
                }
            }
            let opts = if self.activity.is_some() {
                DrawOpts { color: theme.u8(theme.dim), ..meta_opts.clone() }
            } else {
                meta_opts.clone()
            };
            let meta = truncate_to_fit(self.meta.as_str(), (text_w - inset).max(0.0), sugarloaf, &opts);
            sugarloaf.text_mut().draw(text_x + inset, text_y, meta.as_str(), &opts);
            text_y += 20.0 * s;
        }

        if self.activity.is_none() && (self.busy || self.progress.is_some()) {
            let track_y = text_y + 4.0 * s;
            let track_w = w - pad * 2.0;
            sugarloaf.rounded_rect(
                None,
                text_x,
                track_y,
                track_w,
                BUSY_BAR_HEIGHT * s,
                theme.f32(theme.hover),
                DEPTH_ELEMENT,
                2.0 * s,
                ORDER,
            );

            let (fill_x, fill_w) = if let Some(progress) = self.progress {
                (text_x, track_w * progress as f32 / 100.0)
            } else {
                let elapsed = Instant::now()
                    .saturating_duration_since(self.opened_at)
                    .as_millis() as f32;
                let segment_w = (BUSY_BAR_WIDTH * s).min(track_w * 0.6);
                let travel = (track_w - segment_w).max(1.0);
                let phase = ((elapsed / 1100.0) % 1.0) * travel;
                (text_x + phase, segment_w)
            };
            sugarloaf.rounded_rect(
                None,
                fill_x,
                track_y,
                fill_w,
                BUSY_BAR_HEIGHT * s,
                theme.f32(theme.accent),
                DEPTH_ELEMENT + 0.01,
                2.0 * s,
                ORDER + 1,
            );
            text_y += BUSY_BAR_HEIGHT * s + 12.0 * s;
        }

        if let Some(input) = self.input.as_ref() {
            let input_y = text_y + 6.0 * s;
            let input_h = if self.markdown_input {
                MARKDOWN_INPUT_HEIGHT * s
            } else {
                INPUT_ROW_HEIGHT * s
            };
            sugarloaf.rounded_rect(
                None,
                text_x,
                input_y,
                w - pad * 2.0,
                input_h,
                theme.f32(theme.hover),
                DEPTH_ELEMENT,
                5.0 * s,
                ORDER,
            );
            let hover = self.hover_intensity(ModalHoverTarget::Input);
            if hover > 0.0 {
                sugarloaf.rounded_rect(
                    None,
                    text_x,
                    input_y,
                    w - pad * 2.0,
                    input_h,
                    theme.f32_alpha(theme.accent, hover * 0.08),
                    DEPTH_ELEMENT + 0.01,
                    5.0 * s,
                    ORDER + 1,
                );
            }
            self.input_hit_rects
                .push([text_x, input_y, w - pad * 2.0, input_h]);
            sugarloaf.rounded_rect(
                None,
                text_x,
                input_y,
                2.0 * s,
                input_h,
                theme.f32(theme.accent),
                DEPTH_ELEMENT + 0.01,
                1.0 * s,
                ORDER + 1,
            );

            let markdown_source = self
                .markdown_pane
                .as_ref()
                .map(|pane| pane.lines.join("\n"));
            let raw_input = markdown_source.as_deref().unwrap_or(input.value.as_str());
            let is_placeholder = raw_input.is_empty();
            let input_text = if is_placeholder {
                input.placeholder.as_str()
            } else {
                raw_input
            };
            let input_opts = DrawOpts {
                font_size: INPUT_FONT_SIZE * s,
                color: if is_placeholder {
                    theme.u8(theme.muted)
                } else {
                    theme.u8(theme.fg)
                },
                clip_rect: Some([
                    text_x + 12.0 * s,
                    input_y,
                    (w - pad * 2.0 - 24.0 * s).max(0.0),
                    input_h,
                ]),
                ..DrawOpts::default()
            };
            let available = (w - pad * 2.0 - 24.0 * s).max(0.0);
            let input_fit =
                truncate_to_fit(input_text, available, sugarloaf, &input_opts);
            let input_text_x = text_x + 12.0 * s;
            let input_text_y = if self.markdown_input {
                input_y + 10.0 * s
            } else {
                input_y + (input_h - INPUT_FONT_SIZE * s) / 2.0
            };
            if !self.markdown_input {
                sugarloaf.text_mut().draw(
                    input_text_x,
                    input_text_y,
                    input_fit.as_str(),
                    &input_opts,
                );
            }
            let caret_offset = if self.markdown_input || is_placeholder {
                0.0
            } else {
                // Measure only up to the caret within the fitted text so
                // arrow-key movement is visible mid-string.
                let caret_prefix = if self.markdown_input {
                    input_text
                        .chars()
                        .take(self.input_caret)
                        .collect::<String>()
                        .rsplit_once('\n')
                        .map(|(_, tail)| tail.to_string())
                        .unwrap_or_else(|| {
                            input_text.chars().take(self.input_caret).collect()
                        })
                } else {
                    input_fit.chars().take(self.input_caret).collect::<String>()
                };
                sugarloaf
                    .text_mut()
                    .measure(caret_prefix.as_str(), &input_opts)
            };
            // Same thick translucent block as the form fields / editor
            // cursor — one caret silhouette across the app.
            let caret_h = (INPUT_FONT_SIZE * s + 6.0 * s).min(input_h - 6.0 * s);
            if !self.markdown_input {
                sugarloaf.rect(
                    None,
                    input_text_x + caret_offset,
                    input_y + (input_h - caret_h) / 2.0,
                    INPUT_FONT_SIZE * 0.55 * s,
                    caret_h,
                    theme.f32_alpha(theme.accent, 0.55),
                    DEPTH_ELEMENT + 0.02,
                    ORDER + 2,
                );
            }
            if let Some(pane) = self.markdown_pane.as_mut() {
                let editor_rect = [
                    text_x + 2.0 * s,
                    input_y + 2.0 * s,
                    w - pad * 2.0 - 4.0 * s,
                    input_h - 4.0 * s,
                ];
                self.markdown_input_rect = Some(editor_rect);
                crate::editor::markdown::render::render(
                    sugarloaf,
                    pane,
                    editor_rect,
                    theme,
                    None,
                    &[],
                    s,
                    0.0,
                );
                if let Some([caret_x, caret_y, _, caret_h]) = pane.cursor_rect {
                    sugarloaf.rect(
                        None,
                        caret_x,
                        caret_y,
                        (1.6 * s).max(1.0),
                        caret_h.max(16.0 * s),
                        theme.f32(theme.accent),
                        DEPTH_ELEMENT + 0.03,
                        ORDER + 3,
                    );
                }
            }
        }

        self.draw_modal_actions(
            sugarloaf,
            theme,
            [x, y, w, h],
            (s, pad),
            None,
        );
    }

    fn draw_modal_actions(
        &self,
        sugarloaf: &mut (impl crate::primitives::paint::Paint + ?Sized),
        theme: &IdeTheme,
        panel: [f32; 4],
        metrics: (f32, f32),
        form_geometry: Option<FormGeometry>,
    ) {
        let [x, y, w, _] = panel;
        let (s, pad) = metrics;
        let actions_y = form_geometry
            .map_or_else(|| self.actions_y(y), |geometry| geometry.actions[1]);
        let sep_y =
            form_geometry.map_or(actions_y - 4.0 * s - SEPARATOR_HEIGHT, |geometry| {
                (actions_y - 4.0 * s - SEPARATOR_HEIGHT)
                    .max(geometry.body[1] + geometry.body[3])
            });
        draw_form_rect(
            sugarloaf,
            [x + pad, sep_y, w - pad * 2.0, SEPARATOR_HEIGHT],
            panel,
            theme.f32(theme.border),
            0.0,
            ORDER,
        );

        let row_x = x + pad;
        let row_w = w - pad * 2.0;
        let row_h = form_geometry
            .map_or(ACTION_ROW_HEIGHT * s, |geometry| geometry.action_row_height);
        let action_clip = form_geometry.map_or(
            [
                row_x,
                actions_y,
                row_w,
                self.visible_action_count() as f32 * row_h,
            ],
            |geometry| geometry.actions,
        );
        let hint_opts = DrawOpts {
            font_size: META_FONT_SIZE * s,
            color: theme.u8(theme.muted),
            clip_rect: Some(action_clip),
            ..DrawOpts::default()
        };

        let painted_row = |row_y: f32| {
            crate::primitives::geom::intersect_rect(
                [row_x, row_y, row_w, row_h],
                action_clip,
            )
        };
        for (display_idx, button) in self
            .buttons
            .iter()
            .skip(self.scroll_offset)
            .take(self.visible_action_count())
            .enumerate()
        {
            let actual_idx = self.scroll_offset + display_idx;
            let row_y = actions_y + display_idx as f32 * row_h;
            let Some(painted) = painted_row(row_y) else {
                continue;
            };
            // In a FORM, the button highlight only shows once focus has
            // actually walked onto the button row — while typing in a
            // field, a pre-lit "Enter" reads as focused when it isn't.
            // Plain modals (button lists) keep the always-on selection.
            let selected = actual_idx == self.selected_index
                && (self.form.is_none() || self.form_buttons_focused());
            let destructive = button.action.is_destructive();
            if selected {
                sugarloaf.rounded_rect(
                    None,
                    painted[0],
                    painted[1],
                    painted[2],
                    painted[3],
                    if destructive {
                        theme.f32_alpha(theme.red, 0.14)
                    } else {
                        theme.f32(theme.hover)
                    },
                    DEPTH_ELEMENT,
                    8.0 * s,
                    ORDER,
                );
            }
            let hover = self.hover_intensity(ModalHoverTarget::Action(actual_idx));
            if hover > 0.0 {
                sugarloaf.rounded_rect(
                    None,
                    painted[0],
                    painted[1],
                    painted[2],
                    painted[3],
                    if destructive {
                        theme.f32_alpha(theme.red, 0.08 + hover * 0.12)
                    } else {
                        theme.f32_alpha(theme.hover, 0.5 + hover * 0.5)
                    },
                    DEPTH_ELEMENT + 0.01,
                    8.0 * s,
                    ORDER + 1,
                );
            }

            let label_x = row_x + 14.0 * s;
            let label_y = row_y + (row_h - ACTION_FONT_SIZE * s) / 2.0;
            let label_opts = DrawOpts {
                font_size: ACTION_FONT_SIZE * s,
                color: if destructive {
                    theme.u8(theme.red)
                } else {
                    theme.u8(theme.fg)
                },
                bold: selected || destructive || hover > 0.5,
                clip_rect: Some(action_clip),
                ..DrawOpts::default()
            };
            // A long detail must never consume the label's whole row (the
            // Mash Up picker exposed this with a zero-width selected name).
            let hint = truncate_to_fit(
                button.hint.as_str(),
                row_w * 0.58,
                sugarloaf,
                &hint_opts,
            );
            let hint_width = if hint.is_empty() {
                0.0
            } else {
                sugarloaf.text_mut().measure(&hint, &hint_opts)
            };
            let label_budget = (row_w - 28.0 * s - hint_width - 12.0 * s).max(0.0);
            let label = truncate_to_fit(
                button.label.as_str(),
                label_budget,
                sugarloaf,
                &label_opts,
            );
            sugarloaf
                .text_mut()
                .draw(label_x, label_y, label.as_str(), &label_opts);

            if !hint.is_empty() {
                let hint_x = row_x + row_w - 14.0 * s - hint_width;
                let hint_y = row_y + (row_h - META_FONT_SIZE * s) / 2.0;
                sugarloaf.text_mut().draw(hint_x, hint_y, &hint, &hint_opts);
            }
        }

    }

    fn draw_body_line(
        &mut self,
        sugarloaf: &mut (impl crate::primitives::paint::Paint + ?Sized),
        theme: &IdeTheme,
        line: &BodyLine,
        x: f32,
        y: f32,
        w: f32,
        clip: [f32; 4],
        s: f32,
    ) {
        let line_h = BODY_LINE_HEIGHT * s;
        match line.kind {
            BodyLineKind::Mermaid => {
                let Some(diagram) = line.mermaid.as_ref() else {
                    return;
                };
                let h = line_h * line.row_span as f32 - 8.0 * s;
                let rect = [x, y + 4.0 * s, w, h.max(line_h)];
                draw_form_rect(
                    sugarloaf, rect, clip, theme.f32_alpha(theme.black, 0.42),
                    10.0 * s, ORDER,
                );
                let scene = mermaid_scene(diagram, theme, s);
                let Some(bounds) = scene.bounds() else {
                    return;
                };
                let pad = 10.0 * s;
                let avail_w = (rect[2] - pad * 2.0).max(1.0);
                let avail_h = (rect[3] - pad * 2.0).max(1.0);
                let zoom = (avail_w / bounds.width().max(1.0))
                    .min(avail_h / bounds.height().max(1.0))
                    .min(2.0);
                let center = bounds.center();
                let camera = Camera {
                    pan: Vec2::new(
                        rect[0] + rect[2] * 0.5 - center.x * zoom,
                        rect[1] + rect[3] * 0.5 - center.y * zoom,
                    ),
                    zoom,
                };
                render_scene(sugarloaf, &scene, &camera, clip, DEPTH_ELEMENT, ORDER + 1);
            }
            BodyLineKind::Heading | BodyLineKind::Bullet | BodyLineKind::Text => {
                let opts = DrawOpts {
                    font_size: BODY_FONT_SIZE * s,
                    color: theme.u8(if line.kind == BodyLineKind::Heading { theme.fg } else { theme.dim }),
                    bold: line.kind == BodyLineKind::Heading,
                    clip_rect: Some(clip),
                    ..DrawOpts::default()
                };
                let mut text_x = x;
                if line.kind == BodyLineKind::Bullet {
                    if !line.continuation {
                        draw_icon_centered_with_occlusion(
                            sugarloaf, x, [x, y, BODY_FONT_SIZE * s, line_h],
                            "•", &opts, &[], true,
                        );
                    }
                    text_x += (BODY_FONT_SIZE + 8.0) * s;
                }
                let mut byte = 0;
                for link in &line.links {
                    if byte < link.range.start {
                        let text = &line.text[byte..link.range.start];
                        sugarloaf.text_mut().draw(text_x, y, text, &opts);
                        text_x += sugarloaf.text_mut().measure(text, &opts);
                    }
                    let label = &line.text[link.range.clone()];
                    let link_opts = DrawOpts { color: theme.u8(theme.fg), ..opts.clone() };
                    let width = sugarloaf.text_mut().measure(label, &link_opts);
                    let rect = [text_x, y, width, line_h];
                    if let Some(index) = self.record_body_link(&link.url, rect, clip) {
                        let hover = self.hover_intensity(ModalHoverTarget::Link(index));
                        sugarloaf.text_mut().draw(text_x, y, label, &link_opts);
                        draw_form_rect(
                            sugarloaf,
                            [text_x, y + (BODY_FONT_SIZE + 2.0) * s, width, s],
                            clip, theme.f32_alpha(theme.fg, 0.45 + hover * 0.45),
                            0.0, ORDER + 1,
                        );
                    }
                    text_x += width;
                    byte = link.range.end;
                }
                sugarloaf.text_mut().draw(text_x, y, &line.text[byte..], &opts);
            }
            BodyLineKind::Code(lang) => {
                draw_form_rect(
                    sugarloaf, [x, y - 2.0 * s, w, line_h], clip,
                    theme.f32_alpha(theme.surface, 0.55), 3.0 * s, ORDER,
                );
                let mut tx = x + 8.0 * s;
                for (tok, slice) in highlight_line(&line.text, lang) {
                    let opts = DrawOpts {
                        font_size: BODY_FONT_SIZE * s,
                        color: syn_color(tok, theme, false),
                        clip_rect: Some(clip),
                        ..DrawOpts::default()
                    };
                    tx += sugarloaf.text_mut().draw(tx, y, slice, &opts);
                    if tx > x + w {
                        break;
                    }
                }
            }
        }
    }
}

fn truncate_to_fit(
    text: &str,
    available_w: f32,
    sugarloaf: &mut (impl crate::primitives::paint::Paint + ?Sized),
    opts: &DrawOpts,
) -> String {
    if available_w <= 0.0 || text.is_empty() {
        return String::new();
    }
    if sugarloaf.text_mut().measure(text, opts) <= available_w {
        return text.to_string();
    }
    if sugarloaf.text_mut().measure("…", opts) >= available_w {
        return "…".to_string();
    }

    let chars: Vec<char> = text.chars().collect();
    let mut lo = 0usize;
    let mut hi = chars.len();
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let mut candidate: String = chars[..mid].iter().collect();
        candidate.push('…');
        if sugarloaf.text_mut().measure(&candidate, opts) <= available_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }

    let mut out: String = chars[..lo].iter().collect();
    out.push('…');
    out
}

fn markdown_heading(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let rest = trimmed
        .strip_prefix('#')?
        .trim_start_matches('#')
        .trim_start();
    (!rest.is_empty()).then_some(rest)
}

fn markdown_bullet(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("• "))
}

fn lang_from_fence(label: &str) -> Lang {
    let label = label
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_start_matches('.')
        .to_ascii_lowercase();
    match label.as_str() {
        "rs" | "rust" => Lang::Rust,
        "js" | "javascript" | "mjs" | "cjs" => Lang::Javascript,
        "jsx" => Lang::Jsx,
        "ts" | "typescript" => Lang::Typescript,
        "tsx" => Lang::Tsx,
        "py" | "python" => Lang::Python,
        "go" | "golang" => Lang::Go,
        "lua" => Lang::Lua,
        "toml" => Lang::Toml,
        "sql" => Lang::Sql,
        "json" | "jsonc" => Lang::Json,
        "md" | "markdown" => Lang::Markdown,
        _ => Lang::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preview_image() -> NeoismAgentImage {
        NeoismAgentImage {
            filename: "Artwork.png".into(),
            url: "data:image/png;base64,preview".into(),
            mime: "image/png".into(),
        }
    }

    #[test]
    fn image_fit_preserves_aspect_and_spacious_margins() {
        for aspect in [0.01, 0.5, 1.0, 2.0, 100.0] {
            for (viewport, scale) in [([1200.0, 800.0], 1.0), ([320.0, 480.0], 2.0), ([24.0, 16.0], 3.0)] {
                let [x, y, w, h] = image_aspect_fit(viewport, aspect, scale).unwrap();
                assert!((w / h - aspect).abs() < aspect * 0.0001);
                assert!((x * 2.0 + w - viewport[0]).abs() < 0.001);
                assert!((y * 2.0 + h - viewport[1]).abs() < 0.001);
                assert!(x >= (48.0 * scale).min(viewport[0] * 0.12) - 0.001);
                assert!(y >= (72.0 * scale).min(viewport[1] * 0.22) - 0.001);
            }
        }
        assert_eq!(image_aspect_fit([1200.0, 800.0], 2.0, 1.0), Some([48.0, 124.0, 1104.0, 552.0]));
    }

    #[test]
    fn image_fit_rejects_invalid_geometry() {
        for aspect in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(image_aspect_fit([100.0, 100.0], aspect, 1.0).is_none());
        }
        for viewport in [[0.0, 100.0], [100.0, -1.0], [f32::INFINITY, 100.0], [100.0, f32::NAN]] {
            assert!(image_aspect_fit(viewport, 1.0, 1.0).is_none());
        }
        assert!(image_aspect_fit([100.0, 100.0], 1.0, 0.0).is_none());
    }

    #[test]
    fn image_mode_owns_focus_escape_and_wheel_without_an_input() {
        let mut modal = UniversalModal::new();
        modal.open_image(preview_image());
        assert!(modal.is_active() && modal.is_image_open() && modal.is_blocking());
        assert!(modal.owns_editor_focus());
        assert!(!modal.has_input());
        assert_eq!(modal.escape_action(), Some(ModalAction::Close));
        assert!(!modal.close_if_non_blocking());
        assert!(modal.scroll_at(0.0, 0.0, 1200.0, 2.0, -120.0));
        assert!(modal.scroll_at(500.0, 300.0, 1200.0, 2.0, 120.0));
        assert_eq!(modal.body_scroll_offset, 0);
        assert_eq!(modal.scroll_offset, 0);
        assert_eq!(modal.active_rect(1200.0, 2.0), Some([0.0, 0.0, 600.0, 720.0]));
    }

    #[test]
    fn image_hits_are_inert_and_backdrop_or_close_dismiss() {
        let mut modal = UniversalModal::new();
        modal.open_image(preview_image());
        modal.image_rect = Some([100.0, 100.0, 400.0, 300.0]);
        modal.close_rect = Some([550.0, 20.0, 32.0, 32.0]);
        assert_eq!(modal.hit_test(200.0, 200.0, 1200.0, 2.0), Ok(None));
        assert_eq!(modal.hit_test(10.0, 200.0, 1200.0, 2.0), Err(()));
        assert_eq!(modal.hit_test(560.0, 30.0, 1200.0, 2.0), Ok(Some(0)));
        modal.pointer_move(560.0, 30.0, 1200.0, 2.0);
        assert_eq!(modal.pointer_kind(), ModalPointerKind::Pointer);
    }

    #[test]
    fn image_lifecycle_resets_on_close_normal_open_and_replacement() {
        let mut modal = UniversalModal::new();
        modal.open_image(preview_image());
        modal.image_rect = Some([100.0, 100.0, 400.0, 300.0]);
        modal.open_message("Ordinary", "Body");
        assert!(!modal.is_image_open());
        assert!(modal.image.is_none() && modal.image_rect.is_none());
        modal.open_image(preview_image());
        modal.image_rect = Some([100.0, 100.0, 400.0, 300.0]);
        modal.open_image(preview_image());
        assert!(modal.image_rect.is_none());
        modal.close();
        assert!(!modal.is_active() && !modal.is_image_open());
        assert!(modal.image.is_none() && modal.image_rect.is_none());
        assert!(modal.close_rect.is_none());
        assert!(!modal.owns_editor_focus());
        assert!(!modal.scroll_at(0.0, 0.0, 1200.0, 1.0, 120.0));
    }

    #[test]
    fn epub_note_modal_uses_markdown_pane_and_serializes_newlines() {
        let mut modal = UniversalModal::new();
        modal.open(ModalSpec {
            title: "Note".into(),
            body: String::new(),
            meta: String::new(),
            input: Some(ModalInputSpec {
                value: "# Heading".into(),
                placeholder: String::new(),
            }),
            buttons: vec![ModalButton::new(
                "Save",
                "Enter",
                ModalAction::EpubUpdateAnnotation {
                    id: "annotation-1".into(),
                    value: String::new(),
                },
            )],
            busy: false,
            blocking: true,
        });

        assert!(modal.has_markdown_input());
        modal.insert_input_newline();
        modal.push_input("- item");
        assert_eq!(
            modal.markdown_input_source().as_deref(),
            Some("# Heading\n- item")
        );
        assert!(matches!(
            modal.selected_action(),
            Some(ModalAction::EpubUpdateAnnotation { value, .. }) if value == "# Heading\n- item"
        ));
    }

    #[test]
    fn epub_note_modal_scrolls_long_markdown_inside_editor_rect() {
        let mut modal = UniversalModal::new();
        modal.open(ModalSpec {
            title: "Note".into(),
            body: String::new(),
            meta: String::new(),
            input: Some(ModalInputSpec {
                value: (0..40)
                    .map(|line| format!("line {line}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                placeholder: String::new(),
            }),
            buttons: vec![ModalButton::new(
                "Save",
                "Ctrl+Enter",
                ModalAction::EpubUpdateAnnotation {
                    id: "annotation-1".into(),
                    value: String::new(),
                },
            )],
            busy: false,
            blocking: true,
        });
        modal.markdown_input_rect = Some([10.0, 10.0, 400.0, 180.0]);
        let pane = modal.markdown_pane.as_mut().unwrap();
        pane.set_content_height(900.0, 180.0);

        assert!(modal.scroll_markdown_input(20.0, 20.0, -120.0));
        assert!(modal.markdown_pane.as_ref().unwrap().scroll_y > 0.0);
        assert!(!modal.scroll_markdown_input(500.0, 500.0, -120.0));
    }

    #[test]
    fn repeated_newlines_keep_embedded_markdown_in_follow_cursor_mode() {
        let mut modal = UniversalModal::new();
        modal.open(ModalSpec {
            title: "Note".into(),
            body: String::new(),
            meta: String::new(),
            input: Some(ModalInputSpec {
                value: String::new(),
                placeholder: String::new(),
            }),
            buttons: vec![ModalButton::new(
                "Save",
                "Ctrl+Enter",
                ModalAction::EpubUpdateAnnotation {
                    id: "annotation-1".into(),
                    value: String::new(),
                },
            )],
            busy: false,
            blocking: true,
        });

        for _ in 0..30 {
            modal.insert_input_newline();
        }
        let pane = modal.markdown_pane.as_ref().unwrap();
        assert_eq!(pane.cursor_line, 30);
        assert_eq!(pane.lines.len(), 31);
    }

    #[test]
    fn system_form_preserves_custom_submit_action_and_values() {
        let mut modal = UniversalModal::default();
        modal.open_form_with_action(switch_test_form(), ModalAction::SystemFormSubmit);
        assert_eq!(modal.submit_form(), Some(ModalAction::SystemFormSubmit));
        assert!(modal.take_submitted_form().is_some());
        assert!(
            modal.has_action(|action| matches!(action, ModalAction::SystemFormSubmit))
        );
        modal.open_form(switch_test_form());
        assert_eq!(modal.submit_form(), Some(ModalAction::ServerFormSubmit));
    }

    fn switch_test_form() -> ModalFormSpec {
        ModalFormSpec {
            title: "Add Server".into(),
            fields: vec![
                ModalFormField {
                    id: "id".into(),
                    label: String::new(),
                    value: "hidden".into(),
                    placeholder: String::new(),
                    secret: true,
                },
                ModalFormField {
                    id: "address".into(),
                    label: "Address".into(),
                    value: "host".into(),
                    placeholder: String::new(),
                    secret: false,
                },
                ModalFormField {
                    id: "password".into(),
                    label: "Password".into(),
                    value: "secret".into(),
                    placeholder: String::new(),
                    secret: true,
                },
            ],
            submit_label: "Join".into(),
        }
    }

    #[test]
    fn form_switch_focus_cycle_and_arrows_skip_hidden_fields() {
        let mut modal = UniversalModal::new();
        modal.open_form(switch_test_form());
        modal.set_form_switch("Fast tunnel", false, true);
        let draft = modal.form_values();
        assert_eq!(modal.form_focus, 1);
        modal.focus_next_form_field();
        assert_eq!(modal.form_focus, 2);
        modal.focus_next_form_field();
        assert!(modal.form_switch_focused());
        assert!(!modal.form_buttons_focused());
        modal.push_input("ignored");
        assert_eq!(modal.form_values(), draft);
        assert_eq!(modal.activate_form_switch(), Some(true));
        assert!(modal.take_submitted_form().is_none());
        modal.focus_next_form_field();
        assert!(modal.form_buttons_focused());
        modal.focus_previous_form_field();
        assert!(modal.form_switch_focused());
        modal.focus_previous_form_field();
        assert_eq!(modal.form_focus, 2);
        modal.focus_previous_form_field();
        assert_eq!(modal.form_focus, 1);
        modal.form_focus_move(true);
        assert_eq!(modal.form_focus, 2);
        modal.form_focus_move(true);
        assert!(modal.form_switch_focused());
        modal.form_focus_move(true);
        assert!(modal.form_buttons_focused());
        modal.form_focus_move(false);
        assert!(modal.form_switch_focused());
        modal.form_focus_move(false);
        assert_eq!(modal.form_focus, 2);
        modal.form_focus_move(false);
        assert_eq!(modal.form_focus, 1);
        assert_eq!(modal.form_values(), draft);
        modal.set_form_switch("Fast tunnel", true, false);
        modal.focus_next_form_field();
        assert_eq!(modal.form_focus, 2);
        modal.focus_next_form_field();
        assert!(modal.form_buttons_focused());
        modal.focus_previous_form_field();
        assert_eq!(modal.form_focus, 2);
        modal.form_focus_move(true);
        assert!(modal.form_buttons_focused());
        modal.form_focus_move(false);
        assert_eq!(modal.form_focus, 2);
    }

    #[test]
    fn form_switch_and_tab_locks_reject_activation_and_pointer() {
        let mut modal = UniversalModal::new();
        assert_eq!(modal.activate_form_switch(), None);
        modal.open_form(switch_test_form());
        modal.set_form_tabs(vec!["Host".into(), "Join".into()], 1);
        modal.form_tab_rects = vec![[10.0, 10.0, 40.0, 22.0], [60.0, 10.0, 40.0, 22.0]];
        modal.set_form_switch("Fast tunnel", false, true);
        modal.form_switch_rect = Some([10.0, 40.0, 100.0, 28.0]);
        assert!(modal.form_switch_hit(20.0, 50.0));
        assert_eq!(modal.activate_form_switch(), Some(true));
        modal.set_form_switch("Fast tunnel", true, false);
        modal.set_form_tabs_enabled(false);
        assert_eq!(modal.form_switch_checked(), Some(true));
        assert_eq!(modal.activate_form_switch(), None);
        assert!(!modal.form_switch_hit(20.0, 50.0));
        assert!(!modal.form_switch_focused());
        assert_eq!(modal.form_tab_hit(20.0, 20.0), None);
        assert!(!modal.set_form_active_tab(0));
        assert_eq!(modal.form_active_tab(), 1);
        modal.focus_previous_form_field();
        assert!(!modal.form_switch_focused());
        modal.pointer_move(20.0, 50.0, 1200.0, 1.0);
        assert_eq!(modal.pointer_kind(), ModalPointerKind::Default);
    }

    fn long_scroll_form() -> ModalFormSpec {
        let mut spec = switch_test_form();
        for index in 0..8 {
            spec.fields.push(ModalFormField {
                id: format!("extra-{index}"),
                label: format!("Field {index}"),
                value: String::new(),
                placeholder: String::new(),
                secret: false,
            });
        }
        spec
    }

    fn assert_rect_inside(rect: [f32; 4], panel: [f32; 4]) {
        let epsilon = 0.01;
        assert!(rect[0] >= panel[0] - epsilon && rect[1] >= panel[1] - epsilon);
        assert!(rect[0] + rect[2] <= panel[0] + panel[2] + epsilon);
        assert!(rect[1] + rect[3] <= panel[1] + panel[3] + epsilon);
    }

    #[test]
    fn empty_form_description_has_no_rows_or_reserved_space() {
        for scale in [0.75, 1.0, 2.0, 3.0] {
            let mut modal = UniversalModal::new();
            modal.set_scale(scale);
            modal.viewport_height = 2000.0;
            let mut spec = long_scroll_form();
            spec.fields.truncate(2);
            modal.open_form(spec);
            let empty = modal.form_geometry(2400.0, 1.0).unwrap();
            assert!(modal.form_description_lines().is_empty());
            assert_eq!(empty.fields_start, 0.0);

            modal.set_form_description(" \n\t\n ");
            let whitespace = modal.form_geometry(2400.0, 1.0).unwrap();
            assert!(modal.form_description_lines().is_empty());
            assert_eq!(whitespace.fields_start, 0.0);
            assert_eq!(whitespace.panel, empty.panel);
            assert_eq!(whitespace.content_height, empty.content_height);

            modal.set_form_description("First line\nSecond line");
            let described = modal.form_geometry(2400.0, 1.0).unwrap();
            let description_height = 2.0 * BODY_LINE_HEIGHT * scale;
            assert_eq!(described.fields_start, description_height);
            assert!((described.panel[3] - empty.panel[3] - description_height).abs() < 0.01);

            modal.set_form_description("");
            let cleared = modal.form_geometry(2400.0, 1.0).unwrap();
            assert_eq!(cleared.fields_start, 0.0);
            assert_eq!(cleared.panel, empty.panel);
        }
    }

    #[test]
    fn empty_form_description_preserves_meta_and_message_fallback() {
        let mut modal = UniversalModal::new();
        modal.open_form(long_scroll_form());
        modal.meta = "Status remains visible".into();
        let geometry = modal.form_geometry(1200.0, 1.0).unwrap();
        assert!(modal.form_description_lines().is_empty());
        assert_eq!(geometry.fields_start, 20.0 * geometry.scale);

        modal.open_message("Message", "");
        assert!(modal.form.is_none());
        assert!(modal.visible_body_lines().is_empty());
        // Generic message rendering still reserves a row for its fallback text.
        assert_eq!(modal.visible_body_line_count(), 1);
    }

    #[test]
    fn short_form_geometry_bounds_fields_switch_and_full_footer_at_all_scales() {
        for height in [100.0, 150.0, 180.0, 240.0, 320.0, 480.0] {
            for scale in [0.75, 1.0, 2.0, 3.0] {
                let mut modal = UniversalModal::new();
                modal.set_scale(scale);
                modal.viewport_height = height;
                modal.open_form(long_scroll_form());
                modal.set_form_description(
                    "First line\nSecond line\nThird line\nFourth line",
                );
                modal.set_form_tabs(vec!["Host".into(), "Join".into()], 1);
                modal.set_form_switch("Fast tunnel", true, true);
                let geometry = modal.form_geometry(900.0, 1.5).unwrap();
                assert!(geometry.panel[1] >= 0.0);
                assert!(geometry.panel[1] + geometry.panel[3] <= height + 0.01);
                assert!(geometry.scale >= 0.75 && geometry.scale <= scale);
                assert_rect_inside(geometry.body, geometry.panel);
                assert_rect_inside(geometry.actions, geometry.panel);
                assert!(
                    (geometry.actions[3] - 2.0 * geometry.action_row_height).abs() < 0.01
                );
                if let Some(mut switch) = geometry.switch {
                    let clip = if geometry.switch_in_body {
                        geometry.body
                    } else {
                        geometry.panel
                    };
                    if geometry.switch_in_body {
                        switch[1] -= modal.form_scroll;
                    }
                    if let Some(visible) =
                        crate::primitives::geom::intersect_rect(switch, clip)
                    {
                        assert_rect_inside(visible, geometry.panel);
                    }
                }
                for ordinal in 0..10 {
                    if let Some(rect) = crate::primitives::geom::intersect_rect(
                        geometry.field_input(ordinal, modal.form_scroll),
                        geometry.body,
                    ) {
                        assert_rect_inside(rect, geometry.panel);
                        let hit = modal.hit_test(
                            rect[0] + rect[2] * 0.5,
                            rect[1] + rect[3] * 0.5,
                            900.0,
                            1.5,
                        );
                        assert_eq!(hit, Ok(Some(modal.buttons.len() + ordinal + 1)));
                    }
                }
                for index in 0..2 {
                    let row_y = geometry.actions[1]
                        + (index as f32 + 0.5) * geometry.action_row_height;
                    assert_eq!(
                        modal.hit_test(geometry.actions[0] + 10.0, row_y, 900.0, 1.5),
                        Ok(Some(index))
                    );
                }
                // Padding inside the capped panel is not an outside-dismiss hit.
                assert_eq!(
                    modal.hit_test(
                        geometry.panel[0] + 1.0,
                        geometry.body[1] + 1.0,
                        900.0,
                        1.5
                    ),
                    Ok(None)
                );
                assert_eq!(
                    modal.hit_test(geometry.panel[0] - 1.0, geometry.body[1], 900.0, 1.5),
                    Err(())
                );
            }
        }
    }

    #[test]
    fn form_tab_and_arrow_navigation_reveal_fields_without_moving_pinned_controls() {
        let mut modal = UniversalModal::new();
        modal.viewport_height = 240.0;
        modal.set_scale(3.0);
        modal.open_form(long_scroll_form());
        modal.set_form_switch("Fast tunnel", true, true);
        let geometry = modal.form_geometry(1200.0, 1.0).unwrap();
        assert!(!geometry.switch_in_body);
        let pinned = (geometry.switch, geometry.actions);
        for ordinal in 0..10 {
            modal.request_form_reveal();
            let row = geometry.field_row(ordinal, modal.form_scroll);
            assert_rect_inside(row, geometry.body);
            if ordinal < 9 {
                modal.focus_next_form_field();
            }
        }
        assert!(modal.form_scroll > 0.0);
        modal.focus_next_form_field();
        assert!(modal.form_switch_focused());
        modal.focus_next_form_field();
        assert!(modal.form_buttons_focused());
        modal.focus_previous_form_field();
        assert!(modal.form_switch_focused());
        modal.focus_previous_form_field();
        assert_eq!(modal.form_focus, 10);
        for ordinal in (0..9).rev() {
            modal.form_focus_move(false);
            assert_rect_inside(
                geometry.field_row(ordinal, modal.form_scroll),
                geometry.body,
            );
        }
        modal.activate_form_switch();
        assert!(modal.form_switch_focused());
        let after = modal.form_geometry(1200.0, 1.0).unwrap();
        assert_eq!((after.switch, after.actions), pinned);
        modal.set_form_switch("Fast tunnel", true, false);
        assert!(!modal.form_switch_focused());
        assert_eq!(modal.activate_form_switch(), None);
    }

    #[test]
    fn form_wheel_clamps_consumes_outside_and_preserves_footer_and_lock_state() {
        let mut modal = UniversalModal::new();
        modal.viewport_height = 180.0;
        modal.open_form(long_scroll_form());
        modal.set_form_switch("Fast tunnel", true, false);
        modal.set_form_tabs(vec!["Host".into(), "Join".into()], 1);
        modal.set_form_tabs_enabled(false);
        let geometry = modal.form_geometry(1200.0, 1.0).unwrap();
        let x = geometry.body[0] + 10.0;
        let y = geometry.body[1] + 10.0;
        assert!(modal.scroll_at(x, y, 1200.0, 1.0, -100000.0));
        assert_eq!(modal.form_scroll, geometry.max_scroll());
        assert!(!modal.form_reveal_pending);
        assert!(modal.scroll_at(-100.0, -100.0, 1200.0, 1.0, 100.0));
        assert_eq!(modal.form_scroll, geometry.max_scroll());
        assert!(modal.scroll_at(x, geometry.actions[1] + 10.0, 1200.0, 1.0, 100.0));
        assert_eq!(modal.form_scroll, geometry.max_scroll());
        assert!(modal.scroll_at(x, y, 1200.0, 1.0, 100000.0));
        assert_eq!(modal.form_scroll, 0.0);
        modal.scroll_body_page(true);
        assert!(modal.form_scroll > 0.0);
        assert_eq!(modal.form_active_tab(), 1);
        assert!(!modal.form_tabs_enabled());
        assert_eq!(modal.form_switch_checked(), Some(true));
        assert_eq!(modal.activate_form_switch(), None);
        assert_eq!(
            modal.form_geometry(1200.0, 1.0).unwrap().actions,
            geometry.actions
        );
    }

    #[test]
    fn tiny_form_scroll_suffix_reveals_last_option_and_skips_it_when_locked() {
        let mut modal = UniversalModal::new();
        modal.viewport_height = 120.0;
        modal.set_scale(3.0);
        modal.open_form(long_scroll_form());
        modal.set_form_switch("Fast tunnel", true, true);
        let draft = modal.form_values();
        let geometry = modal.form_geometry(1200.0, 1.0).unwrap();
        assert!(geometry.switch_in_body);
        assert_rect_inside(geometry.actions, geometry.panel);
        let switch = geometry.switch.unwrap();
        let last = geometry.field_row(9, 0.0);
        assert!(switch[1] >= last[1] + last[3]);
        assert!(
            (switch[1] - geometry.body[1] + FORM_SWITCH_ROW_HEIGHT * geometry.scale
                - geometry.content_height)
                .abs()
                < 0.01
        );
        modal.form_focus = modal.form.as_ref().unwrap().fields.len() - 1;
        modal.request_form_reveal();
        let field_scroll = modal.form_scroll;
        assert!(field_scroll > 0.0);
        modal.focus_next_form_field();
        assert!(modal.form_switch_focused());
        assert!(modal.form_scroll > field_scroll);
        let visible_switch = [
            switch[0],
            switch[1] - modal.form_scroll,
            switch[2],
            switch[3],
        ];
        assert_rect_inside(visible_switch, geometry.body);
        modal.form_switch_rect = Some(visible_switch);
        assert!(modal.form_switch_hit(visible_switch[0] + 10.0, visible_switch[1] + 10.0));
        assert_eq!(modal.activate_form_switch(), Some(false));
        assert!(modal.form_scroll > 0.0);
        assert!(modal.take_submitted_form().is_none());
        assert_eq!(modal.form_values(), draft);
        modal.focus_next_form_field();
        assert!(modal.form_buttons_focused());
        modal.form_focus_move(false);
        assert!(modal.form_switch_focused());
        modal.form_focus_move(false);
        assert_eq!(modal.form_focus, 10);
        assert!(modal.form_scroll < geometry.max_scroll());
        modal.set_form_switch("Fast tunnel", true, false);
        modal.focus_next_form_field();
        assert!(modal.form_buttons_focused());
        modal.focus_previous_form_field();
        assert_eq!(modal.form_focus, 10);
    }

    #[test]
    fn form_switch_geometry_reserves_scaled_row_and_uses_logical_hits() {
        for scale in [0.75, 1.0, 2.0] {
            let mut modal = UniversalModal::new();
            modal.set_scale(scale);
            modal.viewport_height = 1440.0;
            modal.open_form(switch_test_form());
            let (_, y, _, height) = modal.modal_rect(2400.0, 2.0);
            let actions = modal.actions_y(y);
            modal.set_form_switch("Fast tunnel", false, true);
            let (x, new_y, width, new_height) = modal.modal_rect(2400.0, 2.0);
            assert!((new_height - height - FORM_SWITCH_ROW_HEIGHT * scale).abs() < 0.001);
            assert!(
                (modal.actions_y(new_y) - actions - FORM_SWITCH_ROW_HEIGHT * scale).abs()
                    < 0.001
            );
            let geometry = modal.form_geometry(2400.0, 2.0).unwrap();
            assert!(!geometry.switch_in_body);
            let switch = geometry.switch.unwrap();
            let row_y = switch[1];
            let last = geometry.field_row(1, 0.0);
            assert!(row_y >= last[1] + last[3]);
            assert!((row_y - (geometry.body[1] + geometry.body[3])).abs() < 0.01);
            assert!(row_y + switch[3] <= geometry.actions[1]);
            assert_rect_inside(switch, geometry.panel);
            assert_rect_inside(geometry.actions, geometry.panel);
            modal.form_switch_rect = Some(switch);
            assert!(modal.form_switch_hit(x + width * 0.5, row_y + 10.0 * scale));
            assert!(!modal.form_switch_hit(x - 1.0, row_y));
            modal.pointer_move(x + width * 0.5, row_y + 10.0 * scale, 2400.0, 2.0);
            assert_eq!(modal.pointer_kind(), ModalPointerKind::Pointer);
            let field_y = geometry.field_input(0, 0.0)[1];
            assert_eq!(
                modal.hit_test(x + width * 0.5, field_y + 10.0 * scale, 2400.0, 2.0),
                Ok(Some(modal.buttons.len() + 1))
            );
        }
    }

    #[test]
    fn form_drafts_preserve_secrets_and_switch_locks_reset_on_every_open() {
        let mut modal = UniversalModal::new();
        modal.open_form(switch_test_form());
        assert!(modal
            .form_values()
            .contains(&("password".into(), "secret".into())));
        assert_eq!(modal.form_values(), modal.form_values());
        assert!(modal.take_submitted_form().is_none());
        modal.set_form_switch("Fast tunnel", true, false);
        modal.set_form_tabs_enabled(false);
        modal.open_message("Other modal", "Details");
        assert_eq!(modal.form_switch_checked(), None);
        assert!(modal.form_tabs_enabled());
        assert_eq!(modal.form_tab_count(), 0);
        assert!(modal.form_values().is_empty());
        modal.open_form(switch_test_form());
        modal.set_form_switch("Fast tunnel", true, false);
        modal.set_form_tabs_enabled(false);
        modal.close();
        assert_eq!(modal.form_switch_checked(), None);
        assert!(modal.form_tabs_enabled());
        modal.open_form(switch_test_form());
        assert_eq!(modal.form_switch_checked(), None);
        assert!(modal.form_tabs_enabled());
    }

    #[test]
    fn server_form_skips_hidden_id_and_submits_structured_values() {
        let mut modal = UniversalModal::new();
        modal.open_form(ModalFormSpec {
            title: "Edit server".into(),
            fields: vec![
                ModalFormField {
                    id: "server_id".into(),
                    label: String::new(),
                    value: "id-1".into(),
                    placeholder: String::new(),
                    secret: true,
                },
                ModalFormField {
                    id: "address".into(),
                    label: "Address".into(),
                    value: String::new(),
                    placeholder: "https://host".into(),
                    secret: false,
                },
                ModalFormField {
                    id: "token".into(),
                    label: "Token".into(),
                    value: String::new(),
                    placeholder: "token".into(),
                    secret: true,
                },
            ],
            submit_label: "Save".into(),
        });
        modal.push_input("https://work.example");
        assert!(modal.focus_next_form_field());
        modal.push_input("secret");
        assert_eq!(modal.submit_form(), Some(ModalAction::ServerFormSubmit));
        let values = modal.take_submitted_form().unwrap();
        assert!(values.contains(&("server_id".into(), "id-1".into())));
        assert!(values.contains(&("address".into(), "https://work.example".into())));
        assert!(values.contains(&("token".into(), "secret".into())));
    }

    #[test]
    fn server_form_hit_tests_fields_and_buttons() {
        let mut modal = UniversalModal::new();
        modal.open_form(ModalFormSpec {
            title: "Add server".into(),
            fields: vec![
                ModalFormField {
                    id: "address".into(),
                    label: "Server address".into(),
                    value: String::new(),
                    placeholder: "http://localhost:7878".into(),
                    secret: false,
                },
                ModalFormField {
                    id: "name".into(),
                    label: "Server name".into(),
                    value: String::new(),
                    placeholder: "Home".into(),
                    secret: false,
                },
            ],
            submit_label: "Add server".into(),
        });
        let (x, y, _, _) = modal.modal_rect(1200.0, 1.0);
        let field_y = y + MODAL_PADDING + TITLE_HEIGHT + 12.0 + BODY_LINE_HEIGHT + 20.0;
        let field_hit =
            modal.hit_test(x + MODAL_PADDING + 20.0, field_y + 10.0, 1200.0, 1.0);
        assert_eq!(field_hit, Ok(Some(modal.buttons.len())));

        let actions_y = modal.actions_y(y);
        assert_eq!(
            modal.hit_test(x + MODAL_PADDING + 20.0, actions_y + 10.0, 1200.0, 1.0),
            Ok(Some(0))
        );
        assert_eq!(
            modal.hit_test(
                x + MODAL_PADDING + 20.0,
                actions_y + ACTION_ROW_HEIGHT + 10.0,
                1200.0,
                1.0,
            ),
            Ok(Some(1))
        );

        assert!(!modal.focus_form_hit(0));
        modal.set_selected_index(0);
        assert_eq!(modal.selected_action(), Some(ModalAction::ServerFormSubmit));
        assert!(!modal.focus_form_hit(1));
        modal.set_selected_index(1);
        assert_eq!(modal.selected_action(), Some(ModalAction::Close));
        assert!(modal.focus_form_hit(modal.buttons.len()));

        let geometry = modal.form_geometry(1200.0, 1.0).unwrap();
        let second = geometry.field_input(1, modal.form_scroll);
        let hit_x = second[0] + 20.0;
        let hit_y = second[1] + 10.0;
        assert_eq!(
            modal.hit_test(hit_x, hit_y, 1200.0, 1.0),
            Ok(Some(modal.buttons.len() + 1))
        );
        assert!(modal.focus_form_hit(modal.buttons.len() + 1));
        assert_eq!(modal.form_focus, 1);
        assert!(modal.pointer_move(hit_x, hit_y, 1200.0, 1.0));
        assert_eq!(modal.pointer_kind(), ModalPointerKind::Text);
        assert!(!modal.pointer_move(hit_x, hit_y, 1200.0, 1.0));
        assert!(modal.pointer_leave());
        assert_eq!(modal.pointer_kind(), ModalPointerKind::Default);

        let selected = modal.selected_index;
        assert!(modal.pointer_move(
            x + MODAL_PADDING + 20.0,
            actions_y + ACTION_ROW_HEIGHT + 10.0,
            1200.0,
            1.0,
        ));
        assert_eq!(modal.pointer_kind(), ModalPointerKind::Pointer);
        assert_eq!(modal.selected_index, selected);
    }

    #[test]
    fn invite_copies_exact_capability_and_resets_on_generic_open() {
        let mut modal = UniversalModal::new();
        let url = "https://example.com/join?token=a%2Bb#invite";
        modal.open_fast_server_invite(url.into());
        assert!(modal.is_active() && modal.is_blocking());
        assert!(matches!(modal.selected_action(), Some(ModalAction::CopyInvite { url: value }) if value == url));
        assert!(matches!(modal.escape_action(), Some(ModalAction::Close)));
        modal.invite_hit_rects.push([10.0, 20.0, 100.0, 24.0]);
        assert!(matches!(modal.invite_action_at(20.0, 30.0), Some(ModalAction::CopyInvite { url: value }) if value == url));
        assert!(modal.invite_action_at(0.0, 0.0).is_none());
        modal.mark_invite_copied();
        assert_eq!(modal.meta, "Invite copied");
        modal.close();
        assert!(modal.invite_action_at(20.0, 30.0).is_none());
        modal.open_message("Other", "Details");
        assert!(modal.invite_url.is_none());
        assert!(modal.invite_hit_rects.is_empty());
    }

    #[test]
    fn invite_copy_queue_preserves_other_host_actions() {
        let mut modal = UniversalModal::new();
        modal.queue_host_action(ModalHostAction::Generic { id: "other".into(), value: "keep".into() });
        modal.queue_host_action(ModalHostAction::Generic { id: "copy-invite".into(), value: "exact-url".into() });
        assert_eq!(modal.take_invite_copy().as_deref(), Some("exact-url"));
        assert!(modal.take_invite_copy().is_none());
        assert_eq!(modal.pending_host_actions.len(), 1);
    }

    #[test]
    fn activity_updates_preserve_links_and_stop_the_square_loader_on_completion() {
        let mut modal = UniversalModal::new();
        let activity = ModalActivity::new();
        let url = "https://example.com/auth?state=a)b";
        modal.open_activity_message(activity.clone(), "vercel", "Authorize in your browser.", "Not connected");
        let opened_at = modal.opened_at;
        assert!(modal.needs_redraw());
        assert!(modal.update_activity_message(
            &activity, "Authorize in your browser.", "Waiting for authorization", true,
            Some(("Open authorization page".into(), url.into())),
        ));
        let (_, y, _, height) = modal.modal_rect(1000.0, 1.0);
        let actions_y = modal.actions_y(y);
        assert_eq!(modal.body_link.as_ref().unwrap().1, url);
        assert!(modal.update_activity_message(
            &activity, "Authorize in your browser.", "Connected", false,
            Some(("Open authorization page".into(), url.into())),
        ));
        assert!(!modal.needs_redraw());
        assert_eq!(modal.opened_at, opened_at);
        assert_eq!(modal.body_link.as_ref().unwrap().1, url);
        assert_eq!(modal.modal_rect(1000.0, 1.0).3, height);
        assert_eq!(modal.actions_y(y), actions_y);
        assert!(modal.update_activity_message(&activity, "Connected.", "Connected", false, None));
        assert!(modal.body_link.is_none());
    }

    #[test]
    fn activity_results_never_reopen_or_replace_other_dialogs() {
        let mut modal = UniversalModal::new();
        let first = ModalActivity::new();
        modal.open_activity_message(first.clone(), "vercel", "Authorize", "Waiting");
        modal.close();
        assert!(first.is_cancelled());
        assert!(!modal.update_activity_message(&first, "Connected", "Connected", false, None));
        assert!(!modal.is_active());
        modal.open_activity_message(first.clone(), "vercel", "Late open", "Waiting");
        assert!(!modal.is_active());
        let second = ModalActivity::new();
        modal.open_activity_message(second.clone(), "vercel", "Other attempt", "Waiting");
        assert_ne!(first, second);
        assert!(!modal.update_activity_message(&first, "Old result", "Connected", false, None));
        assert_eq!(modal.body, "Other attempt");
        modal.open_message("vercel", "Unrelated message");
        assert!(second.is_cancelled());
        assert!(!modal.update_activity_message(&second, "Late result", "Connected", false, None));
        assert_eq!(modal.body, "Unrelated message");
    }

    #[test]
    fn destroying_the_modal_cancels_its_background_activity() {
        let activity = ModalActivity::new();
        {
            let mut modal = UniversalModal::new();
            modal.open_activity_message(activity.clone(), "vercel", "Authorize", "Waiting");
            assert!(!activity.is_cancelled());
        }
        assert!(activity.is_cancelled());
    }

    #[test]
    fn structured_message_link_preserves_arbitrary_oauth_state() {
        let mut modal = UniversalModal::new();
        for url in [
            "https://example.com/auth?state=a)b",
            "https://example.com/auth?state=a(b",
            "https://example.com/auth?state=a%2Bb#finish.",
        ] {
            modal.open_link_message("Authorize", "Continue in your browser.", "Open authorization page", url);
            let lines = modal.layout_body(90.0, 1.0, |text, _| text.chars().count() as f32 * 7.0);
            let links: Vec<_> = lines.iter().flat_map(|line| &line.links).collect();
            assert!(links.len() > 1);
            assert!(links.iter().all(|link| link.url == url));
            assert!(lines.iter().all(|line| !line.text.contains("https://")));
        }
        modal.open_message("Other", "Details");
        assert!(modal.body_link.is_none());
    }

    #[test]
    fn compact_form_decision_survives_wider_description_reflow() {
        let mut modal = UniversalModal::new();
        modal.open_form(ModalFormSpec {
            title: "Form".into(),
            fields: vec![ModalFormField {
                id: "name".into(), label: "Name".into(), value: String::new(),
                placeholder: String::new(), secret: false,
            }],
            submit_label: "Save".into(),
        });
        modal.buttons.truncate(1);
        modal.body = "x".repeat(53);
        modal.viewport_height = 240.0;
        for _ in 0..3 {
            modal.form_compact = false;
            let lines = modal.layout_body(516.0, 1.0, |text, _| text.chars().count() as f32 * 10.0);
            assert_eq!(lines.len(), 2);
            modal.body_layout = Some(BodyLayout {
                source: modal.body.clone(), width: 516.0, scale: 1.0,
                font_probe: (10.0, 10.0), lines,
            });
            let geometry = modal.form_geometry(1000.0, 1.0).unwrap();
            assert!(geometry.compact);
            modal.form_compact = geometry.compact;
            let width = geometry.body[2] - 8.0 * geometry.scale;
            let lines = modal.layout_body(width, geometry.scale, |text, _| text.chars().count() as f32 * 10.0);
            assert_eq!(lines.len(), 1);
            modal.body_layout.as_mut().unwrap().lines = lines;
            let final_geometry = modal.form_geometry(1000.0, 1.0).unwrap();
            assert!(final_geometry.compact);
            assert_eq!(final_geometry.pad, geometry.pad);
            assert_eq!(final_geometry.body[2], geometry.body[2]);
        }
    }

    #[test]
    fn body_links_use_clipped_hits_and_preserve_exact_destination() {
        let mut modal = UniversalModal::new();
        let url = "https://example.com/oauth?state=a%2Bb&next=(done)#finish.";
        modal.open_message("Authorize", format!("[Open authorization page]({url})"));
        let clip = [20.0, 30.0, 80.0, 40.0];
        assert_eq!(modal.record_body_link(url, [10.0, 20.0, 100.0, 25.0], clip), Some(0));
        assert!(modal.record_body_link(url, [10.0, 80.0, 100.0, 20.0], clip).is_none());
        assert!(modal.body_link_action_at(15.0, 35.0).is_none());
        assert!(modal.body_link_action_at(25.0, 25.0).is_none());
        assert_eq!(modal.body_link_action_at(25.0, 35.0), Some(ModalAction::OpenUrl { url: url.into() }));
        modal.pointer_move(25.0, 35.0, 1200.0, 1.0);
        assert_eq!(modal.pointer_kind(), ModalPointerKind::Pointer);
        assert!(modal.is_active());
        modal.scroll_body_rows(1);
        assert!(modal.body_link_action_at(25.0, 35.0).is_none());
        modal.record_body_link(url, clip, clip);
        modal.close();
        assert!(modal.body_link_action_at(25.0, 35.0).is_none());
        modal.open_message("Other", "No link");
        assert!(modal.body_link_hit_rects.is_empty());
    }

    #[test]
    fn measured_body_rows_drive_panel_height_actions_and_scrolling() {
        let mut modal = UniversalModal::new();
        let url = format!("https://example.com/{}", "x".repeat(400));
        modal.open_message("Authorize", format!("[{}]({})", url, url));
        let lines = body::layout_lines(&modal.body, 140.0, 1.0, None, |text, _| text.chars().count() as f32 * 7.0);
        assert!(lines.len() > modal.body_row_budget());
        modal.body_layout = Some(BodyLayout {
            source: modal.body.clone(), width: 140.0, scale: 1.0,
            font_probe: (7.0, 7.0), lines,
        });
        let (_, y, _, h) = modal.modal_rect(360.0, 1.0);
        let visible_rows = modal.visible_body_line_count() as f32;
        assert_eq!(modal.actions_y(y), y + MODAL_PADDING + TITLE_HEIGHT + 12.0
            + visible_rows * BODY_LINE_HEIGHT + 14.0 + SEPARATOR_HEIGHT + 4.0);
        assert!(modal.actions_y(y) + ACTION_ROW_HEIGHT <= y + h);
        modal.scroll_body_page(true);
        assert!(modal.body_scroll_offset > 0);
        assert!(modal.visible_body_lines().iter().all(|line| !line.links.is_empty()));
        modal.set_scale(2.0);
        assert!(modal.body_layout.is_none());
        modal.update_progress("New details", "", None, false);
        assert!(modal.body_link_hit_rects.is_empty());
        assert!(modal.body_lines().iter().all(|line| line.links.is_empty()));
    }

    #[test]
    fn body_lines_upgrade_mermaid_fence_to_diagram_line() {
        let mut modal = UniversalModal::new();
        modal.open(ModalSpec {
            title: "Diagram".into(),
            body: "```mermaid\nflowchart LR\nA[Start] --> B{Done}\n```".into(),
            meta: String::new(),
            input: None,
            buttons: Vec::new(),
            busy: false,
            blocking: false,
        });

        let lines = modal.body_lines();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].kind, BodyLineKind::Mermaid);
        assert!(lines[0].mermaid.is_some());
        assert!(lines[0].row_span > 1);
    }

    #[test]
    fn session_rename_modal_keeps_clicked_identity_when_input_changes() {
        let action = ModalAction::AgentRenameSession {
            session_id: "session-a".into(),
            server: "http://localhost".into(),
            directory: Some("/workspace".into()),
            name: String::new(),
        }
        .with_input("Updated title".into());
        assert!(
            matches!(action, ModalAction::AgentRenameSession { session_id, server, directory, name }
            if session_id == "session-a" && server == "http://localhost"
                && directory.as_deref() == Some("/workspace") && name == "Updated title")
        );
    }

    #[test]
    fn determinate_progress_does_not_request_animation_frames() {
        let mut modal = UniversalModal::new();
        modal.open(ModalSpec {
            title: "Updating".into(),
            body: "Starting".into(),
            meta: String::new(),
            input: None,
            buttons: Vec::new(),
            busy: true,
            blocking: false,
        });
        assert!(modal.needs_redraw());
        assert!(modal.update_progress("Downloading", "42% complete", Some(42), true));
        assert_eq!(modal.progress, Some(42));
        assert!(!modal.needs_redraw());
    }
}
