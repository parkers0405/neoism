use std::collections::BTreeMap;

use neoism_agent_core::{AgentConfig, AgentConfigDocument, AgentInfo, ModelRef};
use neoism_agent_plugin_api::AgentSourceSnapshot;
use serde_json::Value;

use super::native::{build_agent, native_agents};

#[derive(Clone)]
pub struct AgentCatalog {
    agents: BTreeMap<String, AgentInfo>,
    default_agent: String,
}

impl AgentCatalog {
    pub fn from_config(config: &AgentConfigDocument) -> Self {
        let mut agents = native_agents();
        for agent in agents.values_mut() {
            merge_json_map(&mut agent.permission, config.permission.clone());
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

    pub fn from_snapshot(snapshot: AgentSourceSnapshot) -> Self {
        Self {
            agents: snapshot
                .agents
                .into_iter()
                .map(|agent| (agent.name.clone(), agent))
                .collect(),
            default_agent: snapshot.default_agent,
        }
    }

    pub fn list(&self) -> Vec<AgentInfo> {
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

    pub fn get(&self, name: &str) -> Option<AgentInfo> {
        self.agents.get(name).cloned().or_else(|| {
            self.agents
                .values()
                .find(|agent| agent.name == name)
                .cloned()
        })
    }

    pub fn default_agent(&self) -> &str {
        &self.default_agent
    }

    pub fn snapshot(&self) -> AgentSourceSnapshot {
        AgentSourceSnapshot {
            agents: self.list(),
            default_agent: self.default_agent.clone(),
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
    merge_json_map(&mut agent.permission, config.permission.clone());
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
    merge_json_map(&mut agent.permission, user_permission.clone());
    agent
}

fn parse_model_ref(value: &str, variant: Option<String>) -> Option<ModelRef> {
    let (provider_id, model_id) = value.split_once('/')?;
    Some(ModelRef {
        provider_id: provider_id.to_string(),
        id: model_id.to_string(),
        connection_id: None,
        variant,
    })
}

fn is_valid_default(agent: Option<&AgentInfo>) -> bool {
    agent
        .map(|agent| agent.mode != "subagent" && !agent.hidden)
        .unwrap_or(false)
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
    use serde_json::json;

    #[test]
    fn config_permissions_and_display_names_are_interpreted_in_builtins() {
        let mut config = AgentConfigDocument::default();
        config.permission.insert("bash".to_string(), json!("deny"));
        config.agent.insert(
            "build".to_string(),
            AgentConfig {
                name: Some("Builder".to_string()),
                permission: BTreeMap::from([("bash".to_string(), json!("allow"))]),
                ..AgentConfig::default()
            },
        );
        let catalog = AgentCatalog::from_config(&config);
        assert_eq!(catalog.get("build").unwrap().name, "Builder");
        assert_eq!(
            catalog.get("Builder").unwrap().permission["bash"],
            json!("allow")
        );
    }

    #[test]
    fn native_agents_preserve_prompts_and_modes() {
        let catalog = AgentCatalog::from_config(&AgentConfigDocument::default());
        assert!(catalog
            .get("build")
            .unwrap()
            .prompt
            .unwrap()
            .contains("GOLDEN STANDARD"));
        assert!(catalog
            .get("plan")
            .unwrap()
            .prompt
            .unwrap()
            .contains("plan agent"));
        assert_eq!(catalog.get("general").unwrap().mode, "subagent");
        assert!(catalog
            .get("explore")
            .unwrap()
            .prompt
            .unwrap()
            .contains("file search specialist"));
    }

    #[test]
    fn configured_default_is_first_and_plan_edits_remain_scoped() {
        let config = AgentConfigDocument {
            default_agent: Some("plan".to_string()),
            ..AgentConfigDocument::default()
        };
        let catalog = AgentCatalog::from_config(&config);
        assert_eq!(
            catalog.list().first().map(|agent| agent.name.as_str()),
            Some("plan")
        );

        let edit = catalog.get("plan").unwrap().permission["edit"]
            .as_object()
            .cloned()
            .unwrap();
        assert_eq!(edit["*"], json!("deny"));
        assert_eq!(edit[".agent/plans/*.md"], json!("allow"));
        assert_eq!(
            catalog.get("build").unwrap().permission["*"],
            json!("allow")
        );
    }

    #[test]
    fn native_prompts_are_product_neutral() {
        let catalog = AgentCatalog::from_config(&AgentConfigDocument::default());
        let prompts = ["build", "plan", "general", "explore"]
            .into_iter()
            .filter_map(|name| catalog.get(name).and_then(|agent| agent.prompt))
            .collect::<Vec<_>>()
            .join("\n")
            .to_ascii_lowercase();
        for assumption in ["neoism", "vault", "product documentation", "durable memory"] {
            assert!(
                !prompts.contains(assumption),
                "provider prompt contains {assumption}"
            );
        }
    }
}
