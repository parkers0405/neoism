pub mod composer;
pub mod finder_search;
pub mod fps;
pub mod hint;
pub mod path_exec;
pub mod run;
pub mod state;
pub mod tabs;

use neoism_ui::panels::{
    breadcrumbs, buffer_tabs, chrome_topbar, command_composer, command_palette,
    completion_menu, context_menu, diagnostics_popup, editor_scroll, finder,
    inline_diagnostics, minimap, notes_sidebar, notifications, search, status_line,
    trail_cursor, yank_flash,
};
use neoism_ui::primitives::ide_theme::IdeTheme;
use neoism_ui::widgets::{island, modal, scrollbar};

use crate::editor::{file_tree, git_diff_panel};
use crate::neoism::{assistant_overlay as assistant, icon as agent_icon, splash_overlay};
use crate::terminal::scroll as terminal_scroll;

use crate::context::renderable::TerminalSnapshot;
use neoism_terminal_core::crosswords::LineDamage;
use neoism_terminal_core::damage::TerminalDamage;
use taffy::NodeId;

use crate::context::renderable::{PendingUpdate, RenderableContent};
use crate::context::ContextManager;
use neoism_backend::config::colors::{term::List, ColorArray, Colors};
use neoism_backend::config::navigation::Navigation;
#[cfg(target_os = "macos")]
use neoism_backend::config::window::Decorations;
use neoism_backend::config::Config;
use neoism_backend::event::EventProxy;
use neoism_backend::sugarloaf::Sugarloaf;
use neoism_terminal_core::colors::term::TermColors;
use neoism_terminal_core::colors::term::DIM_FACTOR;
use neoism_terminal_core::colors::{AnsiColor, NamedColor};
use neoism_terminal_core::crosswords::pos::Pos;
use neoism_terminal_core::crosswords::style::{Style as CellStyle, StyleFlags};
use std::collections::BTreeSet;
use std::ops::RangeInclusive;

/// Identifies a tab strip — used by the drag pipeline to know which
/// strip a drag came from and which strip a release lands in. `Pane`
/// holds the editor's `route_id`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StripRef {
    Workspace,
    Pane(usize),
}

#[derive(Clone, Copy, Debug)]
pub struct TabDropPreview {
    pub target: StripRef,
    pub mouse_x: f32,
}

pub struct Renderer {
    pub(super) is_vi_mode_enabled: bool,
    pub(super) is_game_mode_enabled: bool,
    pub(super) draw_bold_text_with_light_colors: bool,
    #[allow(dead_code)] // grid path doesn't consult this yet
    pub(super) use_drawable_chars: bool,
    pub named_colors: Colors,
    pub colors: List,
    pub navigation: Navigation,
    pub margin: neoism_backend::config::layout::Margin,
    pub island: Option<island::Island>,
    /// Window-top chrome strip: panel toggle + hamburger menu.
    /// Painted in `run.rs` right under the island. Shared with the
    /// web/wasm chrome — see `neoism_ui::panels::chrome_topbar`.
    pub top_bar: chrome_topbar::ChromeTopBar,
    #[cfg(target_os = "macos")]
    pub macos_traffic_light_inset: f32,
    /// Drained each frame by the screen layer — `OpenSettings` /
    /// `OpenThemes` / `OpenExtensions` don't have destination screens
    /// yet, so the host stashes whichever one fired here and the
    /// screen-side click router decides what to do with it.
    #[allow(dead_code)]
    pub pending_top_bar_action: Option<chrome_topbar::TopBarAction>,
    pub command_palette: command_palette::CommandPalette,
    pub finder: finder::Finder,
    pub finder_search: finder_search::NativeSearchService,
    pub context_menu: context_menu::ContextMenu,
    pub completion_menu: completion_menu::CompletionMenu,
    /// Native code pane LSP popup state (completion session + hover
    /// card), fed by `screen/bridges/code/lsp.rs` and drawn by the
    /// completion-menu / hover-popup sites in `run.rs`.
    pub code_lsp: crate::screen::bridges::code::lsp::CodeLspUiState,
    pub(super) unfocused_split_opacity: f32,
    pub(super) unfocused_split_fill: Option<ColorArray>,
    pub(super) last_active: Option<NodeId>,
    /// Last `neoism_backend::sugarloaf::Color` we applied to sugarloaf's window clear via
    /// `set_background_color`. Lets the per-frame "derive bg from
    /// active panel's OSC state" loop avoid redundant resyncs.
    pub(super) last_window_bg: Option<neoism_backend::sugarloaf::Color>,
    pub config_has_blinking_enabled: bool,
    pub config_blinking_interval: u64,
    /// User-picked cursor color (`[neoism] cursor-color`) — re-applied
    /// over the theme accent on every `set_ide_theme` so it survives
    /// theme switches.
    pub cursor_color_override: Option<[f32; 4]>,
    /// Cursor preset (`[neoism] cursor-style`). Rainbow animates
    /// through hues per frame and ignores the static color.
    pub cursor_style: neoism_ui::cursor_style::CursorStyle,
    /// True while any REMOTE peer broadcasts the rainbow preset — set
    /// at paint time so `needs_redraw` keeps frames coming for their
    /// animation too.
    pub remote_rainbow_active: bool,
    pub(crate) ignore_selection_fg_color: bool,
    pub search: search::SearchOverlay,
    pub assistant: assistant::AssistantOverlay,
    pub scrollbar: scrollbar::Scrollbar,
    #[allow(unused)]
    pub option_as_alt: String,
    #[allow(unused)]
    pub macos_use_unified_titlebar: bool,
    // Dynamic background keep track of the original bg color and
    // the same r,g,b with the mutated alpha channel.
    pub dynamic_background: ([f32; 4], neoism_backend::sugarloaf::Color, bool),
    pub custom_mouse_cursor: bool,
    pub trail_cursor_enabled: bool,
    pub trail_cursor: trail_cursor::TrailCursor,
    /// `[neoism] status-fps` — gates the frame-rate pill on the status
    /// line's right cluster. The counter ticks regardless so flipping
    /// the flag on shows a reading within half a second.
    pub status_fps_enabled: bool,
    /// `[neoism] format-on-save` — the code pane formats via LSP
    /// before each save (default true).
    pub code_format_on_save: bool,
    pub fps_counter: fps::FpsCounter,
    pub terminal_block_prompt_animating: bool,
    pub notebook_animating: bool,
    /// True while an animated shader overlay is applied. The overlay
    /// samples the wall clock per drawn frame, so it needs the loop to
    /// keep producing frames even when nothing else on the pane moves
    /// (an idle terminal prompt has no other animation owner).
    pub shader_overlay_active: bool,
    /// Per-pane spring-based smooth scroll for nvim editor surfaces.
    /// Mouse wheel pixel deltas accumulate into a critically-damped
    /// spring; the sub-row residual decays to zero each frame and is
    /// applied as a `set_position` offset on the editor's rich_text,
    /// producing the neovide-style smooth slide. Empty (no per-pane
    /// state) means no animation overhead.
    pub editor_scroll: editor_scroll::EditorScroll,
    /// Pixel-perfect (no-spring) scroll for terminal panes. Tracks a
    /// per-pane sub-row offset that follows wheel input 1:1, with no
    /// decay or animation tail. Whole rows commit to terminal
    /// scrollback as `Scroll::Delta`; the residual stays as a static
    /// `set_position` offset until the next wheel event.
    pub terminal_scroll: terminal_scroll::TerminalScroll,
    /// GPU overlay layered on top of the cell-based splash banner
    /// — ambient pulse + click ripple. Holds animation state
    /// across frames so the ripple decays smoothly between mouse
    /// input and the next render tick.
    pub splash_overlay: splash_overlay::SplashOverlay,
    pub file_tree: file_tree::FileTree,
    pub notes_sidebar: notes_sidebar::NotesSidebar,
    /// Logical mouse position for the notes sidebar's wordmark hover —
    /// pushed by the screen each frame (the renderer owns no input).
    pub notes_sidebar_mouse: Option<(f32, f32)>,
    /// Rect of the agent pane's open inline picker card (/model,
    /// /agents, /sessions, …), refreshed each frame by
    /// `render_neoism_agent_panels`. Folded into
    /// `active_text_occlusion_rects` so chrome text (tab-strip labels,
    /// panels) never bleeds through the modal.
    pub agent_picker_occlusion: Option<[f32; 4]>,
    pub buffer_tabs: buffer_tabs::BufferTabs<crate::neoism::icon::AgentKind>,
    /// Per-pane tab strips, keyed by editor `route_id`. Populated
    /// when a tab is torn out into a new editor split — the new pane
    /// gets its own strip with just that file. The workspace's
    /// primary editor pane keeps using `buffer_tabs` above; entries
    /// here are only secondary (split) editor panes.
    pub pane_tabs: rustc_hash::FxHashMap<
        usize,
        buffer_tabs::BufferTabs<crate::neoism::icon::AgentKind>,
    >,
    /// Per-pane breadcrumbs strips — one per non-primary editor pane,
    /// keyed by the pane's `route_id`. Sits directly below the pane's
    /// `pane_tabs` strip and shows the path of its active tab.
    pub pane_breadcrumbs: rustc_hash::FxHashMap<usize, breadcrumbs::Breadcrumbs>,
    /// Which strip the active drag belongs to. Set in
    /// `handle_buffer_tabs_click` when a drag is armed; consumed in
    /// `handle_buffer_tabs_drag_move` / `..._release` so updates go
    /// to the right `BufferTabs` instance and the release handler
    /// knows where the tab came from.
    pub drag_source: Option<StripRef>,
    /// Cross-strip drag target preview. The source `BufferTabs` can
    /// render its floating tab, but the destination strip needs a
    /// separate hover highlight so moving a tab into/out of a split is
    /// visually obvious before release.
    pub drag_drop_preview: Option<TabDropPreview>,
    pub breadcrumbs: breadcrumbs::Breadcrumbs,
    pub status_line: status_line::StatusLine,
    /// Warp-style sticky command composer for the active terminal pane.
    /// Single instance — only the focused terminal pane gets a chassis;
    /// inactive panes fall back to the cell-grid prompt the shell paints.
    pub command_composer: command_composer::CommandComposer,
    /// Lazy cache of executables found on `PATH` — built once on first
    /// composer render so command-validity coloring (zsh-syntax-style
    /// red for unknown commands) doesn't `read_dir` PATH every frame.
    /// Refresh-on-demand is fine; the cache rebuilds when the user
    /// runs `hash -r` style refreshes is left for future work.
    pub(super) path_executables: Option<rustc_hash::FxHashSet<String>>,
    /// Rust-side same-row diagnostics surface for the active editor.
    /// This is the visible inline diagnostic UI; nvim virtual text and
    /// virtual lines stay disabled.
    pub inline_diagnostics: inline_diagnostics::InlineDiagnostics,
    pub diagnostics_popup: diagnostics_popup::DiagnosticsPopup,
    /// Sugarloaf-clipped overlay anchored to the status-line LSP pill.
    /// Opens on click; closes on outside click. Renders over
    /// editor content (not as a modal) and shows per-server state plus
    /// diagnostics/details for the active buffer.
    pub lsp_popup: neoism_ui::panels::lsp_popup::LspPopup,
    pub minimap: minimap::Minimap,
    pub yank_flash: yank_flash::YankFlash,
    pub modal: modal::UniversalModal,
    pub git_diff_panel: git_diff_panel::GitDiffPanel,
    pub notifications: notifications::Notifications,
    /// True while the active Neoism Agent pane wants continuous frames
    /// (streaming, picker/timeline springs, status row animation). The
    /// screen render path sets this every frame after laying out the
    /// agent panels; `needs_redraw` reads it so the event-loop redraws
    /// continuously instead of waiting for the next input event.
    pub neoism_agent_animating: bool,
    pub theme: IdeTheme,
    /// Multiplier applied to chrome row heights / fonts (file tree,
    /// buffer tabs, breadcrumbs) so Ctrl+/Ctrl- zooms the IDE shell
    /// alongside the editor body. Tracked as `current_font_size /
    /// CHROME_BASELINE_FONT_SIZE` so the chrome stays proportional
    /// to whatever font.size the user has configured AND to the live
    /// zoom level — both compose into the same scalar.
    pub(super) chrome_scale: f32,
    /// Canonical live font size for every terminal/editor surface in this
    /// window. It deliberately lives outside any individual context so a new
    /// tab or workspace cannot reset the next zoom step to its own default.
    pub(super) zoom_font_size: f32,
    /// True once the embedded agent-icon PNGs have been uploaded to
    /// sugarloaf's image store. Gated behind a flag so we register on
    /// the first `run` call (when sugarloaf is in scope) and never
    /// again. False until that first frame.
    pub(super) agent_icons_registered: bool,
    /// Cached "agent currently running in the terminal tab" — refreshed
    /// at most every `AGENT_DETECT_INTERVAL` to avoid querying native
    /// process metadata each frame. `None` means the foreground program is
    /// a plain shell (or not detectable), so the generic terminal glyph
    /// renders.
    pub(super) last_agent: Option<agent_icon::AgentKind>,
    pub(super) last_agent_check: Option<std::time::Instant>,
    /// Native process metadata inspection stays off the render thread. The
    /// worker is created lazily on the first visible terminal-tab probe and
    /// reused for the lifetime of this renderer.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub(super) agent_detection_worker: Option<agent_icon::AgentDetectionWorker>,
    /// In-flight extension install jobs keyed by manifest id. Lives on
    /// the renderer so the per-frame pump in
    /// `bridges/extensions.rs::pump_install_progress` can drain progress
    /// channels and update pane status without sweeping every grid for
    /// transient state.
    pub install_tracker: crate::screen::bridges::extensions::InstallTracker,
    /// Cache of bundled + catalog `ExtensionManifest`s keyed by id,
    /// populated by `bridges/extensions.rs::load_bundled_extension_entries`.
    /// Lets the install/uninstall dispatcher re-resolve a full manifest
    /// from the panel's `ExtensionEntry` id without re-parsing the
    /// registry.
    pub bundled_manifests:
        std::collections::BTreeMap<String, neoism_extensions::ExtensionManifest>,
    /// One-shot guard preventing repeated package-catalog snapshot
    /// fetches. Set the first time the Extensions panel opens; the
    /// language-server rows' install plans resolve once the snapshot
    /// is cached.
    pub catalog_seeded: bool,
}

pub(super) const AGENT_DETECT_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(500);

/// Reference font size for which the chrome submodules' hardcoded
/// FONT_SIZE / row-height constants were tuned. `chrome_scale = 1.0`
/// means "render at these baseline pt values"; other scales multiply
/// every chrome strip uniformly. Sized so a user with the default 14pt
/// terminal font lands at scale 1.0 (chrome at its tuned dimensions).
pub const CHROME_BASELINE_FONT_SIZE: f32 = 14.0;

impl Renderer {
    pub fn new(config: &Config) -> Renderer {
        let mut renderer = Self::new_inner(config);
        // Push the config-derived chrome scale into every submodule
        // (file_tree, buffer_tabs, breadcrumbs, status_line, etc.) so
        // their internal `scale` matches `chrome_scale` from the very
        // first frame. Without this they'd start at the constructor
        // default (1.0) and only catch up after the first
        // `change_font_size` call.
        let initial_scale = renderer.chrome_scale;
        renderer.set_chrome_scale(initial_scale);
        renderer.set_ide_theme(IdeTheme::by_name(&config.neoism.theme));
        renderer.minimap.set_enabled(config.neoism.minimap);
        renderer
    }

    fn new_inner(config: &Config) -> Renderer {
        let colors = List::from(&config.colors);
        let named_colors = config.colors;

        let mut dynamic_background =
            (named_colors.background.0, named_colors.background.1, false);
        if config.window.opacity < 1. {
            dynamic_background.1.a = config.window.opacity as f64;
            dynamic_background.2 = true;
        } else if config.window.background_image.is_some() {
            dynamic_background.1 = neoism_backend::sugarloaf::Color::TRANSPARENT;
            dynamic_background.2 = true;
        }

        let island = if config.navigation.is_enabled() {
            Some(island::Island::new(
                named_colors.tabs,
                named_colors.tabs_active,
                named_colors.tab_border,
                true,
            ))
        } else {
            None
        };
        let top_bar = chrome_topbar::ChromeTopBar::new();
        #[cfg(target_os = "macos")]
        let macos_traffic_light_inset =
            if config.window.decorations != Decorations::Buttonless {
                let traffic_light_x = config
                    .window
                    .macos_traffic_light_position_x
                    .unwrap_or(crate::constants::TRAFFIC_LIGHT_PADDING)
                    as f32;
                traffic_light_x + 68.0
            } else {
                0.0
            };

        Renderer {
            unfocused_split_opacity: config.navigation.unfocused_split_opacity,
            unfocused_split_fill: config.navigation.unfocused_split_fill,
            last_active: None,
            last_window_bg: None,
            use_drawable_chars: config.fonts.use_drawable_chars,
            draw_bold_text_with_light_colors: config.draw_bold_text_with_light_colors,
            macos_use_unified_titlebar: config.window.macos_use_unified_titlebar,
            config_blinking_interval: config.cursor.blinking_interval.clamp(350, 1200),
            option_as_alt: config.option_as_alt.to_lowercase(),
            is_vi_mode_enabled: false,
            config_has_blinking_enabled: config.cursor.blinking,
            cursor_color_override: config
                .neoism
                .cursor_color
                .as_deref()
                .and_then(neoism_ui::cursor_style::parse_hex_color)
                .map(neoism_ui::cursor_style::hex_to_f32),
            cursor_style: neoism_ui::cursor_style::CursorStyle::from_str(
                config.neoism.cursor_style.as_deref().unwrap_or_default(),
            ),
            remote_rainbow_active: false,
            ignore_selection_fg_color: config.ignore_selection_fg_color,
            colors,
            navigation: config.navigation.clone(),
            margin: config.margin,
            island,
            top_bar,
            #[cfg(target_os = "macos")]
            macos_traffic_light_inset,
            pending_top_bar_action: None,
            command_palette: {
                let mut palette = command_palette::CommandPalette::new();
                palette.has_adaptive_theme = config.adaptive_colors.is_some();
                palette
            },
            finder: finder::Finder::new(),
            finder_search: finder_search::NativeSearchService::new(),
            context_menu: context_menu::ContextMenu::new(),
            completion_menu: completion_menu::CompletionMenu::new(),
            code_lsp: Default::default(),
            named_colors,
            dynamic_background,
            search: search::SearchOverlay::default(),
            assistant: assistant::AssistantOverlay::default(),
            scrollbar: scrollbar::Scrollbar::new(config.enable_scroll_bar),
            is_game_mode_enabled: config.renderer.strategy.is_game(),
            custom_mouse_cursor: config.effects.custom_mouse_cursor,
            trail_cursor_enabled: config.effects.trail_cursor,
            trail_cursor: trail_cursor::TrailCursor::new(),
            status_fps_enabled: config.neoism.status_fps,
            code_format_on_save: config.neoism.format_on_save,
            fps_counter: fps::FpsCounter::default(),
            terminal_block_prompt_animating: false,
            notebook_animating: false,
            shader_overlay_active: false,
            editor_scroll: editor_scroll::EditorScroll::new(),
            terminal_scroll: terminal_scroll::TerminalScroll::new(),
            splash_overlay: splash_overlay::SplashOverlay::new(),
            file_tree: file_tree::FileTree::new(),
            notes_sidebar: notes_sidebar::NotesSidebar::default(),
            notes_sidebar_mouse: None,
            agent_picker_occlusion: None,
            buffer_tabs: buffer_tabs::BufferTabs::new(),
            pane_tabs: rustc_hash::FxHashMap::default(),
            pane_breadcrumbs: rustc_hash::FxHashMap::default(),
            drag_source: None,
            drag_drop_preview: None,
            breadcrumbs: breadcrumbs::Breadcrumbs::new(),
            status_line: status_line::StatusLine::new(),
            command_composer: command_composer::CommandComposer::new(),
            path_executables: None,
            inline_diagnostics: inline_diagnostics::InlineDiagnostics::new(),
            diagnostics_popup: diagnostics_popup::DiagnosticsPopup::new(),
            lsp_popup: neoism_ui::panels::lsp_popup::LspPopup::new(),
            minimap: minimap::Minimap::new(),
            yank_flash: yank_flash::YankFlash::new(),
            modal: modal::UniversalModal::new(),
            git_diff_panel: {
                let mut p = git_diff_panel::GitDiffPanel::new();
                git_diff_panel::install_io(&mut p);
                p
            },
            notifications: notifications::Notifications::new(),
            neoism_agent_animating: false,
            theme: IdeTheme::by_name(&config.neoism.theme),
            // Chrome scale tracks the user's configured font size. The
            // hardcoded FONT_SIZE constants in each chrome submodule
            // (file_tree, buffer_tabs, breadcrumbs, status_line) are
            // tuned for `CHROME_BASELINE_FONT_SIZE = 14pt`. Multiplying
            // by `config.fonts.size / 14` makes chrome scale up/down
            // proportionally with the user's terminal/nvim font size,
            // so a user who chose 18pt sees chrome at ~14% larger
            // strips and vice versa. Without this seed, chrome would
            // stay at the 14pt baseline regardless of user config and
            // look tiny next to a large editor font.
            chrome_scale: (config.fonts.size / CHROME_BASELINE_FONT_SIZE).clamp(0.5, 3.0),
            zoom_font_size: config.fonts.size.clamp(6.0, 100.0),
            agent_icons_registered: false,
            last_agent: None,
            last_agent_check: None,
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            agent_detection_worker: None,
            install_tracker: crate::screen::bridges::extensions::InstallTracker::default(
            ),
            bundled_manifests: std::collections::BTreeMap::new(),
            catalog_seeded: false,
        }
    }

    #[inline]
    pub(crate) fn compute_color(
        &self,
        color: &AnsiColor,
        flags: StyleFlags,
        term_colors: &TermColors,
    ) -> ColorArray {
        let dim = flags.contains(StyleFlags::DIM);
        let bold = flags.contains(StyleFlags::BOLD);
        match color {
            AnsiColor::Named(ansi) => {
                match (self.draw_bold_text_with_light_colors, dim, bold) {
                    // If no bright foreground is set, treat it like the BOLD flag doesn't exist.
                    (_, true, true)
                        if ansi == &NamedColor::Foreground
                            && self.named_colors.light_foreground.is_none() =>
                    {
                        self.color(NamedColor::DimForeground as usize, term_colors)
                    }
                    // Draw bold text in bright colors *and* contains bold flag.
                    (true, false, true) => {
                        self.color(ansi.to_light() as usize, term_colors)
                    }
                    // Cell is marked as dim and not bold.
                    (_, true, false) | (false, true, true) => {
                        self.color(ansi.to_dim() as usize, term_colors)
                    }
                    // None of the above, keep original color..
                    _ => self.color(*ansi as usize, term_colors),
                }
            }
            AnsiColor::Spec(rgb) => {
                if !dim {
                    rgb.to_arr()
                } else {
                    rgb.to_arr_with_dim()
                }
            }
            AnsiColor::Indexed(index) => {
                let index = match (dim, index) {
                    (true, 8..=15) => *index as usize - 8,
                    (true, 0..=7) => NamedColor::DimBlack as usize + *index as usize,
                    _ => *index as usize,
                };

                self.color(index, term_colors)
            }
        }
    }

    #[inline]
    pub(crate) fn compute_bg_color(
        &self,
        cell_style: &CellStyle,
        term_colors: &TermColors,
    ) -> ColorArray {
        let dim = cell_style.flags.contains(StyleFlags::DIM);
        let bold = cell_style.flags.contains(StyleFlags::BOLD);
        match cell_style.bg {
            AnsiColor::Named(ansi) => self.color(ansi as usize, term_colors),
            AnsiColor::Spec(rgb) => {
                if dim {
                    (&(rgb * DIM_FACTOR)).into()
                } else {
                    (&rgb).into()
                }
            }
            AnsiColor::Indexed(idx) => {
                let idx = match (self.draw_bold_text_with_light_colors, dim, bold, idx) {
                    (true, false, true, 0..=7) => idx as usize + 8,
                    (false, true, false, 8..=15) => idx as usize - 8,
                    (false, true, false, 0..=7) => {
                        NamedColor::DimBlack as usize + idx as usize
                    }
                    _ => idx as usize,
                };

                self.color(idx, term_colors)
            }
        }
    }

    /// Scan visible rows for kitty Unicode-placeholder cells (U+10EEEE) and
    /// push one `GraphicOverlay` per row-run. Ports the four key behaviors
    /// from ghostty's `graphics_unicode.zig`:
    ///
    /// 1. Per-row `kitty_virtual_placeholder` flag check skips rows
    ///    with no placeholders.
    /// 2. Continuation rules — a cell with missing diacritics inherits
    ///    from the previous cell on the row (`canAppend`,
    ///    `graphics_unicode.zig:506-513`).
    /// 3. Run aggregation — consecutive cells with same image / row /
    ///    sequential column collapse into one Placement
    ///    (`PlacementIterator.next`, `graphics_unicode.zig:36-99`).
    /// 4. Per-run source rect with aspect-fit + centering — handles
    ///    partial visibility (placement scrolled half off-screen) and
    ///    cells that fall in the centering padding
    ///    (`renderPlacement`, `graphics_unicode.zig:212-329`).
    pub(crate) fn push_virtual_placeholder_overlays(
        overlays: &mut Vec<neoism_backend::sugarloaf::GraphicOverlay>,
        snapshot: &TerminalSnapshot,
        origin_x: f32,
        origin_y: f32,
        cell_width: f32,
        cell_height: f32,
    ) {
        use neoism_terminal_core::ansi::kitty_virtual::{
            IncompletePlacement, PlaceholderRun, PLACEHOLDER,
        };

        // Below text — matches ghostty's default for virtual placements.
        const VIRTUAL_Z_INDEX: i32 = -1;

        for (line_idx, row) in snapshot.visible_rows.iter().enumerate() {
            // Per-row dirty flag: skip rows that never had a placeholder
            // written. O(visible_w · visible_h) → O(rows_with_placeholders).
            if !row.kitty_virtual_placeholder {
                continue;
            }

            // Walk the row left-to-right, building a single in-flight run.
            // When the next cell can't extend it (different image, col
            // discontinuity, etc.) we flush the run as one overlay and
            // start a new one. Mirrors `PlacementIterator.next`.
            let mut run: Option<(IncompletePlacement, usize)> = None;

            for (col_idx, square) in row.inner.iter().enumerate() {
                if square.c() != PLACEHOLDER {
                    if let Some((p, start_col)) = run.take() {
                        flush_run(
                            overlays,
                            snapshot,
                            p.complete(),
                            line_idx,
                            start_col,
                            origin_x,
                            origin_y,
                            cell_width,
                            cell_height,
                            VIRTUAL_Z_INDEX,
                        );
                    }
                    continue;
                }

                let style = snapshot.style_set.get(square.style_id());
                let combining: &[char] = square
                    .extras_id()
                    .and_then(|eid| snapshot.extras_table.get(eid))
                    .map(|e| e.zerowidth.as_slice())
                    .unwrap_or(&[]);

                let mut cell = IncompletePlacement::from_cell(
                    style.fg,
                    style.underline_color,
                    combining,
                );

                match &mut run {
                    Some((current, _)) if current.can_append(&cell) => {
                        current.append();
                    }
                    _ => {
                        if let Some((p, start_col)) = run.take() {
                            flush_run(
                                overlays,
                                snapshot,
                                p.complete(),
                                line_idx,
                                start_col,
                                origin_x,
                                origin_y,
                                cell_width,
                                cell_height,
                                VIRTUAL_Z_INDEX,
                            );
                        }
                        // Default missing row/col on the FIRST cell of a
                        // run — matches ghostty's
                        // `graphics_unicode.zig:84-86`. Without this,
                        // a subsequent cell with `Some(col)` couldn't
                        // sequentially extend a run started by a cell
                        // with `None`.
                        if cell.row.is_none() {
                            cell.row = Some(0);
                        }
                        if cell.col.is_none() {
                            cell.col = Some(0);
                        }
                        run = Some((cell, col_idx));
                    }
                }
            }

            if let Some((p, start_col)) = run {
                flush_run(
                    overlays,
                    snapshot,
                    p.complete(),
                    line_idx,
                    start_col,
                    origin_x,
                    origin_y,
                    cell_width,
                    cell_height,
                    VIRTUAL_Z_INDEX,
                );
            }
        }

        /// Look up metadata + image for a completed `PlaceholderRun`,
        /// compute its on-screen geometry via
        /// `kitty_virtual::compute_run_geometry`, and push one
        /// `GraphicOverlay`. Returns silently when the placement isn't
        /// registered, the image isn't transmitted yet, or the run lies
        /// entirely in the aspect-fit centering padding.
        #[allow(clippy::too_many_arguments)]
        fn flush_run(
            overlays: &mut Vec<neoism_backend::sugarloaf::GraphicOverlay>,
            snapshot: &TerminalSnapshot,
            run: PlaceholderRun,
            screen_line: usize,
            start_screen_col: usize,
            origin_x: f32,
            origin_y: f32,
            cell_width: f32,
            cell_height: f32,
            z_index: i32,
        ) {
            let vp = snapshot
                .kitty_virtual_placements
                .get(&(run.image_id, run.placement_id))
                .or_else(|| snapshot.kitty_virtual_placements.get(&(run.image_id, 0)));
            let vp = match vp {
                Some(v) => v,
                None => return,
            };
            let img = match snapshot.kitty_images.get(&run.image_id) {
                Some(i) => i,
                None => return,
            };

            let geom =
                match neoism_terminal_core::ansi::kitty_virtual::compute_run_geometry(
                    &run,
                    vp.columns,
                    vp.rows,
                    img.data.width as u32,
                    img.data.height as u32,
                    cell_width,
                    cell_height,
                    origin_x,
                    origin_y,
                    screen_line,
                    start_screen_col,
                ) {
                    Some(g) => g,
                    None => return,
                };

            overlays.push(neoism_backend::sugarloaf::GraphicOverlay {
                image_id: run.image_id,
                x: geom.x,
                y: geom.y,
                width: geom.width,
                height: geom.height,
                z_index,
                source_rect: geom.source_rect,
            });
        }
    }
}
