//! Panel trait surface and module list.
//!
//! Each chrome surface (file tree, status line, buffer tabs, palette,
//! finder, composer, breadcrumbs, notifications, cursor surfaces, ...)
//! lives in this module. The host's `Chrome` owns a
//! `Vec<Box<dyn Panel>>` and routes `UiEvent`s through them in z-order
//! then draws them in paint order via `sugarloaf`.

use sugarloaf::Sugarloaf;

use crate::event::UiEvent;
use crate::layout::PanelLayout;
use crate::services::Services;
use crate::theme::ChromeTheme;

pub mod agent_pane;
pub mod assistant_overlay;
pub mod buffer_tabs;
pub mod chrome_topbar;
pub mod command_composer;
pub mod command_palette;
pub mod cross_window_drag;
pub mod file_tree;
pub mod finder;
pub mod git_diff;
pub mod pairings_settings;
pub mod status_line;

pub mod breadcrumbs;
pub mod completion_menu;
pub mod custom_cursor;
pub mod editor_scroll;
pub mod git_branch;
pub mod minimap;
pub mod notes_sidebar;
pub mod notifications;
pub mod pane_grid;
pub mod remote_carets;
pub mod search;
pub mod splash_overlay;
pub mod terminal_splash;
pub mod trail_cursor;
pub mod yank_flash;

pub mod context_menu;
pub mod diagnostic_detail;
pub mod diagnostics_popup;
pub mod extensions_page;
pub mod hover_popup;
pub mod inline_diagnostics;
pub mod lsp_popup;
pub mod tags_view;

mod chrome_shim;
pub(crate) mod chrome_shim_more;

pub use buffer_tabs::{AgentIconProvider, AgentLabel, BufferTabs};
pub use chrome_topbar::{
    ChromeTopBar, ServerIndicatorStatus, TopBarAction, CHROME_TOPBAR_HEIGHT,
};
pub use command_composer::CommandComposer;
pub use command_palette::CommandPalette;
pub use extensions_page::{
    ExtensionEntry, ExtensionFilter, ExtensionStatus, ExtensionTab, NeoismExtensionsPane,
};
pub use file_tree::{FileTree, TreeNode};
pub use finder::{Finder, FinderMode};
pub use git_diff::{DiffFile, DiffHunk, DiffLine, GitDiff};
pub use notes_sidebar::NotesSidebar;
pub use pairings_settings::{PairingRow, PairingsSettings, PairingsSettingsAction};
pub use status_line::{
    DiagnosticCounts, DiagnosticPill, GitChangeSummary, LspStatus, Mode, PillRect,
    PrimaryKind, StatusInfo, StatusLine, StatusPalette, STATUS_LINE_HEIGHT,
};
pub use tags_view::{NeoismTagsPane, TagsViewAction};

/// Per-frame context passed to every `Panel` callback.
pub struct PanelContext<'a> {
    pub services: Services<'a>,
    pub theme: &'a ChromeTheme,
    pub time: web_time::Duration,
}

pub trait Panel: Send {
    fn handle_event(&mut self, event: &UiEvent, ctx: &mut PanelContext);
    fn draw(&self, sugarloaf: &mut Sugarloaf, layout: &PanelLayout, ctx: &PanelContext);
    fn wants_focus(&self) -> bool {
        false
    }
    fn name(&self) -> &str;
}
