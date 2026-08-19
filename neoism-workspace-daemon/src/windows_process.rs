//! Hide console children spawned by the workspace daemon.

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 =
    windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP;
#[cfg(windows)]
const DETACHED_PROCESS: u32 = windows_sys::Win32::System::Threading::DETACHED_PROCESS;

#[cfg(windows)]
pub const HIDDEN_CONSOLE: u32 = CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP;

#[cfg(windows)]
pub const DETACHED_HIDDEN: u32 = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW;

#[cfg(windows)]
pub fn hide_std_command(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(HIDDEN_CONSOLE);
}

#[cfg(windows)]
pub fn hide_tokio_command(command: &mut tokio::process::Command) {
    command.creation_flags(HIDDEN_CONSOLE);
}

#[cfg(windows)]
pub fn detach_std_command(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(DETACHED_HIDDEN);
}
