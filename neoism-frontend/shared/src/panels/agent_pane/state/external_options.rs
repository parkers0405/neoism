//! Provider-confirmed ACP option snapshots. Values and order are never synthesized.
use serde_json::Value;

pub fn auth_required(error: &str) -> bool {
    error.contains(" ACP authentication required for ")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalChoice {
    pub value: String,
    pub name: String,
    pub group: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalOption {
    pub id: String,
    pub name: String,
    pub category: String,
    pub current_value: String,
    pub choices: Vec<ExternalChoice>,
}

impl ExternalOption {
    pub fn selected_label(&self) -> &str {
        self.choices
            .iter()
            .find(|choice| choice.value == self.current_value)
            .map_or(self.current_value.as_str(), |choice| choice.name.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalCommand {
    pub name: String,
    pub description: String,
    pub input_hint: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalOptions {
    pub provider: String,
    pub external_session_id: Option<String>,
    pub mode_fallback: bool,
    pub options: Vec<ExternalOption>,
    pub available_commands: Vec<ExternalCommand>,
    pub replay_error: Option<String>,
    pub catalog_stale: bool,
}

impl ExternalOptions {
    pub fn footer_order(&self) -> Vec<usize> {
        self.display_order()
            .into_iter()
            .filter(|&index| {
                let category = self.options[index].category.as_str();
                matches!(category, "model" | "thought_level")
                    || self.provider == "opencode" && category == "mode"
            })
            .collect()
    }

    pub fn display_order(&self) -> Vec<usize> {
        let mut indices: Vec<_> = (0..self.options.len())
            .filter(|&index| {
                !(self.provider == "claude" && self.options[index].category == "mode")
            })
            .collect();
        indices.sort_by_key(|&index| match self.options[index].category.as_str() {
            "mode" => 0,
            "model" => 1,
            "thought_level" => 2,
            _ => 3,
        });
        indices
    }

    pub fn parse(value: &Value, expected_provider: &str) -> Result<Self, String> {
        let provider = value
            .get("provider")
            .and_then(Value::as_str)
            .ok_or("Provider options are missing a provider")?;
        if provider != expected_provider {
            return Err("Provider options belong to another provider".into());
        }
        let external_session_id = match value.get("externalSessionId") {
            Some(Value::Null) => None,
            Some(Value::String(id)) if !id.is_empty() => Some(id.clone()),
            _ => return Err("Provider options have an invalid externalSessionId".into()),
        };
        let mode_fallback = value
            .get("modeFallback")
            .and_then(Value::as_bool)
            .ok_or("Provider options are missing modeFallback")?;
        value
            .get("selectedOptions")
            .and_then(Value::as_object)
            .ok_or("Provider options are missing selectedOptions")?;
        let entries = value
            .get("configOptions")
            .and_then(Value::as_array)
            .ok_or("Provider did not return configOptions")?;
        let mut options = Vec::new();
        for entry in entries {
            if entry.get("type").and_then(Value::as_str) != Some("select") {
                continue;
            }
            let (Some(id), Some(name), Some(current), Some(raw_choices)) = (
                entry.get("id").and_then(Value::as_str),
                entry.get("name").and_then(Value::as_str),
                entry.get("currentValue").and_then(Value::as_str),
                entry.get("options").and_then(Value::as_array),
            ) else {
                continue;
            };
            let category = entry.get("category").and_then(Value::as_str).unwrap_or("");
            let mut choices = Vec::new();
            for choice in raw_choices {
                if let Some(group) = choice.get("options").and_then(Value::as_array) {
                    let group_name = choice
                        .get("name")
                        .and_then(Value::as_str)
                        .or_else(|| choice.get("label").and_then(Value::as_str))
                        .map(str::to_owned);
                    for item in group {
                        push_choice(&mut choices, item, group_name.as_deref());
                    }
                } else {
                    push_choice(&mut choices, choice, None);
                }
            }
            if choices.is_empty() {
                continue;
            }
            options.push(ExternalOption {
                id: id.to_owned(),
                name: name.to_owned(),
                category: category.to_owned(),
                current_value: current.to_owned(),
                choices,
            });
        }
        let mut available_commands = Vec::new();
        if let Some(commands) = value.get("availableCommands").and_then(Value::as_array) {
            for command in commands {
                let (Some(name), Some(description)) = (
                    command.get("name").and_then(Value::as_str),
                    command.get("description").and_then(Value::as_str),
                ) else {
                    continue;
                };
                if name.is_empty()
                    || name.contains('/')
                    || name.chars().any(char::is_whitespace)
                {
                    continue;
                }
                if available_commands
                    .iter()
                    .any(|existing: &ExternalCommand| existing.name == name)
                {
                    continue;
                }
                available_commands.push(ExternalCommand {
                    name: name.to_string(),
                    description: description.to_string(),
                    input_hint: command
                        .pointer("/input/hint")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                });
            }
        }
        let replay_error = value
            .get("replayError")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let catalog_stale = value
            .get("catalogStale")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(Self {
            provider: provider.to_owned(),
            external_session_id,
            mode_fallback,
            options,
            available_commands,
            replay_error,
            catalog_stale,
        })
    }
}

fn push_choice(choices: &mut Vec<ExternalChoice>, item: &Value, group: Option<&str>) {
    let Some(value) = item.get("value").and_then(Value::as_str) else {
        return;
    };
    let name = item.get("name").and_then(Value::as_str).unwrap_or(value);
    choices.push(ExternalChoice {
        value: value.to_owned(),
        name: name.to_owned(),
        group: group.map(str::to_owned),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn auth_cue_requires_explicit_acp_auth_failure() {
        assert!(auth_required("Neoism Agent HTTP 400 Bad Request: Codex ACP authentication required for session/new: Authentication required"));
        assert!(!auth_required(
            "Neoism Agent HTTP 400 Bad Request: Codex ACP session/new failed"
        ));
    }

    #[test]
    fn preview_stale_marker_does_not_change_provider_choices() {
        let response = json!({"provider":"opencode","externalSessionId":null,"modeFallback":false,
            "selectedOptions":{},"catalogStale":true,"configOptions":[{"id":"model","name":"Model",
            "category":"model","type":"select","currentValue":"a","options":[{"value":"a","name":"A"}]}]});
        let cached = ExternalOptions::parse(&response, "opencode").unwrap();
        assert!(cached.catalog_stale);
        assert_eq!(cached.options[0].current_value, "a");
        let mut fresh = response;
        fresh.as_object_mut().unwrap().remove("catalogStale");
        assert!(
            !ExternalOptions::parse(&fresh, "opencode")
                .unwrap()
                .catalog_stale
        );
    }

    #[test]
    fn claude_hides_permissions_mode_without_changing_other_provider_order_or_ids() {
        let snapshot = ExternalOptions::parse(&json!({
            "provider":"claude","externalSessionId":"id","modeFallback":false,"selectedOptions":{},
            "configOptions":[
                {"id":"cache","name":"Cache","category":"model_config","type":"select","currentValue":"on","options":[{"value":"on","name":"On"}]},
                {"id":"thinking","name":"Effort","category":"thought_level","type":"select","currentValue":"high","options":[{"value":"high","name":"High"}]},
                {"id":"model","name":"Model","category":"model","type":"select","currentValue":"x","options":[{"value":"x","name":"X"}]},
                {"id":"mode","name":"Mode","category":"mode","type":"select","currentValue":"code","options":[{"value":"code","name":"Code"}]},
                {"id":"fast","name":"Fast","category":"model_config","type":"select","currentValue":"off","options":[{"value":"off","name":"Off"}]}
            ]
        }), "claude").unwrap();
        assert_eq!(
            snapshot
                .display_order()
                .iter()
                .map(|&index| snapshot.options[index].id.as_str())
                .collect::<Vec<_>>(),
            vec!["model", "thinking", "cache", "fast"]
        );
        assert_eq!(
            snapshot
                .footer_order()
                .iter()
                .map(|&index| snapshot.options[index].id.as_str())
                .collect::<Vec<_>>(),
            vec!["model", "thinking"]
        );
        let mut codex = snapshot.clone();
        codex.provider = "codex".into();
        assert_eq!(
            codex
                .footer_order()
                .iter()
                .map(|&index| codex.options[index].id.as_str())
                .collect::<Vec<_>>(),
            vec!["model", "thinking"]
        );
        assert_eq!(snapshot.options[3].id, "mode");
        assert_eq!(snapshot.options[3].selected_label(), "Code");
        let mut opencode = snapshot.clone();
        opencode.provider = "opencode".into();
        assert_eq!(
            opencode
                .display_order()
                .iter()
                .map(|&index| opencode.options[index].id.as_str())
                .collect::<Vec<_>>(),
            vec!["mode", "model", "thinking", "cache", "fast"]
        );
        assert_eq!(
            opencode
                .footer_order()
                .iter()
                .map(|&index| opencode.options[index].id.as_str())
                .collect::<Vec<_>>(),
            vec!["mode", "model", "thinking"]
        );
        assert_eq!(snapshot.options[0].id, "cache");
        let mut stale = serde_json::json!({"provider":"claude","externalSessionId":"id","modeFallback":false,"selectedOptions":{},"configOptions":[],"replayError":"model unavailable"});
        assert_eq!(
            ExternalOptions::parse(&stale, "claude")
                .unwrap()
                .replay_error
                .as_deref(),
            Some("model unavailable")
        );
        stale.as_object_mut().unwrap().remove("replayError");
        assert!(ExternalOptions::parse(&stale, "claude")
            .unwrap()
            .replay_error
            .is_none());
    }

    #[test]
    fn provider_commands_replace_in_order_and_never_fall_back_to_static_names() {
        let base = json!({"provider":"claude","externalSessionId":"provider-123","modeFallback":false,"selectedOptions":{},"configOptions":[],"availableCommands":[
            {"name":"compact","description":"Provider compact","input":{"hint":"optional instructions"}},
            {"name":"review","description":"Review changes"},
            {"name":"bad/name","description":"Invalid"}
        ]});
        let snapshot = ExternalOptions::parse(&base, "claude").unwrap();
        assert_eq!(
            snapshot
                .available_commands
                .iter()
                .map(|command| command.name.as_str())
                .collect::<Vec<_>>(),
            vec!["compact", "review"]
        );
        assert_eq!(
            snapshot.available_commands[0].input_hint.as_deref(),
            Some("optional instructions")
        );
        let mut empty = base.clone();
        empty["availableCommands"] = json!([]);
        assert!(ExternalOptions::parse(&empty, "claude")
            .unwrap()
            .available_commands
            .is_empty());
        assert!(ExternalOptions::parse(&base, "codex").is_err());
        let mut invalid = base.clone();
        invalid.as_object_mut().unwrap().remove("externalSessionId");
        assert!(ExternalOptions::parse(&invalid, "claude").is_err());
    }

    #[test]
    fn preserves_order_ids_labels_and_grouped_choices() {
        let snapshot = ExternalOptions::parse(&json!({"provider":"opencode","externalSessionId":null,"modeFallback":false,"selectedOptions":{"model":"m2"},"configOptions":[
            {"id":"mode-xyz","name":"Mode","category":"mode","type":"select","currentValue":"plan","options":[{"value":"plan","name":"Plan"}]},
            {"id":"model-id","name":"Model","category":"model","type":"select","currentValue":"m2","options":[{"name":"Family","options":[{"value":"m1","name":"One"},{"value":"m2","name":"Two"}]}]},
            {"id":"future","name":"Future","category":"other","type":"select","currentValue":"x","options":[{"value":"x","name":"X"}]},
            {"id":"uncategorized","name":"Thinking","type":"select","currentValue":"deep","options":[{"value":"deep","name":"Deep"}]},
            {"id":"flag","name":"Flag","category":"model_config","type":"boolean","currentValue":true}
        ]}), "opencode").unwrap();
        assert_eq!(
            snapshot
                .options
                .iter()
                .map(|o| o.id.as_str())
                .collect::<Vec<_>>(),
            vec!["mode-xyz", "model-id", "future", "uncategorized"]
        );
        assert_eq!(snapshot.options[3].category, "");
        assert_eq!(snapshot.options[1].selected_label(), "Two");
        assert_eq!(
            snapshot.options[1].choices[0].group.as_deref(),
            Some("Family")
        );
        assert_eq!(snapshot.options[1].choices[1].value, "m2");
        assert!(ExternalOptions::parse(
            &json!({"provider":"claude","configOptions":[]}),
            "codex"
        )
        .is_err());
    }
}
