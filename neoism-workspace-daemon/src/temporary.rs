//! Parent-death ownership for temporary dedicated hosts, never saved servers.
//! This is not a sandbox. Shells/agents have the host user's privileges.
#[cfg(unix)]
pub fn watch_parent(parent: u32) -> std::io::Result<()> {
    // The service helper creates a fresh session/group before exec. Refuse to
    // start a watcher capable of signaling the desktop's own process group.
    unsafe {
        if libc::getpgrp() != libc::getpid() {
            return Err(std::io::Error::other(
                "Temporary daemon must own a dedicated process group",
            ));
        }
    }
    std::thread::Builder::new()
        .name("fast-parent-watch".into())
        .spawn(move || loop {
            if unsafe { libc::getppid() } as u32 != parent {
                neoism_agent_service_api::fast_hosting::terminate_fast_process_tree(
                    std::process::id(),
                );
                std::process::exit(0);
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        })?;
    Ok(())
}

#[cfg(windows)]
pub fn watch_parent(parent: u32) -> std::io::Result<()> {
    use windows_sys::Win32::{Foundation::CloseHandle, System::Threading::*};
    // A kill-on-close job in the desktop owns descendants. This monitor also
    // covers the short interval between spawn and job assignment.
    let handle = unsafe {
        OpenProcess(0x00100000 /* SYNCHRONIZE */, 0, parent)
    };
    if handle.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    let handle = handle as usize;
    std::thread::Builder::new()
        .name("fast-parent-watch".into())
        .spawn(move || unsafe {
            WaitForSingleObject(handle as _, u32::MAX);
            CloseHandle(handle as _);
            std::process::exit(0);
        })?;
    Ok(())
}

#[cfg(not(any(unix, windows)))]
pub fn watch_parent(_parent: u32) -> std::io::Result<()> {
    Err(std::io::Error::other(
        "Temporary daemon ownership unsupported on this platform",
    ))
}
