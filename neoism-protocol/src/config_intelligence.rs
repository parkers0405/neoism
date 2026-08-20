//! Host-neutral completion logic for Neoism's JSONC configuration.
//!
//! Hosts provide [`ConfigDescriptor`] values (including their runtime
//! suggestions); this module only determines the JSON path at the cursor and
//! turns those descriptors into editor-ready insertions.

use crate::config::{ConfigDescriptor, ConfigOption, ConfigValueKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigCompletion {
    pub label: String,
    pub insert_text: String,
    pub detail: String,
    pub documentation: String,
    /// Absolute byte offset where accepting this item starts replacement.
    pub replace_start: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Key,
    Value,
}

#[derive(Debug)]
struct CursorContext {
    object_path: Vec<String>,
    value_key: Option<String>,
    role: Role,
    in_string: bool,
    prefix: String,
    replace_start: usize,
    closing_quote_present: bool,
}

/// Complete keys or values at `offset` in a JSONC document.
pub fn complete_config(
    text: &str,
    offset: usize,
    descriptors: &[ConfigDescriptor],
) -> Vec<ConfigCompletion> {
    let Some(context) = cursor_context(text, offset.min(text.len())) else {
        return Vec::new();
    };
    match context.role {
        Role::Key => complete_keys(&context, descriptors),
        Role::Value => complete_values(&context, descriptors),
    }
}

/// Resolve the descriptor under a JSONC key or value for hover/help.
pub fn descriptor_at<'a>(
    text: &str,
    offset: usize,
    descriptors: &'a [ConfigDescriptor],
) -> Option<&'a ConfigDescriptor> {
    let context = cursor_context(text, offset.min(text.len()))?;
    let key = match context.role {
        Role::Value => context.value_key?,
        Role::Key => {
            let tail = text.get(context.replace_start..)?;
            tail.split('"').next().unwrap_or_default().to_string()
        }
    };
    let path = if context.object_path.is_empty() {
        key
    } else {
        format!("{}.{}", context.object_path.join("."), key)
    };
    descriptors
        .iter()
        .find(|item| descriptor_path_matches(&item.path, &path))
}

fn complete_keys(
    context: &CursorContext,
    descriptors: &[ConfigDescriptor],
) -> Vec<ConfigCompletion> {
    let parent = context.object_path.join(".");
    let mut rows: Vec<(String, Option<&ConfigDescriptor>)> = Vec::new();
    for descriptor in descriptors {
        if descriptor.path == parent && descriptor.value_kind == ConfigValueKind::Object {
            for key in descriptor_options(descriptor)
                .filter_map(|option| option.value.as_str().map(str::to_string))
                .filter(|key| key.starts_with(&context.prefix))
            {
                if !rows.iter().any(|(existing, _)| existing == &key) {
                    rows.push((key.clone(), None));
                }
            }
            continue;
        }
        let Some(remainder) = descriptor_remainder(&descriptor.path, &parent) else {
            continue;
        };
        let Some(segment) = remainder.split('.').next() else {
            continue;
        };
        if !segment.starts_with(&context.prefix) {
            continue;
        }
        let leaf = (!remainder.contains('.')).then_some(descriptor);
        if let Some(existing) = rows.iter_mut().find(|(name, _)| name == segment) {
            if existing.1.is_none() {
                existing.1 = leaf;
            }
        } else {
            rows.push((segment.to_string(), leaf));
        }
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows.into_iter()
        .map(|(key, leaf)| {
            let value = leaf.map_or_else(
                || "{\n  $0\n}".to_string(),
                |descriptor| json_default(&descriptor.default, descriptor.value_kind),
            );
            let quote = if context.in_string { "" } else { "\"" };
            let close = if context.in_string { "\"" } else { "\"" };
            ConfigCompletion {
                label: key.clone(),
                insert_text: format!("{quote}{key}{close}: {value}"),
                detail: leaf
                    .map(|descriptor| descriptor.label.clone())
                    .unwrap_or_else(|| "Configuration section".to_string()),
                documentation: leaf
                    .map(|descriptor| descriptor.description.clone())
                    .unwrap_or_else(|| format!("Neoism `{key}` settings.")),
                replace_start: context.replace_start,
            }
        })
        .collect()
}

fn complete_values(
    context: &CursorContext,
    descriptors: &[ConfigDescriptor],
) -> Vec<ConfigCompletion> {
    let Some(key) = context.value_key.as_deref() else {
        return Vec::new();
    };
    let path = if context.object_path.is_empty() {
        key.to_string()
    } else {
        format!("{}.{}", context.object_path.join("."), key)
    };
    let Some(descriptor) = descriptors
        .iter()
        .find(|item| descriptor_path_matches(&item.path, &path))
    else {
        return Vec::new();
    };
    let mut suggestions = descriptor_options(descriptor).cloned().collect::<Vec<_>>();
    if suggestions.is_empty() && !descriptor.default.is_null() {
        suggestions.push(ConfigOption {
            value: descriptor.default.clone(),
            label: None,
            description: None,
        });
    }
    suggestions.sort_by(|a, b| option_label(a).cmp(&option_label(b)));
    suggestions.dedup_by(|a, b| a.value == b.value);
    suggestions
        .into_iter()
        .filter(|option| option_label(option).starts_with(&context.prefix))
        .map(|option| {
            let label = option_label(&option);
            let insert_text = match option.value.as_str() {
                Some(value) if context.in_string => {
                    if context.closing_quote_present {
                        value.to_string()
                    } else {
                        format!("{value}\"")
                    }
                }
                _ => {
                    serde_json::to_string(&option.value).unwrap_or_else(|_| "null".into())
                }
            };
            ConfigCompletion {
                label,
                insert_text,
                detail: descriptor.label.clone(),
                documentation: option
                    .description
                    .unwrap_or_else(|| descriptor_documentation(descriptor)),
                replace_start: context.replace_start,
            }
        })
        .collect()
}

fn descriptor_options(
    descriptor: &ConfigDescriptor,
) -> impl Iterator<Item = &ConfigOption> {
    descriptor.options.iter()
}

fn option_label(option: &ConfigOption) -> String {
    option
        .label
        .clone()
        .unwrap_or_else(|| value_label(&option.value))
}

fn descriptor_documentation(descriptor: &ConfigDescriptor) -> String {
    let mut documentation = descriptor.description.clone();
    let constraints = &descriptor.constraints;
    if constraints.min.is_some() || constraints.max.is_some() {
        documentation.push_str("\n\nRange: ");
        documentation.push_str(&match (constraints.min, constraints.max) {
            (Some(min), Some(max)) => format!("{min} to {max}"),
            (Some(min), None) => format!("at least {min}"),
            (None, Some(max)) => format!("at most {max}"),
            (None, None) => String::new(),
        });
        if let Some(unit) = constraints.unit.as_deref() {
            documentation.push(' ');
            documentation.push_str(unit);
        }
    }
    documentation
}

fn descriptor_remainder(pattern: &str, parent: &str) -> Option<String> {
    let pattern = pattern.split('.').collect::<Vec<_>>();
    let parent = if parent.is_empty() {
        Vec::new()
    } else {
        parent.split('.').collect::<Vec<_>>()
    };
    if parent.len() > pattern.len()
        || parent
            .iter()
            .zip(&pattern)
            .any(|(actual, expected)| *expected != "*" && actual != expected)
    {
        return None;
    }
    Some(pattern[parent.len()..].join("."))
}

fn descriptor_path_matches(pattern: &str, actual: &str) -> bool {
    let pattern = pattern.split('.').collect::<Vec<_>>();
    let actual = actual.split('.').collect::<Vec<_>>();
    pattern.len() == actual.len()
        && pattern
            .iter()
            .zip(actual)
            .all(|(expected, actual)| *expected == "*" || *expected == actual)
}

fn json_default(value: &serde_json::Value, kind: ConfigValueKind) -> String {
    if kind == ConfigValueKind::Object
        && value.as_object().is_some_and(|map| map.is_empty())
    {
        return "{\n  $0\n}".to_string();
    }
    if kind == ConfigValueKind::Array
        && value.as_array().is_some_and(|items| items.is_empty())
    {
        return "[\n  $0\n]".to_string();
    }
    if value.is_null() {
        return match kind {
            ConfigValueKind::Boolean => "false".to_string(),
            ConfigValueKind::Integer | ConfigValueKind::Number => "0".to_string(),
            ConfigValueKind::String => "\"\"".to_string(),
            ConfigValueKind::Array => "[]".to_string(),
            ConfigValueKind::Object => "{}".to_string(),
        };
    }
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
}

fn value_label(value: &serde_json::Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
}

fn cursor_context(text: &str, offset: usize) -> Option<CursorContext> {
    let mut object_path = Vec::new();
    let mut object_pushes = Vec::new();
    let mut pending_key: Option<String> = None;
    let mut last_string: Option<String> = None;
    let mut expecting_value = false;
    let mut in_string = false;
    let mut escaped = false;
    let mut line_comment = false;
    let mut block_comment = false;
    let mut string_start = 0usize;
    let mut string_value = String::new();
    let mut string_role = Role::Key;
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < offset {
        let byte = bytes[i];
        if line_comment {
            if byte == b'\n' {
                line_comment = false;
            }
            i += 1;
            continue;
        }
        if block_comment {
            if byte == b'*' && bytes.get(i + 1) == Some(&b'/') {
                block_comment = false;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        if in_string {
            if !byte.is_ascii() {
                let ch = text[i..].chars().next()?;
                string_value.push(ch);
                i += ch.len_utf8();
                continue;
            }
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
                last_string = Some(string_value.clone());
            } else {
                string_value.push(byte as char);
            }
            i += 1;
            continue;
        }
        if byte == b'/' && bytes.get(i + 1) == Some(&b'/') {
            line_comment = true;
            i += 2;
            continue;
        }
        if byte == b'/' && bytes.get(i + 1) == Some(&b'*') {
            block_comment = true;
            i += 2;
            continue;
        }
        match byte {
            b'"' => {
                in_string = true;
                escaped = false;
                string_start = i + 1;
                string_value.clear();
                string_role = if expecting_value {
                    Role::Value
                } else {
                    Role::Key
                };
            }
            b':' => {
                pending_key = last_string.take();
                expecting_value = true;
            }
            b'{' => {
                let pushed = pending_key.take();
                if let Some(key) = pushed.as_ref() {
                    object_path.push(key.clone());
                }
                object_pushes.push(pushed.is_some());
                expecting_value = false;
                last_string = None;
            }
            b'}' => {
                if object_pushes.pop().unwrap_or(false) {
                    object_path.pop();
                }
                expecting_value = false;
                pending_key = None;
                last_string = None;
            }
            b',' => {
                expecting_value = false;
                pending_key = None;
                last_string = None;
            }
            _ => {}
        }
        i += 1;
    }
    let (role, prefix, replace_start) = if in_string {
        (
            string_role,
            text.get(string_start..offset)
                .unwrap_or_default()
                .to_string(),
            string_start,
        )
    } else {
        let start = text[..offset]
            .rfind(|ch: char| !(ch.is_alphanumeric() || ch == '_' || ch == '-'))
            .map_or(0, |index| index + 1);
        (
            if expecting_value {
                Role::Value
            } else {
                Role::Key
            },
            text[start..offset].to_string(),
            start,
        )
    };
    Some(CursorContext {
        object_path,
        value_key: pending_key,
        role,
        in_string,
        prefix,
        replace_start,
        closing_quote_present: in_string && bytes.get(offset) == Some(&b'"'),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConfigCategory, ConfigControl};

    fn descriptor(path: &str, suggestions: &[&str]) -> ConfigDescriptor {
        ConfigDescriptor {
            path: path.to_string(),
            label: path.to_string(),
            description: format!("Configure {path}."),
            value_kind: ConfigValueKind::String,
            default: serde_json::json!(""),
            static_suggestions: suggestions.iter().map(|item| item.to_string()).collect(),
            runtime_suggestions: Vec::new(),
            options: suggestions
                .iter()
                .map(|item| ConfigOption {
                    value: serde_json::json!(item),
                    label: None,
                    description: None,
                })
                .collect(),
            provider: None,
            constraints: Default::default(),
            accepted_kinds: vec![],
            extensible: true,
            category: ConfigCategory::Appearance,
            control: ConfigControl::Text,
        }
    }

    #[test]
    fn completes_nested_key_in_jsonc() {
        let text = "// hi\n{\n  \"appearance\": {\n    \"fo";
        let rows = complete_config(
            text,
            text.len(),
            &[descriptor("appearance.fonts.family", &[])],
        );
        assert_eq!(rows[0].label, "fonts");
        assert_eq!(rows[0].insert_text, "fonts\": {\n  $0\n}");
    }

    #[test]
    fn completes_runtime_style_value_inside_quotes() {
        let text = "{ \"appearance\": { \"theme\": \"tok";
        let rows = complete_config(
            text,
            text.len(),
            &[descriptor(
                "appearance.theme",
                &["pastel_dark", "tokyo_night"],
            )],
        );
        assert_eq!(rows[0].label, "tokyo_night");
        assert_eq!(rows[0].insert_text, "tokyo_night\"");
    }

    #[test]
    fn resolves_descriptor_under_value() {
        let text = "{ \"appearance\": { \"theme\": \"tokyo_night\" } }";
        let descriptors = [descriptor("appearance.theme", &["tokyo_night"])];
        let offset = text.find("night").unwrap();
        assert_eq!(
            descriptor_at(text, offset, &descriptors).unwrap().path,
            "appearance.theme"
        );
    }

    #[test]
    fn object_suggestions_become_nested_keys() {
        let mut lsp = descriptor("agent.lsp", &["rust", "typescript"]);
        lsp.value_kind = ConfigValueKind::Object;
        let text = "{ \"agent\": { \"lsp\": { \"ru";
        let rows = complete_config(text, text.len(), &[lsp]);
        assert_eq!(rows[0].label, "rust");
        assert_eq!(rows[0].insert_text, "rust\": {\n  $0\n}");
    }

    #[test]
    fn wildcard_descriptors_complete_named_agent_fields() {
        let text = "{ \"agent\": { \"agent\": { \"build\": { \"mo";
        let rows =
            complete_config(text, text.len(), &[descriptor("agent.agent.*.model", &[])]);
        assert_eq!(rows[0].label, "model");
    }

    #[test]
    fn typed_numeric_options_insert_json_numbers() {
        let mut size = descriptor("appearance.fonts.size", &[]);
        size.value_kind = ConfigValueKind::Number;
        size.options = vec![ConfigOption {
            value: serde_json::json!(14.5),
            label: Some("Comfortable (14.5 pt)".into()),
            description: Some("A balanced UI size.".into()),
        }];
        let text = "{ \"appearance\": { \"fonts\": { \"size\": ";
        let rows = complete_config(text, text.len(), &[size]);
        assert_eq!(rows[0].label, "Comfortable (14.5 pt)");
        assert_eq!(rows[0].insert_text, "14.5");
        assert_eq!(rows[0].documentation, "A balanced UI size.");
    }

    #[test]
    fn unicode_value_prefix_is_preserved() {
        let text = "{ \"appearance\": { \"theme\": \"café";
        let rows = complete_config(
            &text,
            text.len(),
            &[descriptor("appearance.theme", &["café_dark"])],
        );
        assert_eq!(rows[0].label, "café_dark");
    }
}
