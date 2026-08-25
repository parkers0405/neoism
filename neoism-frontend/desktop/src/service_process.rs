use std::io::{self, Read};
use std::process::{Child, Command, Stdio};

const DAEMON_FLAG: &str = "--neoism-internal-workspace-daemon";

pub(crate) struct ServiceProcess {
    child: Child,
}

impl Drop for ServiceProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub(crate) fn spawn_daemon() -> io::Result<ServiceProcess> {
    spawn(&[DAEMON_FLAG])
}

fn spawn(arguments: &[&str]) -> io::Result<ServiceProcess> {
    let executable = std::env::current_exe()?;
    let mut command = Command::new(executable);
    command
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(
            windows_sys::Win32::System::Threading::CREATE_NO_WINDOW
                | windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP,
        );
    }
    Ok(ServiceProcess {
        child: command.spawn()?,
    })
}

pub(crate) fn maybe_run_internal_service(
) -> Option<Result<(), Box<dyn std::error::Error>>> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    match arguments.first().map(String::as_str) {
        Some(DAEMON_FLAG) => Some(run_daemon()),
        _ => None,
    }
}

fn run_daemon() -> Result<(), Box<dyn std::error::Error>> {
    let _daemon = crate::embedded_daemon::EmbeddedDaemonHandle::spawn()?;
    let mut input = io::stdin().lock();
    let mut buffer = [0_u8; 64];
    while input.read(&mut buffer)? != 0 {}
    Ok(())
}

fn exit_when_parent_closes_stdin() {
    std::thread::Builder::new()
        .name("neoism-service-parent-watch".to_string())
        .spawn(|| {
            let mut input = io::stdin().lock();
            let mut buffer = [0_u8; 64];
            loop {
                match input.read(&mut buffer) {
                    Ok(0) | Err(_) => std::process::exit(0),
                    Ok(_) => {}
                }
            }
        })
        .expect("failed to start service parent watcher");
}
