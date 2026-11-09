//! The composer's `/` commands and the pickers that open above it: the command menu, model,
//! agent and reasoning choices, MCP servers, provider connections, sessions and skills.
//! `plan` parses a line, `catalog` turns daemon catalogs into rows, `picker` filters and
//! selects, `actions` runs choices against the daemon, `view` draws the card, and `effects`
//! plays the fun `/piss` … `/praise` skits.

mod actions;
mod catalog;
pub mod effects;
mod logos;
mod picker;
mod plan;
mod view;

pub use picker::{Kind, Picker};
pub use view::{effect_layer, picker_card, provider_mark};

use catalog::{AuthMethod, ConnectProvider};
use neoism_protocol::agent::{AgentInfo, ProviderConnectionInfo, ProviderInfo};
use std::collections::BTreeMap;

/// The model, agent and reasoning effort new prompts use. Raw ids (`openai/gpt-6.1-sol`,
/// `build`, `medium`); the composer chips title-case them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection {
    pub model: String,
    pub agent: String,
    pub thinking: String,
}

/// Where a `/connect` run is: the provider catalog, then the chosen provider, account, method.
#[derive(Default)]
struct ConnectFlow {
    providers: Vec<ConnectProvider>,
    methods: BTreeMap<String, Vec<AuthMethod>>,
    provider: Option<ConnectProvider>,
    method: Option<AuthMethod>,
    connections: Vec<ProviderConnectionInfo>,
    connection: Option<ProviderConnectionInfo>,
    label: Option<String>,
    /// Naming a new account (`true`) or renaming the chosen one.
    adding: bool,
    attempt_id: Option<String>,
    /// The request whose reply the flow is waiting for, and a status line for it.
    waiting: bool,
}

/// Composer state the slash system owns.
#[derive(Default)]
pub struct Composer {
    pub picker: Option<Picker>,
    pub selection: Selection,
    /// Recently chosen models, newest first.
    pub recent_models: Vec<String>,
    providers: Vec<ProviderInfo>,
    agents: Vec<AgentInfo>,
    mcp_status: serde_json::Value,
    /// The MCP server whose actions are open, and OAuth waiting on it.
    mcp_server: Option<String>,
    connect: Option<ConnectFlow>,
    /// The helper row under the composer (`/hints`).
    pub hints_hidden: bool,
    /// Provider marks tinted for the current theme.
    logos: logos::Logos,
    /// A running `/piss` … `/praise` skit and when its follow-up prompt is due.
    pub effect: Option<effects::Running>,
    /// Esc closed the `/` menu; it stays closed until the line stops being a command.
    slash_dismissed: bool,
    next_request: u64,
    next_notice: u64,
}

impl Composer {
    /// The catalog's context window for `model` (`provider/id`, or an alias's id).
    pub fn context_limit(&self, model: &str) -> Option<u64> {
        let (provider, id) = model.split_once('/')?;
        let models = &self.providers.iter().find(|p| p.id.eq_ignore_ascii_case(provider))?.models;
        models.iter().find(|m| m.id.eq_ignore_ascii_case(id) || m.id.eq_ignore_ascii_case(model))?.context_limit.filter(|l| *l > 0)
    }

    pub fn new(model: Option<&str>, thinking: Option<&str>) -> Composer {
        Composer {
            selection: Selection {
                model: model.unwrap_or_default().to_owned(),
                agent: "build".into(),
                thinking: thinking.unwrap_or("medium").to_owned(),
            },
            ..Composer::default()
        }
    }

    fn request_id(&mut self, prefix: &str) -> String {
        self.next_request += 1;
        format!("rune-{prefix}-{}", self.next_request)
    }

    fn next_notice(&mut self) -> u64 {
        self.next_notice += 1;
        self.next_notice
    }
}
