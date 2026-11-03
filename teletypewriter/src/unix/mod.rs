#![cfg(unix)]

#[cfg(target_os = "macos")]
mod macos;
mod signals;

extern crate libc;

use crate::{ChildEvent, EventedPty, ProcessReadWrite, Winsize, WinsizeBuilder};
use corcovado::unix::EventedFd;
#[cfg(target_os = "macos")]
use macos::*;
use signal_hook::consts as sigconsts;
use signals::Signals;
use std::ffi::{CStr, CString};
use std::fs::File;
use std::io;
use std::io::Error;
use std::mem::MaybeUninit;
use std::ops::Deref;
use std::os::fd::OwnedFd;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::ptr;
use std::sync::Arc;

#[cfg(all(target_os = "linux", not(target_env = "musl")))]
const TIOCSWINSZ: libc::c_ulong = 0x5414;
#[cfg(all(target_os = "linux", target_env = "musl"))]
const TIOCSWINSZ: libc::c_int = 0x5414;
#[cfg(target_os = "freebsd")]
const TIOCSWINSZ: libc::c_ulong = 0x80087467;
#[cfg(target_os = "macos")]
const TIOCSWINSZ: libc::c_ulong = 2148037735;

#[link(name = "util")]
extern "C" {
    fn forkpty(
        main: *mut libc::c_int,
        name: *mut libc::c_char,
        termp: *const libc::termios,
        winsize: *const Winsize,
    ) -> libc::pid_t;

    fn openpty(
        main: *mut libc::c_int,
        child: *mut libc::c_int,
        name: *mut libc::c_char,
        termp: *const libc::termios,
        winsize: *const Winsize,
    ) -> libc::pid_t;

    fn waitpid(
        pid: libc::pid_t,
        status: *mut libc::c_int,
        options: libc::c_int,
    ) -> libc::pid_t;

    fn ptsname(fd: *mut libc::c_int) -> *mut libc::c_char;
}

#[cfg(target_os = "macos")]
fn default_shell_command(shell: &str) {
    let command_shell_string = CString::new(shell).unwrap();
    let command_pointer = command_shell_string.as_ptr();
    let args = CString::new("--login").unwrap();
    let args_pointer = args.as_ptr();
    unsafe {
        libc::execvp(command_pointer, vec![args_pointer].as_ptr());
    }
}

#[cfg(not(target_os = "macos"))]
fn default_shell_command(shell: &str) {
    let command_shell_string = CString::new(shell).unwrap();
    let command_pointer = command_shell_string.as_ptr();
    // let home = std::env::var("HOME").unwrap();
    // let args = CString::new(home).unwrap();
    // let args_pointer = args.as_ptr() as *const i8;
    unsafe {
        libc::execvp(
            command_pointer,
            vec![command_pointer, std::ptr::null()].as_ptr(),
        );
    }
}

pub struct Pty {
    pub child: Child,
    file: File,
    token: corcovado::Token,
    signals_token: corcovado::Token,
    signals: Signals,
    // next_child_event reaps the process; preserve its actual wait status.
    child_exit_status: Option<i32>,
}

impl Pty {
    pub fn child_exit_status(&self) -> Option<i32> {
        self.child_exit_status
    }
}

impl Deref for Pty {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.child
    }
}

impl io::Write for Pty {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match unsafe {
            libc::write(
                *self.child,
                buf.as_ptr() as *const _,
                buf.len() as libc::size_t,
            )
        } {
            n if n >= 0 => Ok(n as usize),
            _ => Err(io::Error::last_os_error()),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl io::Read for Pty {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match unsafe {
            libc::read(
                *self.child,
                buf.as_mut_ptr() as *mut _,
                buf.len() as libc::size_t,
            )
        } {
            n if n >= 0 => Ok(n as usize),
            _ => Err(io::Error::last_os_error()),
        }
    }
}

impl ProcessReadWrite for Pty {
    type Reader = File;
    type Writer = File;

    #[inline]
    fn reader(&mut self) -> &mut File {
        &mut self.file
    }

    #[inline]
    fn read_token(&self) -> corcovado::Token {
        self.token
    }

    #[inline]
    fn writer(&mut self) -> &mut File {
        &mut self.file
    }

    #[inline]
    fn write_token(&self) -> corcovado::Token {
        self.token
    }

    #[inline]
    fn set_winsize(&mut self, winsize: WinsizeBuilder) -> Result<(), std::io::Error> {
        self.child.set_winsize(winsize)
    }

    #[inline]
    fn register(
        &mut self,
        poll: &corcovado::Poll,
        token: &mut dyn Iterator<Item = corcovado::Token>,
        interest: corcovado::Ready,
        poll_opts: corcovado::PollOpt,
    ) -> io::Result<()> {
        self.token = token.next().unwrap();
        poll.register(
            &EventedFd(&self.file.as_raw_fd()),
            self.token,
            interest,
            poll_opts,
        )?;

        self.signals_token = token.next().unwrap();
        poll.register(
            &self.signals,
            self.signals_token,
            corcovado::Ready::readable(),
            corcovado::PollOpt::level(),
        )
    }

    fn reregister(
        &mut self,
        poll: &corcovado::Poll,
        interest: corcovado::Ready,
        poll_opts: corcovado::PollOpt,
    ) -> io::Result<()> {
        poll.reregister(
            &EventedFd(&self.file.as_raw_fd()),
            self.token,
            interest,
            poll_opts,
        )?;

        poll.reregister(
            &self.signals,
            self.signals_token,
            corcovado::Ready::readable(),
            corcovado::PollOpt::level(),
        )
    }

    fn deregister(&mut self, poll: &corcovado::Poll) -> io::Result<()> {
        poll.deregister(&EventedFd(&self.file.as_raw_fd()))?;
        poll.deregister(&self.signals)
    }
}

// From alacritty: https://github.com/alacritty/alacritty/blob/2df8f860b960d7c96efaf4f059fe2fbbdce82bcc/alacritty_terminal/src/tty/mod.rs#L83
/// Check if a terminfo entry exists on the system.
pub fn terminfo_exists(terminfo: &str) -> bool {
    // Get first terminfo character for the parent directory.
    let first = terminfo.get(..1).unwrap_or_default();
    let first_hex = format!("{:x}", first.chars().next().unwrap_or_default() as usize);

    // Return true if the terminfo file exists at the specified location.
    macro_rules! check_path {
        ($path:expr) => {
            if $path.join(first).join(terminfo).exists()
                || $path.join(&first_hex).join(terminfo).exists()
            {
                return true;
            }
        };
    }

    if let Some(dir) = std::env::var_os("TERMINFO") {
        check_path!(PathBuf::from(&dir));
    } else if let Some(home) = dirs::home_dir() {
        check_path!(home.join(".terminfo"));
    }

    if let Ok(dirs) = std::env::var("TERMINFO_DIRS") {
        for dir in dirs.split(':') {
            check_path!(PathBuf::from(dir));
        }
    }

    if let Ok(prefix) = std::env::var("PREFIX") {
        let path = PathBuf::from(prefix);
        check_path!(path.join("etc/terminfo"));
        check_path!(path.join("lib/terminfo"));
        check_path!(path.join("share/terminfo"));
    }

    check_path!(PathBuf::from("/etc/terminfo"));
    check_path!(PathBuf::from("/lib/terminfo"));
    check_path!(PathBuf::from("/usr/share/terminfo"));
    check_path!(PathBuf::from("/boot/system/data/terminfo"));

    // No valid terminfo path has been found.
    false
}

pub fn create_termp(utf8: bool) -> libc::termios {
    // musl libc does not provide c_ispeed and c_ospeed fields in struct termios.
    #[cfg(target_os = "linux")]
    let mut term = libc::termios {
        c_iflag: libc::ICRNL | libc::IXON | libc::IXANY | libc::IMAXBEL | libc::BRKINT,
        c_oflag: libc::OPOST | libc::ONLCR,
        c_cflag: libc::CREAD | libc::CS8 | libc::HUPCL,
        c_lflag: libc::ICANON
            | libc::ISIG
            | libc::IEXTEN
            | libc::ECHO
            | libc::ECHOE
            | libc::ECHOK
            | libc::ECHOKE
            | libc::ECHOCTL,
        c_cc: Default::default(),
        #[cfg(not(target_env = "musl"))]
        c_ispeed: Default::default(),
        #[cfg(not(target_env = "musl"))]
        c_ospeed: Default::default(),
        #[cfg(target_env = "musl")]
        __c_ispeed: Default::default(),
        #[cfg(target_env = "musl")]
        __c_ospeed: Default::default(),
        c_line: 0,
    };

    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    let mut term = libc::termios {
        c_iflag: libc::ICRNL | libc::IXON | libc::IXANY | libc::IMAXBEL | libc::BRKINT,
        c_oflag: libc::OPOST | libc::ONLCR,
        c_cflag: libc::CREAD | libc::CS8 | libc::HUPCL,
        c_lflag: libc::ICANON
            | libc::ISIG
            | libc::IEXTEN
            | libc::ECHO
            | libc::ECHOE
            | libc::ECHOK
            | libc::ECHOKE
            | libc::ECHOCTL,
        c_cc: Default::default(),
        c_ispeed: Default::default(),
        c_ospeed: Default::default(),
    };

    #[cfg(not(target_os = "freebsd"))]
    {
        // Enable utf8 support if requested
        if utf8 {
            term.c_iflag |= libc::IUTF8;
        }
    }

    // Set supported terminal characters
    term.c_cc[libc::VEOF] = 4;
    term.c_cc[libc::VEOL] = 255;
    term.c_cc[libc::VEOL2] = 255;
    term.c_cc[libc::VERASE] = 0x7f;
    term.c_cc[libc::VWERASE] = 23;
    term.c_cc[libc::VKILL] = 21;
    term.c_cc[libc::VREPRINT] = 18;
    term.c_cc[libc::VINTR] = 3;
    term.c_cc[libc::VQUIT] = 0x1c;
    term.c_cc[libc::VSUSP] = 26;
    term.c_cc[libc::VSTART] = 17;
    term.c_cc[libc::VSTOP] = 19;
    term.c_cc[libc::VLNEXT] = 22;
    term.c_cc[libc::VDISCARD] = 15;
    term.c_cc[libc::VMIN] = 1;
    term.c_cc[libc::VTIME] = 0;

    #[cfg(target_os = "macos")]
    {
        term.c_cc[libc::VDSUSP] = 25;
        term.c_cc[libc::VSTATUS] = 20;
    }

    term
}

#[derive(Default)]
struct ShellUser {
    user: String,
    home: String,
    shell: String,
}

impl ShellUser {
    /// look for shell, username, longname, and home dir in the respective environment variables
    /// before falling back on looking in to `passwd`.
    fn from_env() -> Result<Self, Error> {
        let mut buf = [0; 1024];
        let pw = get_pw_entry(&mut buf);

        let user = match std::env::var("USER") {
            Ok(user) => user,
            Err(_) => match pw {
                Ok(ref pw) => pw.name.to_owned(),
                Err(err) => return Err(err),
            },
        };

        let home = match std::env::var("HOME") {
            Ok(home) => home,
            Err(_) => match pw {
                Ok(ref pw) => pw.dir.to_owned(),
                Err(err) => return Err(err),
            },
        };

        #[allow(unused_mut)]
        let mut shell = match std::env::var("SHELL") {
            Ok(env_shell) => env_shell,
            Err(_) => match pw {
                Ok(ref pw) => pw.shell.to_owned(),
                Err(err) => return Err(err),
            },
        };

        Ok(Self { user, home, shell })
    }
}

///
/// Creates a pseudoterminal using spawn.
///
/// The [`create_pty`] creates a pseudoterminal with similar behavior as tty,
/// which is a command in Unix and Unix-like operating systems to print the file name of the
/// terminal connected to standard input. tty stands for TeleTYpewriter.
///
/// It returns two [`Pty`] along with respective process name [`String`] and process id (`libc::pid_`)
///
pub fn create_pty_with_spawn(
    shell: &str,
    args: Vec<String>,
    working_directory: &Option<String>,
    columns: u16,
    rows: u16,
) -> Result<Pty, Error> {
    create_pty_with_spawn_env(shell, args, working_directory, columns, rows, &[])
}

#[cfg(any(target_os = "macos", test))]
fn macos_shell_command(shell: &str, args: &[String]) -> Command {
    let name = shell.rsplit('/').next().unwrap_or(shell);
    // Own the shell itself, not /usr/bin/login's PAM monitor: login forks,
    // discards the shell's wait status, and closes the terminal on SIGHUP.
    // A leading '-' retains login-shell startup (/etc/zprofile's path_helper,
    // user login profiles, etc.), including for the default interactive shell.
    // Command passes literal argv directly, without an intermediate shell.
    let mut command = Command::new(shell);
    command.arg0(format!("-{name}")).args(args);
    command
}

#[cfg(test)]
mod macos_exec_tests {
    use super::macos_shell_command;

    #[test]
    fn login_command_preserves_argument_boundaries() {
        let args = vec!["-c".into(), "printf '%s' \"$value\"; exit 7".into(), "".into()];
        let command = macos_shell_command("/path with spaces/my's shell", &args);
        assert_eq!(command.get_program(), "/path with spaces/my's shell");
        assert_eq!(command.get_args().collect::<Vec<_>>(), args.iter().map(std::ffi::OsStr::new).collect::<Vec<_>>());
    }

    #[test]
    fn owned_login_shell_preserves_pid_status_and_literal_arguments() {
        let values = ["", "two words", "a'b", "$HOME", "$(printf injected)", "*", "semi;colon", "line\nbreak"];
        let mut args = vec!["-c".into(), "printf '%s\\000' \"$0\" \"$$\" \"$@\"; exit 7".into(), "script".into()];
        args.extend(values.iter().map(|value| (*value).into()));
        // sh -c takes its first operand after the script as $0. Use that slot
        // explicitly in the literals test; the no-operand case below tests arg0.
        let child = macos_shell_command("/bin/sh", &args)
            .stdout(std::process::Stdio::piped()).spawn().unwrap();
        let pid = child.id().to_string();
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(7));
        let expected: Vec<u8> = ["script", pid.as_str()].into_iter().chain(values)
            .flat_map(|value| value.bytes().chain(std::iter::once(0))).collect();
        assert_eq!(output.stdout, expected);

        let output = macos_shell_command("/bin/sh", &["-c".into(), "printf '%s' \"$0\"".into()]).output().unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"-sh");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn default_shell_loads_login_profiles_and_keeps_owned_pid() {
        let unique = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("neoism-login-profile-{}-{unique}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        // Use the actual default (no explicit argv) launch shape. ZDOTDIR
        // isolates user startup files without skipping system login profiles.
        std::fs::write(dir.join(".zprofile"), "export NEOISM_LOGIN_PROFILE=loaded\n").unwrap();
        std::fs::write(dir.join(".zlogin"), "printf '%s:%s:%s' \"$NEOISM_LOGIN_PROFILE\" \"$0\" \"$$\"; exit 7\n").unwrap();
        let child = macos_shell_command("/bin/zsh", &[]).env("ZDOTDIR", &dir)
            .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped())
            .spawn().unwrap();
        let pid = child.id();
        let output = child.wait_with_output().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
        assert_eq!(output.status.code(), Some(7));
        assert_eq!(output.stdout, format!("loaded:-zsh:{pid}").as_bytes());
    }
}

pub fn create_pty_with_spawn_env(
    shell: &str,
    args: Vec<String>,
    working_directory: &Option<String>,
    columns: u16,
    rows: u16,
    env: &[(String, String)],
) -> Result<Pty, Error> {
    #[cfg(not(any(target_os = "macos", target_os = "freebsd")))]
    let mut is_controling_terminal = true;

    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    let is_controling_terminal = true;

    let mut main: libc::c_int = 0;
    let mut child: libc::c_int = 0;
    let winsize = Winsize {
        ws_row: rows as libc::c_ushort,
        ws_col: columns as libc::c_ushort,
        ws_width: 0 as libc::c_ushort,
        ws_height: 0 as libc::c_ushort,
    };
    let term = create_termp(true);

    let res = unsafe {
        openpty(
            &mut main as *mut _,
            &mut child as *mut _,
            ptr::null_mut(),
            &term as *const libc::termios,
            &winsize as *const _,
        )
    };

    if res < 0 {
        return Err(Error::other("openpty failed"));
    }

    let mut shell_program = shell;

    let user = match ShellUser::from_env() {
        Ok(data) => data,
        Err(..) => ShellUser {
            shell: shell.to_string(),
            ..Default::default()
        },
    };

    if shell.is_empty() {
        shell_program = &user.shell;
    }

    tracing::info!("spawn {:?} {:?}", shell_program, args);

    let mut builder = {
        #[cfg(target_os = "macos")]
        {
            macos_shell_command(shell_program, &args)
        }

        #[cfg(not(target_os = "macos"))]
        {
            let mut cmd = Command::new(shell_program);
            cmd.args(args);
            cmd
        }
    };

    #[cfg(target_os = "linux")]
    {
        // If running inside a flatpak sandbox.
        // Must retrieve $SHELL from outside the sandbox, so ask the host.
        if std::path::PathBuf::from("/.flatpak-info").exists() {
            builder = Command::new("flatpak-spawn");

            let mut with_args = vec![
                "--host".to_string(),
                "--watch-bus".to_string(),
                "--env=COLORTERM=truecolor".to_string(),
                "--env=TERM=rio".to_string(),
            ];
            with_args.extend(
                env.iter()
                    .map(|(key, value)| format!("--env={key}={value}")),
            );

            if let Some(directory) = working_directory {
                with_args.push(format!(
                    "--directory={}",
                    std::path::Path::new(directory).display()
                ));
            }

            let output = std::process::Command::new("flatpak-spawn")
                .args(["--host", "sh", "-c", "echo $SHELL"])
                .output()?;
            let shell = String::from_utf8_lossy(&output.stdout);

            with_args.push(shell.trim().to_string());
            with_args.push("-l".to_string());

            builder.args(with_args);

            is_controling_terminal = false;
        }
    }

    // Setup child stdin/stdout/stderr as child fd of PTY.
    // Ownership of fd is transferred to the Stdio structs and will be closed by them at the end of
    // this scope. (It is not an issue that the fd is closed three times since File::drop ignores
    // error on libc::close.).
    let owned_child = unsafe { OwnedFd::from_raw_fd(child) };

    builder.stdin(owned_child.try_clone()?);
    builder.stderr(owned_child.try_clone()?);
    builder.stdout(owned_child);

    // Retain login's identity environment while inheriting the caller's PATH;
    // the login shell still runs system/user profiles (including path_helper).
    // No PAM/login monitor is needed inside this already-authenticated app.
    #[cfg(target_os = "macos")]
    builder.env("LOGNAME", &user.user).env("SHELL", &user.shell);
    builder.env("USER", user.user);
    builder.env("HOME", user.home);
    builder.envs(env.iter().map(|(key, value)| (key, value)));

    unsafe {
        builder.pre_exec(move || {
            // Create a new process group.
            let err = libc::setsid();
            if err == -1 {
                return Err(Error::last_os_error());
            }

            if is_controling_terminal {
                set_controlling_terminal(child)?;
            }

            // No longer need child/main fds.
            libc::close(child);
            libc::close(main);

            libc::signal(libc::SIGCHLD, libc::SIG_DFL);
            libc::signal(libc::SIGHUP, libc::SIG_DFL);
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            libc::signal(libc::SIGQUIT, libc::SIG_DFL);
            libc::signal(libc::SIGTERM, libc::SIG_DFL);
            libc::signal(libc::SIGALRM, libc::SIG_DFL);

            Ok(())
        });
    }

    // Handle set working directory option.
    if let Some(dir) = &working_directory {
        builder.current_dir(dir);
    }

    // Prepare signal handling before spawning child.
    let signals =
        Signals::new([sigconsts::SIGCHLD]).expect("error preparing signal handling");

    match builder.spawn() {
        Ok(child_process) => {
            unsafe {
                set_nonblocking(main);
            }

            let ptsname: String = tty_ptsname(main).unwrap_or_else(|_| "".to_string());
            let child_unix = Child {
                lifecycle: std::sync::Mutex::new(ChildLifecycle::Running),
                id: Arc::new(main),
                ptsname,
                pid: Arc::new(child_process.id().try_into().unwrap()),
                process: Some(child_process),
            };

            Ok(Pty {
                child_exit_status: None,
                child: child_unix,
                file: unsafe { File::from_raw_fd(main) },
                token: corcovado::Token::from(0),
                signals,
                signals_token: corcovado::Token::from(0),
            })
        }
        Err(err) => Err(Error::new(
            err.kind(),
            format!(
                "Failed to spawn command '{}': {}",
                builder.get_program().to_string_lossy(),
                err
            ),
        )),
    }
}

///
/// Creates a pseudoterminal using fork.
///
/// The [`create_pty`] creates a pseudoterminal with similar behavior as tty,
/// which is a command in Unix and Unix-like operating systems to print the file name of the
/// terminal connected to standard input. tty stands for TeleTYpewriter.
///
/// It returns two [`Pty`] along with respective process name [`String`] and process id (`libc::pid_`)
///
pub fn create_pty_with_fork(shell: &str, columns: u16, rows: u16) -> Result<Pty, Error> {
    create_pty_with_fork_with_working_dir(shell, None, columns, rows)
}

pub fn create_pty_with_fork_with_working_dir(
    shell: &str,
    working_directory: Option<&str>,
    columns: u16,
    rows: u16,
) -> Result<Pty, Error> {
    let mut main = 0;
    let winsize = Winsize {
        ws_row: rows as libc::c_ushort,
        ws_col: columns as libc::c_ushort,
        ws_width: 0 as libc::c_ushort,
        ws_height: 0 as libc::c_ushort,
    };
    let term = create_termp(true);

    let mut shell_program = shell;

    let user = match ShellUser::from_env() {
        Ok(data) => data,
        Err(..) => ShellUser {
            shell: shell.to_string(),
            ..Default::default()
        },
    };

    if shell.is_empty() {
        tracing::info!("shell configuration is empty, will retrieve from env");
        shell_program = &user.shell;
    }

    let working_directory = match working_directory {
        Some(dir) => Some(CString::new(dir).map_err(|err| {
            Error::new(
                io::ErrorKind::InvalidInput,
                format!("working directory contains NUL byte: {err}"),
            )
        })?),
        None => None,
    };

    tracing::info!("fork {:?}", shell_program);

    // Keep exit notification pending even if the child exits before the
    // parent's PTY worker registers the signal pipe with its poller.
    let signals = Signals::new([sigconsts::SIGCHLD])?;
    match unsafe {
        forkpty(
            &mut main as *mut _,
            ptr::null_mut(),
            &term as *const libc::termios,
            &winsize as *const _,
        )
    } {
        0 => {
            if let Some(dir) = working_directory.as_ref() {
                if unsafe { libc::chdir(dir.as_ptr()) } != 0 {
                    unsafe { libc::_exit(127) };
                }
            }
            default_shell_command(shell_program);
            Err(Error::other(format!(
                "forkpty has reach unreachable with {shell_program}"
            )))
        }
        id if id > 0 => {
            // TODO: Currently we fork the process and don't wait to know if led to failure
            // Whenever it happens it will just simply shut down the teletyperwriter
            // In the future add an option to check before release the method
            let ptsname: String = tty_ptsname(main).unwrap_or_else(|_| "".to_string());
            let child = Child {
                lifecycle: std::sync::Mutex::new(ChildLifecycle::Running),
                id: Arc::new(main),
                ptsname,
                pid: Arc::new(id),
                process: None,
            };

            unsafe {
                set_nonblocking(main);
            }

            Ok(Pty {
                child_exit_status: None,
                child,
                signals,
                file: unsafe { File::from_raw_fd(main) },
                token: corcovado::Token(0),
                signals_token: corcovado::Token(0),
            })
        }
        _ => Err(Error::other(format!(
            "forkpty failed using {shell_program}"
        ))),
    }
}

/// Really only needed on BSD, but should be fine elsewhere.
fn set_controlling_terminal(fd: libc::c_int) -> Result<(), Error> {
    let res = unsafe {
        // TIOSCTTY changes based on platform and the `ioctl` call is different
        // based on architecture (32/64). So a generic cast is used to make sure
        // there are no issues. To allow such a generic cast the clippy warning
        // is disabled.
        #[allow(clippy::cast_lossless)]
        libc::ioctl(fd, libc::TIOCSCTTY as _, 0)
    };

    if res < 0 {
        return Err(Error::last_os_error());
    }

    Ok(())
}

// https://man7.org/linux/man-pages/man2/fcntl.2.html
unsafe fn set_nonblocking(fd: libc::c_int) {
    use libc::{fcntl, F_GETFL, F_SETFL, O_NONBLOCK};

    let res = fcntl(fd, F_SETFL, fcntl(fd, F_GETFL, 0) | O_NONBLOCK);
    assert_eq!(res, 0);
}

#[derive(Debug)]
pub struct Child {
    lifecycle: std::sync::Mutex<ChildLifecycle>,
    pub id: Arc<libc::c_int>,
    pub pid: Arc<libc::pid_t>,
    #[allow(dead_code)]
    ptsname: String,
    #[allow(dead_code)]
    process: Option<std::process::Child>,
}

#[derive(Debug)]
enum ChildLifecycle {
    Running,
    Reaped(i32),
    Gone,
}

impl Child {
    /// The tcgetwinsize function fills in the winsize structure pointed to by
    ///  gws with values that represent the size of the terminal window for which
    ///  fd provides an open file descriptor.  If no error occurs tcgetwinsize()
    ///  returns zero (0).
    ///  The tcsetwinsize function sets the terminal window size, for the terminal
    ///  referenced by fd, to the sizes from the winsize structure pointed to by
    ///  sws.  If no error occurs tcsetwinsize() returns zero (0).
    ///  The winsize structure, defined in <termios.h>, contains (at least) the
    ///  following four fields
    ///  unsigned short ws_row;      /* Number of rows, in characters */
    ///  unsigned short ws_col;      /* Number of columns, in characters */
    ///  unsigned short ws_xpixel;   /* Width, in pixels */
    ///  unsigned short ws_ypixel;   /* Height, in pixels */
    /// If the actual window size of the controlling terminal of a process
    /// changes, the process is sent a SIGWINCH signal.  See signal(7).  Note
    /// simply changing the sizes using tcsetwinsize() does not necessarily
    /// change the actual window size, and if not, will not generate a SIGWINCH.
    pub fn set_winsize(&self, winsize_builder: WinsizeBuilder) -> io::Result<()> {
        let winsize: Winsize = winsize_builder.build();
        match unsafe { libc::ioctl(**self, TIOCSWINSZ, &winsize as *const _) } {
            -1 => Err(io::Error::last_os_error()),
            _ => Ok(()),
        }
    }

    /// Return the child’s exit status if it has already exited. If the child is still running, return Ok(None).
    /// https://linux.die.net/man/2/waitpid
    pub fn waitpid(&self) -> Result<Option<i32>, String> {
        let mut lifecycle = self.lifecycle.lock().unwrap();
        match *lifecycle {
            ChildLifecycle::Reaped(status) => return Ok(Some(status)),
            ChildLifecycle::Gone => return Err("PTY child no longer owned".into()),
            ChildLifecycle::Running => {}
        }
        let mut status = 0 as libc::c_int;
        // If WNOHANG was specified in options and there were no children in a waitable state, then waitid() returns 0 immediately and the state of the siginfo_t structure pointed to by infop is unspecified. To distinguish this case from that where a child was in a waitable state, zero out the si_pid field before the call and check for a nonzero value in this field after the call returns.
        let res =
            unsafe { waitpid(*self.pid, &mut status as *mut libc::c_int, libc::WNOHANG) };
        if res == 0 {
            return Ok(None);
        }

        if res == -1 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ECHILD) {
                *lifecycle = ChildLifecycle::Gone;
            }
            return Err(error.to_string());
        }

        if res != *self.pid {
            tracing::warn!(
                target: "teletypewriter::waitpid",
                watched_pid = *self.pid,
                returned_pid = res,
                status,
                "ignored waitpid result for unexpected child"
            );
            return Ok(None);
        }

        if libc::WIFEXITED(status) || libc::WIFSIGNALED(status) {
            *lifecycle = ChildLifecycle::Reaped(status);
            Ok(Some(status))
        } else {
            tracing::warn!(
                target: "teletypewriter::waitpid",
                pid = *self.pid,
                status,
                "ignored waitpid status that was not process exit"
            );
            Ok(None)
        }
    }

    pub fn close(&self) {
        unsafe {
            libc::close(*self.pid);
        }
    }
}

pub fn kill_pid(pid: i32) {
    unsafe {
        libc::kill(pid, libc::SIGHUP);
    }
}

impl Deref for Child {
    type Target = libc::c_int;
    fn deref(&self) -> &libc::c_int {
        &self.id
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        // Only our unreaped child can be signalled. A cached natural exit must
        // never cause a signal to a PID that the OS may already have reused.
        let _ = self.waitpid();
        if !matches!(*self.lifecycle.get_mut().unwrap(), ChildLifecycle::Running) {
            return;
        }
        let pid = *self.pid;
        if pid <= 0 {
            return;
        }
        // Preserve shell traps/cleanup on ordinary close. Target the owned
        // leader only, never the daemon's/foreground process group.
        unsafe {
            libc::kill(pid, libc::SIGHUP);
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(250);
        // Waiting is off the caller/UI thread, even if the kernel cannot
        // immediately finish a killed child (e.g. uninterruptible I/O).
        if let Err(error) = std::thread::Builder::new()
            .name("neoism-pty-reap".into())
            .spawn(move || reap_after_hangup(pid, deadline))
        {
            tracing::error!(pid, %error, "failed to start PTY child reaper");
        }
    }
}

fn reap_after_hangup(pid: libc::pid_t, deadline: std::time::Instant) {
    let mut escalated = false;
    loop {
        let mut status = 0;
        let options = if escalated { 0 } else { libc::WNOHANG };
        let result = unsafe { libc::waitpid(pid, &mut status, options) };
        if result == pid {
            return;
        }
        if result == -1 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            // In particular, ECHILD relinquishes ownership: never signal a
            // potentially reused PID after another waiter has reaped it.
            if error.raw_os_error() != Some(libc::ECHILD) {
                tracing::error!(pid, %error, "failed to reap PTY child");
            }
            return;
        }
        if std::time::Instant::now() >= deadline {
            // Only waitpid == 0 authorizes escalation. An unreaped child
            // retains its PID even if it exits between this check and kill.
            unsafe { libc::kill(pid, libc::SIGKILL); }
            escalated = true;
        } else {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

pub fn command_per_pid(pid: libc::pid_t) -> String {
    let current_process_name = Command::new("ps")
        .arg("-p")
        .arg(format!("{pid:}"))
        .arg("-o")
        .arg("comm=")
        .output()
        .expect("failed to execute process")
        .stdout;

    std::str::from_utf8(&current_process_name)
        .unwrap_or("")
        .to_string()
}

impl EventedPty for Pty {
    #[inline]
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        // Both spawn paths register before creating the child. The signal
        // pipe retains readiness until drained, including before poll setup.
        // Use the owned child's wait status rather than the coalesced signal.
        self.signals.pending().for_each(drop);
        if self.child_exit_status.is_some() {
            return None;
        }
        match self.child.waitpid() {
            Err(_) | Ok(None) => None,
            Ok(Some(status)) => {
                self.child_exit_status = Some(status);
                Some(ChildEvent::Exited)
            }
        }
    }

    #[inline]
    fn child_event_token(&self) -> corcovado::Token {
        self.signals_token
    }
}

#[derive(Debug)]
struct Passwd<'a> {
    name: &'a str,
    dir: &'a str,
    shell: &'a str,
}

/// Return a Passwd struct with pointers into the provided buf.
///
/// # Unsafety
///
/// If `buf` is changed while `Passwd` is alive, bad thing will almost certainly happen.
fn get_pw_entry(buf: &mut [i8; 1024]) -> Result<Passwd<'_>, Error> {
    // Create zeroed passwd struct.
    let mut entry: MaybeUninit<libc::passwd> = MaybeUninit::uninit();

    let mut res: *mut libc::passwd = ptr::null_mut();

    // Try and read the pw file.
    let uid = unsafe { libc::getuid() };
    let status = unsafe {
        libc::getpwuid_r(
            uid,
            entry.as_mut_ptr(),
            buf.as_mut_ptr() as *mut _,
            buf.len(),
            &mut res,
        )
    };
    let entry = unsafe { entry.assume_init() };

    if status < 0 {
        return Err(Error::other("getpwuid_r failed"));
    }

    if res.is_null() {
        return Err(Error::other("pw not found"));
    }

    // Sanity check.
    assert_eq!(entry.pw_uid, uid);

    // Build a borrowed Passwd struct.
    Ok(Passwd {
        name: unsafe { CStr::from_ptr(entry.pw_name).to_str().unwrap() },
        dir: unsafe { CStr::from_ptr(entry.pw_dir).to_str().unwrap() },
        shell: unsafe { CStr::from_ptr(entry.pw_shell).to_str().unwrap() },
    })
}

/// Unsafe
/// Return tty pts name [`String`]
///
/// # Safety
///
/// This function is unsafe because it contains the usage of `libc::ptsname`
/// from libc that's naturally unsafe.
pub fn tty_ptsname(fd: libc::c_int) -> Result<String, String> {
    let c_str: &CStr = unsafe {
        let name_ptr = ptsname(fd as *mut _);
        CStr::from_ptr(name_ptr)
    };
    let str_slice: &str = c_str.to_str().unwrap();
    let str_buf: String = str_slice.to_owned();

    Ok(str_buf)
}

pub fn foreground_process_name(main_fd: RawFd, shell_pid: u32) -> String {
    let mut pid = unsafe { libc::tcgetpgrp(main_fd) };
    if pid < 0 {
        pid = shell_pid as libc::pid_t;
    }

    #[cfg(not(any(target_os = "macos", target_os = "freebsd")))]
    let comm_path = format!("/proc/{pid}/comm");
    #[cfg(target_os = "freebsd")]
    let comm_path = format!("/compat/linux/proc/{pid}/comm");

    #[cfg(not(target_os = "macos"))]
    let name = match std::fs::read(comm_path) {
        Ok(comm_str) => String::from_utf8_lossy(&comm_str)
            .trim_end()
            .parse()
            .unwrap_or_default(),
        Err(..) => String::from(""),
    };

    #[cfg(target_os = "macos")]
    let name = macos_process_name(pid);

    name
}

pub fn foreground_process_path(
    main_fd: RawFd,
    shell_pid: u32,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mut pid = unsafe { libc::tcgetpgrp(main_fd) };
    if pid < 0 {
        pid = shell_pid as libc::pid_t;
    }

    #[cfg(not(any(target_os = "macos", target_os = "freebsd")))]
    let link_path = format!("/proc/{pid}/cwd");
    #[cfg(target_os = "freebsd")]
    let link_path = format!("/compat/linux/proc/{pid}/cwd");

    #[cfg(not(target_os = "macos"))]
    let cwd = std::fs::read_link(link_path)?;

    #[cfg(target_os = "macos")]
    let cwd = macos_cwd(pid)?;

    Ok(cwd)
}

/// Start a new process in the background.
pub fn spawn_daemon<I, S>(
    program: &str,
    args: I,
    main_fd: RawFd,
    shell_pid: u32,
) -> io::Result<()>
where
    I: IntoIterator<Item = S> + Copy,
    S: AsRef<std::ffi::OsStr>,
{
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Ok(cwd) = foreground_process_path(main_fd, shell_pid) {
        command.current_dir(cwd);
    }
    unsafe {
        command
            .pre_exec(|| {
                match libc::fork() {
                    -1 => return Err(io::Error::last_os_error()),
                    0 => (),
                    _ => libc::_exit(0),
                }

                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }

                Ok(())
            })
            .spawn()?
            .wait()
            .map(|_| ())
    }
}
