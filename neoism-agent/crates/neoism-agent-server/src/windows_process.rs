//! Windows child-process flags for GUI-parented tools.
//!
//! `creation_flags` replaces the whole CreateProcess mask, so every Windows
//! spawn that is not a user-visible PTY must set the complete hide mask in
//! one call. Some Windows 11 builds ignore `CREATE_NO_WINDOW` unless the
//! child is also started without a console window.

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 =
    windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP;
#[cfg(windows)]
const DETACHED_PROCESS: u32 = windows_sys::Win32::System::Threading::DETACHED_PROCESS;

/// Hide a console child of a GUI process. Combine with a new process group
/// so interrupt helpers can still signal the child.
#[cfg(windows)]
pub const HIDDEN_CONSOLE: u32 = CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP;

/// Detach from the parent console/session without allocating a new one.
#[cfg(windows)]
pub const DETACHED_HIDDEN: u32 =
    DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW;

pub fn std_command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    hide_std_command(&mut command);
    command
}

pub fn hide_std_command(command: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(HIDDEN_CONSOLE);
    }
    let _ = command;
}

pub fn hide_tokio_command(command: &mut tokio::process::Command) {
    #[cfg(windows)]
    command.creation_flags(HIDDEN_CONSOLE);
    let _ = command;
}

pub fn detach_std_command(command: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(DETACHED_HIDDEN);
    }
    let _ = command;
}

/// Canonicalize without the Windows `\\?\` verbatim prefix. FFF, git, and
/// most command-line tools join `/`-separated relatives onto this base.
pub fn canonicalize_path(path: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    dunce::canonicalize(path)
}

pub fn canonicalize_path_lossy(path: &std::path::Path) -> std::path::PathBuf {
    canonicalize_path(path).unwrap_or_else(|_| strip_verbatim_prefix(path))
}

pub fn strip_verbatim_prefix(path: &std::path::Path) -> std::path::PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        std::path::PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        std::path::PathBuf::from(rest)
    } else {
        path.to_path_buf()
    }
}

/// Drive roots (`C:\`) and verbatim drive roots (`\\?\C:\`) both count as
/// filesystem roots. `\\?\C:\`.parent()` is `\\?\C:`, so a raw `parent()`
/// check misses them.
pub fn is_filesystem_root(path: &std::path::Path) -> bool {
    use std::path::Component;
    let stripped = strip_verbatim_prefix(path);
    stripped.parent().is_none()
        || matches!(
            stripped.components().next_back(),
            Some(Component::RootDir) | Some(Component::Prefix(_))
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn strips_windows_verbatim_prefixes() {
        assert_eq!(
            strip_verbatim_prefix(Path::new(r"\\?\C:\Users\src")),
            PathBuf::from(r"C:\Users\src")
        );
        assert_eq!(
            strip_verbatim_prefix(Path::new(r"\\?\UNC\server\share")),
            PathBuf::from(r"\\server\share")
        );
        assert_eq!(
            strip_verbatim_prefix(Path::new(r"C:\Users\src")),
            PathBuf::from(r"C:\Users\src")
        );
    }

    #[test]
    fn detects_drive_and_verbatim_roots() {
        assert!(is_filesystem_root(Path::new(r"C:\")));
        assert!(is_filesystem_root(Path::new(r"\\?\C:\")));
        assert!(!is_filesystem_root(Path::new(r"C:\Users")));
        assert!(!is_filesystem_root(Path::new(r"\\?\C:\Users")));
    }
}
