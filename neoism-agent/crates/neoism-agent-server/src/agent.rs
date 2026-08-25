use std::collections::BTreeMap;

use neoism_agent_core::{AgentConfig, AgentInfo, ModelRef, NeoismConfig};
#[cfg(test)]
use serde_json::json;
use serde_json::Value;

#[path = "agent_native.rs"]
mod agent_native;

use agent_native::{build_agent, native_agents};

#[derive(Clone)]
pub(crate) struct AgentCatalog {
    agents: BTreeMap<String, AgentInfo>,
    default_agent: String,
}

impl AgentCatalog {
    pub(crate) fn load(services: &neoism_agent_service_api::AgentServices, directory: &str) -> anyhow::Result<Self> {
        let loaded = crate::config::load(services, directory)?;
        Ok(Self::from_config(&loaded.info))
    }

    pub(crate) fn from_config(config: &NeoismConfig) -> Self {
        let mut agents = native_agents();

        for agent in agents.values_mut() {
            merge_permission(&mut agent.permission, config.permission.clone());
        }

        for (name, override_config) in &config.agent {
            if override_config.disable {
                agents.remove(name);
                continue;
            }

            let entry = agents
                .entry(name.clone())
                .or_insert_with(|| custom_agent(name, &config.permission));
            apply_override(entry, override_config);
        }

        Self::with_default(agents, config.default_agent.clone())
    }

    pub(crate) fn list(&self) -> Vec<AgentInfo> {
        let mut agents = self.agents.values().cloned().collect::<Vec<_>>();
        agents.sort_by(|left, right| {
            let left_default = left.name == self.default_agent;
            let right_default = right.name == self.default_agent;
            right_default
                .cmp(&left_default)
                .then_with(|| left.name.cmp(&right.name))
        });
        agents
    }

    pub(crate) fn get(&self, name: &str) -> Option<AgentInfo> {
        self.agents.get(name).cloned().or_else(|| {
            self.agents
                .values()
                .find(|agent| agent.name == name)
                .cloned()
        })
    }

    pub(crate) fn default_agent(&self) -> &str {
        &self.default_agent
    }

    pub(crate) fn from_runtime(
        agents: Vec<AgentInfo>,
        default_agent: String,
    ) -> Self {
        Self {
            agents: agents
                .into_iter()
                .map(|agent| (agent.name.clone(), agent))
                .collect(),
            default_agent,
        }
    }

    fn with_default(
        agents: BTreeMap<String, AgentInfo>,
        configured: Option<String>,
    ) -> Self {
        let default_agent = configured
            .filter(|name| is_valid_default(agents.get(name)))
            .or_else(|| {
                agents
                    .values()
                    .find(|agent| agent.mode != "subagent" && !agent.hidden)
                    .map(|agent| agent.name.clone())
            })
            .unwrap_or_else(|| "build".to_string());
        Self {
            agents,
            default_agent,
        }
    }
}

fn apply_override(agent: &mut AgentInfo, config: &AgentConfig) {
    if let Some(model) = config
        .model
        .as_deref()
        .and_then(|model| parse_model_ref(model, config.variant.clone()))
    {
        agent.model = Some(model);
    }
    agent.variant = config.variant.clone().or_else(|| agent.variant.clone());
    agent.prompt = config.prompt.clone().or_else(|| agent.prompt.clone());
    agent.description = config
        .description
        .clone()
        .or_else(|| agent.description.clone());
    agent.temperature = config.temperature.or(agent.temperature);
    agent.top_p = config.top_p.or(agent.top_p);
    agent.mode = config.mode.clone().unwrap_or_else(|| agent.mode.clone());
    agent.color = config.color.clone().or_else(|| agent.color.clone());
    agent.hidden = config.hidden.unwrap_or(agent.hidden);
    agent.name = config.name.clone().unwrap_or_else(|| agent.name.clone());
    agent.steps = config.steps.or(agent.steps);
    merge_json_map(&mut agent.options, config.options.clone());
    merge_permission(&mut agent.permission, config.permission.clone());
}

fn custom_agent(name: &str, user_permission: &BTreeMap<String, Value>) -> AgentInfo {
    let mut agent = build_agent();
    agent.name = name.to_string();
    agent.description = None;
    agent.mode = "all".to_string();
    agent.native = false;
    agent.hidden = false;
    agent.prompt = None;
    agent.color = None;
    merge_permission(&mut agent.permission, user_permission.clone());
    agent
}

fn parse_model_ref(value: &str, variant: Option<String>) -> Option<ModelRef> {
    let (provider_id, model_id) = value.split_once('/')?;
    Some(ModelRef {
        provider_id: provider_id.to_string(),
        id: model_id.to_string(),
        variant,
    })
}

fn is_valid_default(agent: Option<&AgentInfo>) -> bool {
    agent
        .map(|agent| agent.mode != "subagent" && !agent.hidden)
        .unwrap_or(false)
}

fn merge_permission(
    target: &mut BTreeMap<String, Value>,
    source: BTreeMap<String, Value>,
) {
    merge_json_map(target, source);
}

fn merge_json_map(target: &mut BTreeMap<String, Value>, source: BTreeMap<String, Value>) {
    for (key, value) in source {
        match (target.get_mut(&key), value) {
            (Some(Value::Object(target)), Value::Object(source)) => {
                for (key, value) in source {
                    target.insert(key, value);
                }
            }
            (_, value) => {
                target.insert(key, value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_agent_core::PermissionAction;

    #[test]
    fn per_agent_permission_overrides_global_permission() {
        let mut config = NeoismConfig::default();
        config.permission.insert("bash".to_string(), json!("deny"));
        config.agent.insert(
            "build".to_string(),
            AgentConfig {
                permission: BTreeMap::from([("bash".to_string(), json!("allow"))]),
                ..AgentConfig::default()
            },
        );

        let catalog = AgentCatalog::from_config(&config);
        let rules =
            crate::permission::from_config_map(&catalog.get("build").unwrap().permission);

        assert_eq!(
            crate::permission::evaluate("bash", "cargo test", &rules).action,
            PermissionAction::Allow
        );
    }

    #[test]
    fn agent_name_override_keeps_key_and_display_lookup() {
        let mut config = NeoismConfig::default();
        config.agent.insert(
            "build".to_string(),
            AgentConfig {
                name: Some("Builder".to_string()),
                ..AgentConfig::default()
            },
        );

        let catalog = AgentCatalog::from_config(&config);

        assert_eq!(catalog.get("build").unwrap().name, "Builder");
        assert_eq!(catalog.get("Builder").unwrap().name, "Builder");
    }

    #[test]
    fn native_build_and_plan_include_ported_prompts() {
        let catalog = AgentCatalog::from_config(&NeoismConfig::default());

        let build = catalog.get("build").unwrap();
        let plan = catalog.get("plan").unwrap();
        let general = catalog.get("general").unwrap();
        let explore = catalog.get("explore").unwrap();

        assert!(build.prompt.unwrap().contains("GOLDEN STANDARD"));
        assert!(plan.prompt.unwrap().contains("plan agent"));
        assert_eq!(general.mode, "subagent");
        assert_eq!(explore.mode, "subagent");
        assert!(explore.prompt.unwrap().contains("file search specialist"));
    }

    #[test]
    fn plan_agent_asks_for_general_edits_but_allows_plan_files() {
        let catalog = AgentCatalog::from_config(&NeoismConfig::default());
        let rules =
            crate::permission::from_config_map(&catalog.get("plan").unwrap().permission);

        assert_eq!(
            crate::permission::evaluate("edit", "src/lib.rs", &rules).action,
            PermissionAction::Ask
        );
        assert_eq!(
            crate::permission::evaluate("edit", ".opencode/plans/next.md", &rules).action,
            PermissionAction::Allow
        );
        assert_eq!(
            crate::permission::evaluate("edit", ".neoism/plans/next.md", &rules).action,
            PermissionAction::Allow
        );
    }
}
