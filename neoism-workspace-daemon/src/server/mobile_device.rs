//! An opt-in TLS listener that serves only the paired-device chat plane.

use super::{extract_bearer, socket::ServiceServerMessage};
use crate::agent::{self, mobile::MobileAgent};
use axum::{
    extract::{ws::{Message, WebSocket, WebSocketUpgrade}, ConnectInfo, DefaultBodyLimit, Path, State},
    http::{header, HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hyper::server::conn::http1;
use hyper_util::{rt::TokioIo, service::TowerToHyperService};
use neoism_protocol::{agent::{AgentClientMessage, AgentServerMessage, MOBILE_CHAT_MAX_MESSAGE_BYTES}, workspace::WorkspaceServerMessage};
use parking_lot::Mutex;
use rand::{rngs::OsRng, RngCore};
use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs, io, path::{Path as FsPath, PathBuf}};
use std::{collections::HashMap, net::{IpAddr, SocketAddr}, sync::Arc, time::{Duration, Instant}};
use subtle::ConstantTimeEq;
use tokio::{net::TcpListener, sync::{watch, Semaphore}, task::JoinHandle, time::timeout};
use url::Url;
use uuid::Uuid;

#[cfg(target_os = "macos")]
mod provision;
mod quick;

const INVITE_TTL: Duration = Duration::from_secs(60);
const MAX_INVITES: usize = 16;
const MAX_DEVICES: usize = 64;
const MAX_PASSWORD_ATTEMPTS: u8 = 5;

type Digest32 = [u8; 32];

#[derive(Clone)]
struct MobileState {
    ca: Option<String>,
    origin: Url,
    name: String,
    operator_key: Digest32,
    agent_endpoint: String,
    credentials: Arc<Mutex<Credentials>>,
    sessions: Arc<Semaphore>,
}

#[cfg_attr(test, derive(Default))]
struct Credentials {
    invites: HashMap<Digest32, Invite>,
    devices: HashMap<Digest32, Device>,
    // Only isolated router tests run without backing storage.
    store: Option<RegistryStore>,
}

struct Invite {
    expires: Instant,
    password: Digest32,
    remaining_attempts: u8,
}

struct Device {
    info: DeviceInfo,
    // Dropping the sender terminates all active sessions for this device.
    // None while a failed durable revoke awaits retry. Keep its ID listable,
    // but neither existing nor new sessions may use this credential.
    alive: Option<watch::Sender<()>>,
}

#[derive(Clone, Deserialize, Serialize)]
struct DeviceInfo {
    device_id: Uuid,
    device_label: String,
}

// This is the entire on-disk schema. Invites, passwords, plaintext bearers,
// and session handles have no representation here.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredDevice {
    token_hash: Digest32,
    info: DeviceInfo,
}

const REGISTRY_FILE: &str = "devices-v1.json";

struct RegistryStore {
    dir: PathBuf,
    // One live authoritative device map, even across multiple daemon processes.
    // Lock a stable sibling, not the inode replaced on each write.
    lock: fs::File,
}

impl Drop for RegistryStore {
    fn drop(&mut self) {
        let _ = self.lock.unlock();
    }
}

fn private_options() -> fs::OpenOptions {
    let mut opts = fs::OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    opts
}

fn check_private(path: &FsPath, directory: bool) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    anyhow::ensure!(if directory { metadata.is_dir() } else { metadata.is_file() }, "unexpected mobile registry file type");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        anyhow::ensure!(metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o077 == 0,
            "mobile registry requires owner-only permissions");
    }
    #[cfg(windows)]
    anyhow::ensure!(crate::windows_acl::check_owner_only(path)?, "mobile registry requires owner-only permissions");
    Ok(())
}

impl RegistryStore {
    fn open(root: &FsPath) -> anyhow::Result<Self> {
        anyhow::ensure!(root.is_absolute(), "mobile registry requires an absolute user state directory");
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.recursive(true).create(root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let meta = fs::symlink_metadata(root)?;
            anyhow::ensure!(meta.is_dir() && meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o022 == 0,
                "mobile registry state root must be user-owned and not writable by others");
        }
        let dir = root.join("mobile-device");
        match builder.recursive(false).create(&dir) {
            Ok(()) => {
                #[cfg(unix)]
                fs::File::open(root)?.sync_all()?;
                #[cfg(windows)]
                crate::windows_acl::harden_owner_only(&dir)?;
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => (),
            Err(err) => return Err(err.into()),
        }
        check_private(&dir, true)?;
        let lock_path = dir.join("registry.lock");
        let lock = private_options().read(true).write(true).create(true).truncate(false).open(&lock_path)?;
        #[cfg(windows)]
        crate::windows_acl::harden_owner_only(&lock_path)?;
        check_private(&lock_path, false)?;
        lock.try_lock().map_err(|e| anyhow::anyhow!("cannot lock mobile device registry: {e}"))?;
        Ok(Self { dir, lock })
    }

    fn load(&self) -> anyhow::Result<Vec<StoredDevice>> {
        let path = self.dir.join(REGISTRY_FILE);
        match fs::symlink_metadata(&path) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            result => { result?; }
        }
        check_private(&path, false)?;
        let file = private_options().read(true).open(path)?;
        anyhow::ensure!(file.metadata()?.len() <= 128 * 1024, "oversized mobile registry");
        let devices: Vec<StoredDevice> = serde_json::from_reader(file)?;
        anyhow::ensure!(devices.len() <= MAX_DEVICES, "too many stored mobile devices");
        Ok(devices)
    }

    fn replace(&self, devices: &[StoredDevice]) -> io::Result<()> {
        let tmp = self.dir.join(format!(".{}.tmp", Uuid::new_v4()));
        let file = private_options().write(true).create_new(true).open(&tmp)?;
        let result = (|| {
            #[cfg(windows)]
            crate::windows_acl::harden_owner_only(&tmp)?;
            serde_json::to_writer(&file, devices)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&tmp, self.dir.join(REGISTRY_FILE))?;
            #[cfg(unix)]
            fs::File::open(&self.dir)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() { let _ = fs::remove_file(tmp); }
        result
    }
}

fn write_error(err: io::Error) -> StatusCode {
    tracing::error!(%err, "could not commit mobile device registry");
    StatusCode::INTERNAL_SERVER_ERROR
}

fn digest(value: &str) -> Digest32 {
    Sha256::digest(value.as_bytes()).into()
}

fn password_digest(code: &str, password: &str) -> Digest32 {
    // The opaque code is a fresh 256-bit salt. Only digests are retained, in
    // memory; guessing with a live code is bounded separately under the lock.
    let mut hash = Sha256::new();
    hash.update(code.as_bytes());
    hash.update(password.as_bytes());
    hash.finalize().into()
}

fn secret(prefix: &str) -> String {
    let mut bytes = [0; 32];
    OsRng.fill_bytes(&mut bytes);
    format!("{prefix}{}", URL_SAFE_NO_PAD.encode(bytes))
}

impl Credentials {
    fn open_default() -> anyhow::Result<Self> {
        Self::open(&crate::persistence::resolve_state_dir(None))
    }

    fn open(root: &FsPath) -> anyhow::Result<Self> {
        let store = RegistryStore::open(root)?;
        let mut devices = HashMap::new();
        let mut ids = std::collections::HashSet::new();
        for StoredDevice { token_hash, info } in store.load()? {
            anyhow::ensure!(ids.insert(info.device_id) && !devices.contains_key(&token_hash), "duplicate stored mobile device");
            devices.insert(token_hash, Device { info, alive: Some(watch::channel(()).0) });
        }
        Ok(Self { devices, invites: HashMap::new(), store: Some(store) })
    }

    fn persist_change(&self, add: Option<StoredDevice>) -> Result<(), StatusCode> {
        if let Some(store) = &self.store {
            let devices = self.devices.iter().filter(|(_, device)| device.alive.is_some())
                .map(|(hash, device)| StoredDevice { token_hash: *hash, info: device.info.clone() })
                .chain(add).collect::<Vec<_>>();
            store.replace(&devices).map_err(write_error)?;
        }
        Ok(())
    }

    fn mint(&mut self, password: &str, now: Instant) -> Result<String, StatusCode> {
        self.invites.retain(|_, invite| invite.expires > now);
        if self.invites.len() >= MAX_INVITES || self.devices.len() >= MAX_DEVICES {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        let code = secret("ndi_");
        self.invites.insert(digest(&code), Invite {
            expires: now + INVITE_TTL,
            password: password_digest(&code, password),
            remaining_attempts: MAX_PASSWORD_ATTEMPTS,
        });
        Ok(code)
    }

    fn claim(&mut self, code: &str, label: String, password: &str, now: Instant) -> Result<String, StatusCode> {
        self.invites.retain(|_, invite| invite.expires > now);
        let key = digest(code);
        let invite = self.invites.get_mut(&key).ok_or(StatusCode::UNAUTHORIZED)?;
        if !bool::from(invite.password.ct_eq(&password_digest(code, password))) {
            invite.remaining_attempts -= 1;
            if invite.remaining_attempts == 0 {
                self.invites.remove(&key);
            }
            return Err(StatusCode::UNAUTHORIZED);
        }
        if self.devices.len() >= MAX_DEVICES {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        let token = secret("ndm_");
        let token_hash = digest(&token);
        let info = DeviceInfo { device_id: Uuid::new_v4(), device_label: label };
        self.persist_change(Some(StoredDevice { token_hash, info: info.clone() }))?;
        self.invites.remove(&key);
        self.devices.insert(token_hash, Device { info, alive: Some(watch::channel(()).0) });
        Ok(token)
    }

    fn authenticate(&self, token: &str) -> Option<(String, watch::Receiver<()>)> {
        if !token.starts_with("ndm_") || token.len() != 47 {
            return None;
        }
        let device = self.devices.get(&digest(token))?;
        Some((device.info.device_label.clone(), device.alive.as_ref()?.subscribe()))
    }

    fn revoke(&mut self, id: Uuid) -> Result<bool, StatusCode> {
        let Some((&hash, device)) = self.devices.iter_mut().find(|(_, d)| d.info.device_id == id) else {
            return Ok(false);
        };
        device.alive = None;
        self.persist_change(None)?;
        self.devices.remove(&hash);
        Ok(true)
    }
}

fn operator_allowed(state: &MobileState, peer: Option<ConnectInfo<SocketAddr>>, headers: &HeaderMap) -> bool {
    // No forwarded headers are trusted. Unix sockets lack ConnectInfo and
    // still require the dedicated operator key, as does genuine loopback TCP.
    peer.is_none_or(|ConnectInfo(addr)| addr.ip().is_loopback())
        && !headers.contains_key(header::ORIGIN)
        && extract_bearer(headers).is_some_and(|key| bool::from(state.operator_key.ct_eq(&digest(&key))))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MintBody { password: String }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaimBody { code: String, device_label: String, #[serde(default)] password: String }

#[derive(Serialize)]
struct InviteReply { invite: String }

#[derive(Serialize)]
struct ClaimReply { token: String }

fn valid_password(password: &str) -> bool {
    !password.trim().is_empty() && password.len() <= 256 && !password.chars().any(char::is_control)
}

fn private_json(value: impl Serialize) -> Response {
    ([(header::CACHE_CONTROL, "no-store")], Json(value)).into_response()
}

async fn invite(State(state): State<MobileState>, peer: Option<ConnectInfo<SocketAddr>>, headers: HeaderMap, Json(body): Json<MintBody>) -> Result<Response, StatusCode> {
    if !operator_allowed(&state, peer, &headers) {
        return Err(StatusCode::FORBIDDEN);
    }
    if !valid_password(&body.password) {
        return Err(StatusCode::BAD_REQUEST);
    }
    mint_invite(&state, &body.password)
}

fn mint_invite(state: &MobileState, password: &str) -> Result<Response, StatusCode> {
    let code = state.credentials.lock().mint(password, Instant::now())?;
    let mut uri = Url::parse("neoism://device").expect("static invite URI");
    uri.query_pairs_mut()
        .append_pair("origin", state.origin.as_str().trim_end_matches('/'))
        .append_pair("code", &code)
        .append_pair("name", &state.name);
    if let Some(ca) = &state.ca { uri.query_pairs_mut().append_pair("ca", ca); }
    if password.is_empty() { uri.query_pairs_mut().append_pair("v", "2"); }
    Ok(private_json(InviteReply { invite: uri.into() }))
}

async fn claim(State(state): State<MobileState>, uri: Uri, Json(body): Json<ClaimBody>) -> Result<Response, StatusCode> {
    let label = body.device_label.trim();
    if uri.query().is_some() || (!body.password.is_empty() && !valid_password(&body.password)) || label.is_empty() || label.len() > 128 || label.chars().any(char::is_control) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let token = state.credentials.lock().claim(&body.code, label.into(), &body.password, Instant::now())?;
    Ok(private_json(ClaimReply { token }))
}

async fn devices(State(state): State<MobileState>, peer: Option<ConnectInfo<SocketAddr>>, headers: HeaderMap) -> Result<Response, StatusCode> {
    if !operator_allowed(&state, peer, &headers) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(private_json(state.credentials.lock().devices.values().map(|d| d.info.clone()).collect::<Vec<_>>()))
}

async fn revoke(State(state): State<MobileState>, peer: Option<ConnectInfo<SocketAddr>>, headers: HeaderMap, Path(id): Path<Uuid>) -> StatusCode {
    if !operator_allowed(&state, peer, &headers) {
        return StatusCode::FORBIDDEN;
    }
    match state.credentials.lock().revoke(id) {
        Ok(true) => StatusCode::NO_CONTENT,
        Ok(false) => StatusCode::NOT_FOUND,
        Err(status) => status,
    }
}

async fn upgrade(State(state): State<MobileState>, uri: Uri, headers: HeaderMap, ws: WebSocketUpgrade) -> Result<Response, StatusCode> {
    if uri.query().is_some() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let (label, alive) = extract_bearer(&headers)
        .and_then(|token| state.credentials.lock().authenticate(&token))
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let permit = state.sessions.clone().try_acquire_owned().map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(ws.max_frame_size(MOBILE_CHAT_MAX_MESSAGE_BYTES).max_message_size(MOBILE_CHAT_MAX_MESSAGE_BYTES).on_upgrade(move |socket| async move {
        let _permit = permit;
        let mut alive = alive;
        tokio::select! {
            biased;
            _ = alive.changed() => {},
            _ = session(socket, state.agent_endpoint, label) => {},
        }
    }))
}

/// A separate input enum keeps all non-chat services out of this dispatch.
#[derive(Deserialize)]
enum Input {
    Workspace { #[serde(default)] request_id: u64, message: WorkspaceInput },
    Agent { request_id: u64, message: AgentClientMessage },
}

#[derive(Deserialize)]
enum WorkspaceInput {
    // Ignore legacy client identity/token fields; the upgrade is authoritative.
    Hello {},
    Ping { nonce: String },
}

async fn send(socket: &mut WebSocket, reply: ServiceServerMessage) -> bool {
    let Ok(text) = serde_json::to_string(&reply) else { return false; };
    matches!(timeout(Duration::from_secs(10), socket.send(Message::Text(text))).await, Ok(Ok(())))
}

fn workspace_reply(request_id: u64, message: WorkspaceServerMessage) -> ServiceServerMessage {
    ServiceServerMessage::WorkspaceReply { request_id, message }
}

fn agent_reply(request_id: u64, message: AgentServerMessage) -> ServiceServerMessage {
    ServiceServerMessage::AgentReply { request_id, message }
}

async fn hello(socket: &mut WebSocket) -> Option<u64> {
    while let Some(Ok(frame)) = socket.recv().await {
        match frame {
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Text(text) => match serde_json::from_str(&text).ok()? {
                Input::Workspace { request_id, message: WorkspaceInput::Hello {} } => return Some(request_id),
                _ => return None,
            },
            _ => return None,
        }
    }
    None
}

async fn session(mut socket: WebSocket, endpoint: String, label: String) {
    let Ok(Some(request_id)) = timeout(Duration::from_secs(5), hello(&mut socket)).await else { return; };
    if !send(&mut socket, workspace_reply(request_id, WorkspaceServerMessage::HelloAck {
        accepted: true, reason: Some("mobile device bearer".into()), peer_identity: None,
        connected_host_id: Some(crate::workspace::machine_host_id()), workspace_lifecycle: false,
    })).await { return; }
    let (tx, mut rx) = agent::reply_queue::channel();
    let mut agent = MobileAgent::new(endpoint, label, tx);
    loop {
        let reply = tokio::select! {
            event = rx.recv() => match event {
                Some(message) => agent_reply(0, message),
                None => return,
            },
            _ = agent.reap(), if agent.has_jobs() => continue,
            frame = socket.recv() => match frame {
                Some(Ok(Message::Text(text))) => match serde_json::from_str(&text) {
                    Ok(Input::Workspace { request_id, message: WorkspaceInput::Ping { nonce } }) =>
                        workspace_reply(request_id, WorkspaceServerMessage::Pong { nonce }),
                    Ok(Input::Agent { request_id, message }) => match agent.admit(message) {
                        Ok(()) => continue,
                        Err(message) => agent_reply(request_id, AgentServerMessage::Error { message: message.into() }),
                    },
                    _ => workspace_reply(0, WorkspaceServerMessage::Error { message: "Not allowed on mobile chat".into() }),
                },
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                _ => return,
            },
        };
        if !send(&mut socket, reply).await { return; }
    }
}

fn operator_router(state: MobileState) -> Router {
    Router::new().route("/mobile-device/invite", post(invite))
        .route("/mobile-device/devices", get(devices))
        .route("/mobile-device/devices/:id", delete(revoke))
        .layer(DefaultBodyLimit::max(4096)).with_state(state)
}

fn phone_router(state: MobileState) -> Router {
    Router::new().route("/mobile-device/claim", post(claim))
        .route("/mobile-device/session", get(upgrade))
        .layer(DefaultBodyLimit::max(4096)).with_state(state)
}

fn https_origin(input: &str) -> anyhow::Result<Url> {
    let origin = Url::parse(input)?;
    anyhow::ensure!(origin.scheme() == "https" && origin.host().is_some() && origin.username().is_empty()
        && origin.password().is_none() && origin.path() == "/" && origin.query().is_none()
        && origin.fragment().is_none(), "NEOISM_MOBILE_ORIGIN must be an HTTPS origin");
    Ok(origin)
}

fn private_bind(ip: IpAddr) -> bool {
    ip.is_loopback() || match ip {
        IpAddr::V4(ip) => {
            let [first, second, ..] = ip.octets();
            first == 100 && (64..=127).contains(&second)
        }
        IpAddr::V6(ip) => ip.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
    }
}

pub(crate) struct Listener(JoinHandle<()>);

impl Drop for Listener {
    fn drop(&mut self) { self.0.abort(); }
}

pub(crate) async fn start() -> anyhow::Result<Option<(Router, Listener)>> {
    let Some(bind) = std::env::var_os("NEOISM_MOBILE_LISTEN") else {
        #[cfg(target_os = "macos")]
        return quick::automatic().map(Some);
        #[cfg(not(target_os = "macos"))]
        return Ok(None);
    };
    let addr: SocketAddr = bind.to_str().ok_or_else(|| anyhow::anyhow!("invalid mobile listen address"))?.parse()?;
    anyhow::ensure!(private_bind(addr.ip()) && addr.port() != 0, "mobile listener requires an explicit Tailscale or loopback address and port");
    let origin = https_origin(&std::env::var("NEOISM_MOBILE_ORIGIN")?)?;
    let operator_key = std::env::var("NEOISM_MOBILE_OPERATOR_KEY")?;
    anyhow::ensure!(URL_SAFE_NO_PAD.decode(&operator_key).is_ok_and(|bytes| bytes.len() == 32),
        "NEOISM_MOBILE_OPERATOR_KEY must be 32 random bytes, base64url without padding");
    let cert = std::env::var_os("NEOISM_MOBILE_TLS_CERT").ok_or_else(|| anyhow::anyhow!("missing NEOISM_MOBILE_TLS_CERT"))?;
    let key = std::env::var_os("NEOISM_MOBILE_TLS_KEY").ok_or_else(|| anyhow::anyhow!("missing NEOISM_MOBILE_TLS_KEY"))?;
    let (state, listener) = listen(addr, origin, digest(&operator_key), cert.as_ref(), key.as_ref(), std::env::var_os("NEOISM_MOBILE_TLS_CA").as_deref().map(std::path::Path::new), false).await?;
    Ok(Some((operator_router(state.clone()).merge(quick::configured(state)?), listener)))
}

fn tls_acceptor(cert: &std::path::Path, key: &std::path::Path) -> anyhow::Result<tokio_rustls::TlsAcceptor> {
    let certs = CertificateDer::pem_file_iter(cert)?
        .collect::<Result<Vec<_>, _>>()?;
    let key = PrivateKeyDer::from_pem_file(key)?;
    let mut tls = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()?.with_no_client_auth().with_single_cert(certs, key)?;
    tls.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(tokio_rustls::TlsAcceptor::from(Arc::new(tls)))
}

async fn listen(addr: SocketAddr, origin: Url, operator_key: Digest32, cert: &std::path::Path, key: &std::path::Path, ca: Option<&std::path::Path>, _automatic: bool) -> anyhow::Result<(MobileState, Listener)> {
    let endpoint = agent::configured_agent_server();
    anyhow::ensure!(agent::mobile::local_endpoint(&endpoint), "mobile chat requires the local Agent endpoint");
    let ca = ca.map(|path| -> anyhow::Result<_> {
        let cert = CertificateDer::from_pem_file(path)?;
        let encoded = URL_SAFE_NO_PAD.encode(cert.as_ref());
        anyhow::ensure!(encoded.len() <= 1400, "mobile pairing CA is too large for a QR code");
        Ok(encoded)
    }).transpose()?;
    let acceptor = Arc::new(parking_lot::RwLock::new(tls_acceptor(cert, key)?));
    let listener = TcpListener::bind(addr).await?;
    let state = MobileState {
        ca,
        origin, name: gethostname::gethostname().to_string_lossy().into_owned(),
        operator_key, agent_endpoint: endpoint,
        credentials: Arc::new(Mutex::new(Credentials::open_default()?)), sessions: Arc::new(Semaphore::new(32)),
    };
    let app = phone_router(state.clone());
    let connections = Arc::new(Semaphore::new(64));
    tracing::info!(%addr, "mobile device HTTPS listener ready");
    let task = tokio::spawn(async move {
        #[cfg(target_os = "macos")]
        let _renewal = _automatic.then(|| {
            let acceptor = acceptor.clone();
            Listener(tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(24 * 60 * 60)).await;
                    let renewal: anyhow::Result<_> = async {
                        let cfg = provision::provision().await?;
                        anyhow::ensure!(cfg.addr == addr, "Tailscale node address changed");
                        tls_acceptor(&cfg.cert, &cfg.key)
                    }.await;
                    match renewal {
                        Ok(next) => *acceptor.write() = next,
                        Err(error) => tracing::warn!(%error, "could not refresh phone listener certificate"),
                    }
                }
            }))
        });
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(peer) => peer,
                Err(_) => { tokio::time::sleep(Duration::from_millis(250)).await; continue; }
            };
            let Ok(permit) = connections.clone().try_acquire_owned() else { continue; };
            let acceptor = acceptor.read().clone();
            let service = TowerToHyperService::new(app.clone());
            tokio::spawn(async move {
                let _permit = permit;
                let _ = stream.set_nodelay(true);
                let Ok(Ok(stream)) = timeout(Duration::from_secs(5), acceptor.accept(stream)).await else { return; };
                let mut http = http1::Builder::new();
                // Disabling keep-alive makes Hyper overwrite the WebSocket's Connection: Upgrade.
                let connection = http.timer(hyper_util::rt::TokioTimer::new())
                    .header_read_timeout(Duration::from_secs(5))
                    .serve_connection(TokioIo::new(stream), service).with_upgrades();
                let _ = timeout(Duration::from_secs(10), connection).await;
            });
        }
    });
    Ok((state, Listener(task)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{SinkExt, StreamExt};
    use serde_json::{json, Value};
    use tokio_tungstenite::{connect_async, tungstenite::{client::IntoClientRequest, Error, Message as Frame}};

    pub(super) fn state() -> MobileState {
        MobileState {
            ca: None,
            origin: https_origin("https://desk.example.ts.net:8443").unwrap(),
            name: "My desktop".into(), operator_key: digest("operator-test-key"),
            agent_endpoint: "http://127.0.0.1:1".into(),
            credentials: Default::default(), sessions: Arc::new(Semaphore::new(2)),
        }
    }

    pub(super) async fn serve(app: Router) -> (SocketAddr, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        (listener.local_addr().unwrap(), tokio::spawn(async move {
            axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
        }))
    }

    #[test]
    fn mobile_registry_restores_devices_not_invites_and_revoke_survives_restart() {
        let root = tempfile::tempdir().unwrap();
        let now = Instant::now();
        let password = "private pairing phrase";
        let mut registry = Credentials::open(root.path()).unwrap();
        assert!(Credentials::open(root.path()).is_err(), "two listeners must not overwrite each other's snapshots");
        let code = registry.mint(password, now).unwrap();
        let pending = registry.mint(password, now).unwrap();
        let token = registry.claim(&code, "phone".into(), password, now).unwrap();
        let id = registry.devices[&digest(&token)].info.device_id;
        let dir = root.path().join("mobile-device");
        let path = dir.join(REGISTRY_FILE);
        let contents = fs::read_to_string(&path).unwrap();
        let records: Vec<StoredDevice> = serde_json::from_str(&contents).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].token_hash, digest(&token));
        for secret in [&token[..], &code, &pending, password] {
            assert!(!contents.contains(secret));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        drop(registry);

        let mut restored = Credentials::open(root.path()).unwrap();
        assert_eq!(restored.claim(&pending, "other".into(), password, now), Err(StatusCode::UNAUTHORIZED));
        let (label, session) = restored.authenticate(&token).unwrap();
        assert_eq!(label, "phone");
        assert_eq!(restored.revoke(id), Ok(true));
        assert!(session.has_changed().is_err(), "revoke must close restored sessions");
        drop(restored);

        let mut reopened = Credentials::open(root.path()).unwrap();
        assert!(reopened.authenticate(&token).is_none());
        assert_eq!(reopened.revoke(id), Ok(false));
    }

    #[test]
    fn mobile_registry_rejects_corruption_without_resetting_devices() {
        let root = tempfile::tempdir().unwrap();
        let mut registry = Credentials::open(root.path()).unwrap();
        let now = Instant::now();
        let code = registry.mint("password", now).unwrap();
        registry.claim(&code, "phone".into(), "password", now).unwrap();
        drop(registry);
        let path = root.path().join("mobile-device").join(REGISTRY_FILE);
        fs::write(&path, b"truncated").unwrap();
        assert!(Credentials::open(root.path()).is_err());
        assert_eq!(fs::read(path).unwrap(), b"truncated");
    }

    #[test]
    fn mobile_registry_failed_rename_denies_access_and_allows_retry() {
        let root = tempfile::tempdir().unwrap();
        let mut registry = Credentials::open(root.path()).unwrap();
        let now = Instant::now();
        let code = registry.mint("password", now).unwrap();
        let path = root.path().join("mobile-device").join(REGISTRY_FILE);
        fs::create_dir(&path).unwrap(); // an atomic file replacement cannot replace a directory
        assert_eq!(registry.claim(&code, "phone".into(), "password", now), Err(StatusCode::INTERNAL_SERVER_ERROR));
        assert!(registry.devices.is_empty());
        fs::remove_dir(&path).unwrap();
        let token = registry.claim(&code, "phone".into(), "password", now).unwrap();
        let id = registry.devices[&digest(&token)].info.device_id;
        let (_, session) = registry.authenticate(&token).unwrap();
        let backup = path.with_extension("bak");
        fs::rename(&path, &backup).unwrap();
        fs::create_dir(&path).unwrap();
        assert_eq!(registry.revoke(id), Err(StatusCode::INTERNAL_SERVER_ERROR));
        assert!(registry.authenticate(&token).is_none());
        assert!(session.has_changed().is_err());
        fs::remove_dir(&path).unwrap();
        fs::rename(backup, path).unwrap();
        assert_eq!(registry.revoke(id), Ok(true));
        drop(registry);
        assert!(Credentials::open(root.path()).unwrap().authenticate(&token).is_none());
    }

    #[test]
    fn mobile_invite_has_single_consumer_expiry_and_password_attempt_limit() {
        let mut registry = Credentials::default();
        let now = Instant::now();
        let code = registry.mint("correct", now).unwrap();
        let claim = |registry: &mut Credentials, code, password, now| registry.claim(code, "phone".into(), password, now);
        assert_eq!(claim(&mut registry, &code, "correct", now + INVITE_TTL), Err(StatusCode::UNAUTHORIZED));
        let code = registry.mint("correct", now).unwrap();
        for _ in 0..MAX_PASSWORD_ATTEMPTS {
            assert_eq!(claim(&mut registry, &code, "bad", now), Err(StatusCode::UNAUTHORIZED));
        }
        assert_eq!(claim(&mut registry, &code, "correct", now), Err(StatusCode::UNAUTHORIZED));
        let code = registry.mint("correct", now).unwrap();
        assert_eq!(claim(&mut registry, &code, "bad", now), Err(StatusCode::UNAUTHORIZED));
        let token = claim(&mut registry, &code, "correct", now).unwrap();
        assert!(registry.authenticate(&token).is_some());
        assert!(registry.authenticate(&code).is_none());
        assert_eq!(claim(&mut registry, &code, "correct", now), Err(StatusCode::UNAUTHORIZED));
        assert!(https_origin("http://desk.example.ts.net").is_err());
        assert!(https_origin("https://desk.example.ts.net/session").is_err());
        assert!(private_bind("100.64.1.2".parse().unwrap()));
        assert!(private_bind("fd7a:115c:a1e0::1".parse().unwrap()));
        assert!(!private_bind("0.0.0.0".parse().unwrap()));
        assert!(!private_bind("192.168.0.10".parse().unwrap()));
    }

    #[test]
    fn mobile_operator_requires_local_peer_and_credential() {
        let state = state();
        let mut headers = HeaderMap::new();
        assert!(!operator_allowed(&state, None, &headers));
        headers.insert(header::AUTHORIZATION, "Bearer operator-test-key".parse().unwrap());
        headers.insert("x-forwarded-for", "127.0.0.1".parse().unwrap());
        assert!(!operator_allowed(&state, Some(ConnectInfo("100.64.1.2:12345".parse().unwrap())), &headers));
        assert!(operator_allowed(&state, Some(ConnectInfo("127.0.0.1:12345".parse().unwrap())), &headers));
        headers.insert(header::ORIGIN, "https://other.example".parse().unwrap());
        assert!(!operator_allowed(&state, None, &headers));
    }

    #[tokio::test]
    async fn mobile_router_and_websocket_authorization() {
        // Exercise the exact two routers locally without requiring a test PKI.
        let state = state();
        let (operator, operator_task) = serve(operator_router(state.clone())).await;
        let (phone, phone_task) = serve(phone_router(state.clone())).await;
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let op = |path: &str| format!("http://{operator}/mobile-device/{path}");
        let remote = |path: &str| format!("http://{phone}/{path}");
        let password = json!({"password":"secret phrase"});
        assert_eq!(http.post(op("invite")).json(&password).send().await.unwrap().status(), 403);
        for path in ["mobile-device/invite", "agent", "pair", "session", "files"] {
            assert_eq!(http.post(remote(path)).json(&password).send().await.unwrap().status(), 404);
        }
        assert_eq!(http.post(op("claim")).json(&password).send().await.unwrap().status(), 404);
        let minted = http.post(op("invite")).bearer_auth("operator-test-key").json(&password).send().await.unwrap();
        assert_eq!(minted.headers()[header::CACHE_CONTROL], "no-store");
        let link = Url::parse(minted.json::<Value>().await.unwrap()["invite"].as_str().unwrap()).unwrap();
        let params: HashMap<_, _> = link.query_pairs().into_owned().collect();
        assert_eq!(link.scheme(), "neoism");
        assert_eq!(link.host_str(), Some("device"));
        assert_eq!(params.len(), 3);
        assert_eq!(params["origin"], "https://desk.example.ts.net:8443");
        assert_eq!(params["name"], "My desktop");
        let claim_url = remote("mobile-device/claim");
        let wrong = json!({"code":params["code"],"device_label":"phone","password":"wrong"});
        assert_eq!(http.post(&claim_url).json(&wrong).send().await.unwrap().status(), 401);
        let right = json!({"code":params["code"],"device_label":"phone","password":"secret phrase"});
        let claimed = http.post(&claim_url).json(&right).send().await.unwrap();
        assert_eq!(claimed.headers()[header::CACHE_CONTROL], "no-store");
        let token = claimed.json::<Value>().await.unwrap()["token"].as_str().unwrap().to_owned();
        assert_eq!(http.post(&claim_url).json(&right).send().await.unwrap().status(), 401);
        let ws = format!("ws://{phone}/mobile-device/session");
        let request = |bearer: &str| {
            let mut req = ws.as_str().into_client_request().unwrap();
            req.headers_mut().insert("Authorization", format!("Bearer {bearer}").parse().unwrap());
            req
        };
        for result in [connect_async(ws.as_str()).await, connect_async(request("operator-test-key")).await] {
            assert!(matches!(result, Err(Error::Http(response)) if response.status() == 401));
        }
        assert!(matches!(connect_async(format!("{ws}?token={token}")).await, Err(Error::Http(response)) if response.status() == 400));
        let (mut socket, _) = connect_async(request(&token)).await.unwrap();
        let send = |json: Value| Frame::Text(json.to_string().into());
        socket.send(send(json!({"Workspace":{"request_id":1,"message":{"Hello":{"token":null,"client_name":"rune"}}}}))).await.unwrap();
        let hello: Value = serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
        assert_eq!(hello["WorkspaceReply"]["message"]["HelloAck"]["accepted"], true);
        assert_eq!(hello["WorkspaceReply"]["message"]["HelloAck"]["workspace_lifecycle"], false);
        for denied in [
            json!({"Files":{"request_id":2,"message":{"ListDir":{"path":""}}}}),
            json!({"Pty":{"request_id":2,"message":"ListSessions"}}),
            json!({"Workspace":{"request_id":2,"message":{"CreateWorkspace":{}}}}),
            json!({"Agent":{"request_id":2,"message":{"ConnectDisconnect":{"provider_id":"a"}}}}),
            json!({"Agent":{"request_id":2,"message":{"DeleteThread":{"session_id":"ses_a"}}}}),
            json!({"Agent":{"request_id":2,"message":{"GetHistory":{"session_id":"../admin"}}}}),
        ] {
            socket.send(send(denied)).await.unwrap();
            let reply: Value = serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
            assert!(reply["AgentReply"]["message"]["Error"].is_object() || reply["WorkspaceReply"]["message"]["Error"].is_object());
        }
        socket.send(send(json!({"Agent":{"request_id":3,"message":"Ping"}}))).await.unwrap();
        let pong: Value = serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
        assert_eq!(pong["AgentReply"]["message"], "Pong");
        assert_eq!(http.get(op("devices")).bearer_auth(&token).send().await.unwrap().status(), 403);
        let list: Value = http.get(op("devices")).bearer_auth("operator-test-key").send().await.unwrap().json().await.unwrap();
        let id = list[0]["device_id"].as_str().unwrap();
        assert_eq!(http.delete(op(&format!("devices/{id}"))).bearer_auth("operator-test-key").send().await.unwrap().status(), 204);
        let closed = timeout(Duration::from_secs(2), socket.next()).await.unwrap();
        assert!(matches!(closed, None | Some(Err(_)) | Some(Ok(Frame::Close(_)))));
        assert!(matches!(connect_async(request(&token)).await, Err(Error::Http(response)) if response.status() == 401));
        operator_task.abort();
        phone_task.abort();
    }

    #[tokio::test]
    async fn mobile_websocket_forwards_only_inline_photos_to_mock_agent() {
        use base64::engine::general_purpose::STANDARD;
        use image_rs::ImageEncoder;
        use rand::{rngs::StdRng, SeedableRng};

        let (posted, mut received) = tokio::sync::mpsc::unbounded_channel();
        let (upstream, upstream_task) = serve(Router::new().route("/v2/health", get(|| async {
            Json(json!({"healthy":true,"version":"test","providerCredentialStore":"test"}))
        })).route(
            "/v2/sessions/test-session/prompt",
            post(move |Json(body): Json<Value>| async move {
                posted.send(body).unwrap();
                StatusCode::NO_CONTENT
            }),
        )).await;
        let mut state = state();
        state.agent_endpoint = format!("http://{upstream}");
        let now = Instant::now();
        let code = state.credentials.lock().mint("secret phrase", now).unwrap();
        let token = state.credentials.lock().claim(&code, "My iPhone".into(), "secret phrase", now).unwrap();
        let (phone, phone_task) = serve(phone_router(state)).await;
        let mut request = format!("ws://{phone}/mobile-device/session").into_client_request().unwrap();
        request.headers_mut().insert("Authorization", format!("Bearer {token}").parse().unwrap());
        let (mut socket, _) = connect_async(request).await.unwrap();
        socket.send(Frame::Text(json!({"Workspace":{"request_id":1,"message":{"Hello":{}}}}).to_string().into())).await.unwrap();
        let hello = timeout(Duration::from_secs(5), socket.next()).await.unwrap().unwrap().unwrap();
        assert!(hello.to_text().unwrap().contains("HelloAck"));

        // A valid PNG whose JSON byte array exceeds the original 1 MiB socket cap.
        let mut pixels = vec![0u8; 512 * 256 * 4];
        StdRng::seed_from_u64(0).fill_bytes(&mut pixels);
        let mut photo = Vec::new();
        image_rs::codecs::png::PngEncoder::new(&mut photo)
            .write_image(&pixels, 512, 256, image_rs::ExtendedColorType::Rgba8).unwrap();
        let inline = |mime, name| json!({"kind":mime,"path":name,"bytes":photo});
        let prompt = |text, attachments: Vec<Value>| json!({"Agent":{"request_id":2,"message":{"SubmitPrompt":{
            "session_id":"test-session","message_id":"msg-photo","text":text,"attachments":attachments,
            "author":"spoofed","mode":"override","delivery":"steer"
        }}}}).to_string();
        for invalid in [
            prompt("Describe this", vec![json!({"kind":"image/png","path":"/host/private.png"})]),
            prompt("Describe this", vec![inline("image/png", "../private.png")]),
            prompt("Describe this", vec![inline("image/png", "file:///host/private.png")]),
            prompt("Describe this", vec![inline("application/pdf", "photo.pdf")]),
            prompt("Describe this", vec![inline("image/jpeg", "photo.jpg")]),
            prompt("Describe this", vec![json!({"kind":"image/png","path":"photo.png","bytes":[1, 2, 3]})]),
            prompt("Describe this", vec![json!({"kind":"image/png","bytes":vec![0u8; neoism_protocol::agent::MOBILE_PHOTO_MAX_BYTES + 1]})]),
            prompt("/goal attempt", vec![inline("image/png", "photo.png")]),
        ] {
            socket.send(Frame::Text(invalid.into())).await.unwrap();
            let reply = timeout(Duration::from_secs(5), socket.next()).await.unwrap().unwrap().unwrap();
            let reply: Value = serde_json::from_str(reply.to_text().unwrap()).unwrap();
            assert_eq!(reply["AgentReply"]["request_id"], 2);
            assert!(reply["AgentReply"]["message"]["Error"].is_object());
        }

        let good = prompt("Describe this", vec![inline("image/png", "photo.png")]);
        assert!(good.len() > 1024 * 1024);
        socket.send(Frame::Text(good.into())).await.unwrap();
        let body = timeout(Duration::from_secs(5), received.recv()).await.unwrap().unwrap();
        assert_eq!(body["messageId"], "msg-photo");
        assert_eq!(body["author"], "My iPhone");
        assert!(body.get("agent").is_none());
        assert_eq!(body["parts"], json!([
            {"type":"text","text":"Describe this"},
            {"type":"file","mime":"image/png","filename":"photo.png","url":format!("data:image/png;base64,{}", STANDARD.encode(&photo))},
        ]));
        assert!(received.try_recv().is_err());
        phone_task.abort();
        upstream_task.abort();
    }
}