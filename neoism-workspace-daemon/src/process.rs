use std::process::Command;

pub(crate) fn background_command(program: &str) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        let executable = std::env::current_exe().unwrap_or_default();
        if executable
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|name| name.eq_ignore_ascii_case("neoism.exe"))
        {
            let mut command = Command::new(executable);
            command
                .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
                .arg("--neoism-internal-background-command")
                .arg(program);
            return command;
        }

        let mut command = Command::new(program);
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        command
    }

    #[cfg(not(windows))]
    Command::new(program)
}
