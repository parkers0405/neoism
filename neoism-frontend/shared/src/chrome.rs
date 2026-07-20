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
mod paint;
pub(crate) use paint::*;

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
