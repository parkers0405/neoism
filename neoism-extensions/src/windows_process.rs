//! Hide console children spawned by the extension installer.

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 =
    windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP;

#[cfg(windows)]
pub const HIDDEN_CONSOLE: u32 = CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP;

#[cfg(windows)]
pub fn hide_tokio_command(command: &mut tokio::process::Command) {
    command.creation_flags(HIDDEN_CONSOLE);
}
