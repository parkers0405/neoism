//! Host process readiness matches the original desktop; never dial an arbitrary port owner.
use std::{io::{self,BufRead,Read},process::{Command,Stdio},sync::mpsc,time::{Duration,Instant},path::PathBuf};
const BOOT_TIMEOUT:Duration=Duration::from_secs(120);
const HOSTED_READINESS_NONCE_ENV:&str="NEOISM_HOSTED_READINESS_NONCE";
const HOSTED_READINESS_PROTOCOL:&str="neoism-hosted-ready-v1";
#[derive(serde::Deserialize)]
struct StartupAcknowledgement{protocol:String,nonce:String,address:std::net::SocketAddr}
pub(crate) fn standalone_executable()->io::Result<PathBuf>{
 if let Some(path)=std::env::var_os("NEOISM_GPUIX_WORKSPACE_DAEMON"){let path=PathBuf::from(path);if path.is_file(){return Ok(path);}return Err(io::Error::new(io::ErrorKind::NotFound,"Configured workspace daemon does not exist"));}
 let beside=std::env::current_exe()?.parent().unwrap().join(if cfg!(windows){"neoism-workspace-daemon.exe"}else{"neoism-workspace-daemon"});
 if beside.is_file(){return Ok(beside);}
 for directory in ["../../../target/activation", "../artifacts"] {
  let development=PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(directory).join(if cfg!(windows){"neoism-workspace-daemon.exe"}else{"neoism-workspace-daemon"});
  if development.is_file(){return Ok(development);}
 }
 Err(io::Error::new(io::ErrorKind::NotFound,"The packaged workspace daemon is missing"))
}
pub(crate) fn launch_persistent_host(
    command: &mut Command,
    port: u16,
    secret: Option<&str>,
    commit: impl FnOnce() -> io::Result<()>,
) -> io::Result<std::net::SocketAddr> {
    use std::io::{Seek, SeekFrom};
    let nonce = uuid::Uuid::new_v4().to_string();
    // Anonymous file avoids pipe deadlock and keeps daemon logs available even
    // after the GUI exits. Read only a bounded tail on startup failure.
    let mut diagnostics = tempfile::tempfile()?;
    command.env(HOSTED_READINESS_NONCE_ENV, &nonce)
        .stdin(Stdio::null()).stdout(Stdio::piped())
        .stderr(Stdio::from(diagnostics.try_clone()?));
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().expect("host readiness stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let result = io::BufReader::new(stdout).take(4096).read_line(&mut line)
            .map(|_| line);
        let _ = tx.send(result);
    });
    let deadline = Instant::now() + BOOT_TIMEOUT;
    let outcome = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Err(format!("server exited during startup ({status})")),
            Err(error) => break Err(format!("could not monitor server: {error}")),
            Ok(None) => {}
        }
        match rx.recv_timeout(Duration::from_millis(25)) {
            Ok(Ok(line)) => {
                let ack = serde_json::from_str::<StartupAcknowledgement>(&line);
                match ack {
                    Ok(ack) if line.ends_with('\n') && line.len() < 4096
                        && ack.protocol == HOSTED_READINESS_PROTOCOL
                        && ack.nonce == nonce
                        && ack.address.ip() == std::net::Ipv4Addr::UNSPECIFIED
                        && ack.address.port() == port => {
                            match child.try_wait() {
                                Ok(None) => break Ok(ack.address),
                                Ok(Some(status)) => break Err(format!("server exited at readiness ({status})")),
                                Err(error) => break Err(format!("could not monitor ready server: {error}")),
                            }
                        },
                    _ => break Err("server returned invalid persistent-host readiness".into()),
                }
            }
            Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Disconnected) =>
                break Err("server closed its readiness stream before startup completed".into()),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if Instant::now() >= deadline {
            break Err("server startup exceeded 120 seconds".into());
        }
    };
    // Metadata must commit before ownership is relinquished or UI sees success.
    // On failure terminate only this still-owned child, never a port owner.
    let outcome = outcome.and_then(|address| commit().map(|_| address)
        .map_err(|error| format!("could not persist hosted relaunch metadata: {error}")));
    match outcome {
        Ok(address) => {
            // Reap without tying persistent lifetime to the desktop owner.
            std::thread::spawn(move || { let _ = child.wait(); });
            Ok(address)
        }
        Err(reason) => {
            let _ = child.kill();
            let _ = child.wait();
            let len = diagnostics.metadata().map(|m| m.len()).unwrap_or(0);
            let _ = diagnostics.seek(SeekFrom::Start(len.saturating_sub(4096)));
            let mut tail = String::new();
            let _ = diagnostics.take(4096).read_to_string(&mut tail);
            // No ANSI/control injection or configured master password in UI.
            let tail = if let Some(secret) = secret.filter(|s| !s.is_empty()) {
                tail.replace(secret, "[redacted]")
            } else { tail };
            let tail: String = tail.chars().filter(|c| !c.is_control() || *c == '\n').collect();
            Err(io::Error::other(format!("{reason}: {}", tail.trim())))
        }
    }
}
