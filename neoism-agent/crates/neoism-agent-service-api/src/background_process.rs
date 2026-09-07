//! Background helpers only; never use this for an interactive PTY shell.

use std::ffi::OsStr;
use std::process::Command;

#[cfg(windows)]
const HIDDEN_CONSOLE: u32 = windows_sys::Win32::System::Threading::CREATE_NO_WINDOW
    | windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP;

/// Match the desktop/workspace-daemon background-command protocol when hosted
/// by the GUI. Standalone servers and test executables launch directly with the
/// same hide mask: they do not implement the GUI's internal dispatch argument.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    #[cfg(windows)]
    {
        command_for_host(
            program.as_ref(),
            &std::env::current_exe().unwrap_or_default(),
        )
    }
    #[cfg(not(windows))]
    Command::new(program)
}

#[cfg(windows)]
fn command_for_host(program: &OsStr, executable: &std::path::Path) -> Command {
    use std::os::windows::process::CommandExt;

    let mut command = if executable
        .file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| name.eq_ignore_ascii_case("neoism.exe"))
    {
        let mut command = Command::new(executable);
        command
            .arg("--neoism-internal-background-command")
            .arg(program);
        command
    } else {
        Command::new(program)
    };
    command.creation_flags(HIDDEN_CONSOLE);
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(windows))]
    #[test]
    fn non_windows_commands_remain_direct_and_preserve_argv() {
        let mut command = command("git");
        command.args(["-C", "a path with spaces", "rev-parse"]);
        assert_eq!(command.get_program(), "git");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["-C", "a path with spaces", "rev-parse"]
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_gui_wraps_program_and_argv_but_standalone_does_not() {
        use std::path::Path;
        let program = OsStr::new(r"C:\Program Files\Git\cmd\git.exe");
        for host in [r"C:\Neoism\neoism.exe", r"C:\Neoism\NEOISM.EXE"] {
            let mut command = command_for_host(program, Path::new(host));
            command.args(["-C", r"C:\a path", "rev-parse"]);
            assert_eq!(command.get_program(), host);
            assert_eq!(
                command.get_args().collect::<Vec<_>>(),
                [
                    OsStr::new("--neoism-internal-background-command"),
                    program,
                    OsStr::new("-C"),
                    OsStr::new(r"C:\a path"),
                    OsStr::new("rev-parse"),
                ]
            );
        }
        for host in ["neoism-agent-server.exe", "test.exe", ""] {
            let mut command = command_for_host(program, Path::new(host));
            command.arg("--version");
            assert_eq!(command.get_program(), program);
            assert_eq!(command.get_args().collect::<Vec<_>>(), ["--version"]);
        }
        assert_eq!(HIDDEN_CONSOLE, 0x0800_0200);
    }
}
