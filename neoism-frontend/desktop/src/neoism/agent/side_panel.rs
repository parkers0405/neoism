//! Desktop re-export shim for the agent side-panel types.
//!
//! All side-panel state (BranchActivity, BranchStatus, SidePanelMode,
//! NeoismAgentSessionEntry, NeoismAgentSidePanel) and its constants
//! now live in the shared crate at
//! `neoism_ui::panels::agent_pane::state::side_panel`. This file
//! re-exports them so existing call sites in the desktop fork
//! (`crate::neoism::agent::side_panel::*`,
//! `super::side_panel::{BranchStatus, ...}`) keep resolving without
//! code changes. The desktop and web frontends now share a single
//! side-panel implementation.

#[allow(unused_imports)]
pub use neoism_ui::panels::agent_pane::state::side_panel::{
    BranchActivity, BranchStatus, BranchStatusTransition, GoalStatus,
    NeoismAgentSessionEntry, NeoismAgentSidePanel, SessionGoal, SidePanelMode, FONT_SIZE,
    FRAME_RADIUS, FRAME_STROKE, ROW_HEIGHT, ROW_PADDING_X, SIDE_PANEL_MIN_PANE_WIDTH,
};
