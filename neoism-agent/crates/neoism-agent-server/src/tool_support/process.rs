use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::Command;

pub(crate) struct CapturedOutput {
    pub(crate) bytes: Vec<u8>,
    pub(crate) truncated: bool,
}

pub(crate) fn read_child_output<T>(
    pipe: Option<T>,
    maximum_bytes: usize,
) -> tokio::task::JoinHandle<anyhow::Result<CapturedOutput>>
where
    T: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut output = Vec::new();
        let mut truncated = false;
        if let Some(mut pipe) = pipe {
            let mut chunk = [0_u8; 16 * 1024];
            loop {
                let read = pipe.read(&mut chunk).await?;
                if read == 0 {
                    break;
                }
                let remaining = maximum_bytes.saturating_sub(output.len());
                if remaining > 0 {
                    output.extend_from_slice(&chunk[..read.min(remaining)]);
                }
                truncated |= read > remaining;
            }
        }
        Ok(CapturedOutput {
            bytes: output,
            truncated,
        })
    })
}

pub(crate) async fn wait_for_cancel(cancel: Option<Arc<AtomicBool>>) {
    let Some(cancel) = cancel else {
        std::future::pending::<()>().await;
        return;
    };
    while !cancel.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub(crate) async fn terminate_child(
    child: &mut tokio::process::Child,
    child_id: Option<u32>,
) {
    terminate_process_group(child_id);
    tokio::time::sleep(Duration::from_millis(100)).await;
    if child.try_wait().ok().flatten().is_none() {
        kill_process_group(child_id);
        let _ = child.start_kill();
    }
    let _ = child.wait().await;
}

#[cfg(unix)]
pub(crate) fn set_new_process_group(command: &mut Command) {
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
}

#[cfg(windows)]
pub(crate) fn set_new_process_group(command: &mut Command) {
    // `creation_flags` REPLACES any previously-set flags (Command exposes
    // no getter), so this must stay the only flag-setting site for
    // commands routed through here.
    crate::windows_process::hide_tokio_command(command);
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn set_new_process_group(_command: &mut Command) {}

/// [`set_new_process_group`] for synchronous `std::process::Command`
/// spawns (the LSP client).
#[cfg(unix)]
pub(crate) fn set_new_process_group_std(command: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
}

#[cfg(windows)]
pub(crate) fn set_new_process_group_std(command: &mut std::process::Command) {
    crate::windows_process::hide_std_command(command);
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn set_new_process_group_std(_command: &mut std::process::Command) {}

#[cfg(unix)]
fn terminate_process_group(child_id: Option<u32>) {
    if let Some(pid) = child_id {
        unsafe {
            libc::kill(-(pid as libc::pid_t), libc::SIGTERM);
        }
    }
}

#[cfg(unix)]
pub(crate) fn kill_process_group(child_id: Option<u32>) {
    if let Some(pid) = child_id {
        unsafe {
            libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
        }
    }
}

// Windows has no signalable process groups; approximate SIGTERM/SIGKILL of
// the group by terminating the process TREE rooted at the child. Both paths
// are the same hard TerminateProcess — Windows offers no graceful,
// console-free equivalent.
#[cfg(windows)]
fn terminate_process_group(child_id: Option<u32>) {
    kill_process_tree(child_id);
}

#[cfg(windows)]
pub(crate) fn kill_process_group(child_id: Option<u32>) {
    kill_process_tree(child_id);
}

#[cfg(windows)]
fn kill_process_tree(child_id: Option<u32>) {
    use std::collections::HashMap;

    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32First, Process32Next, PROCESSENTRY32,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, TerminateProcess, PROCESS_TERMINATE,
    };

    let Some(root) = child_id else {
        return;
    };
    // One snapshot of the whole process table; the parent links are only
    // meaningful at this instant (pids recycle), which is the best Windows
    // offers short of job objects.
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return;
        }
        let mut entry: PROCESSENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32>() as u32;
        if Process32First(snapshot, &mut entry) != 0 {
            loop {
                children
                    .entry(entry.th32ParentProcessID)
                    .or_default()
                    .push(entry.th32ProcessID);
                if Process32Next(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
    }

    // Breadth-first from the root: every parent is enqueued before its
    // children, so the reversed order terminates children before parents
    // and a dying parent can never orphan a still-running grandchild
    // mid-walk.
    let mut ordered = vec![root];
    let mut index = 0;
    while index < ordered.len() {
        let pid = ordered[index];
        index += 1;
        if let Some(kids) = children.get(&pid) {
            for kid in kids {
                // Recycled pids can fabricate parent-link cycles; visit once.
                if *kid != root && !ordered.contains(kid) {
                    ordered.push(*kid);
                }
            }
        }
    }
    unsafe {
        for pid in ordered.iter().rev() {
            let handle = OpenProcess(PROCESS_TERMINATE, 0, *pid);
            if !handle.is_null() {
                TerminateProcess(handle, 1);
                CloseHandle(handle);
            }
        }
    }
}

#[cfg(not(any(unix, windows)))]
fn terminate_process_group(_child_id: Option<u32>) {}

#[cfg(not(any(unix, windows)))]
pub(crate) fn kill_process_group(_child_id: Option<u32>) {}
