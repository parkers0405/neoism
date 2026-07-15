use std::collections::{BTreeMap, BTreeSet};

use neoism_agent_core::{PermissionAction, PermissionRule};
use serde_json::Value;

#[allow(dead_code)]
const EDIT_TOOLS: &[&str] = &["edit", "write", "apply_patch"];

pub(crate) fn evaluate(
    permission: &str,
    pattern: &str,
    rules: &[PermissionRule],
) -> PermissionRule {
    rules
        .iter()
        .rev()
        .find(|rule| {
            wildcard_match(&rule.permission, permission)
                && wildcard_match(&rule.pattern, pattern)
        })
        .cloned()
        .unwrap_or_else(|| PermissionRule {
            permission: permission.to_string(),
            pattern: "*".to_string(),
            action: PermissionAction::Ask,
        })
}

#[allow(dead_code)]
pub(crate) fn merge(
    rulesets: impl IntoIterator<Item = Vec<PermissionRule>>,
) -> Vec<PermissionRule> {
    rulesets.into_iter().flatten().collect()
}

pub(crate) fn from_config_map(config: &BTreeMap<String, Value>) -> Vec<PermissionRule> {
    config
        .iter()
        .flat_map(|(permission, value)| from_config_value(permission, value))
        .collect()
}

#[allow(dead_code)]
pub(crate) fn disabled(tools: &[String], rules: &[PermissionRule]) -> BTreeSet<String> {
    tools
        .iter()
        .filter_map(|tool| {
            let permission = if EDIT_TOOLS.contains(&tool.as_str()) {
                "edit"
            } else {
                tool.as_str()
            };
            let rule = rules
                .iter()
                .rev()
                .find(|rule| wildcard_match(&rule.permission, permission))?;
            (rule.pattern == "*" && rule.action == PermissionAction::Deny)
                .then(|| tool.clone())
        })
        .collect()
}

fn from_config_value(permission: &str, value: &Value) -> Vec<PermissionRule> {
    if let Some(action) = value.as_str().and_then(parse_action) {
        return vec![PermissionRule {
            permission: permission.to_string(),
            pattern: "*".to_string(),
            action,
        }];
    }
    value
        .as_object()
        .map(|object| {
            object
                .iter()
                .filter_map(|(pattern, action)| {
                    action
                        .as_str()
                        .and_then(parse_action)
                        .map(|action| PermissionRule {
                            permission: permission.to_string(),
                            pattern: expand_home(pattern),
                            action,
                        })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_action(value: &str) -> Option<PermissionAction> {
    match value {
        "allow" => Some(PermissionAction::Allow),
        "deny" => Some(PermissionAction::Deny),
        "ask" => Some(PermissionAction::Ask),
        _ => None,
    }
}

fn expand_home(pattern: &str) -> String {
    let Some(home) = std::env::var_os("HOME") else {
        return pattern.to_string();
    };
    let home = home.to_string_lossy();
    if pattern == "~" {
        return home.to_string();
    }
    if let Some(rest) = pattern.strip_prefix("~/") {
        return format!("{home}/{rest}");
    }
    if pattern == "$HOME" {
        return home.to_string();
    }
    if let Some(rest) = pattern.strip_prefix("$HOME/") {
        return format!("{home}/{rest}");
    }
    pattern.to_string()
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    let pattern = pattern.strip_prefix("**/").unwrap_or(pattern);
    wildcard_match_inner(pattern.as_bytes(), value.as_bytes())
}

fn wildcard_match_inner(pattern: &[u8], value: &[u8]) -> bool {
    let (mut pattern_index, mut value_index) = (0, 0);
    let mut star = None;
    let mut star_value_index = 0;

    while value_index < value.len() {
        if pattern_index < pattern.len()
            && (pattern[pattern_index] == value[value_index]
                || pattern[pattern_index] == b'?')
        {
            pattern_index += 1;
            value_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
            star = Some(pattern_index);
            pattern_index += 1;
            star_value_index = value_index;
        } else if let Some(star_index) = star {
            pattern_index = star_index + 1;
            star_value_index += 1;
            value_index = star_value_index;
        } else {
            return false;
        }
    }

    while pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn last_matching_wildcard_rule_wins() {
        let rules = vec![
            PermissionRule {
                permission: "*".to_string(),
                pattern: "*".to_string(),
                action: PermissionAction::Deny,
            },
            PermissionRule {
                permission: "read".to_string(),
                pattern: "*.env.example".to_string(),
                action: PermissionAction::Allow,
            },
            PermissionRule {
                permission: "read".to_string(),
                pattern: "*.env*".to_string(),
                action: PermissionAction::Ask,
            },
            PermissionRule {
                permission: "read".to_string(),
                pattern: "*.env.example".to_string(),
                action: PermissionAction::Allow,
            },
        ];

        assert_eq!(
            evaluate("read", ".env.example", &rules).action,
            PermissionAction::Allow
        );
        assert_eq!(
            evaluate("bash", "cargo test", &rules).action,
            PermissionAction::Deny
        );
    }

    #[test]
    fn dangerous_skip_star_rule_allows_asks_but_not_explicit_denies() {
        // The `dangerouslySkipPermissions` flag injects `"*": "allow"`
        // into the config permission map. BTreeMap order puts "*"
        // first, so explicit entries still win (last match wins).
        let rules = from_config_map(&BTreeMap::from([
            ("*".to_string(), json!("allow")),
            ("bash".to_string(), json!("deny")),
        ]));
        assert_eq!(
            evaluate("external_directory", "/anywhere/*", &rules).action,
            PermissionAction::Allow
        );
        assert_eq!(
            evaluate("edit", "src/main.rs", &rules).action,
            PermissionAction::Allow
        );
        assert_eq!(
            evaluate("bash", "rm -rf /", &rules).action,
            PermissionAction::Deny
        );
    }

    #[test]
    fn config_map_flattens_shorthand_and_pattern_rules() {
        let rules = from_config_map(&BTreeMap::from([
            ("bash".to_string(), json!("ask")),
            ("read".to_string(), json!({ "*": "allow", "*.env": "ask" })),
        ]));

        assert!(rules.iter().any(|rule| {
            rule.permission == "bash"
                && rule.pattern == "*"
                && rule.action == PermissionAction::Ask
        }));
        assert!(rules.iter().any(|rule| {
            rule.permission == "read"
                && rule.pattern == "*.env"
                && rule.action == PermissionAction::Ask
        }));
    }

    #[test]
    fn disabled_detects_top_level_deny_rules() {
        let rules = vec![PermissionRule {
            permission: "edit".to_string(),
            pattern: "*".to_string(),
            action: PermissionAction::Deny,
        }];
        let disabled = disabled(&["write".to_string(), "read".to_string()], &rules);
        assert!(disabled.contains("write"));
        assert!(!disabled.contains("read"));
    }
}
