//! The `/connect` provider-auth flow for the agent GUI.
//!
//! Mirrors opencode's `auth login`, but as an in-GUI multi-stage picker:
//!
//! 1. **Connect a provider** — the catalog split into "Popular" + "Providers",
//!    with a checkmark on providers that are already connected
//!    ([`NeoismAgentPickerKind::Connect`]).
//! 2. **Select auth method** — the chosen provider's OAuth variants and
//!    "Manually enter API Key" ([`NeoismAgentPickerKind::ConnectAuth`]).
//! 3. **Secret entry** — a single-line field (the picker's own query row) for an
//!    API key or an OAuth token ([`NeoismAgentPickerKind::ConnectSecret`]).
//!
//! Backing endpoints (already implemented server-side): `GET /provider`,
//! `GET /provider/auth`, `PUT /auth/:id`, `POST /provider/:id/oauth/authorize`,
//! `POST /provider/:id/oauth/callback`.

use std::collections::{BTreeMap, HashSet};

use serde_json::{json, Value};

use super::*;

/// Providers surfaced first, in this order, under the "Popular" header. The
/// rest fall under "Providers" alphabetically. Ids match the models.dev catalog.
const POPULAR_PROVIDER_IDS: &[&str] = &[
    "claude-code",
    "anthropic",
    "openai",
    "openrouter",
    "github-copilot",
];

/// Sentinel option value for the "Disconnect …" row in the auth-method stage.
pub(in crate::neoism::agent) const DISCONNECT_VALUE: &str = "__disconnect__";

/// One provider row in the connect catalog.
#[derive(Clone)]
pub(in crate::neoism::agent) struct ConnectProvider {
    pub id: String,
    pub name: String,
    pub connected: bool,
}

/// One auth method for a provider. `index` is the method's position in the
/// provider's method list — the selector the server's authorize/callback
/// endpoints accept.
#[derive(Clone)]
pub(in crate::neoism::agent) struct ConnectMethod {
    pub index: usize,
    pub is_api: bool,
    pub label: String,
}

/// In-progress `/connect` state, held on the pane while any connect picker is
/// open.
pub(in crate::neoism::agent) struct ConnectFlow {
    providers: Vec<ConnectProvider>,
    methods_by_provider: BTreeMap<String, Vec<ConnectMethod>>,
    provider: Option<ConnectProvider>,
    method: Option<ConnectMethod>,
}

impl ConnectFlow {
    pub(in crate::neoism::agent) fn provider_id(&self) -> Option<String> {
        self.provider.as_ref().map(|provider| provider.id.clone())
    }
}

impl NeoismAgentPane {
    /// `/connect` entry point: fetch the provider catalog + auth methods and
    /// open stage 1 (the provider list).
    pub(in crate::neoism::agent) fn open_connect_picker(&mut self) {
        let flow = match fetch_connect_flow(&self.server) {
            Ok(flow) => flow,
            Err(error) => {
                self.system_message("Connect", error);
                return;
            }
        };
        if flow.providers.is_empty() {
            self.system_message("Connect", "no providers available");
            return;
        }
        self.connect = Some(flow);
        self.reopen_connect_provider_picker();
    }

    /// (Re)open stage 1 from the already-fetched catalog — used on first entry
    /// and when ESC steps back from the auth-method stage.
    pub(in crate::neoism::agent) fn reopen_connect_provider_picker(&mut self) {
        let Some(flow) = self.connect.as_mut() else {
            return;
        };
        flow.provider = None;
        flow.method = None;
        let options = connect_provider_options(&flow.providers);
        self.picker = Some(NeoismAgentPicker::new(
            NeoismAgentPickerKind::Connect,
            "Connect a provider",
            options,
            0,
        ));
    }

    /// Stage 1 → 2: the user picked a provider; show its auth methods.
    pub(in crate::neoism::agent) fn enter_connect_auth(&mut self, provider_id: &str) {
        let (provider, methods) = {
            let Some(flow) = self.connect.as_ref() else {
                return;
            };
            let Some(provider) = flow
                .providers
                .iter()
                .find(|provider| provider.id == provider_id)
                .cloned()
            else {
                return;
            };
            let methods = flow
                .methods_by_provider
                .get(provider_id)
                .cloned()
                .unwrap_or_default();
            (provider, methods)
        };
        if methods.is_empty() {
            self.system_message(
                "Connect",
                format!("{} exposes no auth methods", provider.name),
            );
            return;
        }
        let title = format!("{} — select auth method", provider.name);
        let connected = provider.connected;
        let provider_name = provider.name.clone();
        if let Some(flow) = self.connect.as_mut() {
            flow.provider = Some(provider);
            flow.method = None;
        }
        let mut options = Vec::new();
        // Already-connected providers get a disconnect affordance up top.
        if connected {
            options.push(NeoismAgentPickerOption::new(
                &format!("Disconnect {provider_name}"),
                "",
                "remove auth",
                DISCONNECT_VALUE,
            ));
        }
        options.extend(methods.iter().map(|method| {
            NeoismAgentPickerOption::new(
                &method.label,
                "",
                if method.is_api { "api key" } else { "oauth" },
                &method.index.to_string(),
            )
        }));
        self.picker = Some(NeoismAgentPicker::new(
            NeoismAgentPickerKind::ConnectAuth,
            &title,
            options,
            0,
        ));
    }

    /// Stage 2 → 3: the user picked an auth method. API-key methods jump
    /// straight to the secret field; OAuth methods first request an
    /// authorization URL (opened in the browser) before prompting for the token.
    pub(in crate::neoism::agent) fn start_connect_method(&mut self, method_index: usize) {
        let (provider, method) = {
            let Some(flow) = self.connect.as_ref() else {
                return;
            };
            let Some(provider) = flow.provider.clone() else {
                return;
            };
            let Some(method) = flow
                .methods_by_provider
                .get(&provider.id)
                .and_then(|methods| {
                    methods.iter().find(|method| method.index == method_index)
                })
                .cloned()
            else {
                return;
            };
            (provider, method)
        };
        if let Some(flow) = self.connect.as_mut() {
            flow.method = Some(method.clone());
        }
        // Claude Code "Connect Meridian" is a one-click connect: nothing to
        // type. It just writes a connected marker (streaming routes through the
        // local Meridian proxy with no per-user credential).
        if provider.id == "claude-code" && method.label.contains("Meridian") {
            let body = json!({ "type": "api", "key": "meridian" });
            match api_request_json(
                &self.server,
                "PUT",
                &format!("/auth/{}", provider.id),
                Some(&body),
            ) {
                Ok(_) => {
                    self.close_connect();
                    self.system_message(
                        "Connected",
                        format!(
                            "{} connected via Meridian. Open /model to pick a model.",
                            provider.name
                        ),
                    );
                }
                Err(error) => self.system_message("Connect", error),
            }
            return;
        }
        if method.is_api {
            self.open_connect_secret_entry(&method);
        } else {
            self.begin_connect_oauth(&provider, &method);
        }
    }

    /// Disconnect the provider chosen in the auth-method stage: remove its
    /// stored auth and refresh the provider list. (A provider still connected
    /// via an environment variable will keep its ✓ — env auth can't be removed
    /// from here.)
    pub(in crate::neoism::agent) fn disconnect_connect_provider(&mut self) {
        let Some(provider) = self.connect.as_ref().and_then(|flow| flow.provider.clone())
        else {
            return;
        };
        match api_request_json(
            &self.server,
            "DELETE",
            &format!("/auth/{}", provider.id),
            None,
        ) {
            Ok(_) => {
                self.system_message(
                    "Disconnected",
                    format!("{} disconnected.", provider.name),
                );
                // Re-fetch so the ✓ and /model eligibility reflect the change.
                self.open_connect_picker();
            }
            Err(error) => self.system_message("Disconnect", error),
        }
    }

    /// Kick off an OAuth method. The server tells us how it completes:
    /// - `method: "auto"` (OpenAI, GitHub Copilot) — a local callback finishes
    ///   the exchange on its own. We open the browser, tell the user it will
    ///   connect automatically, and poll the callback off-thread. No token to
    ///   paste — the user just authorizes and returns.
    /// - `method: "code"` (generic providers) — fall back to pasting a token
    ///   into the secret field.
    fn begin_connect_oauth(
        &mut self,
        provider: &ConnectProvider,
        method: &ConnectMethod,
    ) {
        let body = json!({ "method": method.index, "inputs": {} });
        let value = match api_request_json(
            &self.server,
            "POST",
            &format!("/provider/{}/oauth/authorize", provider.id),
            Some(&body),
        ) {
            Ok(value) => value.unwrap_or(Value::Null),
            Err(error) => {
                self.system_message("Connect", error);
                return;
            }
        };
        let url = value
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let auto = value.get("method").and_then(Value::as_str) == Some("auto");
        let instructions = value
            .get("instructions")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if url.starts_with("http://") || url.starts_with("https://") {
            open_url(&url);
        }
        if auto {
            let mut message = format!(
                "Opened your browser to sign in to {}. Finish there — neoism connects automatically, and you can close the tab when it's done.",
                provider.name
            );
            if !instructions.trim().is_empty() {
                message.push('\n');
                message.push_str(instructions.trim());
            }
            self.system_message(&provider.name, message);
            self.spawn_connect_oauth_wait(provider, method.index);
            // Return to the composer; the outcome arrives as a background update.
            self.close_connect();
        } else {
            if !instructions.trim().is_empty() {
                self.system_message(&provider.name, instructions);
            } else if !url.is_empty() {
                self.system_message(
                    &provider.name,
                    format!(
                        "Authorize in your browser, then paste the token below.\n{url}"
                    ),
                );
            }
            self.open_connect_secret_entry(method);
        }
    }

    /// Poll an auto-completing OAuth callback off the UI thread. The POST blocks
    /// server-side until the browser redirect is captured and the token
    /// exchanged, so the wait must not freeze the pane; the result comes back as
    /// a background update.
    fn spawn_connect_oauth_wait(&self, provider: &ConnectProvider, method_index: usize) {
        let server = self.server.clone();
        let provider_id = provider.id.clone();
        let provider_name = provider.name.clone();
        let background_tx = self.background_sender();
        let _ = std::thread::Builder::new()
            .name(format!("neoism-agent-oauth-{provider_id}"))
            .spawn(move || {
                let body = json!({ "method": method_index });
                let update =
                    match crate::neoism::agent::api::api_request_json_with_read_timeout(
                        &server,
                        "POST",
                        &format!("/provider/{provider_id}/oauth/callback"),
                        Some(&body),
                        Duration::from_secs(300),
                    ) {
                        Ok(_) => NeoismAgentBackgroundUpdate::ConnectOauthFinished {
                            provider_name,
                        },
                        Err(error) => NeoismAgentBackgroundUpdate::ConnectOauthFailed {
                            provider_name,
                            error,
                        },
                    };
                let _ = background_tx.send(update);
            });
    }

    /// Open stage 3: the single-line secret entry. The picker carries no rows;
    /// its query row is the input field.
    fn open_connect_secret_entry(&mut self, method: &ConnectMethod) {
        let (title, placeholder) = if method.is_api {
            ("Manually enter API Key", "API key")
        } else {
            ("Paste OAuth token", "OAuth token")
        };
        let mut picker = NeoismAgentPicker::new(
            NeoismAgentPickerKind::ConnectSecret,
            title,
            Vec::new(),
            0,
        );
        picker.search_placeholder = Some(placeholder.to_string());
        self.picker = Some(picker);
    }

    /// Commit the secret field (Enter). Stores an API key via `PUT /auth/:id`,
    /// or completes OAuth via the callback endpoint. Re-opens the field on an
    /// empty value or a failure so the user can retry.
    pub(in crate::neoism::agent) fn submit_connect_secret(&mut self, secret: String) {
        let secret = secret.trim().to_string();
        let (provider, method) = {
            let Some(flow) = self.connect.as_ref() else {
                self.close_connect();
                return;
            };
            match (flow.provider.clone(), flow.method.clone()) {
                (Some(provider), Some(method)) => (provider, method),
                _ => {
                    self.close_connect();
                    return;
                }
            }
        };
        if secret.is_empty() {
            self.open_connect_secret_entry(&method);
            return;
        }
        let result = if method.is_api {
            let body = json!({ "type": "api", "key": secret });
            api_request_json(
                &self.server,
                "PUT",
                &format!("/auth/{}", provider.id),
                Some(&body),
            )
            .map(|_| ())
        } else {
            let body = json!({ "method": method.index, "code": secret });
            api_request_json(
                &self.server,
                "POST",
                &format!("/provider/{}/oauth/callback", provider.id),
                Some(&body),
            )
            .map(|_| ())
        };
        match result {
            Ok(()) => {
                self.close_connect();
                self.system_message(
                    "Connected",
                    format!(
                        "{} is connected. Open /model to pick one of its models.",
                        provider.name
                    ),
                );
            }
            Err(error) => {
                self.system_message(&provider.name, error);
                self.open_connect_secret_entry(&method);
            }
        }
    }

    pub(in crate::neoism::agent) fn close_connect(&mut self) {
        self.connect = None;
        self.picker = None;
    }
}

/// Fetch the provider catalog (`/provider`) and per-provider auth methods
/// (`/provider/auth`) and fold them into a [`ConnectFlow`].
fn fetch_connect_flow(server: &str) -> Result<ConnectFlow, String> {
    let providers_value =
        api_request_json(server, "GET", "/provider", None)?.unwrap_or(Value::Null);
    let auth_value =
        api_request_json(server, "GET", "/provider/auth", None)?.unwrap_or(Value::Null);

    let connected: HashSet<String> = providers_value
        .get("connected")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    let mut providers = Vec::new();
    for provider in providers_value
        .get("all")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(id) = provider.get("id").and_then(Value::as_str) else {
            continue;
        };
        if id.is_empty() {
            continue;
        }
        let name = provider
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .unwrap_or(id)
            .to_string();
        providers.push(ConnectProvider {
            connected: connected.contains(id),
            id: id.to_string(),
            name,
        });
    }

    let mut methods_by_provider = BTreeMap::new();
    if let Some(map) = auth_value.as_object() {
        for (provider_id, methods_value) in map {
            let methods = methods_value
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
                .filter_map(|(index, method)| {
                    let kind = method.get("type").and_then(Value::as_str)?;
                    let label = method
                        .get("label")
                        .and_then(Value::as_str)
                        .unwrap_or(kind)
                        .to_string();
                    Some(ConnectMethod {
                        index,
                        is_api: kind == "api",
                        label,
                    })
                })
                .collect();
            methods_by_provider.insert(provider_id.clone(), methods);
        }
    }

    Ok(ConnectFlow {
        providers,
        methods_by_provider,
        provider: None,
        method: None,
    })
}

/// Build the stage-1 picker rows: a "Popular" header + the well-known providers
/// in [`POPULAR_PROVIDER_IDS`] order, then a "Providers" header + the rest
/// alphabetically. Connected providers get a leading checkmark.
fn connect_provider_options(
    providers: &[ConnectProvider],
) -> Vec<NeoismAgentPickerOption> {
    let mut popular: Vec<&ConnectProvider> = Vec::new();
    for id in POPULAR_PROVIDER_IDS {
        if let Some(provider) = providers.iter().find(|provider| provider.id == *id) {
            popular.push(provider);
        }
    }
    let mut rest: Vec<&ConnectProvider> = providers
        .iter()
        .filter(|provider| !POPULAR_PROVIDER_IDS.contains(&provider.id.as_str()))
        .collect();
    rest.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    let mut options = Vec::new();
    if !popular.is_empty() {
        options.push(NeoismAgentPickerOption::header("Popular"));
        options.extend(popular.into_iter().map(connect_provider_row));
    }
    if !rest.is_empty() {
        options.push(NeoismAgentPickerOption::header("Providers"));
        options.extend(rest.into_iter().map(connect_provider_row));
    }
    options
}

fn connect_provider_row(provider: &ConnectProvider) -> NeoismAgentPickerOption {
    let title = if provider.connected {
        format!("✓ {}", provider.name)
    } else {
        provider.name.clone()
    };
    NeoismAgentPickerOption::new(
        &title,
        "",
        if provider.connected { "connected" } else { "" },
        &provider.id,
    )
}

/// Open a URL in the user's default browser (best-effort, non-blocking).
fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(not(target_os = "macos"))]
    let program = "xdg-open";
    let _ = std::process::Command::new(program).arg(url).spawn();
}
