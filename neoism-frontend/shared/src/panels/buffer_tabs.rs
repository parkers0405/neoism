//! Buffer tab strip — pure state + input logic.
//!
//! Sits directly below the OS-window tab bar and shows open editor
//! buffers, much like Warp's split-pane title row. Render is left to
//! the host (native: `frontends/neoism/src/chrome/panels/buffer_tabs.rs`
//! supplies an inherent `render(&mut Sugarloaf, ...)` method that
//! consumes the `IdeTheme`); web will paint its own pass off the same
//! state.
//!
//! Generic over an `AgentKind` type `A` so the native frontend can
//! keep using `crate::neoism::icon::AgentKind` for agent-CLI tabs
//! (Claude / Codex / OpenCode) while this crate stays free of
//! frontend-specific types. The bound is just enough to let the panel
//! pick a tab title from the agent: `AgentLabel + Copy + PartialEq`.

use std::path::{Path, PathBuf};
use web_time::Duration;

#[cfg(not(target_arch = "wasm32"))]
use web_time::Instant;

#[cfg(target_arch = "wasm32")]
#[derive(Clone, Copy, Debug)]
pub struct Instant(f64);

#[cfg(target_arch = "wasm32")]
impl Instant {
    fn now() -> Self {
        Self(js_sys::Date::now())
    }

    fn elapsed(&self) -> Duration {
        Duration::from_secs_f64(((js_sys::Date::now() - self.0) / 1000.0).max(0.0))
    }
}

use sugarloaf::text::DrawOpts;
use sugarloaf::{Attributes, Sugarloaf};

use crate::event::{Modifiers, PointerButton, UiEvent, WheelMode};
use crate::primitives::draw_text_with_occlusion;

/// True when `rect` overlaps any occlusion rect — used to drop
/// host-painted icon overlays (which bypass the text occlusion helper)
/// under open modals.
fn rect_occluded(rect: [f32; 4], occlusion_rects: &[[f32; 4]]) -> bool {
    occlusion_rects.iter().any(|occ| {
        rect[0] < occ[0] + occ[2]
            && rect[0] + rect[2] > occ[0]
            && rect[1] < occ[1] + occ[3]
            && rect[1] + rect[3] > occ[1]
    })
}
use crate::layout::PanelLayout;
use crate::panels::file_tree::icon_for_file;
use crate::panels::{Panel, PanelContext};
use crate::primitives::IdeTheme;
use crate::session_layout::{buffer_tabs_scroll_dx, SessionScrollDelta};

// ── Strip dimensions ───────────────────────────────────────────────
//
// Mirrors the legacy native panel's constants exactly so the migration
// preserves pixel layout. Lift these to theme tokens later.

pub const BUFFER_TABS_HEIGHT: f32 = 28.0;
const ICON_FONT_SIZE: f32 = 12.0;
const ICON_GAP: f32 = 6.0;
const FONT_SIZE: f32 = 11.5;
const TAB_PADDING_X: f32 = 12.0;
const CLOSE_BTN_SIZE: f32 = 11.0;
const CLOSE_HIT_SIZE: f32 = 18.0;
const CLOSE_BTN_GAP: f32 = 6.0;
const MIN_TAB_WIDTH: f32 = 96.0;
const MAX_TAB_WIDTH: f32 = 220.0;
/// The comfortable per-tab width every tab keeps regardless of how many
/// tabs are open. Crowded strips no longer crush tabs down toward
/// `MIN_TAB_WIDTH` and truncate titles; instead the strip overflows and
/// scrolls horizontally (the strip is already a scroll surface). Matches
/// the `MAX_TAB_WIDTH` cap so a single tab on a wide strip doesn't
/// balloon past it.
const NATURAL_TAB_WIDTH: f32 = MAX_TAB_WIDTH;
const TITLE_ELLIPSIS: char = '…';

/// Width (logical px, pre-scale) of the trailing "+" new-tab button
/// that sits to the right of the furthest tab. Kept square-ish so the
/// glyph centres cleanly within the strip height.
const NEW_TAB_BTN_WIDTH: f32 = 30.0;

/// Nerd Font "plus" glyph (`nf-fa-plus`, U+F067) used for the
/// new-tab button. Matches the other Nerd Font chrome glyphs
/// (terminal `\u{f489}`, agent `\u{f135}`).
const NEW_TAB_ICON: &str = "\u{f067}";

const TAB_HOVER_ANIM_MS: u64 = 150;

const TERMINAL_TITLE: &str = "Terminal";

// Render-side constants — exposed so the native render shim can read
// the same values that hit-testing & drag math use.
pub mod consts {
    pub const ICON_FONT_SIZE: f32 = super::ICON_FONT_SIZE;
    pub const ICON_GAP: f32 = super::ICON_GAP;
    pub const FONT_SIZE: f32 = super::FONT_SIZE;
    pub const TAB_PADDING_X: f32 = super::TAB_PADDING_X;
    pub const CLOSE_BTN_SIZE: f32 = super::CLOSE_BTN_SIZE;
    pub const CLOSE_HIT_SIZE: f32 = super::CLOSE_HIT_SIZE;
    pub const CLOSE_BTN_GAP: f32 = super::CLOSE_BTN_GAP;
    pub const MIN_TAB_WIDTH: f32 = super::MIN_TAB_WIDTH;
    pub const MAX_TAB_WIDTH: f32 = super::MAX_TAB_WIDTH;
    pub const NEW_TAB_BTN_WIDTH: f32 = super::NEW_TAB_BTN_WIDTH;
    pub const NEW_TAB_ICON: &str = super::NEW_TAB_ICON;
    pub const TITLE_ELLIPSIS: char = super::TITLE_ELLIPSIS;
    pub const TAB_HOVER_ANIM_MS: u64 = super::TAB_HOVER_ANIM_MS;
    pub const TAB_HOVER_SCALE: f32 = 1.035;
    pub const DEPTH: f32 = 0.0;
    pub const ORDER_BG: u8 = 4;
    pub const ORDER_TAB: u8 = 5;
    pub const ORDER_ACCENT: u8 = 6;
    pub const ORDER_TEXT: u8 = 7;
    pub const TERMINAL_TITLE: &str = super::TERMINAL_TITLE;
    // Nerd Font terminal glyph (\u{f489}). Empty string would render
    // nothing — desktop's `render_with_icons` path reads this constant
    // and the web's `render` path hard-codes the same glyph, so they
    // need to stay in sync. See line ~2932 for the matching literal.
    pub const TERMINAL_ICON: &str = "\u{f489}";
}

/// Trait the panel uses to ask the host's agent enum for a display
/// name when populating a tab title. Host implements once on its own
/// `AgentKind` (or whatever moral equivalent it carries).
pub trait AgentLabel {
    fn display_name(&self) -> &str;
}

/// Host-supplied painter for the per-tab agent logo overlay
/// (Claude/Codex/OpenCode PNGs on the native frontend, equivalent
/// assets on web/wasm). The shared `render` path calls this at the
/// exact pixel rect where the native fork used to call
/// `crate::neoism::icon::push_icon_overlay` — the host is responsible
/// for owning the PNG bytes, uploading them to sugarloaf, and
/// pushing/refreshing the `GraphicOverlay`. Passing `None` for the
/// provider keeps the strip on the glyph-only fallback (which is what
/// the web host does today).
///
/// The trait is generic over the host's `AgentKind`-equivalent `A` so
/// the native frontend can keep its existing enum without leaking it
/// into shared.
pub trait AgentIconProvider<A> {
    fn neoism_agent(&self) -> Option<A> {
        None
    }

    /// Paint the agent logo at `(x, y)` (logical pixels) with a
    /// `size × size` bounding box. Implementations should clear and
    /// re-push their overlay each frame — the strip's render path is
    /// immediate-mode.
    fn draw_agent_icon(
        &self,
        sugarloaf: &mut Sugarloaf,
        agent: A,
        x: f32,
        y: f32,
        size: f32,
        source_rect: [f32; 4],
    );
}

/// Zero-sized marker used to satisfy the `render_with_icons` generic
/// from the no-provider `render` path without any heap traffic. Pure
/// helper — host code should never need to name this directly.
struct NoopAgentIcons<A>(std::marker::PhantomData<fn(A)>);

impl<A> AgentIconProvider<A> for NoopAgentIcons<A> {
    fn draw_agent_icon(
        &self,
        _sugarloaf: &mut Sugarloaf,
        _agent: A,
        _x: f32,
        _y: f32,
        _size: f32,
        _source_rect: [f32; 4],
    ) {
    }
}

// ── BufferTab / BufferTabTarget ────────────────────────────────────

/// One row in the strip. Identity is whichever of `path`,
/// `neoism_agent_route_id`, or `terminal_route_id` is set — see
/// `target()` for resolution order.
#[derive(Clone, Debug)]
pub struct BufferTab<A> {
    pub title: String,
    pub modified: bool,
    /// Backing file path. Tabs created from a tree click carry their
    /// path so a click on the tab can re-activate the existing buffer.
    pub path: Option<PathBuf>,
    /// Rust-rendered markdown document tab. Uses `path` as its backing
    /// file but activates a MarkdownPane instead of the code editor.
    pub markdown: bool,
    /// Backing terminal route. `None` with `path == None` is the root
    /// workspace terminal; `Some(route)` is an extra workspace-local
    /// terminal tab stacked beside the editor.
    pub terminal_route_id: Option<usize>,
    /// Rust-rendered Neoism agent route. This is a native surface, not
    /// a PTY-backed agent CLI.
    pub neoism_agent_route_id: Option<usize>,
    /// Rust-rendered "chrome helper page" tab — Extensions today,
    /// Updates / About / etc. tomorrow. Singleton kinds whose body is
    /// painted entirely by a dedicated Rust panel with no file
    /// backing. Generic over [`ChromePageKind`] so new helper pages
    /// just add a variant + a render branch, not a new per-kind
    /// field on every `BufferTab` literal.
    pub chrome_page: Option<ChromePageRef>,
    /// Agent CLI associated with this terminal tab. Stored on the tab
    /// instead of inferred from the active PTY so inactive agent tabs
    /// keep their provider name and logo.
    pub agent_kind: Option<A>,
}

/// What kind of in-chrome helper page this tab points at. Each kind
/// has a fixed title (rendered into the buffer-tabs strip) and a
/// dedicated render path in the host. To add a new helper page:
///   1. Add a variant here + a `title()` arm.
///   2. Add a `Context::<kind>: Option<…>` field if the page needs
///      per-instance state.
///   3. Match the new variant in the host's
///      `activate_workspace_buffer_tab` dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChromePageKind {
    Extensions,
}

impl ChromePageKind {
    pub fn title(self) -> &'static str {
        match self {
            ChromePageKind::Extensions => "Extensions",
        }
    }
}

/// Identifies one chrome helper page tab. `route_id` is the host's
/// `Context::route_id` for the singleton page context so the strip
/// can correlate clicks back to the right pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ChromePageRef {
    pub kind: ChromePageKind,
    pub route_id: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BufferTabTarget {
    File(PathBuf),
    Markdown(PathBuf),
    NeoismAgent(usize),
    ChromePage(ChromePageRef),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BufferTabClosePlan {
    /// Active surface is a terminal tab backed by a host route. The host
    /// closes the route/process first, then mirrors removal back into tabs.
    CloseTerminalRoute { route_id: usize },
    /// Close this buffer tab through [`BufferTabs::close_at`].
    CloseTab { index: usize },
    /// Nothing closeable was found. The active tab is kept unchanged.
    Ignore,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BufferTabPolicyOperation {
    SelectPrevious,
    SelectNext,
    SelectIndex { index: usize },
    MovePrevious,
    MoveNext,
    CloseActive,
    CloseIndex { index: usize },
    Reorder { from: usize, to: usize },
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BufferTabPolicyInput {
    pub len: usize,
    pub active: usize,
    #[serde(default)]
    pub closeable: Vec<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BufferTabPolicyResult {
    pub active: usize,
    pub remove_index: Option<usize>,
    pub move_from: Option<usize>,
    pub move_to: Option<usize>,
    pub changed: bool,
}

fn close_tab_policy_result(
    len: usize,
    active: usize,
    closeable: &[bool],
    index: usize,
    unchanged: BufferTabPolicyResult,
) -> BufferTabPolicyResult {
    if len == 0 || index >= len || !closeable.get(index).copied().unwrap_or(true) {
        return unchanged;
    }
    let next_len = len.saturating_sub(1);
    let next_active = if next_len == 0 {
        0
    } else if index == active {
        index.min(next_len - 1)
    } else if index < active {
        active.saturating_sub(1)
    } else {
        active
    };
    BufferTabPolicyResult {
        active: next_active,
        remove_index: Some(index),
        changed: true,
        ..unchanged
    }
}

fn reorder_tab_policy_result(
    len: usize,
    active: usize,
    from: usize,
    to: usize,
    unchanged: BufferTabPolicyResult,
) -> BufferTabPolicyResult {
    if len <= 1 || from >= len || to >= len || from == to {
        return unchanged;
    }
    let next_active = if active == from {
        to
    } else if from < active && active <= to {
        active - 1
    } else if to <= active && active < from {
        active + 1
    } else {
        active
    };
    BufferTabPolicyResult {
        active: next_active,
        move_from: Some(from),
        move_to: Some(to),
        changed: true,
        ..unchanged
    }
}

/// Shared tab-strip operation policy used by native and web hosts.
///
/// The host still owns side effects (closing PTYs, replaying terminal
/// buffers, opening code files), but selection, movement, and removal
/// bookkeeping should all flow through this policy so desktop and web
/// agree on edge cases.
pub fn apply_buffer_tab_policy(
    input: BufferTabPolicyInput,
    operation: BufferTabPolicyOperation,
) -> BufferTabPolicyResult {
    let len = input.len;
    let active = if len == 0 {
        0
    } else {
        input.active.min(len - 1)
    };
    let unchanged = BufferTabPolicyResult {
        active,
        remove_index: None,
        move_from: None,
        move_to: None,
        changed: false,
    };

    match operation {
        BufferTabPolicyOperation::SelectPrevious => {
            if len <= 1 {
                unchanged
            } else {
                BufferTabPolicyResult {
                    active: if active == 0 { len - 1 } else { active - 1 },
                    changed: true,
                    ..unchanged
                }
            }
        }
        BufferTabPolicyOperation::SelectNext => {
            if len <= 1 {
                unchanged
            } else {
                BufferTabPolicyResult {
                    active: (active + 1) % len,
                    changed: true,
                    ..unchanged
                }
            }
        }
        BufferTabPolicyOperation::SelectIndex { index } => {
            if index >= len || index == active {
                unchanged
            } else {
                BufferTabPolicyResult {
                    active: index,
                    changed: true,
                    ..unchanged
                }
            }
        }
        BufferTabPolicyOperation::MovePrevious => reorder_tab_policy_result(
            len,
            active,
            active,
            active.saturating_sub(1),
            unchanged,
        ),
        BufferTabPolicyOperation::MoveNext => {
            reorder_tab_policy_result(len, active, active, active + 1, unchanged)
        }
        BufferTabPolicyOperation::CloseActive => {
            close_tab_policy_result(len, active, &input.closeable, active, unchanged)
        }
        BufferTabPolicyOperation::CloseIndex { index } => {
            close_tab_policy_result(len, active, &input.closeable, index, unchanged)
        }
        BufferTabPolicyOperation::Reorder { from, to } => {
            reorder_tab_policy_result(len, active, from, to, unchanged)
        }
    }
}

impl<A> BufferTab<A> {
    pub fn target(&self) -> Option<BufferTabTarget> {
        if let Some(page) = self.chrome_page {
            Some(BufferTabTarget::ChromePage(page))
        } else if let Some(route_id) = self.neoism_agent_route_id {
            Some(BufferTabTarget::NeoismAgent(route_id))
        } else if let Some(path) = self.path.clone() {
            if self.markdown || is_markdown_path(&path) {
                Some(BufferTabTarget::Markdown(path))
            } else {
                Some(BufferTabTarget::File(path))
            }
        } else {
            None
        }
    }

    pub fn is_terminal(&self) -> bool {
        self.path.is_none()
            && self.neoism_agent_route_id.is_none()
            && self.chrome_page.is_none()
    }
}

// ── Workspace bookkeeping policy ───────────────────────────────────

/// What a host should do with its per-workspace "active editor path"
/// map after the active buffer tab changes (activate/close/move).
///
/// The desktop fork keeps a `BTreeMap<workspace_id, PathBuf>` of the
/// last path the editor was pointed at so chrome (file tree highlight,
/// breadcrumbs, search-bar pre-fill) can repaint independently of the
/// PTY event loop. Web has the same need. Routing the decision through
/// this enum keeps the "which target keeps a path; which target wipes
/// it" rules in one place instead of spread across activate/close call
/// sites.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceActivePathUpdate {
    /// Insert/overwrite the workspace's remembered path.
    Insert(PathBuf),
    /// Wipe the workspace's remembered path (terminal-only tab or no
    /// editor target at all).
    Remove,
    /// Leave the workspace map untouched — e.g. there's no active
    /// workspace id yet so the host has nothing to update.
    Keep,
}

impl WorkspaceActivePathUpdate {
    /// Path that should be passed to `guard_workspace_buf_enter` so the
    /// host suppresses spurious buffer-enter echoes for the upcoming
    /// activation. Only File/Markdown targets surface as a Some(path);
    /// agent/terminal flows return None.
    pub fn buf_enter_guard(&self) -> Option<PathBuf> {
        match self {
            WorkspaceActivePathUpdate::Insert(path) => Some(path.clone()),
            WorkspaceActivePathUpdate::Remove | WorkspaceActivePathUpdate::Keep => None,
        }
    }
}

/// Decide what to do with the per-workspace active-path map for a
/// freshly activated buffer tab target.
///
/// File and Markdown targets remember their path so the editor chrome
/// (file tree highlight, breadcrumbs) can rehydrate when the workspace
/// is refocused. Neoism agent and chrome-page tabs clear the entry
/// because they don't correspond to a filesystem path.
pub fn workspace_active_path_for_target(
    target: Option<&BufferTabTarget>,
) -> WorkspaceActivePathUpdate {
    match target {
        Some(BufferTabTarget::File(path)) | Some(BufferTabTarget::Markdown(path)) => {
            WorkspaceActivePathUpdate::Insert(path.clone())
        }
        Some(BufferTabTarget::NeoismAgent(_))
        | Some(BufferTabTarget::ChromePage(_))
        | None => WorkspaceActivePathUpdate::Remove,
    }
}

/// Same policy as [`workspace_active_path_for_target`] but with a
/// `keep_when_unset` flag so the close path can no-op when there is no
/// workspace id at all. Used by `close_active_buffer_tab_inner` —
/// activation paths already early-return when workspace_id is None.
pub fn workspace_active_path_after_close(
    target: Option<&BufferTabTarget>,
    workspace_present: bool,
) -> WorkspaceActivePathUpdate {
    if !workspace_present {
        return WorkspaceActivePathUpdate::Keep;
    }
    workspace_active_path_for_target(target)
}

/// Short, human-readable label for a [`BufferTabTarget`] suitable for
/// tracing/logs. The native fork builds the same string in three
/// different match arms (close, activate, drag/move); centralising
/// it here avoids drift.
pub fn buffer_tab_target_label(target: Option<&BufferTabTarget>) -> String {
    match target {
        Some(BufferTabTarget::File(path)) => path.display().to_string(),
        Some(BufferTabTarget::Markdown(path)) => format!("markdown:{}", path.display()),
        Some(BufferTabTarget::NeoismAgent(id)) => format!("neoism-agent:{id}"),
        Some(BufferTabTarget::ChromePage(page)) => {
            format!(
                "chrome-page:{}:{}",
                page.kind.title().to_lowercase(),
                page.route_id
            )
        }
        None => "<none>".to_string(),
    }
}

// ── Cross-strip drag / close policy ────────────────────────────────
//
// The desktop fork's `reinsert_tab_into_strip` and
// `tear_out_file_tab_to_pane` each fork on (workspace vs pane) and
// (primary vs lookup) inline. Lifting the decision part here lets web
// reuse the same rules and lets these paths be unit-tested without
// spinning up a host. The host still owns the actual IO (BufferTabs
// mutations, Sugarloaf side effects).

/// What the host should re-open in the strip when a tab tear-out
/// fails and the dragged tab needs to be restored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReinsertTabKind {
    /// Tab represents a markdown buffer — host calls `open_markdown`.
    Markdown,
    /// Tab represents a plain file buffer — host calls `open_path`.
    Path,
}

/// Plan for `reinsert_tab_into_strip`: which strip to insert into
/// and which open path to take. Always tagged for the source strip
/// so callers don't need to plumb [`StripKey`] separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReinsertTabPlan {
    pub strip: StripKey,
    pub kind: ReinsertTabKind,
}

/// Decide where + how to reinsert a file tab back into a strip after
/// a tear-out is aborted or fails. `markdown` mirrors
/// `BufferTab::markdown` — the legacy `is_markdown_path` check is
/// done by the caller when constructing the tab.
pub fn reinsert_tab_plan(source: StripKey, markdown: bool) -> ReinsertTabPlan {
    ReinsertTabPlan {
        strip: source,
        kind: if markdown {
            ReinsertTabKind::Markdown
        } else {
            ReinsertTabKind::Path
        },
    }
}

/// Decision for `tear_out_file_tab_to_pane`'s post-split cleanup of
/// the source strip — once the dragged tab is moved into a new pane,
/// the source strip may go empty and the host needs to drop both the
/// per-pane tabs entry and the breadcrumbs entry.
///
/// `source_remaining_tab_count` is the number of tabs the source
/// strip has left **after** the bwipeout that moved the tab out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TearOutSourceCleanup {
    /// Host should remove the source pane's `pane_tabs` entry.
    pub drop_source_pane_tabs: bool,
}

/// Decide whether the source pane's strip entry should be removed
/// after a successful tear-out. Workspace source strips are never
/// dropped (they own the workspace's terminal too). Pane source
/// strips drop only if they ended up empty.
pub fn tear_out_source_cleanup(
    source: StripKey,
    source_remaining_tab_count: usize,
) -> TearOutSourceCleanup {
    let drop = matches!(source, StripKey::Pane(_)) && source_remaining_tab_count == 0;
    TearOutSourceCleanup {
        drop_source_pane_tabs: drop,
    }
}

/// Decide whether the drag pipeline should paint a drop-preview
/// overlay on a destination strip. Only cross-strip drops show a
/// preview — dragging inside the same strip is handled by the
/// reorder code path which paints its own floating tab.
///
/// Returns `Some(dest)` when the host should set its drop preview
/// to that strip, `None` to clear the preview.
pub fn drop_preview_target(source: StripKey, dest: Option<StripKey>) -> Option<StripKey> {
    dest.filter(|d| *d != source)
}

/// Decision for the per-frame drag-move pipeline: what `drag_drop_preview`
/// the host should set on the renderer this frame.
///
/// The host owns the `TabDropPreview` value type — this helper only
/// decides which `(target_strip, mouse_x)` pair should appear. `None`
/// means clear the preview; `Some` carries both the destination strip
/// and the current mouse_x to be painted by the renderer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DropPreviewUpdate {
    pub target: StripKey,
    pub mouse_x: f32,
}

/// Compute the `drag_drop_preview` update for one drag-move frame.
///
/// - `source` is the strip the drag started in.
/// - `dest` is the strip the pointer currently hovers in (post-resolve,
///   including any `reveal_hidden_split_for_drag` fallback).
/// - `mouse_x` is the logical-px X of the pointer at this frame.
///
/// Returns `Some(update)` when the host should paint a preview
/// (cross-strip drop), `None` to clear it (same-strip or no-dest).
pub fn drop_preview_update(
    source: StripKey,
    dest: Option<StripKey>,
    mouse_x: f32,
) -> Option<DropPreviewUpdate> {
    drop_preview_target(source, dest).map(|target| DropPreviewUpdate { target, mouse_x })
}

/// Classification of a torn-out drag's release routing — the host
/// matches on this to pick which `tear_out_*_to_pane` / `move_*_between_strips`
/// path to call. Lets the desktop fork keep the IO branches in one
/// place without re-deriving the markdown/file/agent decision inline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabDragReleaseKind {
    /// Tab represents a markdown buffer — host uses the markdown
    /// tear-out / move routines (preserves markdown state).
    Markdown,
    /// Tab represents a plain file buffer — host uses the file
    /// tear-out / move routines (close in source + open in dest).
    File,
    /// Tab represents an agent surface — host uses the agent
    /// tear-out / move routines (preserves PTY/session).
    Agent,
    /// Tab has neither a path nor an agent_kind — host should drop
    /// the release silently (degenerate case).
    Drop,
}

/// Classify a `DragRelease::{TearOut,MoveOut}` payload into the
/// host-side IO routine it should dispatch to.
///
/// `has_path` mirrors `BufferTab::path.is_some()`; `markdown` mirrors
/// `BufferTab::markdown || is_markdown_path(path)` (the desktop fork
/// computes the path-extension fallback because the shared layer does
/// not have an extension table). `has_agent_kind` mirrors
/// `BufferTab::agent_kind.is_some()`.
pub fn tab_drag_release_kind(
    has_path: bool,
    markdown: bool,
    has_agent_kind: bool,
) -> TabDragReleaseKind {
    if has_path {
        if markdown {
            TabDragReleaseKind::Markdown
        } else {
            TabDragReleaseKind::File
        }
    } else if has_agent_kind {
        TabDragReleaseKind::Agent
    } else {
        TabDragReleaseKind::Drop
    }
}

/// Initial-state plan for a freshly-allocated per-pane strip after a
/// successful tear-out into a new pane.
///
/// The host allocates a new `BufferTabs` instance, scales it to chrome
/// scale, opens the dragged tab as the strip's sole entry, and inserts
/// it under the new pane's route. This struct captures the pure
/// decisions (scale, kind, route) so the host site reads top-to-bottom
/// as data → side effect.
#[derive(Debug, Clone, PartialEq)]
pub struct NewPaneStripInit {
    /// Renderer chrome scale to apply to the new strip.
    pub scale: f32,
    /// Which `BufferTabs::open_*` call seeds the strip's first tab.
    pub kind: ReinsertTabKind,
}

/// Build the [`NewPaneStripInit`] for a tear-out destination. Mirrors
/// the markdown/path branch already used by [`reinsert_tab_plan`] so
/// tear-outs and reinserts share their classification.
pub fn new_pane_strip_init(scale: f32, markdown: bool) -> NewPaneStripInit {
    NewPaneStripInit {
        scale,
        kind: if markdown {
            ReinsertTabKind::Markdown
        } else {
            ReinsertTabKind::Path
        },
    }
}

// ── Drop-preview geometry ──────────────────────────────────────────
//
// The drag-drop preview overlay paints two pieces of chrome on the
// destination strip: a tinted fill across the strip and a 2px caret
// at the insertion slot the drop would land on. The math has been
// duplicated inside `render_drop_target_preview` — extracting it
// here lets the web host reuse the slot decision when it paints its
// own preview and lets the math be unit-tested.

/// Layout result for the drop-preview overlay on a destination strip.
///
/// `caret_x` is clamped to the strip rect so the cursor doesn't draw
/// outside the strip when the pointer overshoots. `insert_index` is
/// the slot the drop would land on (0..=tab_count). `tab_width` is
/// the per-tab pixel width computed from the strip and tab count.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DropPreviewGeometry {
    pub insert_index: usize,
    pub tab_width: f32,
    pub caret_x: f32,
}

/// Compute the drop-preview insertion slot + caret position for a
/// destination strip.
///
/// `tab_count` is the strip's current tab count (`0` is treated as
/// `1` to keep `tab_width` non-zero, matching the legacy `max(1)`).
/// `available_width` and `tab_width` should both be in logical px
/// at the strip's render scale.
pub fn drop_preview_geometry(
    x_left: f32,
    available_width: f32,
    mouse_x: f32,
    scroll_x: f32,
    tab_count: usize,
    tab_width: f32,
) -> DropPreviewGeometry {
    let count = tab_count.max(1);
    let safe_tab_width = if tab_width > 0.0 {
        tab_width
    } else {
        available_width.max(1.0)
    };
    let local_x = (mouse_x - x_left + scroll_x).max(0.0);
    let insert_ix_f = (local_x / safe_tab_width).round().clamp(0.0, count as f32);
    let insert_index = insert_ix_f as usize;
    let caret_unclamped = x_left + insert_ix_f * safe_tab_width - scroll_x;
    let caret_x = caret_unclamped.clamp(x_left, x_left + available_width);
    DropPreviewGeometry {
        insert_index,
        tab_width: safe_tab_width,
        caret_x,
    }
}

// ── Hit-test ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabHit {
    /// Click landed on the body of a tab — caller activates it.
    Activate(usize),
    /// Click landed on the close glyph — caller closes the buffer.
    Close(usize),
    /// Click landed on the trailing "+" new-tab button — caller opens
    /// a fresh terminal in the current workspace. Carries no tab index
    /// because it sits in the slot just past the last tab.
    NewTab,
}

// ── StripKey / strip-aware click + drag policy ─────────────────────

/// Renderer-neutral identifier for a tab strip. Mirrors the desktop
/// fork's `crate::host::StripRef` so policy helpers can name a strip
/// without depending on host types.
///
/// `Workspace` is the workspace's primary tab strip (sits below the
/// island/window bar). `Pane(route_id)` is a per-pane tab strip
/// attached to a secondary editor pane keyed by its editor route id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StripKey {
    Workspace,
    Pane(usize),
}

/// Outcome of mapping a pointer-down on the buffer-tabs row to a
/// host action.
///
/// The host owns IO (activating routes, closing PTYs, dragging panes
/// around) — this enum only encodes which strip the click landed in
/// and what kind of host action should fire. Drag arming is signalled
/// alongside `Activate` so the host can stash the drag-source strip
/// without re-running geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StripClickOutcome {
    /// Click landed inside a pane strip's tab body. Host should arm a
    /// drag on that pane strip and activate the tab.
    PaneActivate { strip: StripKey, index: usize },
    /// Click landed on a pane strip's close glyph. Host closes the tab.
    PaneClose { strip: StripKey, index: usize },
    /// Click landed inside the workspace strip's tab body. Host arms a
    /// drag on the workspace strip and activates the tab.
    WorkspaceActivate { index: usize },
    /// Click landed on the workspace strip's close glyph.
    WorkspaceClose { index: usize },
    /// Click landed inside the workspace strip rect but missed any
    /// tab. Host should absorb the event so the pane underneath
    /// doesn't react.
    WorkspaceAbsorb,
    /// Click did not hit any strip — host should pass the event
    /// through to the editor/terminal pane underneath.
    Pass,
}

/// Geometry for the workspace strip used by [`classify_strip_click`].
/// Hosts can pass `None` when the workspace strip is hidden.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorkspaceStripGeometry {
    pub x_left: f32,
    pub y_top: f32,
    pub width: f32,
    pub height: f32,
}

impl WorkspaceStripGeometry {
    fn contains(&self, mouse_x: f32, mouse_y: f32) -> bool {
        mouse_x >= self.x_left
            && mouse_x <= self.x_left + self.width
            && mouse_y >= self.y_top
            && mouse_y <= self.y_top + self.height
    }
}

/// Decide which strip a buffer-tabs row click belongs to and what
/// host action should fire.
///
/// Inputs are the pre-resolved per-strip hits the host already runs
/// against its `BufferTabs` instances:
/// - `pane_hit`: optional `(route_id, hit)` from walking the pane
///   strips; takes priority because they paint on top of the
///   workspace strip when split panes are top-aligned.
/// - `workspace_geometry`: geometry of the workspace strip (`None`
///   when the strip is hidden).
/// - `workspace_hit`: hit-test result against the workspace strip.
///
/// `mouse_x` / `mouse_y` are used only to decide whether the click
/// landed in the workspace strip rect when no tab was hit, so the
/// host can absorb the event instead of leaking it to the pane.
pub fn classify_strip_click(
    pane_hit: Option<(usize, TabHit)>,
    workspace_geometry: Option<WorkspaceStripGeometry>,
    workspace_hit: Option<TabHit>,
    mouse_x: f32,
    mouse_y: f32,
) -> StripClickOutcome {
    if let Some((route_id, hit)) = pane_hit {
        return match hit {
            TabHit::Activate(ix) => StripClickOutcome::PaneActivate {
                strip: StripKey::Pane(route_id),
                index: ix,
            },
            TabHit::Close(ix) => StripClickOutcome::PaneClose {
                strip: StripKey::Pane(route_id),
                index: ix,
            },
            // Pane strips don't paint a "+" button, so this is
            // unreachable in practice. Absorb so a stray hit never
            // leaks into the pane underneath.
            TabHit::NewTab => StripClickOutcome::WorkspaceAbsorb,
        };
    }

    let Some(geometry) = workspace_geometry else {
        return StripClickOutcome::Pass;
    };

    match workspace_hit {
        Some(TabHit::Activate(ix)) => StripClickOutcome::WorkspaceActivate { index: ix },
        Some(TabHit::Close(ix)) => StripClickOutcome::WorkspaceClose { index: ix },
        // The "+" new-tab button is handled by the host before
        // `classify_strip_click` (it needs to spawn a terminal, which
        // this pure policy can't do). Absorb defensively so a NewTab hit
        // that reaches here doesn't fall through to the pane.
        Some(TabHit::NewTab) => StripClickOutcome::WorkspaceAbsorb,
        None => {
            if geometry.contains(mouse_x, mouse_y) {
                StripClickOutcome::WorkspaceAbsorb
            } else {
                StripClickOutcome::Pass
            }
        }
    }
}

/// Sentinel hover/focus index for the trailing "+" new-tab slot. The
/// strip's hover-animation and focus-cursor bookkeeping key tabs by
/// their `0..tabs.len()` index; the "+" sits one slot past the last
/// tab, so it uses `usize::MAX` to stay distinct from any real tab
/// without colliding with `tabs.len()` when the tab list grows.
const NEW_TAB_HOVER_IX: usize = usize::MAX;

fn tab_hit_index(hit: TabHit) -> usize {
    match hit {
        TabHit::Activate(ix) | TabHit::Close(ix) => ix,
        TabHit::NewTab => NEW_TAB_HOVER_IX,
    }
}

// ── Drag lifecycle ─────────────────────────────────────────────────

/// Pixel distance below the strip's bottom edge that the cursor must
/// reach (in logical px) before a drag is treated as a tear-out into
/// a split.
pub const TEAR_OUT_DROP_THRESHOLD_PX: f32 = 36.0;

/// Once tear-out is armed, how far the cursor must travel rightward
/// from the press point before we flip the orientation from a
/// horizontal split (below) to a vertical split (right).
const TEAR_OUT_VERTICAL_X_THRESHOLD_PX: f32 = 160.0;

/// Pixel distance the cursor must travel after a press before we
/// treat it as a drag (vs a plain activation click). Below this we
/// don't lift the tab visually so simple clicks don't flicker.
const DRAG_ACTIVATION_THRESHOLD_PX: f32 = 4.0;

/// Live state for an in-progress drag — covers both "reorder
/// horizontally inside the strip" and "tear out into a split below".
#[derive(Clone, Debug)]
pub struct DragState {
    /// Index of the dragged tab — updated as we swap with neighbors.
    pub current_ix: usize,
    pub press_local_x: f32,
    pub current_local_x: f32,
    pub press_y: f32,
    pub current_y: f32,
    pub grab_offset: f32,
    /// `true` once the cursor has moved past the activation threshold.
    pub active: bool,
    /// `true` once the cursor has descended past the strip's bottom
    /// by `TEAR_OUT_DROP_THRESHOLD_PX`.
    pub tear_out_armed: bool,
    /// Once tear-out is armed, this picks the split orientation:
    /// `true` → horizontal split, `false` → vertical split.
    pub tear_out_horizontal: bool,
}

/// Outcome of a drag release.
pub enum DragRelease<A> {
    /// No drag was active (plain click).
    None,
    /// Drag activated but landed inside the strip — reorder already
    /// applied incrementally inside `update_drag`.
    Reorder,
    /// Drag activated and released over a different tab strip. The
    /// screen layer already knows the destination strip; this variant
    /// only hands it the removed tab.
    MoveOut { tab: BufferTab<A> },
    /// Drag activated AND released past the tear-out threshold below
    /// the strip.
    TearOut {
        #[allow(dead_code)]
        ix: usize,
        tab: BufferTab<A>,
        /// `true` → horizontal split (new pane below the active pane).
        split_down: bool,
    },
}

/// Brief post-release "tab lifted off" animation.
#[derive(Clone, Debug)]
pub struct TearOutAnim {
    pub started_at: Instant,
    pub from_x: f32,
    pub from_y: f32,
    pub width: f32,
    pub title: String,
}

const TEAR_OUT_ANIM_MS: u32 = 180;

// ── BufferTabs (state + behavior) ──────────────────────────────────

/// Buffer tab strip state.
///
/// Most fields are `pub` so the host's render shim can read and lerp
/// scroll / animation state without bouncing through accessor methods.
/// Mutators that keep invariants (`set_active`, `set_focused`,
/// `set_hover`, etc.) still live on the impl below — prefer those when
/// you're not in the per-frame paint loop.
#[derive(Clone)]
pub struct BufferTabs<A> {
    pub visible: bool,
    pub tabs: Vec<BufferTab<A>>,
    pub active: usize,
    /// Cached layout from the most recent render — `(x_left, width)`
    /// per tab in logical pixels. Used by `hit_test` so we never
    /// recompute fitting math on click. Cleared by `set_tabs`.
    pub layout: Vec<(f32, f32)>,
    /// Multiplier applied to base height/font/padding constants so the
    /// strip grows with Ctrl+/- font zoom.
    pub scale: f32,
    /// Current horizontal scroll offset in logical pixels. Lerped each
    /// frame toward `scroll_target_x`.
    pub scroll_x: f32,
    pub scroll_target_x: f32,
    /// Set when `active` changes outside of render.
    pub pending_ensure_active: bool,
    pub drag: Option<DragState>,
    pub tear_out_anim: Option<TearOutAnim>,
    pub hover: Option<TabHit>,
    pub hover_anim_started: Option<Instant>,
    pub hover_from: Option<usize>,
    pub hover_to: Option<usize>,
    pub focused: bool,
    pub focused_index: usize,
    pub focused_cursor_rect: Option<[f32; 4]>,
    /// Most-recent tab index the user activated via a pointer click.
    /// Drained by the host (chrome / wasm bridge) each frame so a
    /// click on tab N can be turned into a `set_active_tab_index(N)`
    /// in the surrounding `Chrome<A>` plus a JS-side bookkeeping
    /// update. `None` when no fresh activation is pending.
    pub pending_activate: Option<usize>,
    /// Tab indices the user requested to close via the X glyph (or
    /// equivalent). Drained by the host each frame; the host removes
    /// the buffer + replays `set_buffer_tabs` so the strip mirrors
    /// the host's bookkeeping list.
    pub pending_closes: Vec<usize>,
    /// Hit-test rect of the trailing "+" new-tab button as
    /// `[x, y, w, h]` in logical pixels, captured each render. `None`
    /// when the strip is hidden / not yet painted. Read by `hit_test`
    /// so a pointer-down in the "+" maps to [`TabHit::NewTab`].
    pub new_tab_rect: Option<[f32; 4]>,
    /// Window-absolute top-left of the strip as painted by the trait
    /// `draw` path, captured each frame. The trait `handle_event`
    /// receives strip-LOCAL pointer coords (the chrome translates by
    /// the panel rect), while render-captured rects like
    /// `new_tab_rect` are window-absolute — this origin bridges the
    /// two for `hit_test`.
    pub panel_origin: (f32, f32),
    /// Set when the user clicked the trailing "+" new-tab button.
    /// Drained by the host each frame (web: spawn a terminal tab,
    /// mirroring desktop's `TabCreateNew`).
    pub pending_new_tab: bool,
}

impl<A> Default for BufferTabs<A> {
    fn default() -> Self {
        BufferTabs::new()
    }
}

mod impl_core;

impl<A: AgentLabel + Copy + PartialEq> BufferTabs<A> {
    pub fn agent_for_route(&self, route_id: usize) -> Option<A> {
        self.tabs
            .iter()
            .find(|tab| tab.terminal_route_id == Some(route_id))
            .and_then(|tab| tab.agent_kind)
    }

    pub fn set_terminal_agent(&mut self, route_id: usize, agent: A) -> bool {
        let Some(tab) = self
            .tabs
            .iter_mut()
            .find(|tab| tab.terminal_route_id == Some(route_id))
        else {
            return false;
        };
        tab.title = agent.display_name().to_string();
        tab.agent_kind = Some(agent);
        self.layout.clear();
        true
    }

    pub fn set_detected_terminal_agents(
        &mut self,
        detected: &[(usize, bool, A)],
    ) -> bool {
        let mut changed = false;
        let mut terminal_count = 0usize;
        for tab in &mut self.tabs {
            if !tab.is_terminal() {
                continue;
            }
            terminal_count += 1;
            let detected_agent = if let Some(route_id) = tab.terminal_route_id {
                detected
                    .iter()
                    .find(|(detected_route, _, _)| *detected_route == route_id)
                    .map(|(_, _, agent)| *agent)
            } else {
                detected
                    .iter()
                    .find(|(_, is_root, _)| *is_root)
                    .map(|(_, _, agent)| *agent)
            };

            let next_title = detected_agent
                .map(|agent| agent.display_name().to_string())
                .unwrap_or_else(|| {
                    if tab.terminal_route_id.is_none() {
                        TERMINAL_TITLE.to_string()
                    } else {
                        format!("Terminal {terminal_count}")
                    }
                });
            if tab.agent_kind != detected_agent || tab.title != next_title {
                tab.agent_kind = detected_agent;
                tab.title = next_title;
                changed = true;
            }
        }
        if changed {
            self.layout.clear();
        }
        changed
    }
}

// ── IdeTheme-aware render (lifted from desktop golden) ──────────────
//
// The desktop frontend previously hosted this render in a native shim
// at `frontends/neoism/src/chrome/panels/buffer_tabs.rs` that wrapped
// `BufferTabs<AgentKind>` in a newtype and added these inherent
// methods on top. The wrapping shim is gone — the render is now part
// of the shared crate, generic over `A`, so every frontend (native
// winit, web wasm) can paint the strip with the rich `IdeTheme`
// palette directly.

const NEOISM_AGENT_ICON: &str = "n";

mod impl_render;

impl<A: Copy> BufferTabs<A> {
    /// Web/wasm `Panel`-path painter — desktop tab-chrome parity.
    ///
    /// Delegates to the exact same `render_with_icons` pass the native
    /// frontend paints with (Obsidian-style two-tone strip, rounded
    /// active tab, hover scale, drag previews, trailing "+" new-tab
    /// button), minus the PNG agent-logo overlay: the web host has no
    /// `AgentIconProvider`, so the Nerd-Font glyph fallback paints
    /// instead. The theme comes from the shared `active_ide_theme`
    /// cell (kept in sync by `Chrome::set_ide_theme`), matching the
    /// chrome_shim panels' pattern.
    ///
    /// Also records `panel_origin`: the trait `handle_event` receives
    /// strip-LOCAL pointer coords (`Chrome::dispatch_to` translates by
    /// the panel rect) while this render captures `new_tab_rect` in
    /// window-absolute pixels — the origin lets `hit_test` reconcile
    /// the two.
    fn draw_visual(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        layout: &PanelLayout,
        _ctx: &PanelContext,
    ) {
        let bounds = layout.bounds;
        self.panel_origin = (bounds.x, bounds.y);
        if bounds.w <= 0.0 || bounds.h <= 0.0 {
            self.layout.clear();
            self.focused_cursor_rect = None;
            self.new_tab_rect = None;
            return;
        }
        let theme = crate::chrome::active_ide_theme();
        self.render_with_icons::<NoopAgentIcons<A>>(
            sugarloaf,
            bounds.x,
            bounds.y,
            bounds.w,
            &theme,
            None,
            None,
            &[],
        );
    }
}

// ── Panel trait impl ───────────────────────────────────────────────
//
// The trait surface stays intentionally narrow on this first lift: the
// strip plays well as a passive view of state set externally, plus a
// minimal "click → activate / close" reflex. Drag, scroll inertia and
// theme-aware paint stay native (the shim in `frontends/neoism/` adds
// inherent methods for those) until the rest of Wave 4 lands. New
// `UiEvent` variants slot in by extending `handle_event` here without
// breaking the native shim.

impl<A: Send + Copy + 'static> Panel for BufferTabs<A> {
    fn handle_event(&mut self, event: &UiEvent, _ctx: &mut PanelContext) {
        match event {
            UiEvent::PointerDown {
                button: PointerButton::Left,
                x,
                y,
                ..
            } => {
                // The host gives us strip-local coordinates by virtue
                // of routing via the panel's `PanelLayout.bounds`.
                // Translate back to window space via the origin the
                // draw path recorded so render-captured rects (the
                // "+" button's `new_tab_rect`) hit-test correctly —
                // the per-tab math is origin-relative either way.
                let (ox, oy) = self.panel_origin;
                if let Some(hit) =
                    self.hit_test(*x + ox, *y + oy, ox, oy, self.last_strip_width())
                {
                    match hit {
                        TabHit::Activate(ix) => {
                            self.set_active(ix);
                            // Surface to the host so chrome /
                            // JS-side bookkeeping can swap the active
                            // tab content over the terminal rect.
                            self.pending_activate = Some(ix);
                        }
                        TabHit::Close(ix) => {
                            // Don't touch the local tabs vec — the
                            // host owns the canonical buffer list and
                            // will call `set_tabs` after acting on
                            // this drain. Closing locally here would
                            // race with that replay.
                            self.pending_closes.push(ix);
                        }
                        // The trailing "+" button — queue a new-tab
                        // intent for the host to drain (web: spawn a
                        // terminal tab, desktop parity TabCreateNew).
                        TabHit::NewTab => {
                            self.pending_new_tab = true;
                        }
                    }
                }
            }
            UiEvent::PointerMove { x, y, .. } => {
                let (ox, oy) = self.panel_origin;
                let hit =
                    self.hit_test(*x + ox, *y + oy, ox, oy, self.last_strip_width());
                self.set_hover(hit);
            }
            UiEvent::PointerLeave => {
                self.set_hover(None);
            }
            UiEvent::Wheel { dx, dy, mode, .. } => {
                let delta = match mode {
                    WheelMode::Pixel => SessionScrollDelta::Pixels { x: *dx, y: *dy },
                    WheelMode::Line => SessionScrollDelta::Lines { x: *dx, y: *dy },
                    WheelMode::Page => SessionScrollDelta::Pixels {
                        x: *dx * self.last_strip_width().max(1.0),
                        y: *dy * self.height().max(1.0),
                    },
                };
                let scroll = buffer_tabs_scroll_dx(delta, 0.5);
                if scroll != 0.0 {
                    self.scroll_by(scroll);
                }
            }
            UiEvent::Resize { .. } => {
                // Next render frame recomputes layout slots against
                // the fresh width; only need to drop cached layout
                // here so a stale hit_test doesn't fire in between.
                self.layout.clear();
                self.pending_ensure_active = !self.tabs.is_empty();
            }
            UiEvent::Focus(focused) => {
                self.set_focused(*focused);
            }
            UiEvent::Key(key) => {
                use crate::event::{KeyState, LogicalKey, NamedKey};
                if !self.focused || key.state != KeyState::Pressed {
                    return;
                }
                let mods = key.modifiers;
                let plain = !mods.intersects(
                    Modifiers::SHIFT | Modifiers::CTRL | Modifiers::ALT | Modifiers::META,
                );
                let ctrl_only = mods.contains(Modifiers::CTRL)
                    && !mods
                        .intersects(Modifiers::SHIFT | Modifiers::ALT | Modifiers::META);
                if !plain && !ctrl_only {
                    return;
                }
                if let LogicalKey::Named(named) = &key.logical {
                    match named {
                        NamedKey::ArrowLeft => {
                            self.move_focused(true);
                        }
                        NamedKey::ArrowRight => {
                            self.move_focused(false);
                        }
                        NamedKey::ArrowDown | NamedKey::Escape => {
                            self.set_focused(false);
                        }
                        NamedKey::Enter => {
                            let ix = self.focused_index();
                            self.set_active(ix);
                            self.pending_activate = Some(ix);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    #[allow(invalid_reference_casting)]
    fn draw(&self, sugarloaf: &mut Sugarloaf, layout: &PanelLayout, ctx: &PanelContext) {
        // The panel trait is shared with cache-by-Cell panels and takes
        // `&self`, but buffer tabs intentionally preserve the native
        // mutable render state: layout slots, scroll interpolation, and
        // transient animation cleanup. Hosts call this on the UI thread.
        let this = unsafe { &mut *(self as *const Self as *mut Self) };
        this.draw_visual(sugarloaf, layout, ctx);
    }

    fn wants_focus(&self) -> bool {
        self.focused
    }

    fn name(&self) -> &str {
        "buffer_tabs"
    }
}

impl<A> BufferTabs<A> {
    /// Width to feed `hit_test` from the trait `handle_event` path.
    /// We don't get the strip width via `UiEvent` — the host routes
    /// strip-local coordinates already, so the latest cached layout
    /// slot widths sum to the visible strip. Falls back to a sentinel
    /// when no frame has rendered yet so the very first event still
    /// resolves to no-op rather than panicking on divide-by-zero.
    fn last_strip_width(&self) -> f32 {
        if self.layout.is_empty() {
            // Nothing rendered yet → pretend the strip is one max-width
            // tab so the click maps to slot 0 if there's one tab.
            MAX_TAB_WIDTH
        } else {
            // Reconstruct the width: total_w = tabs * tab_width; but the
            // layout stores `(x_left, width)` per tab and each slot
            // shares the same width, so just multiply.
            let per = self
                .layout
                .first()
                .map(|(_, w)| *w)
                .unwrap_or(MAX_TAB_WIDTH);
            per * self.tabs.len() as f32
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────────

/// Mirrors `frontends/neoism/src/editor/markdown/state/helpers.rs`.
/// Lifted here so the panel logic doesn't reach back into the native
/// editor crate.
fn is_markdown_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| matches!(ext.to_ascii_lowercase().as_str(), "md" | "markdown" | "mdx"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests;
