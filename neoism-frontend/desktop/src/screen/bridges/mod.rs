// Bridges to the various overlay surfaces and integrations the screen owns.
pub mod acp;
pub mod agent;
pub mod conversation_navigator;
pub mod buffer_tabs;
pub mod code;
pub mod draw;
pub mod epub;
pub mod extensions;
pub mod file_tree;
mod file_browser;
pub(crate) mod conversation_workspaces;
pub mod finder;
pub mod git_diff;
#[cfg(feature = "cef-artifacts")]
pub(crate) mod html_artifacts;
pub mod markdown;
pub mod neoworld;
pub mod notebook;
pub mod palette;
pub mod remarkable;
pub mod tags_view;
pub mod top_bar;
pub mod workspace;
