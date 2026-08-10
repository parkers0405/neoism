use serde_json::Value;

pub(super) fn required_string<'a>(
    arguments: &'a Value,
    key: &str,
) -> anyhow::Result<&'a str> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("tool argument {key} is required"))
}

pub(super) fn optional_string(arguments: &Value, key: &str) -> Option<String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToString::to_string)
}

pub(super) fn usize_arg(arguments: &Value, key: &str) -> Option<usize> {
    arguments
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
}
