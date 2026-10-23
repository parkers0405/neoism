//! Ephemeral, capability-verified machine execution targets. Never persisted.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[derive(Clone, Debug)]
pub(crate) struct AgentSystemSource {
    pub id: String,
    pub name: String,
    pub generation: u64,
    pub system_id: String,
    pub server: String,
    pub home_directory: String,
    pub(crate) authorized: Arc<AtomicBool>,
}

impl PartialEq for AgentSystemSource {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.generation == other.generation
            && self.server == other.server
            && self.name == other.name
            && self.home_directory == other.home_directory
            && self.system_id == other.system_id
    }
}
impl Eq for AgentSystemSource {}
impl AgentSystemSource {
    pub(crate) fn authorized(&self) -> bool {
        self.authorized.load(Ordering::Acquire)
    }
    pub(crate) fn revoke(&self) {
        self.authorized.store(false, Ordering::Release);
    }
    pub(crate) fn key(&self, value: &str) -> String {
        encode_source_key(&self.id, self.generation, value)
    }
}

// JSON tuple encoding is unambiguous for arbitrary host ids, paths and session ids.
const PREFIX: &str = "neoism-system:";
pub(crate) fn encode_source_key(id: &str, generation: u64, value: &str) -> String {
    format!(
        "{PREFIX}{}",
        serde_json::to_string(&(id, generation, value)).unwrap()
    )
}
pub(crate) fn decode_source_key(value: &str) -> Option<(String, u64, String)> {
    serde_json::from_str(value.strip_prefix(PREFIX)?).ok()
}

pub(crate) fn is_system_agent_server(server: &str) -> bool {
    url::Url::parse(server)
        .ok()
        .is_some_and(|url| url.path().trim_end_matches('/') == "/agent/system")
}

pub(crate) fn system_agent_base(endpoint: &str) -> Result<String, String> {
    let mut url = url::Url::parse(endpoint).map_err(|_| "Invalid machine endpoint")?;
    match url.scheme() {
        "ws" => {
            let _ = url.set_scheme("http");
        }
        "wss" => {
            let _ = url.set_scheme("https");
        }
        "http" | "https" => {}
        _ => return Err("Unsupported machine endpoint".into()),
    }
    url.set_path("/agent/system");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.to_string().trim_end_matches('/').to_owned())
}

/// Use the master daemon credential explicitly; never probe with GUI or guest auth.
pub(crate) fn discover_system_source(
    id: String,
    name: String,
    generation: u64,
    endpoint: &str,
    credential: String,
) -> Result<AgentSystemSource, String> {
    let server = system_agent_base(endpoint)?;
    let url = url::Url::parse(&format!("{server}/info"))
        .map_err(|_| "Invalid machine descriptor URL")?;
    let (status, _, body) = super::transport::request(
        url,
        reqwest::Method::GET,
        Some(credential),
        None,
        std::time::Duration::from_secs(15),
    )?;
    if status == 404 || status == 405 {
        return Err("Update Neoism on the remote machine to enable system-wide agent access, then reconnect the system".into());
    }
    if status != 200 {
        return Err(format!("Machine agent authorization failed (HTTP {status}); update the remote machine and reconnect"));
    }
    let info: serde_json::Value =
        serde_json::from_slice(&body).map_err(|_| "Invalid machine agent descriptor")?;
    let home_directory = info
        .get("homeDirectory")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or("Machine agent descriptor is missing homeDirectory; update remote Neoism")?
        .to_owned();
    let system_id = info
        .get("systemId")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or("Machine agent descriptor is missing systemId; update remote Neoism")?
        .to_owned();
    if info.get("capabilityVersion").and_then(|v| v.as_u64()) != Some(1) {
        return Err(
            "Unsupported machine agent capability; update remote Neoism and reconnect"
                .into(),
        );
    }
    Ok(AgentSystemSource {
        id,
        name,
        generation,
        system_id,
        server,
        home_directory,
        authorized: Arc::new(AtomicBool::new(true)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn descriptor_requires_capability_and_uses_explicit_master_auth() {
        use std::io::{Read, Write};
        for (status, body) in [
            (404, "{}"),
            (
                200,
                r#"{"systemId":"host-id","name":"Host","homeDirectory":"/canonical/home","capabilityVersion":1}"#,
            ),
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = format!("ws://{}/ws", listener.local_addr().unwrap());
            let worker = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                while !bytes.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    socket.read_exact(&mut byte).unwrap();
                    bytes.push(byte[0]);
                }
                let request = String::from_utf8(bytes).unwrap();
                assert!(request.starts_with("GET /agent/system/info "));
                assert!(request
                    .to_ascii_lowercase()
                    .contains("authorization: bearer master-test\r\n"));
                write!(socket, "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            });
            let result = discover_system_source(
                "saved".into(),
                "Saved name".into(),
                9,
                &endpoint,
                "master-test".into(),
            );
            worker.join().unwrap();
            if status == 404 {
                assert!(result.unwrap_err().contains("Update Neoism"));
            } else {
                let source = result.unwrap();
                assert_eq!(source.home_directory, "/canonical/home");
                assert_eq!(source.system_id, "host-id");
                assert_eq!(source.id, "saved");
                assert_eq!(source.generation, 9);
            }
        }
    }

    #[test]
    fn same_path_and_session_ids_on_different_sources_never_collide() {
        for value in ["/work/project", "session-1", "a\"/:b"] {
            let a = encode_source_key("a", 1, value);
            assert_ne!(a, encode_source_key("b", 1, value));
            assert_ne!(a, encode_source_key("a", 2, value));
            assert_eq!(decode_source_key(&a), Some(("a".into(), 1, value.into())));
        }
    }
}
