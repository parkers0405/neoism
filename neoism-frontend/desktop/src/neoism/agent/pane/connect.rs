//! The `/connect` provider-auth flow for the agent GUI.
//!
//! Mirrors opencode's `auth login`, but as an in-GUI multi-stage picker:
//!
//! 1. **Connect a provider** — the catalog split into "Popular" + "Providers",
//!    with a checkmark on providers that are already connected
//!    ([`NeoismAgentPickerKind::Connect`]).
//! 2. **Choose an account** — add another account or manage an existing one
//!    ([`NeoismAgentPickerKind::ConnectAccount`]).
//! 3. **Select auth method** — the chosen provider's OAuth variants and
//!    "Manually enter API Key" ([`NeoismAgentPickerKind::ConnectAuth`]).
//! 4. **Secret entry** — a single-line field (the picker's own query row) for an
//!    API key or an OAuth token ([`NeoismAgentPickerKind::ConnectSecret`]).
//!
//! Backing endpoints (already implemented server-side): `GET /provider`,
//! `GET /provider/auth`, `PUT /auth/:id`, `POST /provider/:id/oauth/authorize`,
//! `POST /provider/:id/oauth/callback`.

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{json, Value};

use super::*;
use crate::neoism::agent::api::api_request_json_while_active;

#[cfg(test)]
#[path = "connect_tests.rs"]
mod tests;

/// Providers surfaced first, in this order, under the "Popular" header. The
/// rest fall under "Providers" alphabetically. Ids match the models.dev catalog.
const POPULAR_PROVIDER_IDS: &[&str] = &[
    "claude-code",
    "anthropic",
    "openai",
    "openrouter",
    "github-copilot",
];
const ADD_ACCOUNT_VALUE: &str = "__add_account__";
pub(in crate::neoism::agent) const CONFIRM_DISCONNECT_VALUE: &str = "confirm_disconnect";

#[derive(Clone)]
pub(crate) struct ProviderConnection {
    pub id: String,
    pub label: String,
    pub auth_type: String,
    pub is_default: bool,
}

#[derive(Clone, Copy)]
enum LabelAction {
    Add,
    Rename,
}

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
pub(crate) struct ConnectFlow {
    providers: Vec<ConnectProvider>,
    methods_by_provider: BTreeMap<String, Vec<ConnectMethod>>,
    provider: Option<ConnectProvider>,
    method: Option<ConnectMethod>,
    pub(in crate::neoism::agent) connections: Vec<ProviderConnection>,
    connection: Option<ProviderConnection>,
    label: Option<String>,
    label_action: Option<LabelAction>,
    attempt_id: Option<String>,
}

impl ConnectFlow {
    pub(in crate::neoism::agent) fn provider_id(&self) -> Option<String> {
        self.provider.as_ref().map(|provider| provider.id.clone())
    }

    pub(in crate::neoism::agent) fn reset_account_addition(&mut self) {
        self.label = None;
        self.label_action = None;
        self.method = None;
    }
}

const RETRY_VALUE: &str = "__connect_retry__";

// A request owns only data, never the pane. The existing background sender wakes
// the owning window; there is no timer/poll loop while network I/O is pending.
#[derive(Clone)]
enum ConnectRequest {
    Catalog,
    Accounts {
        provider_id: String,
        model: Option<String>,
    },
    Mutation {
        verb: &'static str,
        path: String,
        body: Option<Value>,
        effect: ConnectEffect,
    },
}

#[derive(Clone)]
enum ConnectEffect {
    Refresh {
        notice: Option<(String, String)>,
    },
    Connected {
        provider_name: String,
    },
    Authorized {
        provider: ConnectProvider,
        method: ConnectMethod,
    },
    Secret {
        provider: ConnectProvider,
        method: ConnectMethod,
    },
    Oauth {
        provider_name: String,
    },
}

pub(crate) enum ConnectOutcome {
    Catalog(ConnectFlow),
    Accounts {
        provider_id: String,
        model: Option<String>,
        connections: Vec<ProviderConnection>,
        flow: Option<ConnectFlow>,
    },
    Response {
        value: Option<Value>,
        browser_error: Option<String>,
    },
}

pub(super) struct PendingConnect {
    token: Arc<AtomicBool>,
    server: String,
    session_id: Option<String>,
    request: ConnectRequest,
    loading: bool,
}

impl Drop for PendingConnect {
    fn drop(&mut self) {
        // Logical cancellation: an already-issued auth mutation cannot be
        // undone, but it must never restore UI or select an account afterwards.
        self.token.store(false, Ordering::Release);
    }
}

impl ConnectRequest {
    fn run(&self, server: &str, token: &AtomicBool) -> Result<ConnectOutcome, String> {
        if !token.load(Ordering::Acquire) {
            return Err("Connect cancelled".into());
        }
        match self {
            Self::Catalog => {
                fetch_connect_flow(server, token).map(ConnectOutcome::Catalog)
            }
            Self::Accounts { provider_id, model } => {
                let connections = fetch_provider_connections(server, provider_id, token)?;
                let flow = if model.is_some() && connections.len() > 1 {
                    Some(fetch_connect_flow(server, token)?)
                } else {
                    None
                };
                Ok(ConnectOutcome::Accounts {
                    provider_id: provider_id.clone(),
                    model: model.clone(),
                    connections,
                    flow,
                })
            }
            Self::Mutation {
                verb,
                path,
                body,
                effect,
            } => {
                let read_timeout = if matches!(effect, ConnectEffect::Oauth { .. }) {
                    Duration::from_secs(300)
                } else {
                    Duration::from_secs(5)
                };
                let value = api_request_json_while_active(
                    server,
                    verb,
                    path,
                    body.as_ref(),
                    read_timeout,
                    token,
                )?;
                // ShellExecute on Windows (and launching the URL helper on
                // Unix) belongs off-thread too. Check cancellation after the
                // authorize request, before opening anything on the desktop.
                if !token.load(Ordering::Acquire) {
                    return Err("Connect cancelled".into());
                }
                let browser_error = if matches!(effect, ConnectEffect::Authorized { .. })
                {
                    value
                        .as_ref()
                        .and_then(|value| value.get("url"))
                        .and_then(Value::as_str)
                        .filter(|url| {
                            url.starts_with("http://") || url.starts_with("https://")
                        })
                        .and_then(|url| open_auth_url(url).err())
                } else {
                    None
                };
                Ok(ConnectOutcome::Response {
                    value,
                    browser_error,
                })
            }
        }
    }
}

/// Shell URL handlers may use COM. Unlike the window thread, the short-lived
/// connect worker has no apartment yet; initialize it for ShellExecute on
/// Windows and balance the successful initialization on this same thread.
fn open_auth_url(url: &str) -> Result<(), String> {
    #[cfg(windows)]
    let _apartment = {
        use windows_sys::Win32::System::Com::{
            CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED,
        };
        struct Apartment;
        impl Drop for Apartment {
            fn drop(&mut self) {
                unsafe {
                    CoUninitialize();
                }
            }
        }
        let result =
            unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
        if result < 0 {
            return Err(format!(
                "Could not initialize browser launcher (HRESULT {result:#x})"
            ));
        }
        Apartment
    };
    crate::background_process::open_url(url)
}

impl NeoismAgentPane {
    fn start_connect_request(&mut self, request: ConnectRequest) {
        let token = Arc::new(AtomicBool::new(true));
        let server = self.server.clone();
        self.pending_connect = Some(PendingConnect {
            token: token.clone(),
            server: server.clone(),
            session_id: self.session_id.clone(),
            request: request.clone(),
            loading: true,
        });
        let mut picker = NeoismAgentPicker::new(
            NeoismAgentPickerKind::Connect,
            "Connect — loading… (Esc to cancel)",
            Vec::new(),
            0,
        );
        picker.set_loading(true);
        self.picker = Some(picker);
        let tx = self.background_sender();
        let worker_token = token.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("neoism-agent-connect".into())
            .spawn(move || {
                let result = request.run(&server, &worker_token);
                if worker_token.load(Ordering::Acquire) {
                    let _ = tx.send(NeoismAgentBackgroundUpdate::ConnectCompleted {
                        token: worker_token,
                        result,
                    });
                }
            })
        {
            self.finish_connect_request(
                token,
                Err(format!("Could not start connect worker: {error}")),
            );
        }
    }

    fn connect_mutation(
        &mut self,
        verb: &'static str,
        path: String,
        body: Option<Value>,
        effect: ConnectEffect,
    ) {
        self.start_connect_request(ConnectRequest::Mutation {
            verb,
            path,
            body,
            effect,
        });
    }

    pub(super) fn connect_request_loading(&self) -> bool {
        self.pending_connect
            .as_ref()
            .is_some_and(|pending| pending.loading)
    }

    pub(super) fn finish_connect_request(
        &mut self,
        token: Arc<AtomicBool>,
        result: Result<ConnectOutcome, String>,
    ) {
        let Some(pending) = self.pending_connect.as_ref() else {
            return;
        };
        if !Arc::ptr_eq(&pending.token, &token) {
            return;
        }
        if !token.load(Ordering::Acquire)
            || pending.server != self.server
            || pending.session_id != self.session_id
            || !self
                .picker
                .as_ref()
                .is_some_and(|picker| picker.kind == NeoismAgentPickerKind::Connect)
        {
            self.pending_connect = None;
            // Session creation can also arrive as a background event. If the
            // old request still owns the loading picker, dismiss it rather
            // than leaving a permanently-loading orphan behind. Never close
            // an unrelated replacement picker.
            if self
                .picker
                .as_ref()
                .is_some_and(|picker| picker.kind == NeoismAgentPickerKind::Connect)
            {
                self.close_connect();
            }
            return;
        }
        let request = pending.request.clone();
        match result {
            Err(error) => {
                self.system_message("Connect", &error);
                // Do not retain an entered credential as a retry payload.
                if let ConnectRequest::Mutation {
                    effect: ConnectEffect::Secret { method, .. },
                    ..
                } = request
                {
                    self.pending_connect = None;
                    self.open_connect_secret_entry(&method);
                } else {
                    self.pending_connect.as_mut().unwrap().loading = false;
                    self.picker = Some(NeoismAgentPicker::new(
                        NeoismAgentPickerKind::Connect,
                        "Connect unavailable",
                        vec![NeoismAgentPickerOption::new(
                            "Retry",
                            &error,
                            "Esc to cancel",
                            RETRY_VALUE,
                        )],
                        0,
                    ));
                }
            }
            Ok(outcome) => {
                self.pending_connect = None;
                match outcome {
                    ConnectOutcome::Catalog(flow) => {
                        if flow.providers.is_empty() {
                            self.close_connect();
                            self.system_message(
                                "Connect",
                                "no providers available; use /connect to retry",
                            );
                        } else {
                            self.connect = Some(flow);
                            self.reopen_connect_provider_picker();
                        }
                    }
                    ConnectOutcome::Accounts {
                        provider_id,
                        model,
                        connections,
                        flow,
                    } => {
                        if let Some(model) = model {
                            self.finish_model_accounts(
                                model,
                                provider_id,
                                connections,
                                flow,
                            );
                        } else {
                            if let Some(flow) = self.connect.as_mut() {
                                flow.connections = connections.clone();
                                flow.connection = None;
                            }
                            self.open_account_picker(
                                NeoismAgentPickerKind::ConnectAccount,
                                &provider_id,
                                &connections,
                            );
                        }
                    }
                    ConnectOutcome::Response {
                        value,
                        browser_error,
                    } => {
                        let ConnectRequest::Mutation { effect, .. } = request else {
                            return;
                        };
                        match effect {
                            ConnectEffect::Refresh { notice } => {
                                if let Some((title, message)) = notice {
                                    self.system_message(&title, message);
                                }
                                self.open_connect_picker();
                            }
                            ConnectEffect::Connected { provider_name }
                            | ConnectEffect::Oauth { provider_name } => {
                                self.finish_connect_secret(&provider_name, value);
                            }
                            ConnectEffect::Secret { provider, .. } => {
                                self.finish_connect_secret(&provider.name, value)
                            }
                            ConnectEffect::Authorized { provider, method } => self
                                .finish_connect_oauth(
                                    &provider,
                                    &method,
                                    value.unwrap_or(Value::Null),
                                    browser_error,
                                ),
                        }
                    }
                }
            }
        }
    }

    pub(in crate::neoism::agent) fn reconcile_model_account(
        &mut self,
        model: String,
    ) -> bool {
        if model.trim().is_empty() {
            self.apply_model_with_connection(model, None);
            return true;
        }
        let provider_id = model
            .split_once('/')
            .map(|(provider, _)| provider)
            .unwrap_or("openai")
            .to_string();
        self.start_connect_request(ConnectRequest::Accounts {
            provider_id,
            model: Some(model),
        });
        true
    }

    fn finish_model_accounts(
        &mut self,
        model: String,
        provider_id: String,
        connections: Vec<ProviderConnection>,
        flow: Option<ConnectFlow>,
    ) {
        let selected_for_provider =
            self.model.split_once('/').map(|(provider, _)| provider)
                == Some(provider_id.as_str());
        if let Some(selected) =
            self.connection_id.clone().filter(|_| selected_for_provider)
        {
            if connections
                .iter()
                .any(|connection| connection.id == selected)
            {
                self.apply_model_with_connection(model, Some(selected));
            } else {
                self.close_connect();
                self.system_message("Account", "The selected provider connection no longer exists. Choose an account explicitly.");
            }
            return;
        }
        if connections.len() <= 1 {
            self.apply_model_with_connection(
                model,
                connections.first().map(|connection| connection.id.clone()),
            );
            return;
        }
        let Some(mut flow) = flow else {
            return;
        };
        flow.provider = flow
            .providers
            .iter()
            .find(|provider| provider.id == provider_id)
            .cloned();
        flow.connections = connections.clone();
        self.connect = Some(flow);
        self.pending_account_model = Some(model);
        self.open_account_picker(
            NeoismAgentPickerKind::ModelAccount,
            &provider_id,
            &connections,
        );
    }

    /// `/connect` entry point: fetch the provider catalog + auth methods and
    /// open stage 1 (the provider list).
    pub(in crate::neoism::agent) fn open_connect_picker(&mut self) {
        self.close_connect();
        self.start_connect_request(ConnectRequest::Catalog);
    }

    /// (Re)open stage 1 from the already-fetched catalog — used on first entry
    /// and when ESC steps back from the auth-method stage.
    pub(in crate::neoism::agent) fn reopen_connect_provider_picker(&mut self) {
        let Some(flow) = self.connect.as_mut() else {
            return;
        };
        flow.provider = None;
        flow.method = None;
        flow.connection = None;
        flow.label = None;
        flow.label_action = None;
        flow.attempt_id = None;
        let options = connect_provider_options(&flow.providers);
        self.picker = Some(NeoismAgentPicker::new(
            NeoismAgentPickerKind::Connect,
            "Connect a provider",
            options,
            0,
        ));
    }

    /// Stage 1 → 2: the user picked a provider; show account management.
    pub(in crate::neoism::agent) fn enter_connect_auth(&mut self, provider_id: &str) {
        if provider_id == RETRY_VALUE {
            if let Some(pending) = self.pending_connect.take() {
                self.start_connect_request(pending.request.clone());
            }
            return;
        }
        if let Some(flow) = self.connect.as_mut() {
            flow.provider = flow
                .providers
                .iter()
                .find(|provider| provider.id == provider_id)
                .cloned();
            flow.connection = None;
        }
        self.start_connect_request(ConnectRequest::Accounts {
            provider_id: provider_id.to_string(),
            model: None,
        });
    }

    pub(in crate::neoism::agent) fn open_connect_auth_methods(&mut self) {
        let (provider, methods) = {
            let Some(flow) = self.connect.as_ref() else {
                return;
            };
            let Some(provider) = flow.provider.clone() else {
                return;
            };
            let methods = flow
                .methods_by_provider
                .get(&provider.id)
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
        let connected = provider.connected
            && !self
                .connect
                .as_ref()
                .is_some_and(|flow| matches!(flow.label_action, Some(LabelAction::Add)));
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

    pub(in crate::neoism::agent) fn open_account_picker(
        &mut self,
        kind: NeoismAgentPickerKind,
        provider_id: &str,
        connections: &[ProviderConnection],
    ) {
        let mut options = Vec::new();
        if kind == NeoismAgentPickerKind::ConnectAccount {
            options.push(NeoismAgentPickerOption::new(
                "Add account",
                "",
                "add",
                ADD_ACCOUNT_VALUE,
            ));
            if !connections.is_empty() {
                options.push(NeoismAgentPickerOption::header("Connected accounts"));
            }
        }
        options.extend(connections.iter().map(account_option));
        let provider_name = self
            .connect
            .as_ref()
            .and_then(|flow| flow.provider.as_ref())
            .filter(|provider| provider.id == provider_id)
            .map(|provider| provider.name.as_str())
            .unwrap_or(provider_id);
        self.picker = Some(NeoismAgentPicker::new(kind, provider_name, options, 0));
    }

    pub(in crate::neoism::agent) fn choose_connect_account(&mut self, value: &str) {
        if value == ADD_ACCOUNT_VALUE {
            if let Some(flow) = self.connect.as_mut() {
                flow.label_action = Some(LabelAction::Add);
                flow.connection = None;
            }
            self.open_account_label("Label new account", "Account label");
            return;
        }
        let Some(connection) = self.connect.as_ref().and_then(|flow| {
            flow.connections
                .iter()
                .find(|connection| connection.id == value)
                .cloned()
        }) else {
            self.system_message(
                "Account",
                "The selected provider connection no longer exists.",
            );
            return;
        };
        if let Some(flow) = self.connect.as_mut() {
            flow.connection = Some(connection.clone());
        }
        let mut options = vec![
            NeoismAgentPickerOption::new(
                "Select account",
                &connection.label,
                "use",
                "select",
            ),
            NeoismAgentPickerOption::new(
                "Rename",
                &connection.label,
                "edit label",
                "rename",
            ),
        ];
        if !connection.is_default {
            options.push(NeoismAgentPickerOption::new(
                "Make default",
                &connection.label,
                "default",
                "default",
            ));
        }
        options.push(NeoismAgentPickerOption::new(
            "Disconnect",
            &connection.label,
            "remove",
            "disconnect",
        ));
        self.picker = Some(NeoismAgentPicker::new(
            NeoismAgentPickerKind::ConnectAccountActions,
            "Account actions",
            options,
            0,
        ));
    }

    pub(in crate::neoism::agent) fn choose_model_account(&mut self, id: String) {
        let Some(model) = self.pending_account_model.take() else {
            return;
        };
        if !self.connect.as_ref().is_some_and(|flow| {
            flow.connections
                .iter()
                .any(|connection| connection.id == id)
        }) {
            self.system_message(
                "Account",
                "The selected provider connection no longer exists.",
            );
            return;
        }
        self.apply_model_with_connection(model, Some(id));
    }

    pub(in crate::neoism::agent) fn run_account_action(&mut self, action: &str) {
        let Some((provider, connection)) = self
            .connect
            .as_ref()
            .and_then(|flow| flow.provider.clone().zip(flow.connection.clone()))
        else {
            return;
        };
        match action {
            "select" => {
                if self.model.split_once('/').map(|(current, _)| current)
                    == Some(provider.id.as_str())
                {
                    self.apply_model_with_connection(
                        self.model.clone(),
                        Some(connection.id),
                    );
                } else {
                    self.connection_id = Some(connection.id);
                    self.close_connect();
                }
                self.system_message("Account", format!("Using {}.", connection.label));
            }
            "rename" => {
                if let Some(flow) = self.connect.as_mut() {
                    flow.label_action = Some(LabelAction::Rename);
                }
                self.open_account_label("Rename account", &connection.label);
            }
            "default" => {
                let path = format!(
                    "/v2/providers/{}/connections/{}/default",
                    provider.id, connection.id
                );
                self.connect_mutation(
                    "POST",
                    path,
                    None,
                    ConnectEffect::Refresh { notice: None },
                );
            }
            "disconnect" => {
                self.picker = Some(NeoismAgentPicker::new(
                    NeoismAgentPickerKind::ConnectConfirm,
                    "Disconnect account?",
                    vec![
                        NeoismAgentPickerOption::new(
                            "Disconnect",
                            &connection.label,
                            "cannot be undone",
                            CONFIRM_DISCONNECT_VALUE,
                        ),
                        NeoismAgentPickerOption::new(
                            "Cancel",
                            "",
                            "keep account",
                            "cancel",
                        ),
                    ],
                    1,
                ))
            }
            _ => {}
        }
    }

    fn open_account_label(&mut self, title: &str, placeholder: &str) {
        let mut picker = NeoismAgentPicker::new(
            NeoismAgentPickerKind::ConnectLabel,
            title,
            Vec::new(),
            0,
        );
        picker.search_placeholder = Some(placeholder.to_string());
        self.picker = Some(picker);
    }

    pub(in crate::neoism::agent) fn submit_account_label(&mut self, label: String) {
        let label = label.trim().to_string();
        if label.is_empty() {
            self.open_account_label("Account label", "Account label");
            return;
        }
        match self.connect.as_ref().and_then(|flow| flow.label_action) {
            Some(LabelAction::Add) => {
                if let Some(flow) = self.connect.as_mut() {
                    flow.label = Some(label);
                }
                self.open_connect_auth_methods();
            }
            Some(LabelAction::Rename) => {
                let Some((provider, connection)) = self
                    .connect
                    .as_ref()
                    .and_then(|flow| flow.provider.clone().zip(flow.connection.clone()))
                else {
                    return;
                };
                let path = format!(
                    "/v2/providers/{}/connections/{}",
                    provider.id, connection.id
                );
                self.connect_mutation(
                    "PATCH",
                    path,
                    Some(json!({"label": label})),
                    ConnectEffect::Refresh { notice: None },
                );
            }
            None => {}
        }
    }

    pub(in crate::neoism::agent) fn confirm_account_disconnect(&mut self, confirm: bool) {
        if !confirm {
            self.close_connect();
            return;
        }
        let Some((provider, connection)) = self
            .connect
            .as_ref()
            .and_then(|flow| flow.provider.clone().zip(flow.connection.clone()))
        else {
            return;
        };
        let path = format!(
            "/v2/providers/{}/connections/{}",
            provider.id, connection.id
        );
        let notice = (self.connection_id.as_deref() == Some(connection.id.as_str())).then(|| (
            "Account".into(), "The selected provider connection was deleted. Choose another account before sending.".into(),
        ));
        self.connect_mutation("DELETE", path, None, ConnectEffect::Refresh { notice });
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
            let label = self.connect.as_ref().and_then(|flow| flow.label.clone());
            let (method, path, body) = if let Some(label) = label {
                (
                    "POST",
                    format!("/v2/providers/{}/connections", provider.id),
                    json!({"label": label, "credential": body, "setDefault": false}),
                )
            } else {
                ("PUT", format!("/v2/providers/{}/auth", provider.id), body)
            };
            self.connect_mutation(
                method,
                path,
                Some(body),
                ConnectEffect::Connected {
                    provider_name: provider.name,
                },
            );
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
        self.connect_mutation(
            "DELETE",
            format!("/v2/providers/{}/auth", provider.id),
            None,
            ConnectEffect::Refresh {
                notice: Some((
                    "Disconnected".into(),
                    format!("{} disconnected.", provider.name),
                )),
            },
        );
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
        let body = json!({ "method": method.index, "inputs": {}, "label": self.connect.as_ref().and_then(|flow| flow.label.clone()), "connectionId": self.connect.as_ref().and_then(|flow| flow.connection.as_ref().map(|connection| connection.id.clone())) });
        self.connect_mutation(
            "POST",
            format!("/v2/providers/{}/oauth/authorize", provider.id),
            Some(body),
            ConnectEffect::Authorized {
                provider: provider.clone(),
                method: method.clone(),
            },
        );
    }

    fn finish_connect_oauth(
        &mut self,
        provider: &ConnectProvider,
        method: &ConnectMethod,
        value: Value,
        browser_error: Option<String>,
    ) {
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
        if let Some(flow) = self.connect.as_mut() {
            flow.attempt_id = value
                .get("attemptId")
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        let authorization_link = (url.starts_with("http://")
            || url.starts_with("https://"))
        .then(|| format!("[{url}]({url})"));
        if auto {
            let mut message = match browser_error {
                Some(error) => format!(
                    "Could not open your browser ({error}). Use the authorization URL below to sign in to {}.",
                    provider.name
                ),
                None => format!(
                    "Opened your browser to sign in to {}. Finish there — neoism connects automatically, and you can close the tab when it's done.",
                    provider.name
                ),
            };
            if !instructions.trim().is_empty() {
                message.push('\n');
                message.push_str(instructions.trim());
            }
            if let Some(link) = &authorization_link {
                message
                    .push_str("\n\nAuthorization URL (click to open; drag to copy):\n");
                message.push_str(link);
            }
            self.system_message(&provider.name, message);
            self.spawn_connect_oauth_wait(
                provider,
                method.index,
                self.connect
                    .as_ref()
                    .and_then(|flow| flow.attempt_id.clone()),
            );
            // Keep a cancellable loading picker until the browser callback settles.
        } else {
            let mut message = if instructions.trim().is_empty() {
                "Authorize in your browser, then paste the token below.".to_string()
            } else {
                instructions
            };
            if let Some(error) = browser_error {
                message.push_str(&format!("\nCould not open your browser: {error}"));
            }
            if let Some(link) = authorization_link {
                message
                    .push_str("\n\nAuthorization URL (click to open; drag to copy):\n");
                message.push_str(&link);
            }
            self.system_message(&provider.name, message);
            self.open_connect_secret_entry(method);
        }
    }

    /// Poll an auto-completing OAuth callback off the UI thread. The POST blocks
    /// server-side until the browser redirect is captured and the token
    /// exchanged, so the wait must not freeze the pane; the result comes back as
    /// a background update.
    fn spawn_connect_oauth_wait(
        &mut self,
        provider: &ConnectProvider,
        method_index: usize,
        attempt_id: Option<String>,
    ) {
        self.connect_mutation(
            "POST",
            format!("/v2/providers/{}/oauth/callback", provider.id),
            Some(json!({ "method": method_index, "attemptId": attempt_id })),
            ConnectEffect::Oauth {
                provider_name: provider.name.clone(),
            },
        );
        if let Some(picker) = self.picker.as_mut() {
            picker.title = "Waiting for browser sign-in… (Esc to cancel)".into();
        }
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
        let (verb, path, body) = if method.is_api {
            let credential = json!({ "type": "api", "key": secret });
            let label = self.connect.as_ref().and_then(|flow| flow.label.clone());
            if let Some(label) = label {
                (
                    "POST",
                    format!("/v2/providers/{}/connections", provider.id),
                    json!({"label": label, "credential": credential, "setDefault": false}),
                )
            } else {
                (
                    "PUT",
                    format!("/v2/providers/{}/auth", provider.id),
                    credential,
                )
            }
        } else {
            (
                "POST",
                format!("/v2/providers/{}/oauth/callback", provider.id),
                json!({ "method": method.index, "code": secret, "attemptId": self.connect.as_ref().and_then(|flow| flow.attempt_id.clone()) }),
            )
        };
        self.connect_mutation(
            verb,
            path,
            Some(body),
            ConnectEffect::Secret { provider, method },
        );
    }

    fn finish_connect_secret(&mut self, provider_name: &str, value: Option<Value>) {
        let connection_id = value.and_then(|value| {
            value
                .get("connectionId")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
        if connection_id.is_some() {
            self.connection_id = connection_id;
        }
        self.close_connect();
        self.system_message(
            "Connected",
            format!(
                "{provider_name} is connected. Open /model to pick one of its models."
            ),
        );
    }

    pub(in crate::neoism::agent) fn close_connect(&mut self) {
        self.pending_connect = None;
        self.pending_account_model = None;
        self.connect = None;
        self.picker = None;
    }
}

/// Fetch the provider catalog (`/provider`) and per-provider auth methods
/// (`/provider/auth`) and fold them into a [`ConnectFlow`].
fn fetch_connect_flow(server: &str, token: &AtomicBool) -> Result<ConnectFlow, String> {
    let providers_value = api_request_json_while_active(
        server,
        "GET",
        "/v2/providers",
        None,
        Duration::from_secs(5),
        token,
    )?
    .unwrap_or(Value::Null);
    let auth_value = api_request_json_while_active(
        server,
        "GET",
        "/v2/providers/auth-methods",
        None,
        Duration::from_secs(5),
        token,
    )?
    .unwrap_or(Value::Null);
    if !providers_value.get("all").is_some_and(Value::is_array) || !auth_value.is_object()
    {
        return Err(
            "The agent returned a malformed provider catalog or auth methods".into(),
        );
    }

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
        connections: Vec::new(),
        connection: None,
        label: None,
        label_action: None,
        attempt_id: None,
    })
}

fn fetch_provider_connections(
    server: &str,
    provider_id: &str,
    token: &AtomicBool,
) -> Result<Vec<ProviderConnection>, String> {
    let value = api_request_json_while_active(
        server,
        "GET",
        &format!("/v2/providers/{provider_id}/connections"),
        None,
        Duration::from_secs(5),
        token,
    )?
    .ok_or_else(|| "The agent returned empty provider accounts".to_string())?;
    if !value.is_array() {
        return Err("The agent returned malformed provider accounts".into());
    }
    Ok(value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|connection| {
            Some(ProviderConnection {
                id: connection.get("connectionId")?.as_str()?.to_string(),
                label: connection.get("label")?.as_str()?.to_string(),
                auth_type: connection.get("authType")?.as_str()?.to_string(),
                is_default: connection
                    .get("isDefault")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
        })
        .collect())
}

fn account_option(connection: &ProviderConnection) -> NeoismAgentPickerOption {
    NeoismAgentPickerOption::new(
        &connection.label,
        &connection.auth_type,
        if connection.is_default {
            "default"
        } else {
            "account"
        },
        &connection.id,
    )
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
