use neoism_agent_core::{ProviderGenerationRequest, ProviderMessage, ProviderRole};
use serde_json::{Map, Value};

pub(super) fn normalize_request(
    mut request: ProviderGenerationRequest,
) -> ProviderGenerationRequest {
    let family = ProviderFamily::new(&request.provider_id, &request.model_id);
    normalize_tool_call_ids(&family, &mut request.messages);
    if family.needs_empty_content_filtering() {
        request.messages =
            anthropic_safe_messages(&request.messages, family.uses_claude_tool_ids());
    }
    if family.is_mistral_like() {
        request.messages = mistral_safe_sequence(&request.messages);
    }
    request
}

pub(super) fn tool_parameters(provider_or_model_id: &str, parameters: &Value) -> Value {
    let lower = provider_or_model_id.to_ascii_lowercase();
    let mut schema = parameters.clone();
    if is_moonshot_like(&lower) {
        schema = sanitize_moonshot_schema(schema);
    }
    if is_gemini_like(&lower) {
        schema = sanitize_gemini_schema(schema);
    }
    schema
}

pub(super) fn apply_openai_compatible_request_quirks(
    request: &ProviderGenerationRequest,
    body: &mut Value,
) {
    let Some(api) = request.api.as_ref() else {
        return;
    };
    let Some(object) = body.as_object_mut() else {
        return;
    };
    match api.npm.as_str() {
        "@openrouter/ai-sdk-provider" | "@llmgateway/ai-sdk-provider" => {
            object
                .entry("usage".to_string())
                .or_insert_with(|| serde_json::json!({ "include": true }));
            if let Some(effort) = reasoning_effort_value(request.variant.as_deref()) {
                object
                    .entry("reasoning".to_string())
                    .or_insert_with(|| serde_json::json!({ "effort": effort }));
            }
        }
        "@ai-sdk/openai" | "@ai-sdk/azure" => {
            object
                .entry("store".to_string())
                .or_insert_with(|| Value::Bool(false));
            if openai_gpt5_reasoning_default(request) {
                object
                    .entry("reasoning_effort".to_string())
                    .or_insert_with(|| Value::String("medium".to_string()));
            }
        }
        _ => {}
    }
    apply_openai_compatible_thinking_flags(request, object);
}

pub(super) fn apply_anthropic_request_quirks(
    request: &ProviderGenerationRequest,
    body: &mut Value,
) {
    let Some(object) = body.as_object_mut() else {
        return;
    };
    if object.contains_key("thinking") || !supports_native_anthropic_thinking(request) {
        return;
    }
    let Some(effort) = reasoning_effort_value(request.variant.as_deref()) else {
        return;
    };
    let max_tokens = object
        .get("max_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(4096);
    let budget = reasoning_budget_tokens(effort, max_tokens);
    if budget > 0 {
        object.insert(
            "thinking".to_string(),
            serde_json::json!({
                "type": "enabled",
                "budget_tokens": budget
            }),
        );
    }
}

fn reasoning_effort_value(variant: Option<&str>) -> Option<&'static str> {
    match variant {
        Some("none") => Some("none"),
        Some("minimal") => Some("minimal"),
        Some("low") => Some("low"),
        Some("medium") => Some("medium"),
        Some("high") => Some("high"),
        Some("xhigh") | Some("max") => Some("xhigh"),
        // GPT-5.6 "ultra" (multi-agent) has no equivalent here; use deepest effort.
        Some("ultra") => Some("xhigh"),
        _ => None,
    }
}

fn apply_openai_compatible_thinking_flags(
    request: &ProviderGenerationRequest,
    object: &mut Map<String, Value>,
) {
    let Some(api) = request.api.as_ref() else {
        return;
    };
    let provider = request.provider_id.to_ascii_lowercase();
    let model = model_key(request);
    if (provider == "alibaba-cn" || provider == "alibaba")
        && api.npm == "@ai-sdk/openai-compatible"
        && !model.contains("kimi-k2-thinking")
    {
        object
            .entry("enable_thinking".to_string())
            .or_insert(Value::Bool(true));
    }
    if provider.contains("zai") || provider.contains("zhipuai") {
        object.entry("thinking".to_string()).or_insert_with(
            || serde_json::json!({ "type": "enabled", "clear_thinking": false }),
        );
    }
    if provider == "baseten"
        || (provider == "opencode"
            && (model.contains("kimi-k2-thinking") || model.contains("glm-4.6")))
    {
        object
            .entry("chat_template_args".to_string())
            .or_insert_with(|| serde_json::json!({ "enable_thinking": true }));
    }
}

fn openai_gpt5_reasoning_default(request: &ProviderGenerationRequest) -> bool {
    if request.variant.is_some() {
        return false;
    }
    let model = model_key(request);
    is_gpt5_family(&model)
        && !model.contains("gpt-5-chat")
        && !model.contains("gpt-5-pro")
}

fn is_gpt5_family(model: &str) -> bool {
    model.split('/').any(|part| {
        part == "gpt-5"
            || part
                .strip_prefix("gpt-5")
                .is_some_and(|rest| rest.starts_with('-') || rest.starts_with('.'))
    })
}

fn supports_native_anthropic_thinking(request: &ProviderGenerationRequest) -> bool {
    let model = model_key(request);
    (model.contains("claude")
        && (model.contains("3-7")
            || model.contains("3.7")
            || model.contains("-4")
            || model.contains("4-")))
        || model.contains("kimi-k2.")
        || model.contains("kimi-k2p")
        || model.contains("k2p")
}

fn reasoning_budget_tokens(effort: &str, max_tokens: u64) -> u64 {
    if max_tokens <= 1024 {
        return max_tokens.saturating_sub(1);
    }
    let requested = match effort {
        "none" => return 0,
        "minimal" => 512,
        "low" => 1024,
        "medium" => 2048,
        "high" => 4096,
        "xhigh" => 8192,
        _ => 0,
    };
    requested.min(max_tokens.saturating_sub(1024)).max(1)
}

fn model_key(request: &ProviderGenerationRequest) -> String {
    request
        .api
        .as_ref()
        .map(|api| api.id.as_str())
        .unwrap_or(&request.model_id)
        .to_ascii_lowercase()
}

#[derive(Clone, Copy, Debug)]
struct ProviderFamily<'a> {
    provider_id: &'a str,
    model_id: &'a str,
}

impl<'a> ProviderFamily<'a> {
    fn new(provider_id: &'a str, model_id: &'a str) -> Self {
        Self {
            provider_id,
            model_id,
        }
    }

    fn provider_lower(self) -> String {
        self.provider_id.to_ascii_lowercase()
    }

    fn model_lower(self) -> String {
        self.model_id.to_ascii_lowercase()
    }

    fn uses_claude_tool_ids(self) -> bool {
        let provider = self.provider_lower();
        let model = self.model_lower();
        provider.contains("anthropic")
            || provider.contains("claude")
            || model.contains("anthropic")
            || model.contains("claude")
    }

    fn needs_empty_content_filtering(self) -> bool {
        self.uses_claude_tool_ids() || self.provider_lower().contains("bedrock")
    }

    fn is_mistral_like(self) -> bool {
        let provider = self.provider_lower();
        let model = self.model_lower();
        provider.contains("mistral")
            || model.contains("mistral")
            || provider.contains("devstral")
            || model.contains("devstral")
    }
}

fn normalize_tool_call_ids(
    family: &ProviderFamily<'_>,
    messages: &mut [ProviderMessage],
) {
    let Some(sanitize) = tool_id_sanitizer(*family) else {
        return;
    };
    for message in messages {
        for call in &mut message.tool_calls {
            call.id = sanitize(&call.id);
        }
        if let Some(id) = message.tool_call_id.as_mut() {
            *id = sanitize(id);
        }
    }
}

fn tool_id_sanitizer(family: ProviderFamily<'_>) -> Option<fn(&str) -> String> {
    if family.is_mistral_like() {
        return Some(sanitize_mistral_tool_id);
    }
    if family.uses_claude_tool_ids() {
        return Some(sanitize_claude_tool_id);
    }
    None
}

fn sanitize_claude_tool_id(id: &str) -> String {
    let sanitized = id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "tool_call".to_string()
    } else {
        sanitized
    }
}

fn sanitize_mistral_tool_id(id: &str) -> String {
    let mut sanitized = id
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .take(9)
        .collect::<String>();
    while sanitized.len() < 9 {
        sanitized.push('0');
    }
    sanitized
}

fn mistral_safe_sequence(messages: &[ProviderMessage]) -> Vec<ProviderMessage> {
    let mut result = Vec::with_capacity(messages.len());
    for (idx, message) in messages.iter().enumerate() {
        result.push(message.clone());
        let next = messages.get(idx + 1);
        if matches!(message.role, ProviderRole::Tool)
            && next.is_some_and(|next| matches!(next.role, ProviderRole::User))
        {
            result.push(ProviderMessage::text(ProviderRole::Assistant, "Done."));
        }
    }
    result
}

fn anthropic_safe_messages(
    messages: &[ProviderMessage],
    split_tool_call_text: bool,
) -> Vec<ProviderMessage> {
    let mut result = Vec::with_capacity(messages.len());
    for message in messages {
        if is_empty_non_tool_message(message) {
            continue;
        }
        if split_tool_call_text
            && matches!(message.role, ProviderRole::Assistant)
            && !message.content.trim().is_empty()
            && !message.tool_calls.is_empty()
        {
            let mut text = message.clone();
            text.tool_calls.clear();
            result.push(text);

            let mut tool_calls = message.clone();
            tool_calls.content.clear();
            result.push(tool_calls);
            continue;
        }
        result.push(message.clone());
    }
    result
}

fn is_empty_non_tool_message(message: &ProviderMessage) -> bool {
    message.content.is_empty()
        && message.attachments.is_empty()
        && message.tool_calls.is_empty()
        && !matches!(message.role, ProviderRole::Tool)
}

fn is_moonshot_like(lower: &str) -> bool {
    lower.contains("kimi") || lower.contains("moonshot")
}

fn is_gemini_like(lower: &str) -> bool {
    lower.contains("gemini") || lower.contains("google")
}

fn sanitize_moonshot_schema(value: Value) -> Value {
    match value {
        Value::Array(items) => {
            Value::Array(items.into_iter().map(sanitize_moonshot_schema).collect())
        }
        Value::Object(object) => {
            if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
                let mut result = Map::new();
                result.insert("$ref".to_string(), Value::String(reference.to_string()));
                return Value::Object(result);
            }
            let mut result = Map::new();
            for (key, value) in object {
                if key == "items" {
                    if let Value::Array(items) = value {
                        result.insert(
                            key,
                            items
                                .into_iter()
                                .next()
                                .map(sanitize_moonshot_schema)
                                .unwrap_or_else(|| Value::Object(Map::new())),
                        );
                        continue;
                    }
                }
                result.insert(key, sanitize_moonshot_schema(value));
            }
            Value::Object(result)
        }
        other => other,
    }
}

fn sanitize_gemini_schema(value: Value) -> Value {
    match value {
        Value::Array(items) => {
            Value::Array(items.into_iter().map(sanitize_gemini_schema).collect())
        }
        Value::Object(object) => sanitize_gemini_object(object),
        other => other,
    }
}

fn sanitize_gemini_object(object: Map<String, Value>) -> Value {
    let mut result = Map::new();
    for (key, value) in object {
        if key == "enum" {
            if let Value::Array(values) = value {
                result.insert(
                    key,
                    Value::Array(
                        values
                            .into_iter()
                            .map(|value| Value::String(enum_value_to_string(value)))
                            .collect(),
                    ),
                );
                continue;
            }
        }
        result.insert(key, sanitize_gemini_schema(value));
    }

    if matches!(
        result.get("type").and_then(Value::as_str),
        Some("integer" | "number")
    ) && result.contains_key("enum")
    {
        result.insert("type".to_string(), Value::String("string".to_string()));
    }

    if result.get("type").and_then(Value::as_str) == Some("object") {
        filter_required_to_properties(&mut result);
    }

    if result.get("type").and_then(Value::as_str) == Some("array")
        && !has_combiner(&result)
    {
        match result.get_mut("items") {
            Some(Value::Object(items)) if !has_schema_intent(items) => {
                items.insert("type".to_string(), Value::String("string".to_string()));
            }
            Some(_) => {}
            None => {
                let mut items = Map::new();
                items.insert("type".to_string(), Value::String("string".to_string()));
                result.insert("items".to_string(), Value::Object(items));
            }
        }
    }

    if result
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind != "object")
        && !has_combiner(&result)
    {
        result.remove("properties");
        result.remove("required");
    }

    Value::Object(result)
}

fn enum_value_to_string(value: Value) -> String {
    match value {
        Value::String(value) => value,
        Value::Number(value) => value.to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

fn filter_required_to_properties(object: &mut Map<String, Value>) {
    let Some(properties) = object
        .get("properties")
        .and_then(Value::as_object)
        .map(|properties| properties.keys().cloned().collect::<Vec<_>>())
    else {
        object.remove("required");
        return;
    };
    let Some(required) = object.get_mut("required").and_then(Value::as_array_mut) else {
        return;
    };
    required.retain(|field| {
        field
            .as_str()
            .is_some_and(|field| properties.iter().any(|property| property == field))
    });
}

fn has_combiner(object: &Map<String, Value>) -> bool {
    ["anyOf", "oneOf", "allOf"]
        .iter()
        .any(|key| object.get(*key).is_some_and(Value::is_array))
}

fn has_schema_intent(object: &Map<String, Value>) -> bool {
    if has_combiner(object) {
        return true;
    }
    [
        "type",
        "properties",
        "items",
        "prefixItems",
        "enum",
        "const",
        "$ref",
        "additionalProperties",
        "patternProperties",
        "required",
        "not",
        "if",
        "then",
        "else",
    ]
    .iter()
    .any(|key| object.contains_key(*key))
}

pub(super) fn model_temperature(model_id: &str) -> Option<f64> {
    let lower = model_id.to_ascii_lowercase();
    if lower.contains("qwen") {
        return Some(0.55);
    }
    if lower.contains("claude") {
        return None;
    }
    if lower.contains("gemini") || lower.contains("glm-4.6") || lower.contains("glm-4.7")
    {
        return Some(1.0);
    }
    if lower.contains("minimax-m2") {
        return Some(1.0);
    }
    if lower.contains("kimi-k2") {
        if ["thinking", "k2.", "k2p", "k2-5"]
            .iter()
            .any(|needle| lower.contains(needle))
        {
            return Some(1.0);
        }
        return Some(0.6);
    }
    None
}

pub(super) fn model_top_p(model_id: &str) -> Option<f64> {
    let lower = model_id.to_ascii_lowercase();
    if lower.contains("qwen") {
        return Some(1.0);
    }
    if [
        "minimax-m2",
        "gemini",
        "kimi-k2.5",
        "kimi-k2p5",
        "kimi-k2-5",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
    {
        return Some(0.95);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_agent_core::{ProviderApiInfo, ProviderToolCall, ToolListItem};
    use serde_json::json;

    #[test]
    fn gemini_schema_converts_integer_enums_and_filters_required() {
        let schema = tool_parameters(
            "gemini-2.5-pro",
            &json!({
                "type": "object",
                "properties": {
                    "level": { "type": "integer", "enum": [1, 2] },
                    "tags": { "type": "array" }
                },
                "required": ["level", "missing"]
            }),
        );
        assert_eq!(schema["properties"]["level"]["type"], "string");
        assert_eq!(schema["properties"]["level"]["enum"], json!(["1", "2"]));
        assert_eq!(schema["properties"]["tags"]["items"]["type"], "string");
        assert_eq!(schema["required"], json!(["level"]));
    }

    #[test]
    fn gemini_schema_fills_schema_empty_array_items() {
        let schema = tool_parameters(
            "google",
            &json!({
                "type": "object",
                "properties": {
                    "matrix": {
                        "type": "array",
                        "items": {
                            "type": "array",
                            "items": {}
                        }
                    },
                    "edits": {
                        "type": "array",
                        "items": {
                            "anyOf": [
                                { "type": "object", "properties": { "old": { "type": "string" } } },
                                { "type": "object", "properties": { "new": { "type": "string" } } }
                            ]
                        }
                    }
                }
            }),
        );
        assert_eq!(
            schema["properties"]["matrix"]["items"]["items"]["type"],
            "string"
        );
        assert!(schema["properties"]["edits"]["items"]["type"].is_null());
        assert!(schema["properties"]["edits"]["items"]["anyOf"].is_array());
    }

    #[test]
    fn moonshot_schema_strips_ref_siblings_and_tuple_items() {
        let schema = tool_parameters(
            "kimi-k2",
            &json!({
                "type": "object",
                "properties": {
                    "ref": { "$ref": "#/$defs/x", "description": "drop" },
                    "tuple": { "type": "array", "items": [{ "type": "string" }, { "type": "number" }] }
                }
            }),
        );
        assert_eq!(schema["properties"]["ref"], json!({ "$ref": "#/$defs/x" }));
        assert_eq!(
            schema["properties"]["tuple"]["items"],
            json!({ "type": "string" })
        );
    }

    #[test]
    fn openrouter_request_quirks_add_usage_and_reasoning() {
        let request = ProviderGenerationRequest {
            session_id: None,
            provider_id: "openrouter".to_string(),
            model_id: "openai/gpt-5".to_string(),
            variant: Some("high".to_string()),
            api: Some(ProviderApiInfo {
                id: "openai/gpt-5".to_string(),
                url: "https://openrouter.ai/api/v1".to_string(),
                npm: "@openrouter/ai-sdk-provider".to_string(),
            }),
            auth_env: Vec::new(),
            messages: Vec::new(),
            tools: Vec::new(),
            options: Default::default(),
            headers: Default::default(),
        };
        let mut body = json!({});

        apply_openai_compatible_request_quirks(&request, &mut body);

        assert_eq!(body["usage"]["include"], true);
        assert_eq!(body["reasoning"]["effort"], "high");
    }

    #[test]
    fn openai_compatible_reasoning_quirks_match_common_thinking_providers() {
        let mut request = ProviderGenerationRequest {
            session_id: None,
            provider_id: "alibaba-cn".to_string(),
            model_id: "qwen3-plus".to_string(),
            variant: Some("xhigh".to_string()),
            api: Some(ProviderApiInfo {
                id: "qwen3-plus".to_string(),
                url: "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string(),
                npm: "@ai-sdk/openai-compatible".to_string(),
            }),
            auth_env: Vec::new(),
            messages: Vec::new(),
            tools: Vec::new(),
            options: Default::default(),
            headers: Default::default(),
        };
        let mut body = json!({});
        apply_openai_compatible_request_quirks(&request, &mut body);
        assert_eq!(body["enable_thinking"], true);

        request.provider_id = "zai".to_string();
        request.model_id = "glm-4.6".to_string();
        request.api.as_mut().unwrap().id = "glm-4.6".to_string();
        let mut body = json!({});
        apply_openai_compatible_request_quirks(&request, &mut body);
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["clear_thinking"], false);
    }

    #[test]
    fn openai_sdk_gpt5_defaults_medium_reasoning_when_variant_is_absent() {
        let request = ProviderGenerationRequest {
            session_id: None,
            provider_id: "openai".to_string(),
            model_id: "gpt-5.5".to_string(),
            variant: None,
            api: Some(ProviderApiInfo {
                id: "gpt-5.5".to_string(),
                url: "https://api.openai.com/v1".to_string(),
                npm: "@ai-sdk/openai".to_string(),
            }),
            auth_env: Vec::new(),
            messages: Vec::new(),
            tools: Vec::new(),
            options: Default::default(),
            headers: Default::default(),
        };
        let mut body = json!({});
        apply_openai_compatible_request_quirks(&request, &mut body);

        assert_eq!(body["store"], false);
        assert_eq!(body["reasoning_effort"], "medium");
    }

    #[test]
    fn normalizes_tool_call_ids_for_model_families() {
        let request = normalize_request(ProviderGenerationRequest {
            session_id: None,
            provider_id: "openai".to_string(),
            model_id: "claude-sonnet-4".to_string(),
            variant: None,
            api: None,
            auth_env: Vec::new(),
            messages: vec![
                ProviderMessage::assistant_tool_call(
                    "",
                    vec![ProviderToolCall {
                        id: "call.1/slash".to_string(),
                        name: "edit".to_string(),
                        input: json!({}),
                    }],
                ),
                ProviderMessage::tool_result("call.1/slash", "edit", "ok", false),
            ],
            tools: vec![ToolListItem {
                id: "edit".to_string(),
                description: String::new(),
                parameters: json!({ "type": "object" }),
            }],
            options: Default::default(),
            headers: Default::default(),
        });
        assert_eq!(request.messages[0].tool_calls[0].id, "call_1_slash");
        assert_eq!(
            request.messages[1].tool_call_id.as_deref(),
            Some("call_1_slash")
        );
    }

    #[test]
    fn normalizes_tool_call_ids_for_provider_families() {
        let request = normalize_request(ProviderGenerationRequest {
            session_id: None,
            provider_id: "anthropic".to_string(),
            model_id: "sonnet-latest".to_string(),
            variant: None,
            api: None,
            auth_env: Vec::new(),
            messages: vec![
                ProviderMessage::assistant_tool_call(
                    "",
                    vec![ProviderToolCall {
                        id: "call.1/slash".to_string(),
                        name: "edit".to_string(),
                        input: json!({}),
                    }],
                ),
                ProviderMessage::tool_result("call.1/slash", "edit", "ok", false),
            ],
            tools: Vec::new(),
            options: Default::default(),
            headers: Default::default(),
        });
        assert_eq!(request.messages[0].tool_calls[0].id, "call_1_slash");
        assert_eq!(
            request.messages[1].tool_call_id.as_deref(),
            Some("call_1_slash")
        );
    }

    #[test]
    fn anthropic_sequence_filters_empty_messages_and_splits_tool_call_text() {
        let request = normalize_request(ProviderGenerationRequest {
            session_id: None,
            provider_id: "amazon-bedrock".to_string(),
            model_id: "anthropic.claude-sonnet-4".to_string(),
            variant: None,
            api: None,
            auth_env: Vec::new(),
            messages: vec![
                ProviderMessage::text(ProviderRole::User, ""),
                ProviderMessage::assistant_tool_call(
                    "I'll read it.",
                    vec![ProviderToolCall {
                        id: "call.1/slash".to_string(),
                        name: "read".to_string(),
                        input: json!({ "path": "README.md" }),
                    }],
                ),
            ],
            tools: Vec::new(),
            options: Default::default(),
            headers: Default::default(),
        });

        assert_eq!(request.messages.len(), 2);
        assert!(matches!(request.messages[0].role, ProviderRole::Assistant));
        assert_eq!(request.messages[0].content, "I'll read it.");
        assert!(request.messages[0].tool_calls.is_empty());
        assert!(matches!(request.messages[1].role, ProviderRole::Assistant));
        assert!(request.messages[1].content.is_empty());
        assert_eq!(request.messages[1].tool_calls[0].id, "call_1_slash");
    }

    #[test]
    fn mistral_sequence_inserts_assistant_between_tool_and_user() {
        let request = normalize_request(ProviderGenerationRequest {
            session_id: None,
            provider_id: "openai".to_string(),
            model_id: "mistral-medium".to_string(),
            variant: None,
            api: None,
            auth_env: Vec::new(),
            messages: vec![
                ProviderMessage::tool_result("abc", "read", "ok", false),
                ProviderMessage::text(ProviderRole::User, "next"),
            ],
            tools: Vec::new(),
            options: Default::default(),
            headers: Default::default(),
        });
        assert!(matches!(request.messages[1].role, ProviderRole::Assistant));
        assert_eq!(request.messages[1].content, "Done.");
    }

    #[test]
    fn model_defaults_match_non_openai_provider_hints() {
        assert_eq!(model_temperature("qwen3-coder"), Some(0.55));
        assert_eq!(model_top_p("qwen3-coder"), Some(1.0));
        assert_eq!(model_temperature("claude-sonnet-4"), None);
        assert_eq!(model_temperature("gemini-3-pro"), Some(1.0));
        assert_eq!(model_top_p("gemini-3-pro"), Some(0.95));
        assert_eq!(model_temperature("kimi-k2"), Some(0.6));
        assert_eq!(model_temperature("kimi-k2-thinking"), Some(1.0));
    }
}
