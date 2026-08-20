//! File-related wire messages.
//!
//! Phase 9: file tree / read / write surface. Paths in every message are
//! workspace-relative; the daemon side rejects `..` traversal and absolute
//! paths with [`FilesServerMessage::Error`].

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FilesClientMessage {
    /// List a directory. `path` is workspace-relative.
    ListDir { path: String },
    /// Stat a single path. `path` is workspace-relative.
    Stat { path: String },
    /// Read a file's bytes. `path` is workspace-relative.
    ReadFile { path: String },
    /// Write bytes to a file (creates if not exists, truncates).
    WriteFile { path: String, bytes: Vec<u8> },
    /// Recursively walk a directory and return all entries.
    WalkTree {
        path: String,
        max_depth: Option<u32>,
    },
    /// Create a new empty file under `dir/name`. Mirrors desktop's
    /// `Screen::create_file_tree_file`. Fails if the file already
    /// exists; parent dirs are created.
    CreateFile { dir: String, name: String },
    /// Create a new directory under `dir/name`. Mirrors desktop's
    /// `Screen::create_file_tree_folder`. Idempotent — `create_dir_all`
    /// under the hood, but `Error` if the target is a non-dir file.
    CreateDir { dir: String, name: String },
    /// Rename or move a path. `from`/`to` are workspace-relative; the
    /// destination's parent dirs are created. Mirrors desktop's
    /// `Screen::rename_file_tree_path` (which also handles moves when
    /// the new name contains a `/`).
    Rename { from: String, to: String },
    /// Delete a file or directory. Directories are removed
    /// recursively. Mirrors desktop's `Screen::delete_file_tree_path`.
    Delete { path: String },
    /// Read the daemon user's shell history (newest last, at most
    /// `max_entries`). Web composers can't touch `~/.zsh_history`
    /// through the workspace-scoped file surface, but the desktop
    /// composer seeds ArrowUp recall from it — this keeps parity.
    ReadShellHistory { max_entries: Option<u32> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FilesServerMessage {
    DirListing {
        path: String,
        entries: Vec<DirEntry>,
    },
    Stat {
        path: String,
        entry: DirEntry,
    },
    FileContent {
        path: String,
        bytes: Vec<u8>,
    },
    FileWritten {
        path: String,
        bytes_written: usize,
    },
    TreeListing {
        path: String,
        entries: Vec<TreeEntry>,
    },
    /// Acknowledgement for `CreateFile` / `CreateDir`. `path` is the
    /// workspace-relative path of the newly created entry.
    FileCreated {
        path: String,
        is_dir: bool,
    },
    /// Acknowledgement for `Rename`. Echoes back both paths so the
    /// caller can update its bookkeeping without re-computing them.
    Renamed {
        from: String,
        to: String,
    },
    /// Acknowledgement for `Delete`. `was_dir` mirrors what the daemon
    /// observed before removal so the caller can refresh tree entries.
    Deleted {
        path: String,
        was_dir: bool,
    },
    /// Reply to `ReadShellHistory`: sanitized command strings, oldest
    /// first, newest last.
    ShellHistory {
        entries: Vec<String>,
    },
    /// Unsolicited push (request_id 0): something changed on disk under
    /// a watched files root. `root` is the absolute root the daemon
    /// watches (the `workspace_root` a client's requests named);
    /// `paths` are the touched absolute paths, debounced. Clients
    /// browsing that root re-list to stay live.
    Changed {
        root: String,
        paths: Vec<String>,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: Option<u64>,
    /// Markdown page icon from the file's `icon:` frontmatter, when it
    /// has one. Filled in by the daemon because a wasm client has no
    /// filesystem to read it from: `notes_sidebar::note_frontmatter_icon`
    /// opens the file directly, which silently yields `None` in the
    /// browser, so notes rows and tabs lost the emoji the desktop shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeEntry {
    pub path: String,
    pub is_dir: bool,
    pub depth: u32,
}
