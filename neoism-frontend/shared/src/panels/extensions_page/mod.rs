//! Zed-style Extensions panel.
//!
//! Skeleton for sub-task 1.1: state container + render entry point so the
//! buffer-tab integration can mount it. Card / header / tab-strip drawing
//! lands in 1.2/1.3.

mod state;
mod view;

pub use state::{
    ExtensionEntry, ExtensionFilter, ExtensionKind, ExtensionStatus, ExtensionTab, KeyResponse,
    LuaPluginAction, LuaPluginLifecycle, LuaPluginPresentation, NeoismExtensionsPane, PaneAction,
};
