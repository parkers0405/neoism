//! Neoism workspace identity, Markdown note indexing, and note graph queries.
//!
//! This crate owns the workspace note graph so desktop UI, CLI commands,
//! daemons, and agent tools all use the same parser, schema, and query logic.

pub mod config;
pub mod frontmatter;
pub mod graph_db;
pub mod link_repair;
pub mod notes;
pub mod query;
pub mod watcher;

pub use config::{
    default_notes_workspace, ensure_notes_workspace, init_workspace,
    link_code_dir_to_workspace_vault, link_workspace_to_vault_project,
    linked_project_for_code_dir, load_workspace, notes_vaults_dir, save_workspace,
    vault_project_links,
};
pub use graph_db::{
    rebuild_note_graph, remove_note_graph_file, replace_note_graph_file,
    workspace_graph_db_path,
};
pub use query::{
    HeadingSummary, LinkSummary, NoteGraph, NoteGraphEdge, NoteGraphNode,
    NoteGraphSummary, NoteQueryLimit, NoteSearchHit, NoteSummary, PropertySummary,
    TagOccurrenceSummary, TagSummary, TaskSummary,
};
pub use watcher::{NoteGraphWatcher, WatcherEvent};
