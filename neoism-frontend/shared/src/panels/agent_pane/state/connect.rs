//! The `/connect` provider-auth flow for the shared (web/wasm) agent pane.
//!
//! This is the web port of the desktop pane's
//! `neoism/agent/pane/connect.rs`. The staged picker UX is identical:
//!
//! 1. **Connect a provider** — the catalog split into "Popular" +
//!    "Providers", with a checkmark on already-connected providers
//!    ([`NeoismAgentPickerKind::Connect`]).
//! 2. **Choose an account** — add another account or manage an existing one
//!    ([`NeoismAgentPickerKind::ConnectAccount`]).
//! 3. **Select auth method** — the chosen provider's OAuth variants and
//!    "Manually enter API Key" ([`NeoismAgentPickerKind::ConnectAuth`]).
//! 4. **Secret entry** — a single-line field (the picker's own query row) for
//!    an API key or an OAuth token ([`NeoismAgentPickerKind::ConnectSecret`]).
//!
//! The difference from desktop is the transport. The desktop pane makes
//! blocking HTTP calls (`api_request_json`) and spawns background threads —
//! neither is available on wasm32. The shared pane instead records each
//! network step on the [`OutboundAgentCommand`] queue for the host to
//! execute, and the host feeds results back through the `apply_connect_*` /
//! `note_connect_*` setters. Browser navigation for OAuth uses the timeline's
//! existing clickable-link path (the auth URL is surfaced in a system
//! message; a click routes to the host, which opens it) — there is no
//! localhost callback listener on the web, so "auto" flows ask the host to
//! await the server-side callback and everything else falls back to the
//! paste-a-code path.
//!
//! Mirrored server endpoints (same as desktop): `GET /provider`,
//! `GET /provider/auth`, `PUT /auth/:id`, `DELETE /auth/:id`,
//! `POST /provider/:id/oauth/authorize`, `POST /provider/:id/oauth/callback`.

use std::collections::{BTreeMap, HashSet};

use serde_json::Value;

use super::*;

/// Providers surfaced first, in this order, under the "Popular" header. The
/// rest fall under "Providers" alphabetically. Ids match the models.dev
/// catalog. Kept identical to the desktop pane so the two hosts group the
/// catalog the same way.
const POPULAR_PROVIDER_IDS: &[&str] = &[
    "claude-code",
    "anthropic",
    "openai",
    "openrouter",
    "github-copilot",
];
const ADD_ACCOUNT_VALUE: &str = "__add_account__";
const ACCOUNT_SELECT_VALUE: &str = "select";
const ACCOUNT_RENAME_VALUE: &str = "rename";
const ACCOUNT_DEFAULT_VALUE: &str = "default";
const ACCOUNT_DISCONNECT_VALUE: &str = "disconnect";
pub(in crate::panels::agent_pane::state) const CONFIRM_DISCONNECT_VALUE: &str = "confirm_disconnect";

#[derive(Clone, Debug)]
pub(in crate::panels::agent_pane::state) struct ProviderConnection {
    pub id: String,
    pub label: String,
    pub auth_type: String,
    pub is_default: bool,
}

#[derive(Clone, Copy)]
enum LabelAction { Add, Rename }

/// Sentinel option value for the "Disconnect …" row in the auth-method stage.
pub(in crate::panels::agent_pane::state) const DISCONNECT_VALUE: &str = "__disconnect__";

/// One provider row in the connect catalog.
#[derive(Clone)]
pub(in crate::panels::agent_pane::state) struct ConnectProvider {
    pub id: String,
    pub name: String,
    pub connected: bool,
}

/// One auth method for a provider. `index` is the method's position in the
/// provider's method list — the selector the server's authorize/callback
/// endpoints accept.
#[derive(Clone)]
pub(in crate::panels::agent_pane::state) struct ConnectMethod {
    pub index: usize,
    pub is_api: bool,
    pub label: String,
}

/// In-progress `/connect` state, held on the pane while any connect picker is
/// open. Defaults to an empty catalog so the picker can open with a "loading"
/// placeholder before the host delivers `apply_connect_catalog`.
#[derive(Default)]
pub(in crate::panels::agent_pane::state) struct ConnectFlow {
    providers: Vec<ConnectProvider>,
    methods_by_provider: BTreeMap<String, Vec<ConnectMethod>>,
    provider: Option<ConnectProvider>,
    method: Option<ConnectMethod>,
    pub(in crate::panels::agent_pane::state) connections: Vec<ProviderConnection>,
    connection: Option<ProviderConnection>,
    label: Option<String>,
    label_action: Option<LabelAction>,
    attempt_id: Option<String>,
}

impl ConnectFlow {
    pub(in crate::panels::agent_pane::state) fn provider_id(&self) -> Option<String> {
        self.provider.as_ref().map(|provider| provider.id.clone())
    }

    pub(in crate::panels::agent_pane::state) fn reset_account_addition(&mut self) {
        self.label = None;
        self.label_action = None;
        self.method = None;
    }
}

impl NeoismAgentPane {
    /// `/connect` entry point. Requests the provider catalog + auth methods
    /// from the host and opens stage 1 (the provider list). Because the fetch
    /// is asynchronous on the web, the picker first shows a loading
    /// placeholder; `apply_connect_catalog` swaps in the real rows when the
    /// host answers.
    pub fn open_connect_picker(&mut self) {
        self.push_outbound(OutboundAgentCommand::RefreshConnectProviders {
            directory: self.directory.clone(),
        });
        // Preserve an already-fetched catalog across a re-open (e.g. after a
        // disconnect); otherwise start from an empty flow that shows the
        // loading placeholder until the host delivers the catalog.
        if self.connect.is_none() {
            self.connect = Some(ConnectFlow::default());
        }
        self.reopen_connect_provider_picker();
    }

    /// (Re)open stage 1 from the current flow — used on first entry and when
    /// ESC steps back from the auth-method stage.
    pub(in crate::panels::agent_pane::state) fn reopen_connect_provider_picker(
        &mut self,
    ) {
        let options = match self.connect.as_mut() {
            Some(flow) => {
                flow.provider = None;
                flow.method = None;
                flow.connection = None;
                flow.label = None;
                flow.label_action = None;
                flow.attempt_id = None;
                if flow.providers.is_empty() {
                    connect_loading_options()
                } else {
                    connect_provider_options(&flow.providers)
                }
            }
            None => return,
        };
        self.picker = Some(NeoismAgentPicker::new(
            NeoismAgentPickerKind::Connect,
            "Connect a provider",
            options,
            0,
        ));
    }

    /// Apply an asynchronously-fetched provider catalog. Rebuilds the flow's
    /// provider list + per-provider auth methods, preserves any in-progress
    /// stage selection by id, and refreshes the stage-1 picker rows if it is
    /// still showing. Mirrors the desktop `fetch_connect_flow` parse, but the
    /// host ferries the raw `/provider` + `/provider/auth` JSON so the parsing
    /// stays in one place.
    pub fn apply_connect_catalog(&mut self, providers: Value, auth: Value) {
        let mut flow = parse_connect_flow(providers, auth);
        // Keep the user's place if they already advanced past stage 1.
        if let Some(existing) = self.connect.as_ref() {
            if let Some(provider_id) = existing.provider_id() {
                flow.provider = flow
                    .providers
                    .iter()
                    .find(|provider| provider.id == provider_id)
                    .cloned();
                flow.method = existing.method.clone();
            }
        }
        self.connect = Some(flow);
        // Only refresh rows while stage 1 is on screen — an open auth /
        // secret picker must not be disturbed by a late catalog refresh.
        let options = self.connect.as_ref().map(|flow| {
            if flow.providers.is_empty() {
                connect_empty_options()
            } else {
                connect_provider_options(&flow.providers)
            }
        });
        if let Some(options) = options {
            if let Some(picker) = self
                .picker
                .as_mut()
                .filter(|picker| picker.kind == NeoismAgentPickerKind::Connect)
            {
                picker.replace_options(options);
            }
        }
    }

    /// Stage 1 → 2: the user picked a provider; show its auth methods.
    pub(in crate::panels::agent_pane::state) fn enter_connect_auth(
        &mut self,
        provider_id: &str,
    ) {
        if let Some(flow) = self.connect.as_mut() {
            flow.provider = flow.providers.iter().find(|provider| provider.id == provider_id).cloned();
            flow.method = None;
            flow.connection = None;
            flow.connections.clear();
        }
        self.push_outbound(OutboundAgentCommand::RefreshProviderConnections {
            provider_id: provider_id.to_string(),
        });
        self.picker = None;
    }

    pub(in crate::panels::agent_pane::state) fn open_connect_auth_methods(&mut self) {
        let (provider, methods) = {
            let Some(flow) = self.connect.as_ref() else {
                return;
            };
            let Some(provider) = flow.provider.clone() else { return; };
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
            && !self.connect.as_ref().is_some_and(|flow| matches!(flow.label_action, Some(LabelAction::Add)));
        let provider_name = provider.name.clone();
        if let Some(flow) = self.connect.as_mut() { flow.provider = Some(provider); flow.method = None; }
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

    pub fn apply_provider_connections(
        &mut self,
        provider_id: String,
        connections: Vec<neoism_protocol::agent::ProviderConnectionInfo>,
    ) {
        let connections = connections.into_iter().map(|connection| ProviderConnection {
            id: connection.connection_id,
            label: connection.label,
            auth_type: connection.auth_type,
            is_default: connection.is_default,
        }).collect::<Vec<_>>();

        if let Some(model) = self.pending_account_model.take() {
            let model_provider = model.split_once('/').map(|(provider, _)| provider).unwrap_or("openai");
            if model_provider != provider_id { return; }
            let selected_for_provider = self.model.split_once('/').map(|(provider, _)| provider) == Some(provider_id.as_str());
            if let Some(selected) = self.connection_id.clone().filter(|_| selected_for_provider) {
                if connections.iter().any(|connection| connection.id == selected) {
                    self.apply_model_with_connection(model, Some(selected));
                } else {
                    self.system_message("Account", "The selected provider connection no longer exists. Choose an account explicitly.");
                }
            } else if connections.len() <= 1 {
                self.apply_model_with_connection(model, connections.first().map(|connection| connection.id.clone()));
            } else {
                if let Some(flow) = self.connect.as_mut() { flow.connections = connections.clone(); }
                self.pending_account_model = Some(model);
                self.open_account_picker(NeoismAgentPickerKind::ModelAccount, &provider_id, &connections);
            }
            return;
        }

        let Some(flow) = self.connect.as_mut() else { return; };
        if flow.provider.as_ref().map(|provider| provider.id.as_str()) != Some(provider_id.as_str()) { return; }
        flow.connections = connections.clone();
        flow.connection = None;
        self.open_account_picker(NeoismAgentPickerKind::ConnectAccount, &provider_id, &connections);
    }

    pub(in crate::panels::agent_pane::state) fn open_account_picker(&mut self, kind: NeoismAgentPickerKind, provider_id: &str, connections: &[ProviderConnection]) {
        let mut options = Vec::new();
        if kind == NeoismAgentPickerKind::ConnectAccount {
            options.push(NeoismAgentPickerOption::new("Add account", "", "add", ADD_ACCOUNT_VALUE));
            if !connections.is_empty() { options.push(NeoismAgentPickerOption::header("Connected accounts")); }
        }
        options.extend(connections.iter().map(account_option));
        let provider_name = self.connect.as_ref()
            .and_then(|flow| flow.provider.as_ref())
            .filter(|provider| provider.id == provider_id)
            .map(|provider| provider.name.as_str())
            .unwrap_or(provider_id);
        self.picker = Some(NeoismAgentPicker::new(kind, provider_name, options, 0));
    }

    pub(in crate::panels::agent_pane::state) fn choose_connect_account(&mut self, value: &str) {
        if value == ADD_ACCOUNT_VALUE {
            if let Some(flow) = self.connect.as_mut() { flow.label_action = Some(LabelAction::Add); flow.connection = None; }
            self.open_account_label("Label new account", "Account label");
            return;
        }
        let Some(connection) = self.connect.as_ref().and_then(|flow| flow.connections.iter().find(|item| item.id == value).cloned()) else {
            self.system_message("Account", "The selected provider connection no longer exists.");
            return;
        };
        if let Some(flow) = self.connect.as_mut() { flow.connection = Some(connection.clone()); }
        let mut options = vec![
            NeoismAgentPickerOption::new("Select account", &connection.label, "use", ACCOUNT_SELECT_VALUE),
            NeoismAgentPickerOption::new("Rename", &connection.label, "edit label", ACCOUNT_RENAME_VALUE),
        ];
        if !connection.is_default { options.push(NeoismAgentPickerOption::new("Make default", &connection.label, "default", ACCOUNT_DEFAULT_VALUE)); }
        options.push(NeoismAgentPickerOption::new("Disconnect", &connection.label, "remove", ACCOUNT_DISCONNECT_VALUE));
        self.picker = Some(NeoismAgentPicker::new(NeoismAgentPickerKind::ConnectAccountActions, "Account actions", options, 0));
    }

    pub(in crate::panels::agent_pane::state) fn choose_model_account(&mut self, connection_id: String) {
        let Some(model) = self.pending_account_model.take() else { return; };
        let exists = self.connect.as_ref().is_some_and(|flow| flow.connections.iter().any(|connection| connection.id == connection_id));
        if !exists { self.system_message("Account", "The selected provider connection no longer exists."); return; }
        self.apply_model_with_connection(model, Some(connection_id));
    }

    pub(in crate::panels::agent_pane::state) fn run_account_action(&mut self, action: &str) {
        let Some((provider, connection)) = self.connect.as_ref().and_then(|flow| flow.provider.clone().zip(flow.connection.clone())) else { return; };
        match action {
            ACCOUNT_SELECT_VALUE => {
                if self.model.split_once('/').map(|(current, _)| current) == Some(provider.id.as_str()) {
                    self.apply_model_with_connection(self.model.clone(), Some(connection.id));
                } else {
                    self.connection_id = Some(connection.id);
                    self.close_connect();
                }
                self.system_message("Account", format!("Using {}.", connection.label));
            }
            ACCOUNT_RENAME_VALUE => { if let Some(flow) = self.connect.as_mut() { flow.label_action = Some(LabelAction::Rename); } self.open_account_label("Rename account", &connection.label); }
            ACCOUNT_DEFAULT_VALUE => { self.push_outbound(OutboundAgentCommand::ConnectSetDefault { provider_id: provider.id, connection_id: connection.id }); self.open_connect_picker(); }
            ACCOUNT_DISCONNECT_VALUE => self.picker = Some(NeoismAgentPicker::new(NeoismAgentPickerKind::ConnectConfirm, "Disconnect account?", vec![NeoismAgentPickerOption::new("Disconnect", &connection.label, "cannot be undone", CONFIRM_DISCONNECT_VALUE), NeoismAgentPickerOption::new("Cancel", "", "keep account", "cancel")], 1)),
            _ => {}
        }
    }

    fn open_account_label(&mut self, title: &str, placeholder: &str) {
        let mut picker = NeoismAgentPicker::new(NeoismAgentPickerKind::ConnectLabel, title, Vec::new(), 0);
        picker.search_placeholder = Some(placeholder.to_string());
        self.picker = Some(picker);
    }

    pub(in crate::panels::agent_pane::state) fn submit_account_label(&mut self, label: String) {
        let label = label.trim().to_string();
        if label.is_empty() { self.open_account_label("Account label", "Account label"); return; }
        let Some(action) = self.connect.as_ref().and_then(|flow| flow.label_action) else { return; };
        match action {
            LabelAction::Add => { if let Some(flow) = self.connect.as_mut() { flow.label = Some(label); } self.open_connect_auth_methods(); }
            LabelAction::Rename => {
                let Some((provider, connection)) = self.connect.as_ref().and_then(|flow| flow.provider.clone().zip(flow.connection.clone())) else { return; };
                self.push_outbound(OutboundAgentCommand::ConnectRename { provider_id: provider.id, connection_id: connection.id, label });
                self.open_connect_picker();
            }
        }
    }

    pub(in crate::panels::agent_pane::state) fn confirm_account_disconnect(&mut self, confirm: bool) {
        if confirm {
            let Some((provider, connection)) = self.connect.as_ref().and_then(|flow| flow.provider.clone().zip(flow.connection.clone())) else { return; };
            self.push_outbound(OutboundAgentCommand::ConnectDisconnect { provider_id: provider.id, connection_id: Some(connection.id.clone()) });
            if self.connection_id.as_deref() == Some(connection.id.as_str()) {
                self.system_message("Account", "The selected provider connection was deleted. Choose another account before sending.");
            }
            self.open_connect_picker();
        } else { self.close_connect(); }
    }

    /// Stage 2 → 3: the user picked an auth method. API-key methods jump
    /// straight to the secret field; OAuth methods first request an
    /// authorization URL before prompting for the token. Claude Code
    /// "Connect Meridian" is a one-click connect — nothing to type.
    pub(in crate::panels::agent_pane::state) fn start_connect_method(
        &mut self,
        method_index: usize,
    ) {
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
        // Claude Code "Connect Meridian" writes a connected marker with no
        // per-user credential; streaming routes through the local Meridian
        // proxy.
        if provider.id == "claude-code" && method.label.contains("Meridian") {
            self.push_outbound(OutboundAgentCommand::ConnectStoreApiKey {
                provider_id: provider.id.clone(),
                key: "meridian".to_string(),
                label: self.connect.as_ref().and_then(|flow| flow.label.clone()),
                connection_id: self.connect.as_ref().and_then(|flow| flow.connection.as_ref().map(|connection| connection.id.clone())),
            });
            let provider_name = provider.name.clone();
            self.close_connect();
            self.system_message(
                "Connected",
                format!(
                    "{provider_name} connected via Meridian. Open /model to pick a model."
                ),
            );
            return;
        }
        if method.is_api {
            self.open_connect_secret_entry(&method);
        } else {
            self.begin_connect_oauth(&provider, &method);
        }
    }

    /// Disconnect the provider chosen in the auth-method stage: ask the host
    /// to remove its stored auth and re-fetch the catalog so the ✓ and
    /// `/model` eligibility reflect the change. (A provider still connected
    /// via an environment variable keeps its ✓ — env auth can't be removed
    /// from here.)
    pub(in crate::panels::agent_pane::state) fn disconnect_connect_provider(&mut self) {
        let Some(provider) = self.connect.as_ref().and_then(|flow| flow.provider.clone())
        else {
            return;
        };
        self.push_outbound(OutboundAgentCommand::ConnectDisconnect {
            provider_id: provider.id.clone(),
            connection_id: self.connect.as_ref().and_then(|flow| flow.connection.as_ref().map(|connection| connection.id.clone())),
        });
        self.system_message("Disconnected", format!("{} disconnected.", provider.name));
        // Re-fetch and reopen stage 1 (matches desktop's post-disconnect
        // `open_connect_picker`). The pending catalog refresh arrives via
        // `apply_connect_catalog`.
        self.open_connect_picker();
    }

    /// Kick off an OAuth method. Requests the authorization URL from the host;
    /// the outcome arrives via [`apply_connect_oauth_url`](Self::apply_connect_oauth_url).
    fn begin_connect_oauth(
        &mut self,
        provider: &ConnectProvider,
        method: &ConnectMethod,
    ) {
        self.push_outbound(OutboundAgentCommand::ConnectOauthAuthorize {
            provider_id: provider.id.clone(),
            method_index: method.index,
            label: self.connect.as_ref().and_then(|flow| flow.label.clone()),
            connection_id: self.connect.as_ref().and_then(|flow| flow.connection.as_ref().map(|connection| connection.id.clone())),
        });
        self.system_message(provider.name.as_str(), "Requesting a sign-in link…");
        // Keep the flow (provider + method) so the authorize result can
        // continue; drop the picker while we wait.
        self.picker = None;
    }

    /// Apply an OAuth authorize response fetched by the host:
    /// - surface the auth URL as a clickable link (the host opens it),
    /// - `auto` flows (OpenAI, GitHub Copilot) ask the host to await the
    ///   server-side callback — there is nothing for the user to paste,
    /// - everything else opens the paste-a-token secret field.
    pub fn apply_connect_oauth_url(
        &mut self,
        url: String,
        auto: bool,
        instructions: String,
        attempt_id: Option<String>,
    ) {
        if let Some(flow) = self.connect.as_mut() { flow.attempt_id = attempt_id; }
        let Some((provider, method)) = self
            .connect
            .as_ref()
            .and_then(|flow| flow.provider.clone().zip(flow.method.clone()))
        else {
            return;
        };
        let mut message = String::new();
        if !instructions.trim().is_empty() {
            message.push_str(instructions.trim());
        }
        if url.starts_with("http://") || url.starts_with("https://") {
            if !message.is_empty() {
                message.push_str("\n\n");
            }
            message.push_str("Authorization URL (click to open; drag to copy):\n");
            message.push_str(&format!("[{url}]({url})"));
        }
        if !message.trim().is_empty() {
            self.system_message(provider.name.as_str(), message);
        }
        if auto {
            // No token to paste: let the host block on the callback and report
            // the result through `note_connect_finished` / `note_connect_failed`.
            self.push_outbound(OutboundAgentCommand::ConnectOauthCallback {
                provider_id: provider.id.clone(),
                method_index: method.index,
                code: None,
                attempt_id: self.connect.as_ref().and_then(|flow| flow.attempt_id.clone()),
            });
            self.system_message(
                provider.name.as_str(),
                format!(
                    "Finish signing in to {} in your browser — neoism connects automatically.",
                    provider.name
                ),
            );
            self.close_connect();
        } else {
            self.open_connect_secret_entry(&method);
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

    /// Commit the secret field (Enter). Stores an API key (`PUT /auth/:id`) or
    /// completes OAuth via the callback endpoint. Re-opens the field on an
    /// empty value so the user can retry; the request itself resolves through
    /// `note_connect_finished` / `note_connect_failed`.
    pub(in crate::panels::agent_pane::state) fn submit_connect_secret(
        &mut self,
        secret: String,
    ) {
        let secret = secret.trim().to_string();
        let Some((provider, method)) = self
            .connect
            .as_ref()
            .and_then(|flow| flow.provider.clone().zip(flow.method.clone()))
        else {
            self.close_connect();
            return;
        };
        if secret.is_empty() {
            self.open_connect_secret_entry(&method);
            return;
        }
        if method.is_api {
            self.push_outbound(OutboundAgentCommand::ConnectStoreApiKey {
                provider_id: provider.id.clone(),
                key: secret,
                label: self.connect.as_ref().and_then(|flow| flow.label.clone()),
                connection_id: self.connect.as_ref().and_then(|flow| flow.connection.as_ref().map(|connection| connection.id.clone())),
            });
        } else {
            self.push_outbound(OutboundAgentCommand::ConnectOauthCallback {
                provider_id: provider.id.clone(),
                method_index: method.index,
                code: Some(secret),
                attempt_id: self.connect.as_ref().and_then(|flow| flow.attempt_id.clone()),
            });
        }
        self.system_message(provider.name.as_str(), "Connecting…");
        // Keep `self.connect` so a failure can reopen the field for a retry.
        self.picker = None;
    }

    /// Host callback: the connect request succeeded.
    pub fn note_connect_finished(&mut self, provider_name: String) {
        self.note_connect_finished_with_connection(provider_name, None);
    }

    pub fn note_connect_finished_with_connection(
        &mut self,
        provider_name: String,
        connection_id: Option<String>,
    ) {
        if connection_id.is_some() {
            self.connection_id = connection_id;
        }
        self.close_connect();
        self.system_message(
            "Connected",
            format!("{provider_name} connected. Open /model to pick one of its models."),
        );
    }

    /// Host callback: the connect request failed. Reopens the secret field for
    /// a retry when a method is still selected (the API-key / paste-a-code
    /// path), mirroring the desktop's submit-error behaviour.
    pub fn note_connect_failed(&mut self, provider_name: String, error: String) {
        self.system_message(provider_name.as_str(), error);
        if let Some(method) = self.connect.as_ref().and_then(|flow| flow.method.clone()) {
            self.open_connect_secret_entry(&method);
        }
    }

    pub(in crate::panels::agent_pane::state) fn close_connect(&mut self) {
        self.connect = None;
        self.picker = None;
    }
}

/// Parse the provider catalog (`/provider`) and per-provider auth methods
/// (`/provider/auth`) into a [`ConnectFlow`].
fn parse_connect_flow(providers_value: Value, auth_value: Value) -> ConnectFlow {
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

    ConnectFlow {
        providers,
        methods_by_provider,
        provider: None,
        method: None,
        connections: Vec::new(),
        connection: None,
        label: None,
        label_action: None,
        attempt_id: None,
    }
}

fn account_option(connection: &ProviderConnection) -> NeoismAgentPickerOption {
    NeoismAgentPickerOption::new(
        &connection.label,
        &connection.auth_type,
        if connection.is_default { "default" } else { "account" },
        &connection.id,
    )
}

/// Build the stage-1 picker rows: a "Popular" header + the well-known
/// providers in [`POPULAR_PROVIDER_IDS`] order, then a "Providers" header +
/// the rest alphabetically. Connected providers get a leading checkmark.
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

fn connect_loading_options() -> Vec<NeoismAgentPickerOption> {
    vec![NeoismAgentPickerOption::new(
        "Loading providers...",
        "Fetching from Neoism",
        "loading",
        "",
    )]
}

fn connect_empty_options() -> Vec<NeoismAgentPickerOption> {
    vec![NeoismAgentPickerOption::new(
        "No providers available",
        "The agent server returned no providers",
        "empty",
        "",
    )]
}
