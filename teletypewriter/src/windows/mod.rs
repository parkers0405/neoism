mod child;
mod conpty;
mod pipes;
mod spsc;

use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::{OsStr, OsString};
use std::io::{self};
use std::iter::once;
use std::mem;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::TryRecvError;

use crate::windows::child::ChildExitWatcher;
use crate::{ChildEvent, EventedPty, ProcessReadWrite, Winsize, WinsizeBuilder};
use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
    TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, TerminateProcess,
    CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
};

use conpty::Conpty as Backend;
use pipes::{EventedAnonRead as ReadPipe, EventedAnonWrite as WritePipe};

pub struct Pty {
    // Backend is required to be the first field, to ensure correct drop order. Dropping
    // `conout` before `backend` will cause a deadlock (with Conpty).
    backend: Backend,
    conout: ReadPipe,
    conin: WritePipe,
    read_token: corcovado::Token,
    write_token: corcovado::Token,
    child_event_token: corcovado::Token,
    child_watcher: ChildExitWatcher,
}

// Creates conpty instead of pty
// Windows Pseudo Console (ConPTY)
pub fn create_pty(
    shell: &str,
    args: Vec<String>,
    working_directory: &Option<String>,
    columns: u16,
    rows: u16,
) -> Result<Pty, std::io::Error> {
    // The shell is passed through verbatim (it may already be a full
    // command line from user config); each argument is quoted per the
    // CommandLineToArgvW rules so paths with spaces survive.
    let mut exec = shell.to_string();
    for arg in &args {
        exec.push(' ');
        exec.push_str(&quote_cmdline_arg(arg));
    }
    conpty::new(&exec, working_directory, columns, rows)
}

/// Quote a single command-line argument per the MSVCRT/CommandLineToArgvW
/// rules: wrap in double quotes when it contains whitespace, quotes, or is
/// empty; backslash runs are doubled before a quote (2n+1 before a literal
/// quote, 2n before the closing quote) and left alone everywhere else.
fn quote_cmdline_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.chars().any(|c| matches!(c, ' ' | '\t' | '"')) {
        return arg.to_string();
    }

    let mut quoted = String::with_capacity(arg.len() + 2);
    quoted.push('"');
    let mut backslashes = 0usize;
    for c in arg.chars() {
        if c == '\\' {
            backslashes += 1;
        } else {
            if c == '"' {
                // 2n+1 backslashes total before a literal quote.
                quoted.extend(std::iter::repeat('\\').take(backslashes + 1));
            }
            backslashes = 0;
        }
        quoted.push(c);
    }
    // 2n backslashes total before the closing quote.
    quoted.extend(std::iter::repeat('\\').take(backslashes));
    quoted.push('"');
    quoted
}

/// Name-parity alias with the Unix API so cross-platform callers can spawn
/// without cfg-gating on the function name.
pub fn create_pty_with_spawn(
    shell: &str,
    args: Vec<String>,
    working_directory: &Option<String>,
    columns: u16,
    rows: u16,
) -> Result<Pty, std::io::Error> {
    create_pty(shell, args, working_directory, columns, rows)
}

impl Pty {
    fn new(
        backend: impl Into<Backend>,
        conout: impl Into<ReadPipe>,
        conin: impl Into<WritePipe>,
        child_watcher: ChildExitWatcher,
    ) -> Self {
        Self {
            backend: backend.into(),
            conout: conout.into(),
            conin: conin.into(),
            read_token: 0.into(),
            write_token: 0.into(),
            child_event_token: 0.into(),
            child_watcher,
        }
    }

    pub fn child_watcher(&self) -> &ChildExitWatcher {
        &self.child_watcher
    }

    pub fn child_exit_code(&self) -> io::Result<Option<u32>> {
        self.child_watcher.exit_code()
    }

    pub fn child_pid(&self) -> Option<std::num::NonZeroU32> {
        self.child_watcher.pid()
    }
}

impl ProcessReadWrite for Pty {
    type Reader = ReadPipe;
    type Writer = WritePipe;

    #[inline]
    fn register(
        &mut self,
        poll: &corcovado::Poll,
        token: &mut dyn Iterator<Item = corcovado::Token>,
        interest: corcovado::Ready,
        poll_opts: corcovado::PollOpt,
    ) -> io::Result<()> {
        self.read_token = token.next().unwrap();
        self.write_token = token.next().unwrap();

        if interest.is_readable() {
            poll.register(
                &self.conout,
                self.read_token,
                corcovado::Ready::readable(),
                poll_opts,
            )?
        } else {
            poll.register(
                &self.conout,
                self.read_token,
                corcovado::Ready::empty(),
                poll_opts,
            )?
        }
        if interest.is_writable() {
            poll.register(
                &self.conin,
                self.write_token,
                corcovado::Ready::writable(),
                poll_opts,
            )?
        } else {
            poll.register(
                &self.conin,
                self.write_token,
                corcovado::Ready::empty(),
                poll_opts,
            )?
        }

        self.child_event_token = token.next().unwrap();
        poll.register(
            self.child_watcher.event_rx(),
            self.child_event_token,
            corcovado::Ready::readable(),
            poll_opts,
        )?;

        Ok(())
    }

    #[inline]
    fn reregister(
        &mut self,
        poll: &corcovado::Poll,
        interest: corcovado::Ready,
        poll_opts: corcovado::PollOpt,
    ) -> io::Result<()> {
        if interest.is_readable() {
            poll.reregister(
                &self.conout,
                self.read_token,
                corcovado::Ready::readable(),
                poll_opts,
            )?;
        } else {
            poll.reregister(
                &self.conout,
                self.read_token,
                corcovado::Ready::empty(),
                poll_opts,
            )?;
        }
        if interest.is_writable() {
            poll.reregister(
                &self.conin,
                self.write_token,
                corcovado::Ready::writable(),
                poll_opts,
            )?;
        } else {
            poll.reregister(
                &self.conin,
                self.write_token,
                corcovado::Ready::empty(),
                poll_opts,
            )?;
        }

        poll.reregister(
            self.child_watcher.event_rx(),
            self.child_event_token,
            corcovado::Ready::readable(),
            poll_opts,
        )?;

        Ok(())
    }

    #[inline]
    fn deregister(&mut self, poll: &corcovado::Poll) -> io::Result<()> {
        poll.deregister(&self.conout)?;
        poll.deregister(&self.conin)?;
        poll.deregister(self.child_watcher.event_rx())?;
        Ok(())
    }

    #[inline]
    fn reader(&mut self) -> &mut Self::Reader {
        &mut self.conout
    }

    #[inline]
    fn read_token(&self) -> corcovado::Token {
        self.read_token
    }

    #[inline]
    fn writer(&mut self) -> &mut Self::Writer {
        &mut self.conin
    }

    #[inline]
    fn write_token(&self) -> corcovado::Token {
        self.write_token
    }

    #[inline]
    fn set_winsize(
        &mut self,
        winsize_builder: WinsizeBuilder,
    ) -> Result<(), std::io::Error> {
        let winsize: Winsize = winsize_builder.build();
        self.backend.on_resize(winsize);
        Ok(())
    }
}

impl EventedPty for Pty {
    fn child_event_token(&self) -> corcovado::Token {
        self.child_event_token
    }

    fn next_child_event(&mut self) -> Option<ChildEvent> {
        match self.child_watcher.event_rx().try_recv() {
            Ok(ev) => Some(ev),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(ChildEvent::Exited),
        }
    }
}

fn cmdline(shell: &str) -> String {
    if !shell.is_empty() {
        return shell.to_string();
    }

    once("powershell")
        // .chain(shell.args().iter().map(|a| a.as_ref()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Converts the string slice into a Windows-standard representation for "W"-
/// suffixed function variants, which accept UTF-16 encoded string values.
pub fn win32_string<S: AsRef<OsStr> + ?Sized>(value: &S) -> Vec<u16> {
    OsStr::new(value).encode_wide().chain(once(0)).collect()
}

pub fn spawn_daemon<I, S>(program: &str, args: I) -> io::Result<()>
where
    I: IntoIterator<Item = S> + Copy,
    S: AsRef<OsStr>,
{
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
}

/// Terminate the process with the given PID. Failures are ignored, the
/// same as the Unix `kill(pid, SIGHUP)` counterpart.
pub fn kill_pid(pid: i32) {
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid as u32);
        if !handle.is_null() {
            TerminateProcess(handle, 1);
            CloseHandle(handle);
        }
    }
}

/// ConPTY has no terminfo database, so entries never exist on Windows.
pub fn terminfo_exists(_terminfo: &str) -> bool {
    false
}

/// Full image path of the process with the given PID, or an empty string
/// if the process is gone or cannot be queried. (The Unix counterpart
/// shells out to `ps -o comm=`; the image path is the closest Windows
/// equivalent.)
pub fn command_per_pid(pid: i32) -> String {
    process_image_path(pid as u32)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// One row of a Toolhelp process snapshot.
#[derive(Clone)]
struct ProcessEntry {
    pid: u32,
    parent_pid: u32,
    exe: String,
}

fn snapshot_processes() -> io::Result<Vec<ProcessEntry>> {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }

        let mut entries = Vec::new();
        let mut entry: PROCESSENTRY32W = mem::zeroed();
        entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;

        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                let len = entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExeFile.len());
                entries.push(ProcessEntry {
                    pid: entry.th32ProcessID,
                    parent_pid: entry.th32ParentProcessID,
                    exe: String::from_utf16_lossy(&entry.szExeFile[..len]),
                });
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }

        CloseHandle(snapshot);
        Ok(entries)
    }
}

/// Creation time as a raw FILETIME tick count, for ordering only.
fn process_creation_time(pid: u32) -> Option<u64> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut creation: FILETIME = mem::zeroed();
        let mut exit: FILETIME = mem::zeroed();
        let mut kernel: FILETIME = mem::zeroed();
        let mut user: FILETIME = mem::zeroed();
        let ok =
            GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user);
        CloseHandle(handle);
        if ok == 0 {
            return None;
        }
        Some(((creation.dwHighDateTime as u64) << 32) | creation.dwLowDateTime as u64)
    }
}

fn process_image_path(pid: u32) -> Option<PathBuf> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            buf.as_mut_ptr(),
            &mut len,
        );
        CloseHandle(handle);
        if ok == 0 {
            return None;
        }
        Some(PathBuf::from(OsString::from_wide(&buf[..len as usize])))
    }
}

/// Best guess at the "foreground" process of a ConPTY session: the most
/// recently created leaf in the process tree rooted at the shell. There
/// is no tcgetpgrp equivalent for ConPTY, so this walks a Toolhelp
/// snapshot instead.
fn foreground_process(shell_pid: u32) -> Option<ProcessEntry> {
    if shell_pid == 0 {
        return None;
    }
    let entries = snapshot_processes().ok()?;

    let mut by_pid: HashMap<u32, usize> = HashMap::new();
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (idx, entry) in entries.iter().enumerate() {
        by_pid.insert(entry.pid, idx);
        if entry.parent_pid != entry.pid {
            children
                .entry(entry.parent_pid)
                .or_default()
                .push(entry.pid);
        }
    }

    // Restrict the walk to descendants of the shell. The visited set
    // guards against parent-pid cycles introduced by PID reuse.
    let mut visited: HashSet<u32> = HashSet::new();
    let mut queue: VecDeque<u32> = VecDeque::from([shell_pid]);
    let mut leaves: Vec<u32> = Vec::new();
    while let Some(pid) = queue.pop_front() {
        if !visited.insert(pid) || !by_pid.contains_key(&pid) {
            continue;
        }
        match children.get(&pid) {
            Some(kids) => queue.extend(kids),
            None => leaves.push(pid),
        }
    }

    // Most recently created leaf wins; processes we cannot query
    // (e.g. elevated children) rank lowest.
    let best = leaves
        .into_iter()
        .max_by_key(|pid| process_creation_time(*pid).unwrap_or(0))?;
    by_pid.get(&best).map(|&idx| entries[idx].clone())
}

/// Executable name (without the `.exe` suffix) of the foreground process
/// of the ConPTY session rooted at `shell_pid`, or an empty string if it
/// cannot be determined.
///
/// Unlike the Unix version there is no PTY fd / process group, so only
/// the shell PID is taken and the foreground process is inferred from
/// the process tree (see [`foreground_process`]).
pub fn foreground_process_name(shell_pid: u32) -> String {
    let Some(entry) = foreground_process(shell_pid) else {
        return String::new();
    };
    let mut name = entry.exe;
    if name.to_ascii_lowercase().ends_with(".exe") {
        name.truncate(name.len() - 4);
    }
    name
}

/// Full image path of the foreground process of the ConPTY session
/// rooted at `shell_pid`.
///
/// Unlike the Unix version (which reports the process **cwd** via
/// `/proc/<pid>/cwd` — unreadable for another process on Windows without
/// undocumented PEB spelunking), this reports the executable path from
/// `QueryFullProcessImageNameW`. Signature also differs: no PTY fd, see
/// [`foreground_process_name`].
pub fn foreground_process_path(
    shell_pid: u32,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let entry = foreground_process(shell_pid)
        .ok_or("no live process found for the ConPTY session")?;
    process_image_path(entry.pid)
        .ok_or_else(|| "could not query the foreground process image path".into())
}

#[cfg(test)]
mod cmdline_tests {
    use super::quote_cmdline_arg;

    #[test]
    fn plain_args_are_untouched() {
        assert_eq!(quote_cmdline_arg("-NoLogo"), "-NoLogo");
        assert_eq!(
            quote_cmdline_arg("C:\\Windows\\notepad.exe"),
            "C:\\Windows\\notepad.exe"
        );
    }

    #[test]
    fn spaces_and_quotes_are_escaped() {
        assert_eq!(quote_cmdline_arg(""), "\"\"");
        assert_eq!(
            quote_cmdline_arg("C:\\Program Files\\x"),
            "\"C:\\Program Files\\x\""
        );
        // Trailing backslash run is doubled before the closing quote.
        assert_eq!(quote_cmdline_arg("C:\\a b\\"), "\"C:\\a b\\\\\"");
        // Literal quote gets 2n+1 backslashes.
        assert_eq!(quote_cmdline_arg("say \"hi\""), "\"say \\\"hi\\\"\"");
        assert_eq!(quote_cmdline_arg("tail\\\""), "\"tail\\\\\\\"\"");
    }
}
