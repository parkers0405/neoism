//! Cross-platform chrome assembly.
//!
//! `Chrome` owns every panel that has been lifted into `neoism-ui`,
//! lays them out within a window-sized viewport, distributes
//! `UiEvent`s in modal-aware priority order, and orchestrates the
//! per-frame draw pass through `sugarloaf`.
//!
//! Both the native (winit) and web (wasm) frontends construct a
//! single `Chrome` per top-level window and call
//! [`Chrome::set_layout`] + [`Chrome::handle_event`] + [`Chrome::draw`]
//! every frame.
//!
//! # Panel inventory
//!
//! As of the chrome wave the assembly hosts the eight panels that
//! already live in `neoism-ui`:
//!
//! - [`StatusLine`] — bottom strip with mode + diagnostics + git
//!   summary, always visible.
//! - [`BufferTabs`] — top strip with the open buffer tabs, generic
//!   over the host's agent label type.
//! - [`FileTree`] — left sidebar column. Optional: hosts that don't
//!   want a sidebar leave it `None`.
//! - [`CommandPalette`] — centered modal command launcher.
//! - [`Finder`] — centered modal multi-mode finder.
//! - [`GitDiff`] — full-window diff overlay.
//! - [`CommandComposer`] — sticky Warp-style command bar above the
//!   status line.
//!
//! Markdown editor state at `editor::markdown` is owned by individual
//! buffer tabs rather than the chrome, so it is not listed here.
//!
//! # Event priority
//!
//! Each call to [`Chrome::handle_event`] walks the panels in the order
//! produced by [`Chrome::event_priority_order`]:
//!
//! 1. Visible modal overlays (`CommandPalette` → `Finder` →
//!    `CommandComposer` → `GitDiff`).
//! 2. The top of the explicit `focus_stack`.
//! 3. The remaining background panels (status line, buffer tabs,
//!    file tree) in z-order.
//!
//! Keyboard-shaped events (`Key`, `Text`, `Composition`) stop at the
//! first modal that consumes them — modals "swallow" the keyboard.
//! Pointer-shaped events propagate through every panel whose layout
//! rect contains the cursor; this lets a click outside a visible
//! modal still reach the background panels for hit-testing without
//! the modal first having to forward.
//!
//! # Layout
//!
//! [`Chrome::set_layout`] takes a window viewport and writes per-panel
//! rects into its [`ChromeLayout`]. The math is deliberately
//! pixel-literal — designed to match the legacy native chrome — but
//! callers can post-process the layout if they need a custom strip
//! height or sidebar width.

use std::sync::RwLock;
use web_time::Duration;

use crate::animation::CriticallyDampedSpring;
use crate::input::SimpleInputBuffer;
use crate::layout::{ChromeLayout, Rect};
use crate::panels::agent_pane::state::NeoismAgentPane;
use crate::panels::breadcrumbs::Breadcrumbs;
use crate::panels::completion_menu::CompletionMenu;
use crate::panels::context_menu::ContextMenu;
use crate::panels::diagnostics_popup::DiagnosticsPopup;
use crate::panels::file_tree::FILE_TREE_WIDTH;
use crate::panels::minimap::Minimap;
use crate::panels::notifications::Notifications;
use crate::panels::search::SearchOverlay;
use crate::panels::splash_overlay::SplashOverlay;
use crate::panels::trail_cursor::TrailCursor;
use crate::panels::yank_flash::YankFlash;

mod config;
mod content;
mod draw;
mod events;
mod pages;
mod paint;
pub use content::EditorPaneKind;
pub(crate) use paint::*;

/// Host-declared description of what one pane leaf currently displays.
/// The web bridge pushes one entry per visible pane (keyed by the pane
/// grid's `external_id`) so the chrome can render UNFOCUSED pane
/// surfaces — parked editor panes resolved by `path` — and paint an
/// honest labeled placeholder for surfaces it cannot host yet.
#[derive(Clone, Debug, PartialEq)]
pub struct PaneSurfaceInfo {
    pub external_id: u64,
    /// Surface kind string (`"terminal"`, `"editor"`, `"markdown"`, …).
    pub kind: String,
    /// Backing file for editor-like surfaces; used to resolve parked
    /// panes.
    pub path: Option<std::path::PathBuf>,
    /// Display title for the placeholder label.
    pub title: Option<String>,
}

/// Zero-state placeholder for the stateless `git_branch` module.
/// The module itself only exposes free helpers (`branch_for`,
/// `change_summary_for`, `repo_root_for`); the chrome doesn't own any
/// per-instance state for it. We keep an installable wrapper so the
/// bridge can call `install_git_branch(GitBranch::default())` for
/// symmetry with the other panels, and so a future caller has a slot
/// to hang configuration on if the module grows state.
#[derive(Default, Debug, Clone, Copy)]
pub struct GitBranch;

impl GitBranch {
    pub fn new() -> Self {
        Self
    }
}

/// State holder for the custom mouse-cursor sprite. The module exposes
/// a free `draw(sugarloaf, x, y, scale)`; the desktop renderer feeds it
/// the live `Mouse` position directly. The web bridge has no equivalent
/// host-side mouse hook, so this struct caches the latest pointer
/// position pushed from JS through `ChromeBridge::set_custom_cursor`.
/// `Chrome::draw` paints the sprite from the cached position when
/// `visible` is `true`.
#[derive(Default, Debug, Clone, Copy)]
pub struct CustomCursor {
    /// Pointer position in physical pixels (matches the desktop's
    /// `Mouse.x` / `Mouse.y` convention so the free draw fn doesn't
    /// have to grow a second coordinate space).
    pub x: f32,
    pub y: f32,
    /// Whether the sprite should paint this frame. The web bridge
    /// flips this off when the pointer leaves the canvas so the
    /// last-known position doesn't ghost in the corner.
    pub visible: bool,
}

impl CustomCursor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Update the cached pointer position. `visible = false` hides the
    /// sprite on the next paint without forgetting the last position.
    pub fn set_position(&mut self, x: f32, y: f32, visible: bool) {
        self.x = x;
        self.y = y;
        self.visible = visible;
    }
}
use crate::panels::chrome_topbar::{ChromeTopBar, TopBarAction};
use crate::panels::git_diff::GitDiffPanel;
use crate::panels::notes_sidebar::NotesSidebar;
use crate::panels::pane_grid::PaneGrid;
use crate::panels::{
    BufferTabs, CommandComposer, CommandPalette, FileTree, Finder, GitDiff, StatusLine,
};
use crate::primitives::IdeTheme;
use crate::theme::ChromeTheme;

/// Process-wide "currently active IdeTheme" cell. Chrome owns the
/// authoritative copy on its instance (`Chrome::ide_theme`); we mirror
/// it here so the slim `Panel::draw` adapter shims in
/// `panels::chrome_shim` / `panels::chrome_shim_more` can read the
/// same palette without holding a reference to the parent `Chrome`
/// (the shim methods are `impl Panel for …` and only get
/// `&PanelContext`, which doesn't yet carry an `IdeTheme`).
///
/// Updated by [`Chrome::set_ide_theme`] and read via
/// [`active_ide_theme`]. Wasm is single-threaded; the native chrome
/// only constructs one `Chrome` per window so contention is minimal.
static ACTIVE_IDE_THEME: RwLock<Option<IdeTheme>> = RwLock::new(None);

/// Snapshot of the active IdeTheme, falling back to
/// `IdeTheme::default()` (pastel_dark) when no `Chrome::set_ide_theme`
/// has run yet. Cheap: `IdeTheme` is `Copy`.
pub fn active_ide_theme() -> IdeTheme {
    ACTIVE_IDE_THEME
        .read()
        .ok()
        .and_then(|g| *g)
        .unwrap_or_default()
}

/// Publish the process-wide active theme. `Chrome::set_ide_theme`
/// (web) calls this internally; the DESKTOP renderer must call it from
/// its own `set_ide_theme` — it doesn't drive a `Chrome`, and without
/// the publish every `active_ide_theme()` consumer (shims, the agent
/// wordmark tint) silently renders with pastel_dark defaults.
pub fn publish_active_ide_theme(theme: IdeTheme) {
    if let Ok(mut cell) = ACTIVE_IDE_THEME.write() {
        *cell = Some(theme);
    }
}

/// Default width of the file-tree sidebar in logical pixels. The
/// host may shrink the tree by calling [`Chrome::set_file_tree_width`]
/// before [`Chrome::set_layout`] re-runs.
pub const DEFAULT_FILE_TREE_WIDTH: f32 = FILE_TREE_WIDTH;

/// Default fixed height of the command composer above the status line.
pub const COMMAND_COMPOSER_HEIGHT: f32 = 56.0;

/// Default centered-modal width for command palette / finder. Hosts
/// can override on a per-frame basis if they want a different modal
/// width by post-mutating `layout.command_palette` / `layout.finder`.
const MODAL_WIDTH: f32 = 720.0;

/// Default centered-modal height for command palette / finder.
const MODAL_HEIGHT: f32 = 420.0;

/// Symbolic identifier for the seven panels the chrome owns. Used to
/// drive the focus stack and to walk the panels in priority order
/// without resorting to `Box<dyn Panel>` storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PanelKey {
    StatusLine,
    BufferTabs,
    TopBar,
    FileTree,
    CommandPalette,
    Finder,
    GitDiff,
    CommandComposer,
    // Slim panels lifted in Wave 6F + W3-A. None of these currently
    // participate in the focus stack (they paint over existing rects
    // or are data-driven popovers that pull focus implicitly). The
    // variants exist so future routing waves can address them.
    Breadcrumbs,
    CompletionMenu,
    Minimap,
    Notifications,
    DiagnosticsPopup,
    ContextMenu,
    Search,
    GitBranch,
    CustomCursor,
    TrailCursor,
    YankFlash,
}

/// Full cross-platform chrome assembly. Generic over the host's agent
/// label type `A` so the buffer-tabs strip can keep its
/// `AgentLabel + Copy + PartialEq` API without dragging the host's
/// `AgentKind` enum into this crate.
pub struct Chrome<A: Send + Copy + 'static = ()> {
    /// Per-frame rects for each panel. Re-computed by `set_layout`.
    layout: ChromeLayout,
    /// Resolved chrome palette. Panels read this through
    /// `PanelContext::theme`.
    theme: ChromeTheme,
    /// Resolved IdeTheme (richer palette: bg/fg/surface/syn_*) used
    /// by the splash overlay and the slim adapter shims. Mirrored to
    /// the process-wide [`ACTIVE_IDE_THEME`] cell so the shims can
    /// read it without a back-reference to `Chrome`.
    ide_theme: IdeTheme,
    /// User-picked cursor color (`[neoism] cursor-color`) — overrides
    /// the theme-derived cursor color and survives theme switches.
    cursor_color_override: Option<[f32; 4]>,
    /// Cursor preset (`[neoism] cursor-style`). Animated presets
    /// (rainbow) ignore both the theme and the override color.
    cursor_style: crate::cursor_style::CursorStyle,
    /// Width of the file-tree sidebar in logical pixels. Honored by
    /// `set_layout` when `file_tree.is_some()`.
    file_tree_width: f32,
    /// Resolved terminal cell width/height in logical pixels. Set by
    /// the host via [`Chrome::set_cell_metrics`]; defaults to the
    /// terminal renderer's 8x16 fallback.
    cell_w: f32,
    cell_h: f32,
    /// Global chrome text/spacing multiplier. Mirrors desktop's
    /// `Renderer::chrome_scale`: `1.0` means the 14px/8x16 baseline.
    chrome_scale: f32,
    /// Optional host-reserved strip between the top bar and buffer tabs.
    /// Web uses this for the shared workspace Island so the top bar stays
    /// above both workspaces and buffer tabs while all lower chrome shifts down.
    top_workspace_strip_h: f32,
    /// Host obstruction below editable chrome. Status remains pinned to the
    /// physical viewport bottom (mobile keyboard exclusion).
    bottom_content_inset: f32,
    /// Host capability: only the shared web frontend enables the narrow Agent
    /// side-panel takeover/button. A narrow desktop window keeps desktop UI.
    mobile_web_agent_panel_enabled: bool,
    mobile_agent_narrow: bool,
    desktop_agent_panel_open_before_narrow: Option<bool>,
    /// Host-fed animation phase in seconds modulo the same 10k-second
    /// window desktop uses. Web supplies this from `performance.now()`
    /// because `SystemTime::now()` panics on wasm.
    animation_phase: f32,
    /// Which buffer-tab the user is viewing. `0` is the live terminal
    /// pane (cells + splash); any other index shows the cached text
    /// content in [`Chrome::tab_content`] over the same rect.
    active_tab_index: usize,
    /// Plain-text content for the currently-active non-Terminal tab.
    /// Hosts push this via [`Chrome::set_tab_content`].
    tab_content: Option<String>,
    /// Host-fed terminal input snapshot that the command composer
    /// renders. Native drives this from `TerminalInputBuffer`; the web
    /// bridge mirrors pending shell input into this simpler POD buffer.
    terminal_input: SimpleInputBuffer,
    /// Source language for the active tab — drives syntax-highlight
    /// token colors when painting the file-viewer pane.
    tab_lang: crate::syntax::Lang,
    /// Lazily-constructed markdown pane for `.md` tabs. The file-viewer
    /// branch in `Chrome::draw` renders block-aware markdown (headings,
    /// lists, code, quotes, dividers) by walking
    /// `MarkdownPane.blocks` instead of the per-line syntax highlighter
    /// when `tab_lang == Lang::Markdown`. Hosts seed via
    /// [`Chrome::set_markdown_content`] whenever they push tab content
    /// for a `.md` path; cleared when the host pushes a non-markdown
    /// tab.
    markdown_pane: Option<crate::editor::markdown::MarkdownPane>,
    /// Hosted native code editor pane for the active non-markdown text
    /// tab (desktop hosts one `CodePane` per tab Context; the web
    /// chrome hosts ONE at a time and re-seeds it on tab switch, the
    /// same model `markdown_pane` uses). Seeded through
    /// [`Chrome::open_editor_file`]; painted by `Chrome::draw` inside
    /// the terminal rect whenever [`Chrome::active_editor_pane_kind`]
    /// says the pane belongs to the active tab.
    code_pane: Option<crate::editor::code::CodePane>,
    /// Hosted `.ipynb` notebook pane (owns an inner `MarkdownPane`
    /// that the shared markdown renderer paints — desktop parity with
    /// `bridges/markdown/render.rs`'s notebook branch).
    notebook_pane: Option<crate::editor::notebook::NotebookPane>,
    /// Hosted `.neodraw` sketch pane, painted through
    /// `editor::neodraw::render_pane`.
    draw_pane: Option<crate::editor::neodraw::DrawPane>,
    /// Buffer-tab index the hosted editor pane (code / notebook /
    /// draw) belongs to. The pane only paints while this matches
    /// `active_tab_index`; the host re-calls `open_editor_file` on
    /// every tab activation so a shifted index self-heals.
    editor_pane_tab: Option<usize>,
    /// Set by `Chrome::draw` while the hosted editor pane still has an
    /// animation in flight (code scroll glide, notebook eased scroll,
    /// draw graph sim) so `animations_active()` keeps the host's
    /// frame pump running.
    editor_pane_animating: bool,
    /// CRDT binding for the hosted code pane (web co-editing + the
    /// daemon-owned single-writer save). Lives next to the pane so
    /// both are dropped/re-bound together on tab switches. The wasm
    /// bridge drives it through [`Chrome::code_editor_parts_mut`].
    code_doc_binding: Option<crate::editor::code::doc_sync::CodeDocBinding>,
    /// Shared LSP session layer for the hosted code pane — desktop's
    /// `Renderer::code_lsp` twin (completion / hover / actions /
    /// rename / diagnostics state machines). The host installs an
    /// `LspService` backend and feeds results; `Chrome::draw` pumps it
    /// and hosts its popups (completion menu + hover card).
    pub code_lsp: crate::editor::code::lsp_session::CodeLspUi,
    /// LSP status-pill popup ("Server Details" card) — opened by the
    /// status line's LSP pill, fed from daemon `LspSnapshot` pushes.
    pub lsp_popup: crate::panels::lsp_popup::LspPopup,
    /// Panes displaced from the hosted slots by a tab switch, keyed by
    /// path. Desktop keeps one pane per tab Context; the web chrome
    /// hosts one slot per kind, so displaced panes park here and are
    /// restored by [`Chrome::open_editor_file`] — cursor, undo, and
    /// unsaved edits survive a tab round-trip. Bounded by the set of
    /// files opened this session (same order as the host's per-tab
    /// content cache).
    parked_code_panes:
        std::collections::HashMap<std::path::PathBuf, crate::editor::code::CodePane>,
    parked_notebook_panes: std::collections::HashMap<
        std::path::PathBuf,
        crate::editor::notebook::NotebookPane,
    >,
    parked_draw_panes:
        std::collections::HashMap<std::path::PathBuf, crate::editor::neodraw::DrawPane>,

    pub status_line: StatusLine,
    pub buffer_tabs: BufferTabs<A>,
    /// Window-top chrome strip: panel toggle + hamburger menu. Visible
    /// by default; hosts can hide it with `top_bar.set_visible(false)`.
    pub top_bar: ChromeTopBar,
    pub file_tree: Option<FileTree>,
    pub command_palette: CommandPalette,
    pub finder: Finder,
    pub git_diff: GitDiff,
    /// Rich right-side git diff panel — desktop's Alt+G side column
    /// (file_tree-style chrome, Warp-style content), lifted into the
    /// shared crate. Layout reserves its width off the content
    /// column's right edge while visible. Data arrives from a native
    /// `GitDiffIo` provider on desktop and from daemon pushes
    /// (`host_set_files` / `host_set_diff_text`) on web.
    pub git_diff_panel: GitDiffPanel,
    /// Left notes sidebar — desktop's Alt+N column. Docks right of
    /// the file tree; entry data comes from local fs on desktop and
    /// from daemon listings (`set_entries_from_host`) on web.
    pub notes_sidebar: NotesSidebar,
    pub command_composer: CommandComposer,
    /// Cwd of the terminal owning the visible composer. This remains
    /// terminal-local even though the status-line cwd is workspace-global.
    terminal_cwd_label: Option<String>,
    /// Golden-standard split/pane controller. Owns the canonical
    /// [`crate::session_layout::tree::SessionTree`] and turns pointer /
    /// keyboard interactions into tree mutations + host actions (Zed/VS
    /// Code style splits, divider resize, drag-to-split, adopt-as-tab).
    /// Hosts subdivide the content (`terminal`) rect through this piece
    /// via `pane_grid.set_content(..)` and drive it through its `on_*` /
    /// `split_*` methods, draining `pane_grid.take_actions()` each frame.
    /// Other chrome pieces query it (focused pane, pane rects) so they
    /// "know about" the live split topology.
    pub pane_grid: PaneGrid,
    /// Host-declared per-pane surface descriptors (see
    /// [`PaneSurfaceInfo`]). Consulted by the unfocused-pane render
    /// pass while the grid is split.
    pane_surfaces: Vec<PaneSurfaceInfo>,
    /// Per-pane tab strips keyed by pane external id — the web twin of
    /// desktop's `Renderer::pane_tabs` map (host/mod.rs). Stacked
    /// (non-top-aligned) panes render their strip inside their own
    /// rect; layout reserves the row via `ChromeLayout::panes`.
    pane_tabs: std::collections::HashMap<u64, BufferTabs<A>>,
    /// Per-pane breadcrumbs (desktop `Renderer::pane_breadcrumbs`):
    /// sits under the pane's strip and shows its active tab's path.
    pane_breadcrumbs: std::collections::HashMap<u64, Breadcrumbs>,
    /// External ids of panes the HOST painted this frame (live
    /// terminal grids). The chrome's unfocused-pane pass skips these
    /// so it never paints a placeholder over live cells.
    host_drawn_panes: Vec<u64>,
    /// Shared Rust-rendered agent pane. Installed by hosts that want the
    /// Neoism Agent tab to paint through chrome instead of a
    /// frontend-local agent pane.
    pub agent_pane: Option<NeoismAgentPane>,
    /// Animated NEOISM wordmark + menu shown over an empty terminal
    /// pane. Painted last among the background layers so it sits on
    /// top of the terminal cells but under the composer and modals.
    pub splash_overlay: SplashOverlay,
    terminal_splash_dismissed: bool,

    /// Slim panels lifted in Wave 6F. These don't have their own
    /// `PanelKey` slot in the focus stack yet — they paint over
    /// existing layout rects (breadcrumbs strip over the tab bar,
    /// notifications stack inside the terminal column, etc.). Wired
    /// here so Chrome can issue a single `.draw()` per frame; routing
    /// per-host UiEvent into them lands in a follow-up wave.
    pub breadcrumbs: Breadcrumbs,
    pub notifications: Notifications,
    pub completion_menu: CompletionMenu,
    pub search_overlay: SearchOverlay,
    pub minimap: Minimap,
    pub yank_flash: YankFlash,
    pub trail_cursor: TrailCursor,
    /// LSP diagnostics popover anchored under the cursor. Data-driven
    /// — stays hidden until the host pushes `PopupItem`s via the
    /// panel's `refresh_items` / `open` calls.
    pub diagnostics_popup: DiagnosticsPopup,
    /// Right-click / completion context menu. Data-driven — stays
    /// hidden until the host opens it.
    pub context_menu: ContextMenu,
    /// Installable handle for the stateless `git_branch` module
    /// (free-function helpers; no per-instance state). Owned here
    /// so the bridge's install ordering matches the other panels.
    pub git_branch: GitBranch,
    /// Installable handle for the stateless `custom_cursor` module
    /// (free-function sprite draw; no per-instance state).
    pub custom_cursor: CustomCursor,

    /// Full-screen Settings overlay (desktop `renderer.settings`
    /// twin). Opened via [`Chrome::open_settings_page`]; owns all
    /// input while active and paints last through the late-overlay
    /// pass. See `chrome/pages.rs`.
    pub settings_page: crate::panels::settings_page::NeoismSettingsPane,
    /// Extensions catalog page — body of the
    /// `ChromePageKind::Extensions` buffer tab (read-only on web).
    pub extensions_page: crate::panels::extensions_page::NeoismExtensionsPane,
    /// NeoWorld pet page — body of the `ChromePageKind::NeoWorld`
    /// buffer tab. `None` until the host installs a pane (a preview
    /// pet is auto-installed on first paint as a fallback).
    pub neoworld_pane: Option<crate::panels::neoworld::NeoWorldPane>,
    /// Chrome-owned universal modal (About dialog today). Late-overlay
    /// painted, input-owning while active.
    pub modal: crate::widgets::modal::UniversalModal,
    /// A non-dismissible connection-loss gate hosted by `modal`.
    /// Kept separately so notifications can be repainted above its late pass.
    connection_gate_active: bool,
    /// Workspace-scoped, reusable file chooser. Unlike the generic modal it
    /// dims but does not suppress the canvas beneath it.
    pub file_browser: crate::panels::file_browser::FileBrowserModal,
    /// Settings actions queued for the host to persist/route —
    /// drained via [`Chrome::drain_settings_actions`].
    pending_settings_actions: Vec<crate::panels::settings_page::SettingsAction>,
    /// Extensions page intents for the host (OpenRepository) —
    /// drained via [`Chrome::drain_extensions_actions`].
    pending_extensions_actions: Vec<crate::panels::extensions_page::PaneAction>,
    /// NeoWorld pet snapshots awaiting persistence — drained via
    /// [`Chrome::drain_neoworld_snapshots`].
    pending_neoworld_snapshots: Vec<neoism_neoworld_core::PetState>,

    /// Top-of-stack panel receives keyboard events first among the
    /// non-modal panels. Hosts push when a panel gains focus and pop
    /// when it loses focus.
    focus_stack: Vec<PanelKey>,

    /// Rubber-banded spring for the file-viewer pane's smooth scroll.
    /// Spring's `position` is the *remaining* offset toward the
    /// target — each draw frame ticks it toward zero and lerps the
    /// effective scroll. Lets neovide-style pixel scroll feel like
    /// rubber instead of snap-to-line.
    scroll_spring: CriticallyDampedSpring,
    /// Current scroll offset (in logical pixels) applied to the
    /// file-viewer paint when `active_tab_index != 0`. Wheel events
    /// add directly into this and bump the spring; the spring's
    /// per-frame tick interpolates back to a settled value.
    scroll_offset_px: f32,
    /// Last pointer position the chrome observed, in window
    /// coordinates. Used to decide whether a `Wheel` event landed
    /// inside the file-viewer rect (the `Wheel` variant itself
    /// doesn't carry a cursor position).
    last_pointer_pos: (f32, f32),

    /// Buffer-tab close intents drained out of `buffer_tabs` and
    /// queued for the host bridge. `set_buffer_tabs` clears these
    /// on every replay, so the host should drain right after
    /// `handle_event` to avoid stale entries.
    pending_buffer_tab_closes: Vec<usize>,
    /// Most-recent buffer-tab activate intent drained out of
    /// `buffer_tabs`. The host pulls this every frame and updates
    /// JS-side bookkeeping; chrome already calls
    /// `set_active_tab_index` itself before queueing.
    pending_buffer_tab_activate: Option<usize>,
    /// "+"-button new-tab intent drained out of `buffer_tabs`. The
    /// host pulls this every frame and spawns its native new-terminal
    /// tab (desktop `TabCreateNew` parity).
    pending_buffer_tab_new: bool,

    /// Top-bar action that needs host handling (Settings / Themes /
    /// Extensions — the screens themselves don't exist yet). Drained
    /// by the host bridge each frame via
    /// [`Chrome::drain_top_bar_action`]. `TogglePanel` is consumed
    /// inside chrome and never lands here.
    pending_top_bar_action: Option<TopBarAction>,

    /// Paths the user activated in the git side panel / notes sidebar
    /// (Enter or click on a row). Drained by the host and turned into
    /// open-buffer intents, same as the file-tree open queue.
    pending_panel_open_paths: Vec<String>,
    /// Set when the git side panel just opened (or asked to refresh)
    /// and the wasm host should fetch status + diffs from the daemon.
    pending_git_panel_refresh: bool,
    /// Set when the notes sidebar just opened and the wasm host
    /// should list the notes tree through the daemon.
    pending_notes_refresh: bool,
    /// Viewport from the last `set_layout` call so panel toggles that
    /// change column widths can relayout immediately.
    last_viewport: Option<Rect>,
    /// Workspace root the host dialed into — repo root for the git
    /// side panel and base for the `notes/` directory.
    workspace_root_path: Option<std::path::PathBuf>,

    /// Vault directory the notes sidebar lists, as resolved by the HOST
    /// (`linked_project_for_code_dir` -> `notes_workspace_dir()`, e.g.
    /// `~/Neoism/Vaults/Personal/Projects/MyProject`). Notes live in
    /// vaults, never in a workspace-local `notes/` folder, so this is
    /// tracked separately from `workspace_root_path` and cannot be
    /// derived from it. `None` means the host linked no vault, which
    /// drives the sidebar's "no linked vault" empty state.
    notes_vault_root: Option<std::path::PathBuf>,

    /// "Share with phone" QR sheet. The host resolves the reachable URL
    /// (only the daemon knows its routable address) and calls
    /// `ShareSheet::show`; this panel only encodes and paints it.
    pub share_sheet: crate::panels::share_sheet::ShareSheet,

    /// Previous-frame [`Chrome::draw`] timestamp. `None` on first frame
    /// so the trail cursor can teleport to the initial destination
    /// instead of animating from a stale origin. Used to compute the
    /// per-frame `dt` fed into [`TrailCursor::animate`] so the
    /// neovide-style beam spring advances regardless of the host's
    /// frame cadence.
    last_draw_time: Option<Duration>,
}

impl<A: Send + Copy + 'static> Default for Chrome<A> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A: Send + Copy + 'static> Chrome<A> {
    /// Split borrow of the hosted code pane and its LSP session layer —
    /// the wasm bridge routes daemon LSP replies and key hooks through
    /// both at once (twin of [`Chrome::code_editor_parts_mut`]).
    pub fn code_lsp_parts_mut(
        &mut self,
    ) -> (
        Option<&mut crate::editor::code::CodePane>,
        &mut crate::editor::code::lsp_session::CodeLspUi,
    ) {
        (self.code_pane.as_mut(), &mut self.code_lsp)
    }
}
