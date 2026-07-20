// Auto-split from screen/mod.rs. See sibling mod.rs for the Screen struct and
// the constructor/core methods. This file is part of the impl Screen<'_> block.

use super::*;
use crate::terminal::blocks::input::TerminalInputBufferHostExt;
use neoism_backend::clipboard::Clipboard;
use neoism_terminal_core::crosswords::pos::{Direction, Line};
use neoism_window::event::ElementState;
use neoism_window::keyboard::{Key, ModifiersState, NamedKey};
use std::path::PathBuf;

mod block_overlay;
mod context_menu;
mod key_bindings;
mod modal;
mod splash_island;

/// Compact, readable label for an LSP location row in the picker: the
/// file's last two path components plus a 1-based line number (e.g.
/// `src/main.rs:42`). Keeps rows scannable without the full absolute
/// path blowing out the modal width.
fn lsp_location_label(uri: &str, line: u32) -> String {
    let path = uri.strip_prefix("file://").unwrap_or(uri);
    let mut parts = path.rsplit('/');
    let last = parts.next().unwrap_or(path);
    let label = match parts.next() {
        Some(parent) if !parent.is_empty() => format!("{parent}/{last}"),
        _ => last.to_string(),
    };
    format!("{label}:{}", line.saturating_add(1))
}

/// One-line summary shown under an LSP picker, noting truncation when the
/// result set exceeds the row cap. `noun` is singularized ("symbol",
/// "location") and pluralized here.
fn picker_summary(noun: &str, total: usize, cap: usize) -> String {
    if total > cap {
        format!("{total} {noun}s · showing first {cap}")
    } else {
        format!("{total} {noun}{}", if total == 1 { "" } else { "s" })
    }
}

/// Map the host-owned [`neoism_ui::widgets::modal::ModalAction`] enum
/// onto the shared-policy [`neoism_ui::chrome_policy::ModalActionTag`].
///
/// Lives outside `impl Screen` so it stays a pure projection — the
/// shared policy never needs to depend on the host modal crate, but
/// the host still gets a single source of truth for "which arm is
/// which dispatch class".
fn modal_action_policy_tag(
    action: &neoism_ui::widgets::modal::ModalAction,
) -> neoism_ui::chrome_policy::ModalActionTag {
    use neoism_ui::chrome_policy::ModalActionTag as Tag;
    use neoism_ui::widgets::modal::ModalAction as A;
    match action {
        A::Close => Tag::Close,
        // Footer settings menu one-shots — plain UI opens, same class
        // as Close (no fs mutation, no prompt).
        A::NotesOpenGraph | A::NotesOpenCreateMenu => Tag::Close,
        A::InstallLsp { .. } => Tag::InstallLsp,
        A::InstallPythonKernel => Tag::InstallPythonKernel,
        A::InstallTreesitter { .. } => Tag::InstallTreesitter,
        A::ApplyTheme { .. } => Tag::ApplyTheme,
        A::ApplyShaderOverlay { .. } => Tag::ApplyShaderOverlay,
        A::ApplyMashupPack { .. } => Tag::ApplyMashupPack,
        A::RunEditorCommand { .. } => Tag::RunEditorCommand,
        A::RunEditorCommandWithInput { .. } => Tag::RunEditorCommandWithInput,
        A::OpenLspLocation { .. } => Tag::OpenLspLocation,
        A::ApplyLspCodeAction { .. } => Tag::ApplyLspCodeAction,
        A::InstallAgent { .. } => Tag::InstallAgent,
        A::RunAgent { .. } => Tag::RunAgent,
        A::AcpPermission { .. } => Tag::AcpPermission,
        A::FileTreeEdit { .. } => Tag::FileTreeEdit,
        A::FileTreeCopy { .. } => Tag::FileTreeCopy,
        A::FileTreePaste { .. } => Tag::FileTreePaste,
        A::FileTreePromptDelete { .. } => Tag::FileTreePromptDelete,
        A::FileTreeDelete { .. } => Tag::FileTreeDelete,
        A::FileTreePromptNewFile { .. } => Tag::FileTreePromptNewFile,
        A::NotesPromptNewFile { .. } => Tag::NotesPromptNewFile,
        A::FileTreePromptNewFolder { .. } => Tag::FileTreePromptNewFolder,
        A::FileTreePromptRename { .. } => Tag::FileTreePromptRename,
        A::FileTreeNewFile { .. } => Tag::FileTreeNewFile,
        A::NotesNewFile { .. } => Tag::NotesNewFile,
        A::NotesNewDrawing { .. } => Tag::NotesNewDrawing,
        A::NotesPromptIcon { .. } => Tag::NotesPromptIcon,
        A::NotesSetIcon { .. } => Tag::NotesSetIcon,
        A::FileTreeNewFolder { .. } => Tag::FileTreeNewFolder,
        A::FileTreeRename { .. } => Tag::FileTreeRename,
        A::RenameTab { .. } => Tag::RenameTab,
        A::NotesVaultPromptAdd => Tag::NotesVaultPromptAdd,
        A::ServerFormSubmit => Tag::ServerFormSubmit,
        A::ServerRemoveConfirm { .. } => Tag::ServerRemoveConfirm,
        A::NotesVaultAdd { .. } => Tag::NotesVaultAdd,
        A::NotesVaultPromptRename => Tag::NotesVaultPromptRename,
        A::NotesVaultRename { .. } => Tag::NotesVaultRename,
        A::NotesVaultSwitch { .. } => Tag::NotesVaultSwitch,
        A::NotesVaultOpenVaultsRoot => Tag::NotesVaultOpenVaultsRoot,
        A::NotesVaultLinkCurrentWorkspace => Tag::NotesVaultLinkCurrentWorkspace,
        A::NotesVaultPromptLinkProject { .. } => Tag::NotesVaultPromptLinkProject,
        A::NotesVaultLinkProject { .. } => Tag::NotesVaultLinkProject,
        A::NotesVaultShareWithRemarkable { .. } => Tag::NotesVaultShareWithRemarkable,
    }
}
