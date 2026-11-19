//! Blocking client shared by the CLI and the desktop's network worker.
use crate::protocol::{Action, Discovery, DisplayNameRequest, Event, Invite, InviteRequest, Member, Outcome, Reaction, ScheduledPost, Snapshot};
use reqwest::{blocking, header, Url};
use serde::de::DeserializeOwned;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use crate::protocol::{Attachment, MAX_ATTACHMENT_BYTES};

#[derive(Debug)]
pub struct ClientError(String);

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ClientError {}

#[derive(Clone)]
pub struct Client {
    endpoint: Url,
    http: blocking::Client,
}

impl Client {
    pub fn new(endpoint: &str, token: &str, profile: Option<&str>) -> Result<Self, ClientError> {
        let mut endpoint = Url::parse(endpoint.trim())
            .map_err(|_| ClientError("invalid Neoboard endpoint".into()))?;
        let loopback = endpoint.host_str().is_some_and(|host| {
            host == "localhost" || host.trim_matches(['[', ']']).parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
        });
        if !(endpoint.scheme() == "https" || endpoint.scheme() == "http" && loopback)
            || endpoint.host().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(ClientError("use an HTTPS endpoint or a loopback HTTP endpoint".into()));
        }
        if !endpoint.path().ends_with('/') {
            endpoint.set_path(&format!("{}/", endpoint.path()));
        }
        if token.trim().is_empty() {
            return Err(ClientError("missing Neoboard credential".into()));
        }
        let mut headers = header::HeaderMap::new();
        let mut auth = header::HeaderValue::from_str(&format!("Bearer {}", token.trim()))
            .map_err(|_| ClientError("invalid Neoboard credential".into()))?;
        auth.set_sensitive(true);
        headers.insert(header::AUTHORIZATION, auth);
        if let Some(profile) = profile {
            let value = header::HeaderValue::from_str(profile)
                .map_err(|_| ClientError("invalid actor profile".into()))?;
            headers.insert("x-neochat-profile", value);
        }
        let builder = blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .default_headers(headers)
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(35));
        let http = if loopback { builder.no_proxy() } else { builder }
            .build()
            .map_err(network_error)?;
        Ok(Self { endpoint, http })
    }

    pub fn discover(data_dir: Option<&Path>, profile: Option<&str>) -> Result<Self, ClientError> {
        let dir = local_data_dir(data_dir)?;
        let Discovery { url, token } = local_credentials(&dir)?;
        Self::new(&url, &token, profile)
    }

    pub fn discover_admin(data_dir: Option<&Path>) -> Result<Self, ClientError> {
        let dir = local_data_dir(data_dir)?;
        let Discovery { url, .. } = local_credentials(&dir)?;
        let token = std::fs::read_to_string(dir.join("admin-token"))
            .map_err(|err| ClientError(format!("cannot read Neoboard admin credential: {err}")))?;
        Self::new(&url, &token, None)
    }

    pub fn endpoint(&self) -> &str {
        self.endpoint.as_str()
    }

    pub fn snapshot(&self) -> Result<Snapshot, ClientError> {
        self.request(self.http.get(self.url("v1/snapshot")))
    }

    pub fn members(&self, channel: &str) -> Result<Vec<Member>, ClientError> {
        self.request(self.http.get(self.url("v1/members")).query(&[("channel", channel)]))
    }
    pub fn my_member(&self)->Result<Member,ClientError>{self.request(self.http.get(self.url("v1/me")))}

    /// Updates the authenticated actor, regardless of any action profile header.
    pub fn set_display_name(&self, name: &str) -> Result<(), ClientError> {
        self.send(self.http.post(self.url("v1/display-name")).json(&DisplayNameRequest { name: name.into() }))?;
        Ok(())
    }

    pub fn reactions(&self) -> Result<Vec<Reaction>, ClientError> {
        self.request(self.http.get(self.url("v1/reactions")))
    }

    pub fn attach(&self, id: u64, name: &str, bytes: Vec<u8>) -> Result<Attachment, ClientError> {
        if bytes.len() > MAX_ATTACHMENT_BYTES { return Err(ClientError("attachment exceeds 8 MiB".into())); }
        self.request(self.http.post(self.url(&format!("v1/messages/{id}/attachments"))).query(&[("name",name)]).header(header::CONTENT_TYPE,"application/octet-stream").body(bytes))
    }

    pub fn attachments(&self, id: u64) -> Result<Vec<Attachment>, ClientError> {
        self.request(self.http.get(self.url(&format!("v1/messages/{id}/attachments"))))
    }

    pub fn download_attachment(&self, id: u64) -> Result<Vec<u8>, ClientError> {
        use std::io::Read;
        let response = self.send(self.http.get(self.url(&format!("v1/attachments/{id}"))))?;
        let mut bytes = Vec::new();
        response.take(MAX_ATTACHMENT_BYTES as u64 + 1).read_to_end(&mut bytes).map_err(|e| ClientError(e.to_string()))?;
        if bytes.len() > MAX_ATTACHMENT_BYTES { return Err(ClientError("attachment exceeds 8 MiB".into())); }
        Ok(bytes)
    }

    pub fn schedules(&self) -> Result<Vec<ScheduledPost>, ClientError> {
        self.request(self.http.get(self.url("v1/schedules")))
    }

    pub fn events(&self, after: u64, wait_ms: u64) -> Result<Vec<Event>, ClientError> {
        self.request(self.http.get(self.url("v1/events")).query(&[("after", after), ("wait_ms", wait_ms.min(25_000))]))
    }

    pub fn action(&self, action: &Action) -> Result<Outcome, ClientError> {
        self.action_with_key(action, None)
    }

    pub fn action_with_key(&self, action: &Action, key: Option<&str>) -> Result<Outcome, ClientError> {
        let mut request = self.http.post(self.url("v1/actions")).json(action);
        if let Some(key) = key {
            request = request.header("idempotency-key", key);
        }
        self.request(request)
    }

    pub fn invite(&self, invite: &InviteRequest) -> Result<Invite, ClientError> {
        self.request(self.http.post(self.url("v1/admin/invites")).json(invite))
    }

    pub fn revoke_invite(&self, id: u64) -> Result<(), ClientError> {
        self.send(self.http.delete(self.url(&format!("v1/admin/invites/{id}"))))?;
        Ok(())
    }

    fn ping(&self) -> Result<(), ClientError> {
        let response = self.http.get(self.url("v1/events")).query(&[("after", i64::MAX)])
            .timeout(Duration::from_millis(350)).send().map_err(network_error)?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(ClientError(format!("Neoboard daemon returned {}", response.status())))
        }
    }

    fn url(&self, path: &str) -> Url {
        self.endpoint.join(path).expect("static API path")
    }

    fn request<T: DeserializeOwned>(&self, request: blocking::RequestBuilder) -> Result<T, ClientError> {
        self.send(request)?.json().map_err(network_error)
    }

    fn send(&self, request: blocking::RequestBuilder) -> Result<blocking::Response, ClientError> {
        let response = request.send().map_err(network_error)?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let reason = response.json::<serde_json::Value>().ok().and_then(|body| {
            body.get("error").or_else(|| body.get("message")).and_then(|value| value.as_str()).map(str::to_owned)
        });
        Err(ClientError(match reason {
            Some(reason) => format!("{status}: {reason}"),
            None => format!("Neoboard request failed: {status}"),
        }))
    }
}

fn network_error(err: reqwest::Error) -> ClientError {
    ClientError(err.without_url().to_string())
}

pub fn local_data_dir(explicit: Option<&Path>) -> Result<PathBuf, ClientError> {
    explicit.map(Path::to_owned)
        .or_else(|| std::env::var_os("NEOCHAT_DATA_DIR").map(PathBuf::from))
        .or_else(|| dirs::data_local_dir().map(|path| path.join("neochat")))
        .ok_or_else(|| ClientError("no Neoboard data directory; pass --data-dir".into()))
}

fn local_credentials(dir: &Path) -> Result<Discovery, ClientError> {
    let endpoint_path = dir.join("endpoint.json");
    let record = std::fs::read_to_string(&endpoint_path)
        .map_err(|err| ClientError(format!("{}: {err}", endpoint_path.display())))?;
    serde_json::from_str(&record).map_err(|err| ClientError(format!("invalid local endpoint file: {err}")))
}

pub fn daemon_command() -> Command {
    if let Some(program) = std::env::var_os("NEOCHAT_DAEMON") {
        return Command::new(program);
    }
    let name = format!("neochatd{}", std::env::consts::EXE_SUFFIX);
    let sibling = std::env::current_exe().ok().and_then(|path| path.parent().map(|p| p.join(&name)));
    Command::new(sibling.filter(|p| p.is_file()).unwrap_or_else(|| PathBuf::from(name)))
}

pub fn connect_local(data_dir: Option<&Path>, profile: Option<&str>) -> Result<Client, ClientError> {
    let dir = local_data_dir(data_dir)?;
    let probe = || -> Result<Client, ClientError> {
        let client = Client::discover(Some(&dir), profile)?;
        client.ping()?;
        Ok(client)
    };
    if let Ok(client) = probe() {
        return Ok(client);
    }

    let mut command = daemon_command();
    command.arg("--data-dir").arg(&dir).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000 | 0x00000200);
    }
    let mut child = command.spawn().map_err(|err| ClientError(format!("could not start neochatd: {err}")))?;
    std::thread::spawn(move || { let _ = child.wait(); });
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        match probe() {
            Ok(client) => return Ok(client),
            Err(err) if Instant::now() >= end => {
                return Err(ClientError(format!("neochatd did not become ready: {err}; run `neochat serve` to see startup errors")));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(80)),
        }
    }
}