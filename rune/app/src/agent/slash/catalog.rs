//! Picker rows built from daemon catalogs, with the original projections: the model catalog
//! (`api_mapping::model_options_from_providers_json` + `gpuix/src/agent/model-picker.ts`), the
//! reasoning list, agents, MCP servers and their actions (`pickers_state.rs`), and the
//! `/connect` stages (`state/connect.rs`).

use super::picker::{Icon, Row, Status};
use neoism_protocol::agent::{AgentInfo, ProviderConnectionInfo, ProviderInfo};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const ADD_PROVIDER: &str = "action:add-provider";
pub const ADD_ACCOUNT: &str = "__add_account__";
pub const DISCONNECT_PROVIDER: &str = "__disconnect__";
pub const DONE: &str = "__connect_done__";
pub const RETRY: &str = "__connect_retry__";

/// The provider logo atlas index for a provider id (`design/generated.ts`, `providerLogos`).
pub fn provider_logo(provider: &str) -> Option<usize> {
    Some(match provider.trim().to_lowercase().as_str() {
        "openrouter" => 0,
        "anthropic" | "claude" | "claude-code" => 1,
        "openai" | "chatgpt" | "codex" => 2,
        "google" | "google-vertex" | "vertex-ai" => 3,
        "github-copilot" | "copilot" => 4,
        "vercel" | "vercel-ai-gateway" => 5,
        "opencode" | "open-code" => 6,
        "xai" | "x-ai" | "grok" => 7,
        _ => return None,
    })
}

fn provider_id(model: &str) -> Option<&str> { model.split_once('/').map(|(p, _)| p).filter(|p| !p.is_empty()) }

fn provider_rank(id: &str) -> usize {
    match id { "opencode" => 0, "openai" => 1, "anthropic" => 2, "claude-code" => 3, _ => 4 }
}

/// Every model, grouped under provider headers: preferred providers first, then by name;
/// within a provider by name. Footer is the context window (`128k ctx`).
pub fn models(providers: &[ProviderInfo]) -> Vec<Row> {
    let mut groups: Vec<(&ProviderInfo, Vec<Row>)> = providers.iter().filter(|p| !p.id.is_empty()).map(|p| {
        let name = if p.name.is_empty() { &p.id } else { &p.name };
        let mut rows: Vec<Row> = p.models.iter().map(|m| {
            let footer = m.context_limit.filter(|c| *c > 0).map(|c| format!("{}k ctx", (c as f32 / 1000.0).round() as u64)).unwrap_or_default();
            let title = if m.name.is_empty() { &m.id } else { &m.name };
            let value = if m.id.starts_with(&format!("{}/", p.id)) { m.id.clone() } else { format!("{}/{}", p.id, m.id) };
            Row::new(title.clone(), "", footer, value).section(name.clone())
        }).collect();
        rows.sort_by_key(|r| r.title.to_lowercase());
        (p, rows)
    }).filter(|(_, rows)| !rows.is_empty()).collect();
    groups.sort_by(|(a, _), (b, _)| provider_rank(&a.id).cmp(&provider_rank(&b.id)).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    groups.into_iter().flat_map(|(p, rows)| std::iter::once(Row::header(if p.name.is_empty() { p.id.clone() } else { p.name.clone() })).chain(rows)).collect()
}

/// The model picker's first screen: up to three recent models (the current one first), then
/// providers with their model counts, then "Add Provider".
pub fn model_root(catalog: &[Row], current: &str, recent: &[String]) -> Vec<Row> {
    let available: BTreeMap<&str, &Row> = catalog.iter().filter(|r| !r.header).map(|r| (r.value.as_str(), r)).collect();
    let mut out = Vec::new();
    let mut seen = Vec::new();
    for value in std::iter::once(current).chain(recent.iter().map(String::as_str)) {
        if seen.len() == 3 || seen.contains(&value) || !value.contains('/') { continue; }
        let Some(row) = available.get(value) else { continue };
        seen.push(value);
        let is_current = value == current;
        out.push(Row::new(row.title.clone(), row.section.clone(), if is_current { "selected" } else { "" }, value)
            .current(is_current).icon(provider_id(value).and_then(provider_logo).map(Icon::Provider)));
    }
    if !out.is_empty() { out.insert(0, Row::header("Recent models")); }
    out.push(Row::header("Providers"));
    let mut providers: Vec<(String, String, usize, bool)> = Vec::new();
    for row in catalog.iter().filter(|r| !r.header) {
        let Some(id) = provider_id(&row.value) else { continue };
        match providers.iter_mut().find(|p| p.0 == id) {
            Some(p) => { p.2 += 1; p.3 |= row.value == current; }
            None => providers.push((id.to_owned(), row.section.clone(), 1, row.value == current)),
        }
    }
    out.extend(providers.into_iter().map(|(id, name, count, current)| {
        Row::new(name, "", format!("{count} {}", if count == 1 { "model" } else { "models" }), id.clone())
            .current(current).icon(provider_logo(&id).map(Icon::Provider))
    }));
    out.push(Row::new("Add Provider", "", "", ADD_PROVIDER).icon(Some(Icon::Glyph("+"))));
    out
}

/// One provider's models (the second model screen).
pub fn provider_models(catalog: &[Row], provider: &str, current: &str) -> Vec<Row> {
    let prefix = format!("{provider}/");
    catalog.iter().filter(|r| !r.header && r.value.starts_with(&prefix)).map(|r| {
        let mut row = r.clone();
        row.current = r.value == current;
        row
    }).collect()
}

/// Reasoning effort, with `ultra` only for models that support it.
pub fn thinking(model: &str, current: &str) -> Vec<Row> {
    let mut rows = vec![
        Row::new("none", "Use model default reasoning", "default", ""),
        Row::new("low", "Fastest reasoning", "think", "low"),
        Row::new("medium", "Balanced reasoning", "think", "medium"),
        Row::new("high", "More reasoning", "think", "high"),
        Row::new("xhigh", "Maximum reasoning", "think", "xhigh"),
    ];
    if model.to_lowercase().contains("gpt-5.6") {
        rows.push(Row::new("ultra", "Multi-agent reasoning (GPT-5.6)", "think", "ultra"));
    }
    rows.into_iter().map(|r| { let current = r.value == current; r.current(current) }).collect()
}

pub fn agents(agents: &[AgentInfo], current: &str) -> Vec<Row> {
    agents.iter().filter(|a| a.mode.as_deref() != Some("subagent")).map(|a| {
        Row::new(a.name.clone(), a.description.clone(), "", a.name.clone()).current(a.name == current)
    }).collect()
}

fn mcp_scope(entry: &Value) -> &'static str {
    match entry["configScope"].as_str() { Some("global") => "Global", Some("workspace") => "Workspace", _ => "Unknown scope" }
}

/// MCP servers with their state (`mcp_options_from_status`). Each row's value carries what the
/// actions stage needs.
pub fn mcp_servers(status: &Value) -> Vec<Row> {
    let Some(servers) = status.as_object() else { return Vec::new() };
    servers.iter().map(|(name, entry)| {
        let state = entry.get("status").filter(|s| s.is_object()).unwrap_or(entry);
        let oauth = entry["oauthCapable"].as_bool().unwrap_or(false);
        let status = state["status"].as_str().unwrap_or("failed");
        let error = state["error"].as_str().unwrap_or_default();
        let not_started = error == "MCP client runtime is not connected yet";
        let (description, footer, dot) = match status {
            "connected" => ("Available to Neoism".to_owned(), "connected", Status::Good),
            "disabled" if oauth => ("Click to authenticate with OAuth".to_owned(), "needs auth", Status::Warning),
            "disabled" => ("Disabled in configuration".to_owned(), "disabled", Status::Muted),
            "needs_auth" => ("Click to authenticate with OAuth".to_owned(), "needs auth", Status::Warning),
            "needs_client_registration" => (if error.is_empty() { "Click to register and authenticate".to_owned() } else { error.to_owned() }, "needs registration", Status::Warning),
            "failed" if not_started && oauth => ("OAuth configured; click to authenticate again".to_owned(), "ready", Status::Muted),
            "failed" if not_started => ("Ready to connect".to_owned(), "ready", Status::Muted),
            _ => (error.to_owned(), "failed", Status::Error),
        };
        let writable = entry["configWritable"].as_bool().unwrap_or(true);
        let description = format!("{}{} \u{b7} {description}", mcp_scope(entry), if writable { "" } else { " (read-only)" });
        let value = json!({
            "name": name,
            "configScope": entry.get("configScope"),
            "enabled": entry["enabled"].as_bool().unwrap_or(status != "disabled"),
            "connected": entry["runtimeConnected"].as_bool().unwrap_or(status == "connected"),
            "oauthCapable": oauth,
            "hasCredentials": entry["hasCredentials"].as_bool().unwrap_or(false),
            "configWritable": writable,
        });
        Row::new(name.clone(), description, footer, value.to_string()).status(Some(dot))
    }).collect()
}

/// What can be done to one MCP server (`mcp_action_options`).
pub fn mcp_actions(entry: &Value) -> Vec<Row> {
    let name = entry["name"].as_str().unwrap_or_default();
    let flag = |k: &str| entry[k].as_bool().unwrap_or(false);
    let (enabled, connected, oauth, credentials, writable) = (flag("enabled"), flag("connected"), flag("oauthCapable"), flag("hasCredentials"), flag("configWritable"));
    let action = |title: &str, description: String, footer: &str, action: &str| Row::new(title, description, footer, json!({"name": name, "action": action}).to_string());
    let scope = mcp_scope(entry);
    let mut rows = Vec::new();
    if writable {
        rows.push(if enabled {
            action("Disable", format!("Persist enabled: false in {scope} config and stop the runtime"), "config", "disable")
        } else {
            action("Enable", format!("Persist enabled: true in {scope} config"), "config", "enable")
        });
    }
    if enabled {
        rows.push(if connected {
            action("Disconnect", "Stop this MCP runtime without disabling it".into(), "runtime", "disconnect")
        } else if oauth && !credentials {
            action("Connect", "Authenticate with OAuth and connect this MCP".into(), "OAuth", "authenticate")
        } else {
            action("Connect", "Start this MCP runtime now".into(), "runtime", "connect")
        });
    }
    if oauth && credentials { rows.push(action("Reauthenticate", "Open the MCP OAuth flow again".into(), "OAuth", "authenticate")); }
    if credentials { rows.push(action("Log out", "Remove saved OAuth credentials".into(), "OAuth", "logout")); }
    rows
}

/// A provider in the `/connect` catalog.
#[derive(Clone, Debug, PartialEq)]
pub struct ConnectProvider {
    pub id: String,
    pub name: String,
    pub connected: bool,
}

/// One way to sign in to a provider; `index` is what the daemon expects back.
#[derive(Clone, Debug, PartialEq)]
pub struct AuthMethod {
    pub index: usize,
    pub label: String,
    pub api: bool,
}

const POPULAR: &[&str] = &["claude-code", "anthropic", "openai", "openrouter", "github-copilot"];

/// `GET /provider` (`{all, connected}`) and `GET /provider/auth` as the daemon relays them.
pub fn parse_connect(providers: &Value, auth: &Value) -> (Vec<ConnectProvider>, BTreeMap<String, Vec<AuthMethod>>) {
    let connected: Vec<&str> = providers["connected"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let list = providers["all"].as_array().into_iter().flatten().filter_map(|p| {
        let id = p["id"].as_str().filter(|id| !id.is_empty() && *id != "claude-code")?;
        let name = p["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(id);
        Some(ConnectProvider { id: id.into(), name: name.into(), connected: connected.contains(&id) })
    }).collect();
    let methods = auth.as_object().into_iter().flatten().map(|(provider, methods)| {
        let methods = methods.as_array().into_iter().flatten().enumerate().filter_map(|(index, m)| {
            let kind = m["type"].as_str()?;
            Some(AuthMethod { index, label: m["label"].as_str().unwrap_or(kind).into(), api: kind == "api" })
        }).collect();
        (provider.clone(), methods)
    }).collect();
    (list, methods)
}

/// Stage 1: "Popular" in a fixed order, then the rest by name; connected ones get a check.
pub fn connect_providers(providers: &[ConnectProvider]) -> Vec<Row> {
    let row = |p: &ConnectProvider| Row::new(if p.connected { format!("\u{2713} {}", p.name) } else { p.name.clone() }, "", if p.connected { "connected" } else { "" }, p.id.clone())
        .icon(provider_logo(&p.id).map(Icon::Provider));
    let popular: Vec<&ConnectProvider> = POPULAR.iter().filter_map(|id| providers.iter().find(|p| p.id == *id)).collect();
    let mut rest: Vec<&ConnectProvider> = providers.iter().filter(|p| !POPULAR.contains(&p.id.as_str())).collect();
    rest.sort_by_key(|p| p.name.to_lowercase());
    let mut rows = Vec::new();
    if !popular.is_empty() { rows.push(Row::header("Popular")); rows.extend(popular.into_iter().map(row)); }
    if !rest.is_empty() { rows.push(Row::header("Providers")); rows.extend(rest.into_iter().map(row)); }
    if rows.is_empty() { rows.push(Row::new("No providers available", "The agent server returned no providers", "empty", "")); }
    rows
}

/// Stage 2: add an account, or manage a connected one.
pub fn connect_accounts(connections: &[ProviderConnectionInfo]) -> Vec<Row> {
    let mut rows = vec![Row::new("Add account", "", "add", ADD_ACCOUNT)];
    if !connections.is_empty() { rows.push(Row::header("Connected accounts")); }
    rows.extend(connections.iter().map(|c| Row::new(c.label.clone(), c.auth_type.clone(), if c.is_default { "default" } else { "account" }, c.connection_id.clone())));
    rows
}

/// Stage 3: the provider's sign-in methods (and disconnect when already connected).
pub fn connect_methods(provider: &ConnectProvider, methods: &[AuthMethod]) -> Vec<Row> {
    let mut rows = Vec::new();
    if provider.connected { rows.push(Row::new(format!("Disconnect {}", provider.name), "", "remove auth", DISCONNECT_PROVIDER)); }
    rows.extend(methods.iter().map(|m| Row::new(m.label.clone(), "", if m.api { "api key" } else { "oauth" }, m.index.to_string())));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_protocol::agent::ModelInfo;

    fn catalog() -> Vec<ProviderInfo> {
        let model = |id: &str, name: &str, ctx| ModelInfo { id: id.into(), name: name.into(), context_limit: ctx };
        vec![
            ProviderInfo { id: "deepseek".into(), name: "DeepSeek".into(), models: vec![model("chat", "DeepSeek Chat", Some(64000))] },
            ProviderInfo { id: "openai".into(), name: "OpenAI".into(), models: vec![model("gpt-6.1-sol", "GPT-6.1 Sol", Some(400000)), model("gpt-5", "GPT-5", None)] },
        ]
    }

    #[test]
    fn models_group_by_provider_preferred_first() {
        let rows = models(&catalog());
        assert_eq!(rows[0].title, "OpenAI");
        assert!(rows[0].header);
        assert_eq!(rows[1].value, "openai/gpt-5");
        assert_eq!(rows[2].footer, "400k ctx");
        assert_eq!(rows[3].title, "DeepSeek");
    }

    #[test]
    fn model_root_lists_recent_then_providers() {
        let rows = models(&catalog());
        let root = model_root(&rows, "openai/gpt-6.1-sol", &["openai/gpt-6.1-sol".into(), "gone/x".into()]);
        let titles: Vec<&str> = root.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(titles, ["Recent models", "GPT-6.1 Sol", "Providers", "OpenAI", "DeepSeek", "Add Provider"]);
        assert_eq!(root[1].footer, "selected");
        assert_eq!(root[3].footer, "2 models");
        assert!(root[3].current);
        assert_eq!(provider_models(&rows, "openai", "openai/gpt-5").iter().filter(|r| r.current).count(), 1);
    }

    #[test]
    fn mcp_rows_and_actions() {
        let status = json!({"webflow": {"status": {"status": "needs_auth"}, "oauthCapable": true, "enabled": true, "configScope": "global"}});
        let rows = mcp_servers(&status);
        assert_eq!(rows[0].footer, "needs auth");
        assert_eq!(rows[0].description, "Global \u{b7} Click to authenticate with OAuth");
        let entry: Value = serde_json::from_str(&rows[0].value).unwrap();
        let titles: Vec<String> = mcp_actions(&entry).into_iter().map(|r| r.title).collect();
        assert_eq!(titles, ["Disable", "Connect"]);
    }

    #[test]
    fn connect_groups_popular_first() {
        let (providers, methods) = parse_connect(
            &json!({"all": [{"id": "zai", "name": "Z.ai"}, {"id": "openai", "name": "OpenAI"}, {"id": "claude-code", "name": "Claude Code"}], "connected": ["openai"]}),
            &json!({"openai": [{"type": "oauth", "label": "ChatGPT"}, {"type": "api"}]}),
        );
        let rows = connect_providers(&providers);
        let titles: Vec<&str> = rows.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(titles, ["Popular", "\u{2713} OpenAI", "Providers", "Z.ai"]);
        let auth = connect_methods(&providers[1], &methods["openai"]);
        assert_eq!(auth.iter().map(|r| r.footer.as_str()).collect::<Vec<_>>(), ["remove auth", "oauth", "api key"]);
    }
}
