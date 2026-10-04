//! Neoism's language-neutral plugin contract and optional embedded Lua host.
//!
//! Rendering never crosses this boundary. Lua publishes immutable Rust data
//! and typed host actions; Sugarloaf and `neoism-ui` retain ownership of every
//! frame, layout, animation, input event, and scroll operation.

mod types;
pub use types::*;

mod contract;
pub use contract::*;

mod resources;
pub use resources::*;

mod ecosystem;
pub use ecosystem::*;

mod platform;
pub use platform::*;

#[cfg(feature = "runtime")]
mod runtime;
#[cfg(feature = "runtime")]
pub use runtime::*;

pub const API_VERSION: u32 = 1;

/// Stable built-in selector names. Child selectors inherit from their dotted
/// parents, so `agent.chat.message.user` falls back through `agent.chat.message`
/// and `agent.chat` before preserving the Rust site's original value.
pub mod selector {
    pub const APP: &str = "app";
    pub const CHROME_TOP: &str = "chrome.top";
    pub const CHROME_BOTTOM: &str = "chrome.bottom";
    pub const BUFFER_TABS: &str = "buffer-tabs";
    pub const BREADCRUMBS: &str = "breadcrumbs";
    pub const STATUS: &str = "status";
    pub const STATUS_ITEM: &str = "status.item";
    pub const COMPOSER: &str = "composer";
    pub const FILE_TREE: &str = "file-tree";
    pub const FILE_TREE_ROW: &str = "file-tree.row";
    pub const FILE_TREE_ROW_SELECTED: &str = "file-tree.row.selected";
    pub const FILE_TREE_ROW_HOVER: &str = "file-tree.row.hover";
    pub const FILE_TREE_ICON: &str = "file-tree.icon";
    pub const NOTES_TREE: &str = "notes-tree";
    pub const NOTES_TREE_ROW: &str = "notes-tree.row";
    pub const NOTES_TREE_ROW_SELECTED: &str = "notes-tree.row.selected";
    pub const NOTES_TREE_ROW_HOVER: &str = "notes-tree.row.hover";
    pub const NOTES_TREE_ICON: &str = "notes-tree.icon";
    pub const AGENT_CHAT: &str = "agent.chat";
    pub const AGENT_MESSAGE: &str = "agent.chat.message";
    pub const AGENT_USER_MESSAGE: &str = "agent.chat.message.user";
    pub const AGENT_ASSISTANT_MESSAGE: &str = "agent.chat.message.assistant";
    pub const AGENT_TOOL: &str = "agent.chat.tool";
    pub const AGENT_TOOL_RESULT: &str = "agent.chat.tool.result";
    pub const AGENT_SIDEBAR: &str = "agent.sidebar";
    pub const EDITOR: &str = "editor";
    pub const MARKDOWN: &str = "markdown";
    pub const TERMINAL: &str = "terminal";
    pub const GIT: &str = "git";
    pub const SETTINGS: &str = "settings";
    pub const PALETTE: &str = "palette";
    pub const FINDER: &str = "finder";
    pub const NOTIFICATION: &str = "notification";
    pub const MODAL: &str = "modal";
}
