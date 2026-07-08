use std::collections::{hash_map::DefaultHasher, BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::Context;
use neoism_agent_core::{
    ModelCacheCost, ModelCost, ModelInfo, ModelLimit, ModelStatus, ProviderApiInfo,
    ProviderCapabilities, ProviderInfo, ProviderInterleaved, ProviderModalities,
    ProviderSource, UserModel,
};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::RwLock;

use crate::provider::provider_api_supported;

const DEFAULT_SOURCE: &str = "https://models.dev";
const CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const CODEX_OPENAI_CONTEXT_LIMIT: u64 = 400_000;
const CODEX_OPENAI_INPUT_LIMIT: u64 = 272_000;
const CODEX_OPENAI_OUTPUT_LIMIT: u64 = 128_000;

#[derive(Clone, Debug, Default)]
pub(crate) struct GenerationMetadata {
    pub(crate) api: Option<ProviderApiInfo>,
    pub(crate) auth_env: Vec<String>,
    pub(crate) limit: Option<ModelLimit>,
    pub(crate) cost: Option<ModelCost>,
    pub(crate) options: BTreeMap<String, Value>,
    pub(crate) headers: BTreeMap<String, String>,
}

#[derive(Clone)]
pub(crate) struct ProviderCatalog {
    source: String,
    path_override: Option<PathBuf>,
    cache_path: PathBuf,
    client: reqwest::Client,
    cached: Arc<RwLock<Option<Vec<ProviderInfo>>>>,
}

impl ProviderCatalog {
    pub(crate) fn from_env() -> Self {
        let source = std::env::var("NEOISM_AGENT_MODELS_URL")
            .unwrap_or_else(|_| DEFAULT_SOURCE.to_string());
        let cache_path =
            PathBuf::from(crate::default_cache_dir()).join(if source == DEFAULT_SOURCE {
                "models.json".to_string()
            } else {
                format!("models-{}.json", stable_hash(&source))
            });
        Self {
            source,
            path_override: std::env::var("NEOISM_AGENT_MODELS_PATH")
                .ok()
                .map(PathBuf::from),
            cache_path,
            client: reqwest::Client::new(),
            cached: Arc::new(RwLock::new(None)),
        }
    }

    pub(crate) async fn providers(&self) -> anyhow::Result<Vec<ProviderInfo>> {
        if let Some(providers) = self.cached.read().await.as_ref().cloned() {
            return Ok(providers);
        }

        let providers = self.load().await?;
        *self.cached.write().await = Some(providers.clone());
        Ok(providers)
    }

    pub(crate) async fn refresh(&self, force: bool) -> anyhow::Result<()> {
        if !force && self.cache_fresh() {
            return Ok(());
        }
        let raw = self.fetch_api().await?;
        write_cache(&self.cache_path, &raw)?;
        *self.cached.write().await = Some(parse_models(&raw)?);
        Ok(())
    }

    async fn load(&self) -> anyhow::Result<Vec<ProviderInfo>> {
        if let Some(path) = &self.path_override {
            if let Some(raw) = read_to_string(path) {
                return parse_models(&raw);
            }
        }

        if let Some(raw) = read_to_string(&self.cache_path) {
            if self.cache_fresh() || fetch_disabled() {
                return parse_models(&raw);
            }

            match self.refresh(false).await {
                Ok(()) => {
                    return Ok(self
                        .cached
                        .read()
                        .await
                        .as_ref()
                        .cloned()
                        .unwrap_or_default());
                }
                Err(_) => return parse_models(&raw),
            }
        }

        if fetch_disabled() {
            return Ok(Vec::new());
        }

        let raw = self.fetch_api().await?;
        let providers = parse_models(&raw)?;
        let _ = write_cache(&self.cache_path, &raw);
        Ok(providers)
    }

    async fn fetch_api(&self) -> anyhow::Result<String> {
        Ok(self
            .client
            .get(format!("{}/api.json", self.source.trim_end_matches('/')))
            .header(
                reqwest::header::USER_AGENT,
                format!("neoism-agent/{}", env!("CARGO_PKG_VERSION")),
            )
            .timeout(Duration::from_secs(10))
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?)
    }

    fn cache_fresh(&self) -> bool {
        self.cache_path
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .map(|age| age < CACHE_TTL)
            .unwrap_or(false)
    }
}

fn parse_models(raw: &str) -> anyhow::Result<Vec<ProviderInfo>> {
    let providers: BTreeMap<String, ModelsDevProvider> =
        serde_json::from_str(raw).context("failed to parse models.dev catalog")?;
    let mut providers = providers
        .into_values()
        .map(from_models_dev_provider)
        .collect::<Vec<_>>();
    providers.push(claude_code_provider());
    Ok(providers)
}

fn claude_code_provider() -> ProviderInfo {
    let base_url = std::env::var("CLAUDE_CODE_PROXY_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:3456/v1".to_string());
    let mut models = BTreeMap::new();
    for (id, name, context, output, reasoning) in [
        (
            "claude-sonnet-4-6",
            "Claude Sonnet 4.6 (Claude Code)",
            200_000,
            64_000,
            true,
        ),
        (
            "claude-opus-4-6",
            "Claude Opus 4.6 (Claude Code)",
            200_000,
            32_000,
            true,
        ),
        (
            "claude-opus-4-7",
            "Claude Opus 4.7 (Claude Code)",
            200_000,
            32_000,
            true,
        ),
        (
            "claude-opus-4-8",
            "Claude Opus 4.8 (Claude Code)",
            200_000,
            32_000,
            true,
        ),
        (
            "claude-fable-5",
            "Claude Fable 5 (Claude Code)",
            200_000,
            32_000,
            true,
        ),
        (
            "claude-haiku-4-5",
            "Claude Haiku 4.5 (Claude Code)",
            200_000,
            32_000,
            false,
        ),
    ] {
        models.insert(
            id.to_string(),
            ModelInfo {
                id: id.to_string(),
                provider_id: "claude-code".to_string(),
                name: name.to_string(),
                api: ProviderApiInfo {
                    id: id.to_string(),
                    url: base_url.clone(),
                    npm: "@ai-sdk/anthropic".to_string(),
                },
                family: Some("claude".to_string()),
                capabilities: ProviderCapabilities {
                    temperature: true,
                    reasoning,
                    attachment: true,
                    tool_call: true,
                    input: modalities(None),
                    output: modalities(None),
                    interleaved: ProviderInterleaved::default(),
                },
                cost: ModelCost::default(),
                limit: ModelLimit {
                    context,
                    input: Some(context),
                    output,
                },
                status: ModelStatus::Active,
                options: BTreeMap::new(),
                headers: BTreeMap::new(),
                release_date: "2025-01-01".to_string(),
                variants: None,
            },
        );
    }
    ProviderInfo {
        id: "claude-code".to_string(),
        name: "Claude Code".to_string(),
        source: ProviderSource::Custom,
        env: Vec::new(),
        key: None,
        options: BTreeMap::new(),
        models,
    }
}

fn from_models_dev_provider(provider: ModelsDevProvider) -> ProviderInfo {
    let models = provider
        .models
        .iter()
        .flat_map(|(key, model)| {
            let mut entries =
                vec![(key.clone(), from_models_dev_model(&provider, model, None))];
            if let Some(modes) = model
                .experimental
                .as_ref()
                .and_then(|experimental| experimental.modes.as_ref())
            {
                entries.extend(modes.iter().map(|(mode, options)| {
                    let id = format!("{}-{mode}", model.id);
                    let mut variant = from_models_dev_model(&provider, model, Some(mode));
                    variant.id = id.clone();
                    variant.name = format!("{} {}", model.name, title_case(mode));
                    if let Some(cost) = &options.cost {
                        variant.cost = model_cost(Some(cost));
                    }
                    if let Some(provider_options) = &options.provider {
                        variant.options =
                            provider_options.body.clone().unwrap_or_default();
                        variant.headers =
                            provider_options.headers.clone().unwrap_or_default();
                    }
                    (id, variant)
                }));
            }
            entries
        })
        .collect();

    ProviderInfo {
        id: provider.id,
        name: provider.name,
        source: ProviderSource::Custom,
        env: provider.env,
        key: None,
        options: BTreeMap::new(),
        models,
    }
}

/// Well-known API base URL for a native AI-SDK adapter, used when the catalog
/// leaves the URL blank (the SDK would otherwise supply it). Only the adapters
/// neoism can actually stream through are listed; generic
/// `@ai-sdk/openai-compatible` providers always carry an explicit URL from the
/// catalog, so they aren't defaulted here.
/// Env var name that overrides a provider's base URL, e.g. provider `openrouter`
/// → `NEOISM_AGENT_BASE_URL_OPENROUTER`, `claude-code` → `..._CLAUDE_CODE`.
fn provider_base_url_env_key(provider_id: &str) -> String {
    let suffix = provider_id
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch.to_ascii_uppercase() } else { '_' })
        .collect::<String>();
    format!("NEOISM_AGENT_BASE_URL_{suffix}")
}

fn default_base_url_for_npm(npm: &str) -> Option<&'static str> {
    Some(match npm {
        "@ai-sdk/openai" => "https://api.openai.com/v1",
        "@ai-sdk/anthropic" => "https://api.anthropic.com/v1",
        "@ai-sdk/xai" => "https://api.x.ai/v1",
        "@ai-sdk/groq" => "https://api.groq.com/openai/v1",
        "@ai-sdk/mistral" => "https://api.mistral.ai/v1",
        "@ai-sdk/cerebras" => "https://api.cerebras.ai/v1",
        "@ai-sdk/perplexity" => "https://api.perplexity.ai",
        "@ai-sdk/deepinfra" => "https://api.deepinfra.com/v1/openai",
        "@ai-sdk/togetherai" => "https://api.together.xyz/v1",
        _ => return None,
    })
}

fn from_models_dev_model(
    provider: &ModelsDevProvider,
    model: &ModelsDevModel,
    mode: Option<&str>,
) -> ModelInfo {
    let npm = model
        .provider
        .as_ref()
        .and_then(|provider| provider.npm.clone())
        .or(provider.npm.clone())
        .unwrap_or_else(|| "@ai-sdk/openai-compatible".to_string());
    let mut api = model
        .provider
        .as_ref()
        .and_then(|provider| provider.api.clone())
        .or(provider.api.clone())
        .unwrap_or_default();
    // models.dev omits a base URL for native-SDK providers (e.g. xAI, Groq,
    // Mistral) that rely on the AI SDK's built-in endpoint. neoism's
    // OpenAI-compatible adapter needs an explicit URL, so fall back to the
    // provider's well-known endpoint — otherwise the whole provider is dropped
    // from `/connect` and `/model` for having an "unsupported" (empty-URL) API.
    if api.trim().is_empty() {
        if let Some(default_url) = default_base_url_for_npm(&npm) {
            api = default_url.to_string();
        }
    }
    // Per-provider base-URL override, e.g. `NEOISM_AGENT_BASE_URL_OPENROUTER`
    // or `NEOISM_AGENT_BASE_URL_CLAUDE_CODE`. Lets a user route a provider
    // through a gateway/proxy (LiteLLM, Cloudflare AI Gateway) or a self-hosted
    // OpenAI-compatible server without editing the catalog. Wins over both the
    // catalog URL and the built-in default.
    if let Ok(override_url) = std::env::var(provider_base_url_env_key(&provider.id)) {
        let override_url = override_url.trim();
        if !override_url.is_empty() {
            api = override_url.to_string();
        }
    }
    let provider_options = model.provider.as_ref();
    ModelInfo {
        id: mode
            .map(|mode| format!("{}-{mode}", model.id))
            .unwrap_or_else(|| model.id.clone()),
        provider_id: provider.id.clone(),
        name: mode
            .map(|mode| format!("{} {}", model.name, title_case(mode)))
            .unwrap_or_else(|| model.name.clone()),
        api: ProviderApiInfo {
            id: model.id.clone(),
            url: api,
            npm,
        },
        family: model.family.clone(),
        capabilities: ProviderCapabilities {
            temperature: model.temperature,
            reasoning: model.reasoning,
            attachment: model.attachment,
            tool_call: model.tool_call,
            input: modalities(
                model
                    .modalities
                    .as_ref()
                    .map(|modalities| &modalities.input),
            ),
            output: modalities(
                model
                    .modalities
                    .as_ref()
                    .map(|modalities| &modalities.output),
            ),
            interleaved: model.interleaved.clone().unwrap_or_default(),
        },
        cost: model_cost(model.cost.as_ref()),
        limit: ModelLimit {
            context: model.limit.context,
            input: model.limit.input,
            output: model.limit.output,
        },
        status: model.status.clone().unwrap_or(ModelStatus::Active),
        options: provider_options
            .and_then(|provider| provider.body.clone())
            .unwrap_or_default(),
        headers: provider_options
            .and_then(|provider| provider.headers.clone())
            .unwrap_or_default(),
        release_date: model.release_date.clone(),
        variants: None,
    }
}

fn model_cost(cost: Option<&ModelsDevCost>) -> ModelCost {
    ModelCost {
        input: cost.map(|cost| cost.input).unwrap_or(0.0),
        output: cost.map(|cost| cost.output).unwrap_or(0.0),
        cache: ModelCacheCost {
            read: cost.and_then(|cost| cost.cache_read).unwrap_or(0.0),
            write: cost.and_then(|cost| cost.cache_write).unwrap_or(0.0),
        },
        experimental_over_200k: cost
            .and_then(|cost| cost.context_over_200k.as_ref())
            .map(|cost| Box::new(model_cost(Some(cost)))),
    }
}

fn modalities(values: Option<&Vec<Modality>>) -> ProviderModalities {
    let contains = |modality| {
        values
            .map(|values| values.contains(&modality))
            .unwrap_or(false)
    };
    ProviderModalities {
        text: contains(Modality::Text),
        audio: contains(Modality::Audio),
        image: contains(Modality::Image),
        video: contains(Modality::Video),
        pdf: contains(Modality::Pdf),
    }
}

pub(crate) fn default_model_ids(providers: &[ProviderInfo]) -> BTreeMap<String, String> {
    providers
        .iter()
        .filter_map(|provider| {
            sorted_models(provider)
                .first()
                .map(|model| (provider.id.clone(), model.id.clone()))
        })
        .collect()
}

pub(crate) fn effective_provider_catalog(
    providers: &[ProviderInfo],
) -> Vec<ProviderInfo> {
    let mut output = providers.to_vec();
    let snapshot = output.clone();
    for provider in &mut output {
        if provider.id != "openai" {
            continue;
        }
        for model in provider.models.values_mut() {
            apply_codex_openai_effective_metadata(
                &snapshot,
                provider.id.as_str(),
                model.id.as_str(),
                &mut model.limit,
                &mut model.cost,
            );
        }
    }
    output
}

pub(crate) fn usable_provider_catalog(
    providers: &[ProviderInfo],
    connected_ids: &[String],
) -> Vec<ProviderInfo> {
    let connected = connected_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut output = effective_provider_catalog(providers);
    for provider in &mut output {
        let provider_connected = connected.contains(provider.id.as_str());
        let provider_id = provider.id.clone();
        provider.models.retain(|_, model| {
            model_available_in_picker(model)
                && model_supported_in_picker(&provider_id, model)
                // `claude-code` (routed through the Meridian proxy) is no longer
                // shown unconditionally — like every other provider it must be
                // connected first (via `/connect` → "Connect Meridian", which
                // writes an auth-store marker). `opencode` still exposes its
                // free models before connecting.
                && if provider_id == "opencode" && !provider_connected {
                    public_free_model(model)
                } else {
                    provider_connected
                }
        });
    }
    output.retain(|provider| !provider.models.is_empty());
    output
}

fn model_available_in_picker(model: &ModelInfo) -> bool {
    !matches!(&model.status, ModelStatus::Alpha | ModelStatus::Deprecated)
}

fn model_supported_in_picker(provider_id: &str, model: &ModelInfo) -> bool {
    provider_id == "openai" || provider_api_supported(&model.api)
}

/// Whether a provider could ever appear in the `/model` picker — i.e. it has at
/// least one model neoism can actually stream through (a supported adapter).
/// Used to gate the `/connect` list: there's no point offering to connect a
/// provider we can't use (e.g. `google`/Gemini, `amazon-bedrock`).
pub(crate) fn provider_connectable(provider: &ProviderInfo) -> bool {
    provider
        .models
        .values()
        .any(|model| model_supported_in_picker(&provider.id, model))
}

fn public_free_model(model: &ModelInfo) -> bool {
    model.cost.input == 0.0 && model.cost.output == 0.0
}

pub(crate) fn generation_metadata(
    providers: &[ProviderInfo],
    model: &UserModel,
) -> GenerationMetadata {
    let Some(provider) = providers
        .iter()
        .find(|provider| provider.id == model.provider_id)
    else {
        return GenerationMetadata::default();
    };
    let mut candidates = Vec::new();
    if let Some(variant) = model.variant.as_deref().filter(|value| !value.is_empty()) {
        candidates.push(format!("{}-{variant}", model.model_id));
    }
    candidates.push(model.model_id.clone());
    let model_info = candidates
        .iter()
        .find_map(|candidate| provider.models.get(candidate));
    let Some(model_info) = model_info else {
        return GenerationMetadata {
            auth_env: provider.env.clone(),
            ..GenerationMetadata::default()
        };
    };
    let mut headers = model_info.headers.clone();
    apply_default_headers(&model_info.api, &mut headers);
    let mut limit = model_info.limit.clone();
    let mut cost = model_info.cost.clone();
    apply_codex_openai_effective_metadata(
        providers,
        model.provider_id.as_str(),
        model.model_id.as_str(),
        &mut limit,
        &mut cost,
    );
    GenerationMetadata {
        api: Some(model_info.api.clone()),
        auth_env: provider.env.clone(),
        limit: Some(limit),
        cost: Some(cost),
        options: model_info.options.clone(),
        headers,
    }
}

fn apply_codex_openai_effective_metadata(
    providers: &[ProviderInfo],
    provider_id: &str,
    model_id: &str,
    limit: &mut ModelLimit,
    cost: &mut ModelCost,
) {
    if provider_id != "openai" || !uses_codex_subscription_limits(model_id, limit) {
        return;
    }
    if let Some(codex_limit) = codex_catalog_limit(providers, model_id) {
        *limit = codex_limit;
    } else {
        limit.context = CODEX_OPENAI_CONTEXT_LIMIT;
        limit.input = Some(CODEX_OPENAI_INPUT_LIMIT);
        limit.output = CODEX_OPENAI_OUTPUT_LIMIT;
    }
    *cost = ModelCost::default();
}

fn codex_catalog_limit(providers: &[ProviderInfo], model_id: &str) -> Option<ModelLimit> {
    providers
        .iter()
        .find(|provider| provider.id == "github-copilot")
        .and_then(|provider| provider.models.get(model_id))
        .map(|model| model.limit.clone())
        .filter(|limit| limit.context > 0)
}

fn uses_codex_subscription_limits(model_id: &str, limit: &ModelLimit) -> bool {
    model_id.starts_with("gpt-5.4")
        || model_id.starts_with("gpt-5.5")
        || (model_id.contains("codex") && limit.context > CODEX_OPENAI_CONTEXT_LIMIT)
}

fn apply_default_headers(api: &ProviderApiInfo, headers: &mut BTreeMap<String, String>) {
    if matches!(
        api.npm.as_str(),
        "@openrouter/ai-sdk-provider" | "@llmgateway/ai-sdk-provider"
    ) {
        headers
            .entry("HTTP-Referer".to_string())
            .or_insert_with(|| "https://neoism.ai/".to_string());
        headers
            .entry("X-Title".to_string())
            .or_insert_with(|| "neoism".to_string());
    }
}

fn sorted_models(provider: &ProviderInfo) -> Vec<&ModelInfo> {
    let mut models = provider.models.values().collect::<Vec<_>>();
    models.sort_by(|left, right| {
        model_rank(right)
            .cmp(&model_rank(left))
            .then_with(|| right.id.cmp(&left.id))
    });
    models
}

fn model_rank(model: &ModelInfo) -> i32 {
    const PRIORITY: &[&str] = &["gpt-5", "claude-sonnet-4", "big-pickle", "gemini-3-pro"];
    PRIORITY
        .iter()
        .position(|needle| model.id.contains(needle))
        .map(|index| 100 - index as i32)
        .unwrap_or_else(|| if model.id.contains("latest") { 1 } else { 0 })
}

fn read_to_string(path: &PathBuf) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn write_cache(path: &PathBuf, raw: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, raw)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

fn fetch_disabled() -> bool {
    std::env::var("NEOISM_AGENT_DISABLE_MODELS_FETCH")
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

fn stable_hash(value: &str) -> String {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

fn title_case(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[derive(Clone, Debug, Deserialize)]
struct ModelsDevProvider {
    api: Option<String>,
    name: String,
    #[serde(default)]
    env: Vec<String>,
    id: String,
    npm: Option<String>,
    #[serde(default)]
    models: BTreeMap<String, ModelsDevModel>,
}

#[derive(Clone, Debug, Deserialize)]
struct ModelsDevModel {
    id: String,
    name: String,
    family: Option<String>,
    release_date: String,
    #[serde(default)]
    attachment: bool,
    #[serde(default)]
    reasoning: bool,
    #[serde(default)]
    temperature: bool,
    #[serde(default)]
    tool_call: bool,
    interleaved: Option<ProviderInterleaved>,
    cost: Option<ModelsDevCost>,
    limit: ModelsDevLimit,
    modalities: Option<ModelsDevModalities>,
    experimental: Option<ModelsDevExperimental>,
    status: Option<ModelStatus>,
    provider: Option<ModelsDevModelProvider>,
}

#[derive(Clone, Debug, Deserialize)]
struct ModelsDevCost {
    input: f64,
    output: f64,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
    context_over_200k: Option<Box<ModelsDevCost>>,
}

#[derive(Clone, Debug, Deserialize)]
struct ModelsDevLimit {
    context: u64,
    input: Option<u64>,
    output: u64,
}

#[derive(Clone, Debug, Deserialize)]
struct ModelsDevModalities {
    #[serde(default)]
    input: Vec<Modality>,
    #[serde(default)]
    output: Vec<Modality>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Modality {
    Text,
    Audio,
    Image,
    Video,
    Pdf,
}

#[derive(Clone, Debug, Deserialize)]
struct ModelsDevExperimental {
    modes: Option<BTreeMap<String, ModelsDevMode>>,
}

#[derive(Clone, Debug, Deserialize)]
struct ModelsDevMode {
    cost: Option<ModelsDevCost>,
    provider: Option<ModelsDevModeProvider>,
}

#[derive(Clone, Debug, Deserialize)]
struct ModelsDevModeProvider {
    body: Option<BTreeMap<String, Value>>,
    headers: Option<BTreeMap<String, String>>,
}

#[derive(Clone, Debug, Deserialize)]
struct ModelsDevModelProvider {
    npm: Option<String>,
    api: Option<String>,
    body: Option<BTreeMap<String, Value>>,
    headers: Option<BTreeMap<String, String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_metadata_uses_model_api_options_headers_and_default_headers() {
        let providers = parse_models(
            r#"{
              "openrouter": {
                "id": "openrouter",
                "name": "OpenRouter",
                "env": ["OPENROUTER_API_KEY"],
                "npm": "@openrouter/ai-sdk-provider",
                "api": "https://openrouter.ai/api/v1",
                "models": {
                  "openai/gpt-5": {
                    "id": "openai/gpt-5",
                    "name": "GPT-5",
                    "release_date": "2026-01-01",
                    "limit": { "context": 128000, "output": 4096 },
                    "provider": {
                      "api": "https://openrouter.ai/api/v1",
                      "npm": "@openrouter/ai-sdk-provider",
                      "body": { "temperature": 0.2 },
                      "headers": { "X-Test": "yes" }
                    }
                  }
                }
              }
            }"#,
        )
        .unwrap();
        let model = UserModel {
            provider_id: "openrouter".to_string(),
            model_id: "openai/gpt-5".to_string(),
            variant: None,
        };

        let metadata = generation_metadata(&providers, &model);

        assert_eq!(
            metadata.api.as_ref().map(|api| api.npm.as_str()),
            Some("@openrouter/ai-sdk-provider")
        );
        assert_eq!(metadata.auth_env, vec!["OPENROUTER_API_KEY"]);
        assert_eq!(
            metadata.limit.as_ref().map(|limit| limit.context),
            Some(128_000)
        );
        assert_eq!(metadata.options["temperature"], 0.2);
        assert_eq!(metadata.headers["X-Test"], "yes");
        assert_eq!(metadata.headers["HTTP-Referer"], "https://neoism.ai/");
        assert_eq!(metadata.headers["X-Title"], "neoism");
    }

    #[test]
    fn generation_metadata_uses_copilot_codex_limits_for_openai_oauth_models() {
        let providers = parse_codex_limit_fixture();
        let model = UserModel {
            provider_id: "openai".to_string(),
            model_id: "gpt-5.5".to_string(),
            variant: None,
        };

        let metadata = generation_metadata(&providers, &model);
        let limit = metadata.limit.expect("limit");
        let cost = metadata.cost.expect("cost");

        assert_eq!(limit.context, 400_000);
        assert_eq!(limit.input, Some(272_000));
        assert_eq!(limit.output, 128_000);
        assert_eq!(cost.input, 0.0);
        assert_eq!(cost.output, 0.0);
        assert_eq!(cost.cache.read, 0.0);
        assert_eq!(cost.cache.write, 0.0);
    }

    #[test]
    fn effective_provider_catalog_uses_copilot_codex_limits_for_ui_models() {
        let providers = effective_provider_catalog(&parse_codex_limit_fixture());
        let openai = providers
            .iter()
            .find(|provider| provider.id == "openai")
            .expect("openai provider");
        let model = openai.models.get("gpt-5.5").expect("gpt-5.5 model");

        assert_eq!(model.limit.context, 400_000);
        assert_eq!(model.limit.input, Some(272_000));
        assert_eq!(model.limit.output, 128_000);
        assert_eq!(model.cost.input, 0.0);
        assert_eq!(model.cost.output, 0.0);
    }

    #[test]
    fn usable_provider_catalog_shows_free_opencode_and_connected_supported_models() {
        let providers = parse_models(
            r#"{
              "opencode": {
                "id": "opencode",
                "name": "OpenCode Zen",
                "env": ["OPENCODE_API_KEY"],
                "npm": "@ai-sdk/openai-compatible",
                "api": "https://opencode.ai/zen/v1",
                "models": {
                  "free": {
                    "id": "free",
                    "name": "Free",
                    "release_date": "2026-01-01",
                    "limit": { "context": 200000, "output": 32000 },
                    "cost": { "input": 0, "output": 0 }
                  },
                  "paid": {
                    "id": "paid",
                    "name": "Paid",
                    "release_date": "2026-01-01",
                    "limit": { "context": 200000, "output": 32000 },
                    "cost": { "input": 1, "output": 2 }
                  },
                  "old-free": {
                    "id": "old-free",
                    "name": "Old Free",
                    "release_date": "2026-01-01",
                    "status": "deprecated",
                    "limit": { "context": 200000, "output": 32000 },
                    "cost": { "input": 0, "output": 0 }
                  }
                }
              },
              "openai": {
                "id": "openai",
                "name": "OpenAI",
                "env": ["OPENAI_API_KEY"],
                "npm": "@ai-sdk/openai",
                "models": {
                  "gpt": {
                    "id": "gpt",
                    "name": "GPT",
                    "release_date": "2026-01-01",
                    "limit": { "context": 128000, "output": 32000 }
                  }
                }
              },
              "anthropic": {
                "id": "anthropic",
                "name": "Anthropic",
                "env": ["ANTHROPIC_API_KEY"],
                "npm": "@ai-sdk/anthropic",
                "api": "https://api.anthropic.com/v1",
                "models": {
                  "sonnet": {
                    "id": "sonnet",
                    "name": "Sonnet",
                    "release_date": "2026-01-01",
                    "limit": { "context": 200000, "output": 32000 }
                  }
                }
              },
              "google": {
                "id": "google",
                "name": "Google",
                "env": ["GOOGLE_GENERATIVE_AI_API_KEY"],
                "npm": "@ai-sdk/google",
                "api": "https://generativelanguage.googleapis.com/v1beta",
                "models": {
                  "gemini": {
                    "id": "gemini",
                    "name": "Gemini",
                    "release_date": "2026-01-01",
                    "limit": { "context": 200000, "output": 32000 }
                  }
                }
              }
            }"#,
        )
        .unwrap();

        let usable = usable_provider_catalog(
            &providers,
            &["openai".to_string(), "google".to_string()],
        );

        let opencode = usable
            .iter()
            .find(|provider| provider.id == "opencode")
            .expect("opencode free provider");
        assert_eq!(
            opencode.models.keys().cloned().collect::<Vec<_>>(),
            vec!["free".to_string()]
        );
        let openai = usable
            .iter()
            .find(|provider| provider.id == "openai")
            .expect("connected openai provider");
        assert!(openai.models.contains_key("gpt"));
        assert!(usable.iter().all(|provider| provider.id != "anthropic"));
        assert!(usable.iter().all(|provider| provider.id != "google"));
    }

    fn parse_codex_limit_fixture() -> Vec<ProviderInfo> {
        parse_models(
            r#"{
              "openai": {
                "id": "openai",
                "name": "OpenAI",
                "env": ["OPENAI_API_KEY"],
                "models": {
                  "gpt-5.5": {
                    "id": "gpt-5.5",
                    "name": "GPT-5.5",
                    "release_date": "2026-04-23",
                    "limit": { "context": 1050000, "input": 922000, "output": 128000 },
                    "cost": { "input": 1.25, "output": 10.0, "cache_read": 0.125, "cache_write": 1.25 }
                  }
                }
              },
              "github-copilot": {
                "id": "github-copilot",
                "name": "GitHub Copilot",
                "env": ["GITHUB_COPILOT_TOKEN"],
                "models": {
                  "gpt-5.5": {
                    "id": "gpt-5.5",
                    "name": "GPT-5.5",
                    "release_date": "2026-04-23",
                    "limit": { "context": 400000, "input": 272000, "output": 128000 },
                    "cost": { "input": 5.0, "output": 30.0, "cache_read": 0.5 }
                  }
                }
              }
            }"#,
        )
        .unwrap()
    }
}
