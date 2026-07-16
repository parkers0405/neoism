use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

const REGISTRY_FILE: &str = "servers.json";
const CREDENTIALS_FILE: &str = "server-credentials.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedServer {
    pub id: String,
    pub name: String,
    pub endpoint: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct RegistryFile {
    #[serde(default)]
    servers: Vec<SavedServer>,
    #[serde(default)]
    window_profiles: HashMap<String, WindowProfile>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WindowProfile {
    #[serde(default)]
    pub servers: HashMap<String, ServerWorkspaceSubscription>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerWorkspaceSubscription {
    #[serde(default)]
    pub subscribed_workspace_ids: Vec<String>,
    #[serde(default)]
    pub last_active_workspace_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CredentialsFile {
    #[serde(default)]
    tokens: HashMap<String, String>,
}

#[derive(Debug)]
pub struct ServerRegistry {
    directory: PathBuf,
    data: RegistryFile,
    credentials: CredentialsFile,
}

impl ServerRegistry {
    pub fn load(directory: PathBuf) -> io::Result<Self> {
        let data = read_json(&directory.join(REGISTRY_FILE))?.unwrap_or_default();
        let credentials =
            read_json(&directory.join(CREDENTIALS_FILE))?.unwrap_or_default();
        Ok(Self {
            directory,
            data,
            credentials,
        })
    }

    pub fn servers(&self) -> &[SavedServer] {
        &self.data.servers
    }

    pub fn server(&self, id: &str) -> Option<&SavedServer> {
        self.data.servers.iter().find(|server| server.id == id)
    }

    pub fn token(&self, id: &str) -> Option<&str> {
        self.credentials.tokens.get(id).map(String::as_str)
    }

    pub fn workspace_subscription(
        &self,
        profile_id: &str,
        server_id: &str,
    ) -> ServerWorkspaceSubscription {
        self.data
            .window_profiles
            .get(profile_id)
            .and_then(|profile| profile.servers.get(server_id))
            .cloned()
            .unwrap_or_default()
    }

    pub fn set_workspace_subscription(
        &mut self,
        profile_id: &str,
        server_id: &str,
        subscription: ServerWorkspaceSubscription,
    ) -> Result<(), String> {
        self.data
            .window_profiles
            .entry(profile_id.to_string())
            .or_default()
            .servers
            .insert(server_id.to_string(), subscription);
        self.persist().map_err(|error| error.to_string())
    }

    pub fn add(
        &mut self,
        address: &str,
        name: Option<&str>,
        token: Option<&str>,
    ) -> Result<SavedServer, String> {
        let (address, inline_name) = match address.split_once('|') {
            Some((address, name)) => (address.trim(), non_empty(Some(name))),
            None => (address, None),
        };
        let name = non_empty(name).or(inline_name);
        let endpoint = normalize_server_address(address)?;
        if self
            .data
            .servers
            .iter()
            .any(|server| server.endpoint == endpoint)
        {
            return Err("that server is already saved".into());
        }

        let server = SavedServer {
            id: Uuid::new_v4().to_string(),
            name: normalized_name(name, &endpoint),
            endpoint,
        };
        self.data.servers.push(server.clone());
        if let Some(token) = non_empty(token) {
            self.credentials
                .tokens
                .insert(server.id.clone(), token.to_string());
        }
        self.persist().map_err(|error| error.to_string())?;
        Ok(server)
    }

    pub fn remove(&mut self, id: &str) -> io::Result<bool> {
        let previous_len = self.data.servers.len();
        self.data.servers.retain(|server| server.id != id);
        self.credentials.tokens.remove(id);
        let removed = previous_len != self.data.servers.len();
        if removed {
            self.persist()?;
        }
        Ok(removed)
    }

    pub fn update(
        &mut self,
        id: &str,
        address: &str,
        name: Option<&str>,
        token: Option<&str>,
    ) -> Result<SavedServer, String> {
        let endpoint = normalize_server_address(address)?;
        if self
            .data
            .servers
            .iter()
            .any(|server| server.id != id && server.endpoint == endpoint)
        {
            return Err("that server is already saved".into());
        }
        let server = self
            .data
            .servers
            .iter_mut()
            .find(|server| server.id == id)
            .ok_or_else(|| format!("unknown saved server `{id}`"))?;
        server.endpoint = endpoint.clone();
        server.name = normalized_name(name, &endpoint);
        let updated = server.clone();
        match non_empty(token) {
            Some(token) => {
                self.credentials
                    .tokens
                    .insert(id.to_string(), token.to_string());
            }
            None => {
                self.credentials.tokens.remove(id);
            }
        }
        self.persist().map_err(|error| error.to_string())?;
        Ok(updated)
    }

    fn persist(&self) -> io::Result<()> {
        fs::create_dir_all(&self.directory)?;
        write_json_atomic(&self.directory.join(REGISTRY_FILE), &self.data, false)?;
        self.persist_credentials()
    }

    fn persist_credentials(&self) -> io::Result<()> {
        fs::create_dir_all(&self.directory)?;
        write_json_atomic(
            &self.directory.join(CREDENTIALS_FILE),
            &self.credentials,
            true,
        )
    }
}

pub fn normalize_server_address(address: &str) -> Result<String, String> {
    let address = address.trim();
    let mut url = Url::parse(address)
        .map_err(|error| format!("invalid server address: {error}"))?;
    let websocket_scheme = match url.scheme() {
        "http" => Some("ws"),
        "https" => Some("wss"),
        "ws" | "wss" => None,
        scheme => return Err(format!("unsupported server scheme `{scheme}`")),
    };
    if let Some(scheme) = websocket_scheme {
        url.set_scheme(scheme)
            .map_err(|_| "could not convert server address to WebSocket".to_string())?;
    }
    if url
        .query_pairs()
        .any(|(key, _)| key == "token" || key == "auth_token")
    {
        return Err(
            "put the access token in the token field, not in the server address".into(),
        );
    }
    if url.host_str().is_none() {
        return Err("server address must include a host".into());
    }
    match url.path() {
        "" | "/" => url.set_path("/session"),
        "/session" => {}
        path => {
            return Err(format!(
                "unsupported daemon path `{path}`; expected `/session`"
            ))
        }
    }
    Ok(url.to_string().trim_end_matches('/').to_string())
}

fn normalized_name(name: Option<&str>, endpoint: &str) -> String {
    if let Some(name) = non_empty(name) {
        return name.to_string();
    }
    Url::parse(endpoint)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_else(|| endpoint.to_string())
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> io::Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn write_json_atomic<T: Serialize>(
    path: &Path,
    value: &T,
    secret: bool,
) -> io::Result<()> {
    let temporary = path.with_extension("tmp");
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    fs::write(&temporary, bytes)?;
    #[cfg(unix)]
    if secret {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir() -> PathBuf {
        std::env::temp_dir().join(format!("neoism-server-registry-{}", Uuid::new_v4()))
    }

    #[test]
    fn normalizes_http_addresses_for_the_session_socket() {
        assert_eq!(
            normalize_server_address("https://neoism.example.com").unwrap(),
            "wss://neoism.example.com/session"
        );
        assert_eq!(
            normalize_server_address("ws://127.0.0.1:7878/").unwrap(),
            "ws://127.0.0.1:7878/session"
        );
    }

    #[test]
    fn rejects_credentials_in_addresses() {
        assert!(normalize_server_address(
            "wss://neoism.example.com/session?token=secret"
        )
        .unwrap_err()
        .contains("token field"));
    }

    #[test]
    fn credentials_are_stored_outside_the_registry() {
        let directory = test_dir();
        let mut registry = ServerRegistry::load(directory.clone()).unwrap();
        let server = registry
            .add(
                "https://neoism.example.com",
                Some("Home"),
                Some("secret-token"),
            )
            .unwrap();

        let public = fs::read_to_string(directory.join(REGISTRY_FILE)).unwrap();
        let secret = fs::read_to_string(directory.join(CREDENTIALS_FILE)).unwrap();
        assert!(!public.contains("secret-token"));
        assert!(secret.contains("secret-token"));

        let loaded = ServerRegistry::load(directory.clone()).unwrap();
        assert_eq!(loaded.server(&server.id).unwrap().name, "Home");
        assert_eq!(loaded.token(&server.id), Some("secret-token"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn workspace_subscriptions_are_scoped_by_window_profile_and_server() {
        let directory = test_dir();
        let mut registry = ServerRegistry::load(directory.clone()).unwrap();
        registry
            .set_workspace_subscription(
                "window-a",
                "local",
                ServerWorkspaceSubscription {
                    subscribed_workspace_ids: vec!["notes".into()],
                    last_active_workspace_id: Some("notes".into()),
                },
            )
            .unwrap();
        registry
            .set_workspace_subscription(
                "window-a",
                "work",
                ServerWorkspaceSubscription {
                    subscribed_workspace_ids: vec!["neoism".into(), "website".into()],
                    last_active_workspace_id: Some("neoism".into()),
                },
            )
            .unwrap();
        registry
            .set_workspace_subscription(
                "window-b",
                "work",
                ServerWorkspaceSubscription {
                    subscribed_workspace_ids: vec!["website".into()],
                    last_active_workspace_id: Some("website".into()),
                },
            )
            .unwrap();

        let loaded = ServerRegistry::load(directory.clone()).unwrap();
        assert_eq!(
            loaded
                .workspace_subscription("window-a", "local")
                .subscribed_workspace_ids,
            vec!["notes"]
        );
        assert_eq!(
            loaded
                .workspace_subscription("window-a", "work")
                .subscribed_workspace_ids,
            vec!["neoism", "website"]
        );
        assert_eq!(
            loaded
                .workspace_subscription("window-b", "work")
                .last_active_workspace_id
                .as_deref(),
            Some("website")
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
