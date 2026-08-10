use std::io::{self, Read};
use std::process::{Child, Command, Stdio};

const AGENT_FLAG: &str = "--neoism-internal-agent-server";
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

pub(crate) fn spawn_agent(hostname: &str, port: u16) -> io::Result<ServiceProcess> {
    spawn(&[AGENT_FLAG, hostname, &port.to_string()])
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
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    Ok(ServiceProcess {
        child: command.spawn()?,
    })
}

pub(crate) fn maybe_run_internal_service(
) -> Option<Result<(), Box<dyn std::error::Error>>> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    match arguments.first().map(String::as_str) {
        Some(AGENT_FLAG) => Some(run_agent(&arguments)),
        Some(DAEMON_FLAG) => Some(run_daemon()),
        _ => None,
    }
}

fn run_agent(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let hostname = arguments
        .get(1)
        .cloned()
        .ok_or("internal agent service is missing hostname")?;
    let port = arguments
        .get(2)
        .ok_or("internal agent service is missing port")?
        .parse::<u16>()?;
    exit_when_parent_closes_stdin();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .thread_name("neoism-agent-service")
        .build()?;
    runtime.block_on(neoism_agent_server::listen(
        neoism_agent_server::ServerOptions {
            hostname,
            port,
            cors: Vec::new(),
        },
    ))?;
    Ok(())
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
