//! Desktop input-buffer shim.
//!
//! The full `TerminalInputBuffer` state machine — cursor, text,
//! history, completion cycle, command-block snapshots, flash state,
//! the `submit_with_context` lifecycle — lives in
//! `neoism_ui::terminal_blocks::input` so the web frontend can share
//! it. Desktop re-exports the shared type and adds an extension trait
//! that resolves the persistent-history file paths via `dirs`, which
//! is host-only and stays out of the shared crate.

pub use neoism_ui::terminal_blocks::TerminalInputBuffer;

use std::path::PathBuf;

use super::shell::{
    default_terminal_favorites_path, default_terminal_history_path,
    default_zsh_history_path, legacy_config_terminal_history_path,
};
use neoism_ui::TerminalShellKind;

/// Host-side path resolution for persistent history. Mirrors the
/// previous inherent `TerminalInputBuffer::enable_persistent_history_*`
/// methods that the desktop used to call directly. The implementations
/// live here (not in `neoism-ui`) because they depend on `dirs`,
/// `tracing`, and the file-migration `std::fs::copy` dance that only
/// makes sense for a real OS.
pub trait TerminalInputBufferHostExt {
    fn enable_persistent_favorites_default(&mut self);
    fn enable_persistent_history_for_shell(&mut self, shell_kind: TerminalShellKind);
    fn enable_persistent_history_default(&mut self);
}

impl TerminalInputBufferHostExt for TerminalInputBuffer {
    fn enable_persistent_favorites_default(&mut self) {
        let Some(path) = default_terminal_favorites_path() else {
            return;
        };
        self.enable_persistent_favorites(path.clone());
        tracing::info!(
            target: "neoism::terminal_history",
            path = %path.display(),
            "persistent terminal favorites enabled"
        );
    }

    fn enable_persistent_history_for_shell(&mut self, shell_kind: TerminalShellKind) {
        self.set_shell_kind(shell_kind);
        if shell_kind == TerminalShellKind::Zsh {
            if let Some(path) = default_zsh_history_path() {
                self.enable_zsh_history(path.clone());
                tracing::info!(
                    target: "neoism::terminal_history",
                    path = %path.display(),
                    "zsh history enabled"
                );
                return;
            }
        }
        self.enable_persistent_history_default();
    }

    fn enable_persistent_history_default(&mut self) {
        let Some(path) = default_terminal_history_path() else {
            return;
        };
        let legacy_path: Option<PathBuf> = legacy_config_terminal_history_path()
            .filter(|legacy_path| legacy_path != &path && legacy_path.exists());

        if !path.exists() {
            if let Some(legacy_path) = legacy_path.as_ref() {
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                match std::fs::copy(legacy_path, &path) {
                    Ok(_) => {
                        tracing::info!(
                            target: "neoism::terminal_history",
                            path = %path.display(),
                            legacy_path = %legacy_path.display(),
                            "migrated legacy terminal history out of config directory"
                        );
                    }
                    Err(err) => {
                        tracing::warn!(
                            target: "neoism::terminal_history",
                            path = %path.display(),
                            legacy_path = %legacy_path.display(),
                            "failed to migrate legacy terminal history: {err:?}"
                        );
                    }
                }
            }
        }

        self.enable_persistent_history(path.clone());
        tracing::info!(
            target: "neoism::terminal_history",
            path = %path.display(),
            "persistent terminal history enabled"
        );
    }
}
