use std::ffi::OsStr;
use std::process::Command;

#[cfg(windows)]
const BACKGROUND_COMMAND_ARG: &str = "--neoism-internal-background-command";

pub(crate) fn command(program: impl AsRef<OsStr>) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let executable = std::env::current_exe().unwrap_or_default();
        if !executable
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| name.eq_ignore_ascii_case("neoism.exe"))
        {
            let mut command = Command::new(program);
            command.creation_flags(
                windows_sys::Win32::System::Threading::CREATE_NO_WINDOW
                    | windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP,
            );
            return command;
        }
        let mut command = Command::new(executable);
        command
            .arg(BACKGROUND_COMMAND_ARG)
            .arg(program)
            .creation_flags(
                windows_sys::Win32::System::Threading::CREATE_NO_WINDOW
                    | windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP,
            );
        return command;
    }
    #[cfg(not(windows))]
    Command::new(program)
}

#[cfg(windows)]
pub(crate) fn run_background_command() -> Option<i32> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, GetExitCodeProcess, WaitForSingleObject, CREATE_NO_WINDOW,
        CREATE_UNICODE_ENVIRONMENT, INFINITE, PROCESS_INFORMATION, STARTF_USESHOWWINDOW,
        STARTF_USESTDHANDLES, STARTUPINFOW,
    };

    let mut args = std::env::args_os();
    let _executable = args.next()?;
    if args.next()?.to_str()? != BACKGROUND_COMMAND_ARG {
        return None;
    }
    let program = args.next()?;
    let mut command_line = Vec::new();
    push_quoted_arg(&mut command_line, &program);
    for arg in args {
        command_line.push(b' ' as u16);
        push_quoted_arg(&mut command_line, &arg);
    }
    command_line.push(0);

    let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    startup.dwFlags = STARTF_USESHOWWINDOW | STARTF_USESTDHANDLES;
    startup.wShowWindow = windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE as u16;
    startup.hStdInput = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    startup.hStdOutput = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    startup.hStdError = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
    let mut process: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let created = unsafe {
        CreateProcessW(
            std::ptr::null(),
            command_line.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
            CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT,
            std::ptr::null(),
            std::ptr::null(),
            &startup,
            &mut process,
        )
    };
    if created == 0 {
        eprintln!(
            "failed to start background command: {}",
            std::io::Error::last_os_error()
        );
        return Some(1);
    }
    unsafe {
        WaitForSingleObject(process.hProcess, INFINITE);
    }
    let mut exit_code = 1;
    unsafe {
        GetExitCodeProcess(process.hProcess, &mut exit_code);
        CloseHandle(process.hThread);
        CloseHandle(process.hProcess);
    }
    Some(exit_code as i32)
}

#[cfg(windows)]
fn push_quoted_arg(command_line: &mut Vec<u16>, arg: &OsStr) {
    use std::os::windows::ffi::OsStrExt;

    let arg = arg.encode_wide().collect::<Vec<_>>();
    if !arg.is_empty() && !arg.iter().any(|c| matches!(*c, 9 | 32 | 34)) {
        command_line.extend(arg);
        return;
    }
    command_line.push(34);
    let mut backslashes = 0usize;
    for character in arg {
        if character == 92 {
            backslashes += 1;
            continue;
        } else {
            if character == 34 {
                command_line.extend(std::iter::repeat_n(92, backslashes * 2 + 1));
            } else {
                command_line.extend(std::iter::repeat_n(92, backslashes));
            }
            backslashes = 0;
        }
        command_line.push(character);
    }
    command_line.extend(std::iter::repeat_n(92, backslashes * 2));
    command_line.push(34);
}

#[cfg(target_os = "windows")]
pub(crate) fn open_url(url: &str) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let operation = OsStr::new("open")
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let url = OsStr::new(url)
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            url.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if result as isize > 32 {
        Ok(())
    } else {
        Err(format!(
            "Windows could not open the URL (ShellExecute error {result:?})"
        ))
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn open_url(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = command("open");
        command.arg(url);
        command
    };
    #[cfg(not(target_os = "macos"))]
    let mut command = {
        let mut command = command("xdg-open");
        command.arg(url);
        command
    };
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}
