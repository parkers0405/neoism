use std::path::Path;

use serde_json::{Map, Value};

pub(super) fn parse_markdown(
    path: &Path,
) -> anyhow::Result<(Map<String, Value>, String)> {
    let text = std::fs::read_to_string(path)?;
    if !text.starts_with("---") {
        return Ok((Map::new(), text));
    }
    let rest = text.strip_prefix("---").unwrap_or(&text);
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    let Some(index) = rest.find("\n---") else {
        return Ok((Map::new(), text));
    };
    let frontmatter = &rest[..index];
    let content = rest[index + "\n---".len()..]
        .strip_prefix('\n')
        .unwrap_or_default()
        .to_string();
    let yaml = serde_yaml::from_str::<serde_yaml::Value>(frontmatter)?;
    let data = serde_json::to_value(yaml)?;
    Ok((data.as_object().cloned().unwrap_or_default(), content))
}
