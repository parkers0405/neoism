#[cfg(any(unix, windows))]
use neoism_backend::event::{EventProxy, RioEvent, RioEventType};
#[cfg(unix)]
use std::fs;
#[cfg(any(unix, windows))]
use std::io;
#[cfg(unix)]
use std::io::{BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(any(unix, windows))]
use std::path::Path;
#[cfg(any(unix, windows))]
use std::path::PathBuf;
#[cfg(any(unix, windows))]
use std::time::Duration;

use neoism_backend::event::InviteActivationToken;

pub const NEW_WINDOW_ARG: &str = "--new-window";
// On unix this overrides the socket path; on Windows it overrides the full
// pipe name (both are used by tests to isolate instances).
#[cfg(any(unix, windows))]
const IPC_SOCKET_ENV: &str = "NEOISM_IPC_SOCKET";

#[derive(Debug)]
pub struct ExternalCommandListener {
    #[cfg(unix)]
    path: PathBuf,
}

impl Drop for ExternalCommandListener {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            let _ = fs::remove_file(&self.path);
        }
        // Windows needs no cleanup: the pipe name vanishes when the last
        // server instance handle closes with the process.
    }
}

#[cfg(any(unix, windows))]
#[derive(Clone, serde::Serialize, serde::Deserialize, Eq, PartialEq)]
enum ExternalCommand {
    OpenInviteUris {
        uris: Vec<String>,
        window: Option<(Option<PathBuf>, Vec<PathBuf>)>,
        #[serde(default)]
        activation: Option<InviteActivationToken>,
    },
    NewWindow {
        working_dir: Option<PathBuf>,
        open_paths: Vec<PathBuf>,
    },
}

#[cfg(any(unix, windows))]
impl ExternalCommand {
    fn parse(line: &str) -> Option<Self> {
        let line = line.trim_end_matches(['\r', '\n']);
        if let Some(json) = line.strip_prefix("open-invite-uris\t") {
            let command: Self = serde_json::from_str(json).ok()?;
            return matches!(command, Self::OpenInviteUris { .. }).then_some(command);
        }
        let Some(rest) = line.strip_prefix("new-window") else {
            return None;
        };
        if rest.is_empty() {
            return Some(Self::NewWindow {
                working_dir: None,
                open_paths: Vec::new(),
            });
        }
        let encoded = rest.strip_prefix('\t')?;
        let mut fields = encoded.split('\t');
        let working_dir = match fields.next()? {
            "" => None,
            encoded => percent_decode(encoded).map(PathBuf::from),
        };
        let open_paths = fields
            .map(|encoded| percent_decode(encoded).map(PathBuf::from))
            .collect::<Option<Vec<_>>>()?;
        Some(Self::NewWindow {
            working_dir,
            open_paths,
        })
    }

    fn wire_name(self) -> String {
        match self {
            Self::OpenInviteUris { .. } => format!(
                "open-invite-uris\t{}",
                serde_json::to_string(&self).expect("IPC serialization")
            ),
            Self::NewWindow {
                working_dir,
                open_paths,
            } => {
                if working_dir.is_none() && open_paths.is_empty() {
                    return "new-window".to_string();
                }

                let mut encoded = String::from("new-window\t");
                if let Some(path) = working_dir {
                    encoded.push_str(&percent_encode(&path.to_string_lossy()));
                }
                for path in open_paths {
                    encoded.push('\t');
                    encoded.push_str(&percent_encode(&path.to_string_lossy()));
                }
                encoded
            }
        }
    }
}

#[cfg(any(unix, windows))]
impl std::fmt::Debug for ExternalCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OpenInviteUris { uris, .. } => f
                .debug_struct("OpenInviteUris")
                .field("count", &uris.len())
                .finish(),
            Self::NewWindow {
                working_dir,
                open_paths,
            } => f
                .debug_struct("NewWindow")
                .field("working_dir", working_dir)
                .field("open_paths", open_paths)
                .finish(),
        }
    }
}

/// Deliver opaque URIs, optionally preserving an accompanying new-window/file request.
pub fn request_open_invite_uris(
    uris: Vec<String>,
    window: Option<(Option<std::path::PathBuf>, Vec<std::path::PathBuf>)>,
    activation: Option<InviteActivationToken>,
) -> std::io::Result<bool> {
    #[cfg(any(unix, windows))]
    {
        request_command(ExternalCommand::OpenInviteUris {
            uris,
            window,
            activation,
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (uris, window, activation);
        Ok(false)
    }
}

#[cfg(any(unix, windows))]
fn dispatch_invite_command(
    event_proxy: &EventProxy,
    uris: Vec<String>,
    window: Option<(Option<PathBuf>, Vec<PathBuf>)>,
    activation: Option<InviteActivationToken>,
) {
    let id = unsafe { neoism_window::window::WindowId::dummy() };
    if let Some((working_dir, open_paths)) = window {
        event_proxy.send_event(
            RioEventType::Rio(RioEvent::CreateWindowWithOptions {
                working_dir,
                open_paths,
            }),
            id,
        );
    }
    event_proxy.send_event(
        RioEventType::Rio(RioEvent::OpenInviteUris { uris, activation }),
        id,
    );
}

#[cfg(any(unix, windows))]
pub fn request_new_window_with_options(
    working_dir: Option<PathBuf>,
    open_paths: Vec<PathBuf>,
) -> io::Result<bool> {
    request_command(ExternalCommand::NewWindow {
        working_dir,
        open_paths,
    })
}

#[cfg(not(any(unix, windows)))]
pub fn request_new_window_with_options(
    _working_dir: Option<std::path::PathBuf>,
    _open_paths: Vec<std::path::PathBuf>,
) -> std::io::Result<bool> {
    Ok(false)
}

#[cfg(unix)]
pub fn listen_for_external_commands(
    event_proxy: EventProxy,
) -> Option<ExternalCommandListener> {
    let path = socket_path();
    let listener = match bind_socket(&path) {
        Ok(listener) => listener,
        Err(err) => {
            tracing::debug!(
                path = %path.display(),
                "external command listener disabled: {err}"
            );
            return None;
        }
    };

    let thread_path = path.clone();
    let spawn_result = std::thread::Builder::new()
        .name("neoism-ipc".to_string())
        .spawn(move || listen_loop(listener, event_proxy));

    if let Err(err) = spawn_result {
        tracing::warn!(
            path = %thread_path.display(),
            "failed to spawn external command listener: {err}"
        );
        let _ = fs::remove_file(&thread_path);
        return None;
    }

    Some(ExternalCommandListener { path })
}

#[cfg(windows)]
pub fn listen_for_external_commands(
    event_proxy: EventProxy,
) -> Option<ExternalCommandListener> {
    use tokio::net::windows::named_pipe::ServerOptions;

    let pipe = pipe_name();
    // Named pipes are async-only in tokio; the listener thread drives its own
    // single-threaded runtime, mirroring the dedicated unix accept thread.
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            tracing::debug!(pipe = %pipe, "external command listener disabled: {err}");
            return None;
        }
    };

    // `first_pipe_instance` makes creation fail when another Neoism process
    // already owns the name — the same "someone is already listening" probe
    // bind_socket performs on unix.
    let first_instance = {
        let _guard = runtime.enter();
        ServerOptions::new().first_pipe_instance(true).create(&pipe)
    };
    let first_instance = match first_instance {
        Ok(instance) => instance,
        Err(err) => {
            tracing::debug!(pipe = %pipe, "external command listener disabled: {err}");
            return None;
        }
    };

    let thread_pipe = pipe.clone();
    let spawn_result = std::thread::Builder::new()
        .name("neoism-ipc".to_string())
        .spawn(move || runtime.block_on(listen_loop(pipe, first_instance, event_proxy)));

    if let Err(err) = spawn_result {
        tracing::warn!(
            pipe = %thread_pipe,
            "failed to spawn external command listener: {err}"
        );
        return None;
    }

    Some(ExternalCommandListener {})
}

#[cfg(not(any(unix, windows)))]
pub fn listen_for_external_commands(
    _event_proxy: neoism_backend::event::EventProxy,
) -> Option<ExternalCommandListener> {
    None
}

#[cfg(unix)]
fn request_command(command: ExternalCommand) -> io::Result<bool> {
    let path = socket_path();
    let mut stream = match UnixStream::connect(&path) {
        Ok(stream) => stream,
        Err(err) if is_missing_listener(&err) => return Ok(false),
        Err(err) => return Err(err),
    };

    stream.set_read_timeout(Some(Duration::from_millis(500)))?;
    stream.set_write_timeout(Some(Duration::from_millis(500)))?;
    stream.write_all(command.wire_name().as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    let mut response = String::new();
    let mut reader = BufReader::new(stream);
    reader.read_line(&mut response)?;
    match response.trim_end_matches(['\r', '\n']) {
        "ok" => Ok(true),
        other => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unexpected Neoism IPC response: {other}"),
        )),
    }
}

#[cfg(windows)]
fn allow_invite_server_foreground(
    client: &tokio::net::windows::named_pipe::NamedPipeClient,
) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;
    use windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;

    // Only authorize the peer of the connected pipe, never a PID supplied by
    // argv or the URL. Keep the client alive while querying its borrowed handle.
    // Foreground permission is best-effort: Windows can still deny the grant.
    unsafe {
        let mut server_pid = 0;
        if GetNamedPipeServerProcessId(client.as_raw_handle().cast(), &mut server_pid)
            != 0
            && server_pid != 0
            && server_pid != u32::MAX
        {
            let _ = AllowSetForegroundWindow(server_pid);
        }
    }
}

#[cfg(windows)]
fn request_command(command: ExternalCommand) -> io::Result<bool> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::windows::named_pipe::ClientOptions;
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY};

    let pipe = pipe_name();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        // ERROR_PIPE_BUSY means every instance is mid-accept; the listener
        // stands up a fresh instance right after each connect, so retry
        // briefly before concluding nobody is listening.
        let deadline = std::time::Instant::now() + Duration::from_millis(500);
        let mut client = loop {
            match ClientOptions::new().open(&pipe) {
                Ok(client) => break client,
                Err(err) if err.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) => {
                    return Ok(false);
                }
                Err(err) if err.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                    if std::time::Instant::now() >= deadline {
                        return Ok(false);
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(err) => return Err(err),
            }
        };

        // The launcher owns the browser's foreground gesture. Transfer its
        // permission before sending bytes that can dispatch the invite event.
        if matches!(&command, ExternalCommand::OpenInviteUris { .. }) {
            allow_invite_server_foreground(&client);
        }
        let mut request = command.wire_name().into_bytes();
        request.push(b'\n');
        tokio::time::timeout(Duration::from_millis(500), client.write_all(&request))
            .await
            .map_err(|_| {
                io::Error::new(io::ErrorKind::TimedOut, "Neoism IPC write timed out")
            })??;

        let response = tokio::time::timeout(Duration::from_millis(500), async {
            let mut response = Vec::new();
            let mut buf = [0u8; 64];
            loop {
                let read = client.read(&mut buf).await?;
                if read == 0 {
                    break;
                }
                response.extend_from_slice(&buf[..read]);
                if response.contains(&b'\n') {
                    break;
                }
            }
            io::Result::Ok(response)
        })
        .await
        .map_err(|_| {
            io::Error::new(io::ErrorKind::TimedOut, "Neoism IPC response timed out")
        })??;

        let response = String::from_utf8_lossy(&response);
        match response.trim_end_matches(['\r', '\n']) {
            "ok" => Ok(true),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unexpected Neoism IPC response: {other}"),
            )),
        }
    })
}

#[cfg(unix)]
fn listen_loop(listener: UnixListener, event_proxy: EventProxy) {
    for incoming in listener.incoming() {
        match incoming {
            Ok(mut stream) => {
                if let Err(err) = handle_stream(&mut stream, &event_proxy) {
                    tracing::debug!("external command failed: {err}");
                }
            }
            Err(err) => {
                tracing::debug!("external command listener accept failed: {err}");
            }
        }
    }
}

#[cfg(windows)]
async fn listen_loop(
    pipe: String,
    mut server: tokio::net::windows::named_pipe::NamedPipeServer,
    event_proxy: EventProxy,
) {
    use tokio::net::windows::named_pipe::ServerOptions;

    loop {
        if let Err(err) = server.connect().await {
            tracing::debug!("external command listener accept failed: {err}");
            // A failed ConnectNamedPipe can leave the instance unusable;
            // replace it before waiting again.
            match ServerOptions::new().create(&pipe) {
                Ok(next) => server = next,
                Err(err) => {
                    tracing::debug!("external command listener disabled: {err}");
                    return;
                }
            }
            continue;
        }
        // Stand up the next instance before serving this connection so a
        // burst of clients never finds the pipe name unbound.
        let mut stream = match ServerOptions::new().create(&pipe) {
            Ok(next) => std::mem::replace(&mut server, next),
            Err(err) => {
                tracing::debug!(
                    "external command listener replacement instance failed: {err}"
                );
                let mut stream = server;
                if let Err(err) = handle_stream(&mut stream, &event_proxy).await {
                    tracing::debug!("external command failed: {err}");
                }
                return;
            }
        };
        // Commands are handled serially, matching the unix accept loop.
        if let Err(err) = handle_stream(&mut stream, &event_proxy).await {
            tracing::debug!("external command failed: {err}");
        }
        // Dropping `stream` disconnects the served instance; buffered response
        // bytes stay readable by the client until it drains them.
    }
}

#[cfg(unix)]
fn handle_stream(stream: &mut UnixStream, event_proxy: &EventProxy) -> io::Result<()> {
    let mut line = String::new();
    {
        let mut reader = BufReader::new(&mut *stream);
        reader.read_line(&mut line)?;
    }

    match ExternalCommand::parse(&line) {
        Some(ExternalCommand::NewWindow {
            working_dir,
            open_paths,
        }) => {
            event_proxy.send_event(
                RioEventType::Rio(RioEvent::CreateWindowWithOptions {
                    working_dir,
                    open_paths,
                }),
                unsafe { neoism_window::window::WindowId::dummy() },
            );
            stream.write_all(b"ok\n")?;
        }
        Some(ExternalCommand::OpenInviteUris {
            uris,
            window,
            activation,
        }) => {
            dispatch_invite_command(event_proxy, uris, window, activation);
            stream.write_all(b"ok\n")?;
        }
        None => {
            stream.write_all(b"err unknown-command\n")?;
        }
    }
    stream.flush()
}

#[cfg(windows)]
async fn handle_stream(
    stream: &mut tokio::net::windows::named_pipe::NamedPipeServer,
    event_proxy: &EventProxy,
) -> io::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let mut line = String::new();
    {
        let mut reader = BufReader::new(&mut *stream);
        reader.read_line(&mut line).await?;
    }

    match ExternalCommand::parse(&line) {
        Some(ExternalCommand::NewWindow {
            working_dir,
            open_paths,
        }) => {
            event_proxy.send_event(
                RioEventType::Rio(RioEvent::CreateWindowWithOptions {
                    working_dir,
                    open_paths,
                }),
                unsafe { neoism_window::window::WindowId::dummy() },
            );
            stream.write_all(b"ok\n").await?;
        }
        Some(ExternalCommand::OpenInviteUris {
            uris,
            window,
            activation,
        }) => {
            dispatch_invite_command(event_proxy, uris, window, activation);
            stream.write_all(b"ok\n").await?;
        }
        None => {
            stream.write_all(b"err unknown-command\n").await?;
        }
    }
    stream.flush().await
}

#[cfg(any(unix, windows))]
fn percent_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'.' | b'-' | b'_' => {
                encoded.push(byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

#[cfg(any(unix, windows))]
fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return None;
            }
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok()?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

#[cfg(unix)]
fn bind_socket(path: &Path) -> io::Result<UnixListener> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }

    if path.exists() {
        match UnixStream::connect(path) {
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "another Neoism process is already listening",
                ));
            }
            Err(err) if is_missing_listener(&err) => {
                let _ = fs::remove_file(path);
            }
            Err(err) => return Err(err),
        }
    }

    UnixListener::bind(path)
}

#[cfg(unix)]
fn socket_path() -> PathBuf {
    if let Some(socket) = std::env::var_os(IPC_SOCKET_ENV) {
        let path = PathBuf::from(socket);
        if !path.as_os_str().is_empty() {
            return path;
        }
    }

    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!("neoism-{}", current_uid()))
        });
    let install = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .map(|path| std::fs::canonicalize(&path).unwrap_or(path))
        .unwrap_or_default();
    let mut install_hash = 0xcbf29ce484222325u64;
    for byte in install.as_os_str().as_encoded_bytes() {
        install_hash ^= u64::from(*byte);
        install_hash = install_hash.wrapping_mul(0x100000001b3);
    }
    base.join("neoism")
        .join(format!("command-{install_hash:016x}.sock"))
}

#[cfg(windows)]
fn pipe_name() -> String {
    use std::os::windows::ffi::OsStrExt;

    if let Ok(name) = std::env::var(IPC_SOCKET_ENV) {
        if !name.is_empty() {
            return name;
        }
    }

    // The pipe namespace is machine-global, so scope by user the way the unix
    // socket path scopes by uid. Percent-encoding keeps the name legal (pipe
    // names may not contain backslashes) whatever the username holds.
    let user = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "default".to_string());
    let domain = std::env::var("USERDOMAIN").unwrap_or_default();
    let install = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .map(|path| dunce::canonicalize(&path).unwrap_or(path))
        .unwrap_or_default();

    // FNV-1a is sufficient here: the hash only keeps independent portable,
    // stable, and development installs from sharing a pipe name.
    let mut install_hash = 0xcbf29ce484222325u64;
    for word in install.as_os_str().encode_wide() {
        for byte in word.to_le_bytes() {
            install_hash ^= u64::from(byte);
            install_hash = install_hash.wrapping_mul(0x100000001b3);
        }
    }
    format!(
        r"\\.\pipe\neoism-command-{}-{:016x}",
        percent_encode(&format!("{domain}\\{user}")),
        install_hash
    )
}

#[cfg(unix)]
fn current_uid() -> u32 {
    unsafe { libc::geteuid() }
}

#[cfg(unix)]
fn is_missing_listener(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::NotFound
            | io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::NotConnected
    )
}

#[cfg(test)]
#[path = "app/invite_activation.rs"]
mod activation_api_compile;

#[cfg(all(test, any(unix, windows)))]
mod tests {
    use super::*;

    #[test]
    fn invite_protocol_and_serde_round_trip_without_logging_secrets() {
        let command = ExternalCommand::OpenInviteUris {
            activation: InviteActivationToken::new("browser-secret-token".into()),
            uris: vec![
                "neoism://join?invite=https%3A%2F%2Ffast.example%2Fx%23secret".into(),
                "NEOISM:invalid\n\tsecret".into(),
            ],
            window: Some((
                Some(PathBuf::from("/tmp/work")),
                vec![PathBuf::from("/tmp/a.md")],
            )),
        };
        let wire = command.clone().wire_name();
        assert!(!wire.contains('\n'));
        assert_eq!(
            ExternalCommand::parse(&format!("{wire}\n")),
            Some(command.clone())
        );
        assert_eq!(
            serde_json::from_str::<ExternalCommand>(
                &serde_json::to_string(&command).unwrap()
            )
            .unwrap(),
            command
        );
        assert!(!format!("{command:?}").contains("secret"));
        assert!(!format!(
            "{:?}",
            RioEvent::OpenInviteUris {
                uris: vec!["secret".into()],
                activation: InviteActivationToken::new("browser-secret-token".into()),
            }
        )
        .contains("secret"));
        assert!(ExternalCommand::parse("open-invite-uris\t{bad}").is_none());
    }

    #[test]
    fn invite_activation_is_bounded_redacted_and_backwards_compatible() {
        assert!(InviteActivationToken::new(String::new()).is_none());
        assert!(InviteActivationToken::new("a".repeat(4097)).is_none());
        assert!(InviteActivationToken::new("opaque\nsecret".into()).is_none());
        assert!(InviteActivationToken::new("a".repeat(4096)).is_some());
        let token = InviteActivationToken::new("opaque-secret".into()).unwrap();
        assert!(!format!("{token:?}").contains("opaque-secret"));
        assert_eq!(serde_json::to_string(&token).unwrap(), "\"opaque-secret\"");
        for value in [String::new(), "x".repeat(4097), "x\0y".into()] {
            let json = serde_json::json!({"OpenInviteUris": {
                "uris": ["neoism:invalid"], "window": null, "activation": value
            }});
            assert!(
                ExternalCommand::parse(&format!("open-invite-uris\t{json}")).is_none()
            );
        }
        let legacy = "open-invite-uris\t{\"OpenInviteUris\":{\"uris\":[\"neoism:invalid\"],\"window\":null}}";
        assert!(matches!(
            ExternalCommand::parse(legacy),
            Some(ExternalCommand::OpenInviteUris {
                activation: None,
                ..
            })
        ));
    }

    #[test]
    fn parses_new_window_command() {
        assert_eq!(
            ExternalCommand::parse("new-window\n"),
            Some(ExternalCommand::NewWindow {
                working_dir: None,
                open_paths: Vec::new()
            })
        );
        assert_eq!(
            ExternalCommand::parse("new-window\r\n"),
            Some(ExternalCommand::NewWindow {
                working_dir: None,
                open_paths: Vec::new()
            })
        );
        assert_eq!(ExternalCommand::parse("open\n"), None);
    }

    #[test]
    fn parses_new_window_command_with_working_dir() {
        assert_eq!(
            ExternalCommand::parse("new-window\t/tmp/neoism%20bench\n"),
            Some(ExternalCommand::NewWindow {
                working_dir: Some(PathBuf::from("/tmp/neoism bench")),
                open_paths: Vec::new()
            })
        );
    }

    #[test]
    fn parses_new_window_command_with_open_paths() {
        assert_eq!(
            ExternalCommand::parse(
                "new-window\t/tmp/repo\t/tmp/repo/a%20b.md\t/tmp/c.rs\n"
            ),
            Some(ExternalCommand::NewWindow {
                working_dir: Some(PathBuf::from("/tmp/repo")),
                open_paths: vec![
                    PathBuf::from("/tmp/repo/a b.md"),
                    PathBuf::from("/tmp/c.rs"),
                ],
            })
        );
        assert_eq!(
            ExternalCommand::parse("new-window\t\t/tmp/repo/a.md\n"),
            Some(ExternalCommand::NewWindow {
                working_dir: None,
                open_paths: vec![PathBuf::from("/tmp/repo/a.md")],
            })
        );
    }

    #[test]
    fn wire_format_round_trips() {
        let command = ExternalCommand::NewWindow {
            working_dir: Some(PathBuf::from("/tmp/neoism bench")),
            open_paths: vec![
                PathBuf::from("/tmp/repo/a b.md"),
                PathBuf::from("/tmp/c.rs"),
            ],
        };
        assert_eq!(
            ExternalCommand::parse(&command.clone().wire_name()),
            Some(command)
        );
    }
}
