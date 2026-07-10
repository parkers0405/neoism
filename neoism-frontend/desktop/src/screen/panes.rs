// Auto-split from screen/mod.rs. See sibling mod.rs for the Screen struct and
// the constructor/core methods. This file is part of the impl Screen<'_> block.

use super::*;
use neoism_backend::clipboard::Clipboard;
use neoism_protocol::workspace::{PaneSplitAxis, PaneSplitPlacement};
use neoism_ui::session_layout::{
    active_tab_move_to_split_stack_plan, buffer_tabs_scroll_dx,
    hidden_split_drag_reveal_route, ordered_secondary_routes_with_orphans,
    pane_strip_position, PaneStripGeomInput, SessionMovableTabKind, SessionScrollDelta,
    SessionTabMoveDestination, SessionTabStripRef, DIVIDER_KEYBOARD_STEP_HORIZONTAL,
    DIVIDER_KEYBOARD_STEP_VERTICAL,
};
use neoism_window::event::ElementState;
use neoism_window::keyboard::{Key, NamedKey};
use std::path::{Path, PathBuf};

/// Convert the desktop host's `StripRef` to the shared
/// `panels::buffer_tabs::StripKey` used by policy helpers. Both
/// enums are deliberately isomorphic — keeping them as separate
/// types lets the host layer stay independent of the shared crate
/// while letting policy live in `neoism-ui`.
#[inline]
fn strip_ref_to_key(
    source: crate::host::StripRef,
) -> neoism_ui::panels::buffer_tabs::StripKey {
    match source {
        crate::host::StripRef::Workspace => {
            neoism_ui::panels::buffer_tabs::StripKey::Workspace
        }
        crate::host::StripRef::Pane(route) => {
            neoism_ui::panels::buffer_tabs::StripKey::Pane(route)
        }
    }
}

mod buffer_tabs_input;
mod close_focus;
mod cross_window_dnd;
mod grid_workspace;
mod open_create;
mod terminal_exit;

/// Thin adapter so the desktop fork's `ContextGrid` plays the
/// [`neoism_ui::session_layout::ContextGridLike`] trait — letting the
/// `handle_terminal_exit` grid walk run inside the shared crate
/// without leaking sugarloaf/taffy types.
struct ContextGridDescriptorAdapter<'a>(&'a crate::layout::ContextGrid<EventProxy>);

impl<'a> neoism_ui::session_layout::ContextGridLike for ContextGridDescriptorAdapter<'a> {
    fn owns_route(&self, route_id: u64) -> bool {
        self.0.node_by_route_id(route_id as usize).is_some()
    }

    fn describe_closing_route(
        &self,
        grid_index: usize,
        route_id: u64,
    ) -> neoism_ui::session_layout::ClosingContextSlot {
        let route_id_usize = route_id as usize;
        let workspace_id = self.0.workspace_route_id();
        let is_workspace_root = workspace_id == Some(route_id_usize);
        let (shell_pid, is_terminal_context) = self
            .0
            .node_by_route_id(route_id_usize)
            .and_then(|node| self.0.contexts().get(&node))
            .map(|item| {
                let context = item.context();
                // Windows contexts have no shell_pid field; 0 is the same
                // "no process to signal" sentinel editor panes use.
                #[cfg(not(target_os = "windows"))]
                let shell_pid = context.shell_pid;
                #[cfg(target_os = "windows")]
                let shell_pid = 0u32;
                (
                    shell_pid,
                    context.editor.is_none()
                        && context.markdown.is_none()
                        && context.neoism_agent.is_none()
                        && context.neoism_tags.is_none(),
                )
            })
            .unwrap_or((0, false));
        neoism_ui::session_layout::ClosingContextSlot {
            grid_index,
            workspace_id: workspace_id.map(|id| id as u64),
            is_workspace_root,
            shell_pid,
            is_terminal_context,
        }
    }
}

/// "Looks like a project root" — heuristic used to decide whether to
/// install full live filesystem watching and full-fat search, or a
/// lightweight loose mode. The user opening a terminal at `~/` shouldn't
/// pay the cost of recursively watching `~/Library` for FSEvents or
/// letting `rg` walk every macOS cache folder. Any of:
///
/// - any `.git` (file or dir — submodules use a file pointer),
/// - any common project manifest (Cargo, npm, Python, Go, JVM, CMake, etc.),
/// - or an editor-config marker (`.vscode/`, `.idea/`),
///
/// flips us into full mode. Otherwise we treat the directory as a loose
/// browse target. Stats only — no recursion, no canonicalize.
///
/// Thin re-export of [`neoism_ui::context_policy::is_project_workspace`]
/// so the shared crate and the desktop fork always agree on the
/// markers set. See that module for the full marker list and tests.
pub(crate) fn is_project_workspace(path: &Path) -> bool {
    neoism_ui::context_policy::is_project_workspace(path)
}
