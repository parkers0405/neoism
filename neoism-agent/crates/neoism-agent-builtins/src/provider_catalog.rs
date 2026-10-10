use std::collections::{hash_map::DefaultHasher, BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::Context;
use neoism_agent_core::{
    ModelCacheCost, ModelCost, ModelInfo, ModelLimit, ModelStatus, ProviderApiInfo,
    ProviderAuthMode, ProviderCapabilities, ProviderConfig, ProviderInfo,
    ProviderInterleaved, ProviderModalities, ProviderModelConfig, ProviderSource,
    UserModel,
};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::{Mutex, RwLock};

use crate::provider::provider_api_supported;

const DEFAULT_SOURCE: &str = "https://models.dev";
const CACHE_TTL: Duration = Duration::from_secs(5 * 60);
// Conservative fallback from Codex's bundled models-manager/models.json.
// Only account-scoped /models metadata can authorize a larger window;
// platform API and Copilot catalogs are not subscription-limit authorities.
const CODEX_OPENAI_CONTEXT_LIMIT: u64 = 272_000;
// Conservative upstream allowance when the optional percentage is missing or
// invalid. Account metadata currently advertises 95; never assume 100 instead.
const CODEX_EFFECTIVE_CONTEXT_PERCENT: u64 = 95;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct CodexModelMetadata {
    #[serde(default, deserialize_with = "optional_metadata_u64")]
    pub context_window: Option<u64>,
    #[serde(default, deserialize_with = "optional_metadata_u64")]
    pub max_context_window: Option<u64>,
    #[serde(default, deserialize_with = "optional_metadata_u64")]
    pub effective_context_window_percent: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct GenerationMetadata {
    pub api: Option<ProviderApiInfo>,
    pub auth_env: Vec<String>,
    pub limit: Option<ModelLimit>,
    pub cost: Option<ModelCost>,
    pub options: BTreeMap<String, Value>,
    pub headers: BTreeMap<String, String>,
}

// A malformed optional limit must not discard the whole authenticated catalog
// (or the other valid fields). Strings, negatives, floats and overflow are absent.
fn optional_metadata_u64<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<u64>, D::Error> {
    Ok(Value::deserialize(deserializer)?.as_u64())
}

#[derive(Clone, Debug, Default)]
pub enum OpenAiModelAccess {
    #[default]
    Api,
    Codex(BTreeMap<String, CodexModelMetadata>),
}

#[derive(Clone)]
pub struct ProviderCatalog {
    source: String,
    path_override: Option<PathBuf>,
    cache_path: PathBuf,
    client: reqwest::Client,
    cached: Arc<RwLock<Option<Vec<ProviderInfo>>>>,
    cold_load_gate: Arc<Mutex<()>>,
    refresh_gate: Arc<Mutex<()>>,
    configured: Arc<BTreeMap<String, ProviderConfig>>,
    discovered: Arc<RwLock<BTreeMap<String, (std::time::Instant, Vec<String>)>>>,
    discovery_gate: Arc<Mutex<()>>,
}

impl ProviderCatalog {
    pub fn from_env_with_config(configured: BTreeMap<String, ProviderConfig>) -> Self {
        let source = std::env::var("NEOISM_AGENT_MODELS_URL")
            .unwrap_or_else(|_| DEFAULT_SOURCE.to_string());
        let cache_path = crate::default_cache_dir().join(if source == DEFAULT_SOURCE {
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
            cold_load_gate: Arc::new(Mutex::new(())),
            refresh_gate: Arc::new(Mutex::new(())),
            configured: Arc::new(configured),
            discovered: Arc::new(RwLock::new(BTreeMap::new())),
            discovery_gate: Arc::new(Mutex::new(())),
        }
    }

    pub async fn providers_for_access(
        &self,
        access: &OpenAiModelAccess,
        restrict_to_listed: bool,
    ) -> anyhow::Result<Vec<ProviderInfo>> {
        let raw = self.providers().await?;
        Ok(effective_catalog_with_config(
            &raw,
            access,
            &self.configured,
            restrict_to_listed,
        ))
    }

    pub async fn providers(&self) -> anyhow::Result<Vec<ProviderInfo>> {
        if let Some(providers) = self.cached.read().await.as_ref().cloned() {
            return self.apply_configured(providers).await;
        }

        // Only the first cache miss performs disk/network work. Other callers
        // wait for that result instead of launching duplicate catalog fetches.
        let _load = self.cold_load_gate.lock().await;
        if let Some(providers) = self.cached.read().await.as_ref().cloned() {
            return self.apply_configured(providers).await;
        }
        let (providers, refresh_stale) = self.load().await?;
        *self.cached.write().await = Some(providers.clone());
        if refresh_stale {
            self.schedule_refresh();
        }
        self.apply_configured(providers).await
    }

    async fn apply_configured(
        &self,
        mut providers: Vec<ProviderInfo>,
    ) -> anyhow::Result<Vec<ProviderInfo>> {
        for (provider_id, config) in self.configured.iter() {
            let existing = providers
                .iter()
                .position(|provider| provider.id == *provider_id);
            let is_custom = existing.is_none();
            let auth = config.auth.unwrap_or(if is_custom {
                ProviderAuthMode::None
            } else {
                ProviderAuthMode::Required
            });
            let base_url = config
                .options
                .base_url
                .clone()
                .or_else(|| {
                    existing.and_then(|index| {
                        providers[index]
                            .models
                            .values()
                            .next()
                            .map(|model| model.api.url.clone())
                    })
                })
                .unwrap_or_default()
                .trim_end_matches('/')
                .to_string();
            let npm = config
                .npm
                .clone()
                .unwrap_or_else(|| "@ai-sdk/openai-compatible".to_string());
            let mut provider = existing
                .map(|index| providers.remove(index))
                .unwrap_or_else(|| ProviderInfo {
                    id: provider_id.clone(),
                    name: config.name.clone().unwrap_or_else(|| provider_id.clone()),
                    source: ProviderSource::Config,
                    env: Vec::new(),
                    key: None,
                    options: BTreeMap::new(),
                    models: BTreeMap::new(),
                });
            if let Some(name) = &config.name {
                provider.name = name.clone();
            }
            if !config.env.is_empty() {
                provider.env = config.env.clone();
            }

            if config.discover_models && !base_url.is_empty() {
                for model_id in self.discover_model_ids(&base_url, auth).await {
                    provider.models.entry(model_id.clone()).or_insert_with(|| {
                        local_model(
                            provider_id,
                            &model_id,
                            &model_id,
                            &base_url,
                            &npm,
                            auth,
                            config.compatibility.stream_usage,
                            config.compatibility.reasoning_effort,
                            None,
                        )
                    });
                }
            }
            for (model_id, model_config) in &config.models {
                let wire_id = model_config.id.as_deref().unwrap_or(model_id);
                let model =
                    provider.models.entry(model_id.clone()).or_insert_with(|| {
                        local_model(
                            provider_id,
                            model_id,
                            wire_id,
                            &base_url,
                            &npm,
                            auth,
                            config.compatibility.stream_usage,
                            config.compatibility.reasoning_effort,
                            Some(model_config),
                        )
                    });
                apply_model_config(
                    model,
                    provider_id,
                    model_id,
                    &base_url,
                    &npm,
                    auth,
                    config.compatibility.stream_usage,
                    config.compatibility.reasoning_effort,
                    model_config,
                );
            }
            for model in provider.models.values_mut() {
                if !base_url.is_empty() {
                    model.api.url = base_url.clone();
                }
                if config.npm.is_some() {
                    model.api.npm = npm.clone();
                }
                model.api.auth = auth;
                model.api.stream_usage = Some(config.compatibility.stream_usage);
                model.api.reasoning_effort = Some(config.compatibility.reasoning_effort);
            }
            providers.push(provider);
        }
        Ok(providers)
    }

    async fn discover_model_ids(
        &self,
        base_url: &str,
        auth: ProviderAuthMode,
    ) -> Vec<String> {
        const TTL: Duration = Duration::from_secs(60);
        if let Some((loaded, models)) = self.discovered.read().await.get(base_url) {
            if loaded.elapsed() < TTL {
                return models.clone();
            }
        }
        let _gate = self.discovery_gate.lock().await;
        if let Some((loaded, models)) = self.discovered.read().await.get(base_url) {
            if loaded.elapsed() < TTL {
                return models.clone();
            }
        }
        // Stored credentials are intentionally unavailable to catalog discovery.
        // Authenticated endpoints should declare models manually.
        if auth == ProviderAuthMode::Required {
            return Vec::new();
        }
        let discovered = async {
            let response = self
                .client
                .get(format!("{base_url}/models"))
                .timeout(Duration::from_secs(3))
                .send()
                .await?
                .error_for_status()?;
            if response
                .content_length()
                .is_some_and(|length| length > 1_048_576)
            {
                anyhow::bail!("local model catalog is too large");
            }
            let bytes = response.bytes().await?;
            if bytes.len() > 1_048_576 {
                anyhow::bail!("local model catalog is too large");
            }
            let payload: OpenAiModelsResponse = serde_json::from_slice(&bytes)?;
            let mut ids = payload
                .data
                .into_iter()
                .map(|model| model.id)
                .filter(|id| !id.trim().is_empty() && id.len() <= 512)
                .take(512)
                .collect::<Vec<_>>();
            ids.sort();
            ids.dedup();
            Ok::<_, anyhow::Error>(ids)
        }
        .await;
        match discovered {
            Ok(ids) => {
                self.discovered.write().await.insert(
                    base_url.to_string(),
                    (std::time::Instant::now(), ids.clone()),
                );
                ids
            }
            Err(error) => {
                tracing::debug!(%error, %base_url, "local model discovery failed");
                self.discovered
                    .read()
                    .await
                    .get(base_url)
                    .map(|(_, ids)| ids.clone())
                    .unwrap_or_default()
            }
        }
    }

    pub async fn refresh(&self, force: bool) -> anyhow::Result<()> {
        let _refresh = self.refresh_gate.lock().await;
        if !force && self.cache_fresh().await {
            return Ok(());
        }
        let raw = self.fetch_api().await?;
        let providers = parse_models_async(raw.clone()).await?;
        write_cache_async(self.cache_path.clone(), raw).await?;
        *self.cached.write().await = Some(providers);
        Ok(())
    }

    async fn load(&self) -> anyhow::Result<(Vec<ProviderInfo>, bool)> {
        if let Some(path) = &self.path_override {
            if let Some(raw) = read_to_string_async(path.clone()).await {
                return Ok((parse_models_async(raw).await?, false));
            }
        }

        if let Some(raw) = read_to_string_async(self.cache_path.clone()).await {
            let providers = parse_models_async(raw).await?;
            let refresh_stale = !self.cache_fresh().await && !fetch_disabled();
            return Ok((providers, refresh_stale));
        }

        if fetch_disabled() {
            return Ok((Vec::new(), false));
        }

        let raw = self.fetch_api().await?;
        let providers = parse_models_async(raw.clone()).await?;
        let _ = write_cache_async(self.cache_path.clone(), raw).await;
        Ok((providers, false))
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

    async fn cache_fresh(&self) -> bool {
        tokio::fs::metadata(&self.cache_path)
            .await
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .map(|age| age < CACHE_TTL)
            .unwrap_or(false)
    }

    fn schedule_refresh(&self) {
        let catalog = self.clone();
        tokio::spawn(async move {
            if let Err(error) = catalog.refresh(false).await {
                tracing::debug!(%error, "background model catalog refresh failed");
            }
        });
    }
}

async fn parse_models_async(raw: String) -> anyhow::Result<Vec<ProviderInfo>> {
    tokio::task::spawn_blocking(move || parse_models(&raw))
        .await
        .context("model catalog parser task failed")?
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
            "claude-sonnet-5",
            "Claude Sonnet 5 (Claude Code)",
            200_000,
            64_000,
            true,
        ),
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
                    auth: ProviderAuthMode::Required,
                    tool_call: Some(true),
                    stream_usage: None,
                    reasoning_effort: None,
                    reasoning: None,
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
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
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
            auth: ProviderAuthMode::Required,
            tool_call: Some(model.tool_call),
            stream_usage: None,
            reasoning_effort: None,
            reasoning: None,
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

#[derive(Deserialize)]
struct OpenAiModelsResponse {
    data: Vec<OpenAiModelEntry>,
}

#[derive(Deserialize)]
struct OpenAiModelEntry {
    id: String,
}

fn local_model(
    provider_id: &str,
    picker_id: &str,
    wire_id: &str,
    base_url: &str,
    npm: &str,
    auth: ProviderAuthMode,
    stream_usage: bool,
    reasoning_effort: bool,
    config: Option<&ProviderModelConfig>,
) -> ModelInfo {
    let config = config.cloned().unwrap_or_default();
    ModelInfo {
        id: picker_id.to_string(),
        provider_id: provider_id.to_string(),
        name: config.name.unwrap_or_else(|| picker_id.to_string()),
        api: ProviderApiInfo {
            id: wire_id.to_string(),
            url: base_url.to_string(),
            npm: npm.to_string(),
            auth,
            tool_call: Some(config.tool_call.unwrap_or(false)),
            stream_usage: Some(stream_usage),
            reasoning_effort: Some(reasoning_effort),
            reasoning: None,
        },
        family: config.family,
        capabilities: ProviderCapabilities {
            attachment: config.attachment.unwrap_or(false),
            reasoning: config.reasoning.unwrap_or(false),
            temperature: config.temperature.unwrap_or(false),
            tool_call: config.tool_call.unwrap_or(false),
            input: ProviderModalities {
                text: true,
                ..ProviderModalities::default()
            },
            output: ProviderModalities {
                text: true,
                ..ProviderModalities::default()
            },
            interleaved: ProviderInterleaved::default(),
        },
        cost: ModelCost::default(),
        limit: config.limit.unwrap_or_default(),
        status: ModelStatus::Active,
        options: config.options,
        headers: config.headers,
        release_date: String::new(),
        variants: None,
    }
}

fn apply_model_config(
    model: &mut ModelInfo,
    provider_id: &str,
    picker_id: &str,
    base_url: &str,
    npm: &str,
    auth: ProviderAuthMode,
    stream_usage: bool,
    reasoning_effort: bool,
    config: &ProviderModelConfig,
) {
    model.id = picker_id.to_string();
    model.provider_id = provider_id.to_string();
    model.api.id = config.id.clone().unwrap_or_else(|| picker_id.to_string());
    if !base_url.is_empty() {
        model.api.url = base_url.to_string();
    }
    model.api.npm = npm.to_string();
    model.api.auth = auth;
    model.api.tool_call = config.tool_call.or(model.api.tool_call);
    model.api.stream_usage = Some(stream_usage);
    model.api.reasoning_effort = Some(reasoning_effort);
    if let Some(name) = &config.name {
        model.name = name.clone();
    }
    if let Some(family) = &config.family {
        model.family = Some(family.clone());
    }
    if let Some(value) = config.attachment {
        model.capabilities.attachment = value;
    }
    if let Some(value) = config.reasoning {
        model.capabilities.reasoning = value;
    }
    if let Some(value) = config.temperature {
        model.capabilities.temperature = value;
    }
    if let Some(value) = config.tool_call {
        model.capabilities.tool_call = value;
    }
    if let Some(limit) = &config.limit {
        model.limit = limit.clone();
    }
    model.options.extend(config.options.clone());
    model.headers.extend(config.headers.clone());
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

pub fn default_model_ids(providers: &[ProviderInfo]) -> BTreeMap<String, String> {
    providers
        .iter()
        .filter_map(|provider| {
            sorted_models(provider)
                .first()
                .map(|model| (provider.id.clone(), model.id.clone()))
        })
        .collect()
}

pub fn effective_provider_catalog(
    providers: &[ProviderInfo],
    openai_access: &OpenAiModelAccess,
) -> Vec<ProviderInfo> {
    effective_catalog_with_config(providers, openai_access, &BTreeMap::new(), true)
}

fn effective_catalog_with_config(
    providers: &[ProviderInfo],
    access: &OpenAiModelAccess,
    configured: &BTreeMap<String, ProviderConfig>,
    restrict_to_listed: bool,
) -> Vec<ProviderInfo> {
    let mut output = providers.to_vec();
    for provider in &mut output {
        if provider.id != "openai" {
            continue;
        }
        if let OpenAiModelAccess::Codex(models) = access {
            if restrict_to_listed {
                // Match the actual wire ID, not a picker alias or variant label.
                provider
                    .models
                    .retain(|_, model| models.contains_key(&model.api.id));
            }
            for (key, model) in &mut provider.models {
                let explicit = configured
                    .get("openai")
                    .and_then(|config| {
                        config.models.get(key).or_else(|| {
                            // Catalog mode entries use <wire-id>-<mode>. They
                            // must not bypass a lower limit on their base model.
                            // Do not spread limits across unrelated aliases.
                            key.strip_prefix(model.api.id.as_str())
                                .filter(|suffix| suffix.starts_with('-'))
                                .and_then(|_| config.models.get(&model.api.id))
                        })
                    })
                    .and_then(|config| config.limit.as_ref());
                apply_codex_limit(&mut model.limit, models.get(&model.api.id), explicit);
                if let Some(explicit) = explicit {
                    model.limit.output = model.limit.output.min(explicit.output);
                }
                model.cost = ModelCost::default();
            }
        }
    }
    output
}

pub fn usable_provider_catalog(
    providers: &[ProviderInfo],
    connected_ids: &[String],
    openai_access: &OpenAiModelAccess,
) -> Vec<ProviderInfo> {
    let connected = connected_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut output = effective_provider_catalog(providers, openai_access);
    for provider in &mut output {
        let provider_connected = connected.contains(provider.id.as_str());
        let provider_id = provider.id.clone();
        provider.models.retain(|_, model| {
            model_available_in_picker(model)
                && model_supported_in_picker(&provider_id, model)
                && provider_connected
        });
    }
    output.retain(|provider| !provider.models.is_empty());
    output
}

pub fn connect_provider_catalog(
    providers: &[ProviderInfo],
    connected_ids: &[String],
    openai_access: &OpenAiModelAccess,
) -> Vec<ProviderInfo> {
    let connectable = providers
        .iter()
        .filter(|provider| provider_connectable(provider))
        .map(|provider| provider.id.as_str())
        .collect::<BTreeSet<_>>();
    let connected = connected_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut output = effective_provider_catalog(providers, openai_access);
    output.retain(|provider| connectable.contains(provider.id.as_str()));
    for provider in &mut output {
        if provider.id == "opencode" && !connected.contains(provider.id.as_str()) {
            provider.models.clear();
        }
    }
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
pub fn provider_connectable(provider: &ProviderInfo) -> bool {
    provider
        .models
        .values()
        .any(|model| model_supported_in_picker(&provider.id, model))
}

/// Read generation metadata from an already auth-resolved provider catalog.
/// Auth/account limit resolution belongs to providers_for_access, not a second
/// clamp here: picker and generation must consume the same resolved capacity.
pub fn generation_metadata(
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
    let model_info = candidates.iter().find_map(|candidate| {
        provider.models.get(candidate).or_else(|| {
            provider
                .models
                .values()
                .find(|info| info.id == *candidate || info.api.id == *candidate)
        })
    });
    let Some(model_info) = model_info else {
        return GenerationMetadata {
            auth_env: provider.env.clone(),
            ..GenerationMetadata::default()
        };
    };
    let mut headers = model_info.headers.clone();
    apply_default_headers(&model_info.api, &mut headers);
    let mut api = model_info.api.clone();
    api.reasoning = Some(model_info.capabilities.reasoning);
    GenerationMetadata {
        api: Some(api),
        auth_env: provider.env.clone(),
        limit: Some(model_info.limit.clone()),
        cost: Some(model_info.cost.clone()),
        options: model_info.options.clone(),
        headers,
    }
}

/// Whether OpenAI requests will use the Codex ChatGPT-subscription service:
/// `OpenAiRuntime::stream` sends every request over the OAuth Responses path
/// whenever the stored auth is OAuth, regardless of any API key. Codex account
/// metadata and platform API windows differ, so limit resolution must follow
/// the same dispatch rule the runtime uses.
#[cfg(test)]
pub async fn openai_codex_oauth(auth_store: &crate::auth_store::AuthStore) -> bool {
    matches!(
        auth_store.get("openai").await,
        Ok(Some(neoism_agent_core::AuthInfo::OAuth { .. }))
    )
}

fn apply_codex_limit(
    limit: &mut ModelLimit,
    metadata: Option<&CodexModelMetadata>,
    explicit: Option<&ModelLimit>,
) {
    // Codex is authoritative: use this account model's maximum automatically,
    // then its advertised context, then the conservative bundled fallback.
    // Never inherit the platform API catalog's context or input capacity.
    let ceiling = metadata
        .and_then(|m| {
            m.max_context_window
                .filter(|n| *n > 0)
                .or(m.context_window.filter(|n| *n > 0))
        })
        .unwrap_or(CODEX_OPENAI_CONTEXT_LIMIT);
    limit.context = explicit
        .map(|configured| configured.context.min(ceiling))
        .unwrap_or(ceiling);
    let percent = metadata
        .and_then(|m| m.effective_context_window_percent)
        .filter(|n| (1..=100).contains(n))
        .unwrap_or(CODEX_EFFECTIVE_CONTEXT_PERCENT);
    // Multiply in u128 before flooring: saturating u64 multiplication would
    // incorrectly shrink a large valid context, and unchecked multiplication
    // would wrap. With percent <= 100 the result always fits u64.
    let effective_input = ((limit.context as u128 * percent as u128) / 100) as u64;
    limit.input = Some(
        explicit
            .and_then(|configured| configured.input)
            .unwrap_or(effective_input)
            .min(effective_input),
    );
    // This percentage is the upstream allowance, not Neoism's server reserve.
    // session_prompt retains its existing additional conservative reserve. No
    // provenance field exists to prove these policies overlap; do not remove it.
    // /models advertises no output maximum. Preserve the real catalog/config
    // value (including unknown=0); never invent an output cap to fake headroom.
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

async fn read_to_string_async(path: PathBuf) -> Option<String> {
    tokio::fs::read_to_string(path).await.ok()
}

async fn write_cache_async(path: PathBuf, raw: String) -> anyhow::Result<()> {
    tokio::task::spawn_blocking(move || {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, raw)?;
        std::fs::rename(tmp, path)?;
        anyhow::Ok(())
    })
    .await
    .context("model catalog cache writer task failed")?
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
    fn metadata_for_auth(
        providers: &[ProviderInfo],
        model: &UserModel,
        oauth: bool,
    ) -> GenerationMetadata {
        let access = if oauth {
            codex_access(providers)
        } else {
            OpenAiModelAccess::Api
        };
        let effective =
            effective_catalog_with_config(providers, &access, &BTreeMap::new(), false);
        generation_metadata(&effective, model)
    }

    #[tokio::test]
    async fn configured_local_models_are_keyless_and_override_discovery_defaults() {
        let mut models = BTreeMap::new();
        models.insert(
            "qwen".to_string(),
            ProviderModelConfig {
                name: Some("Qwen local".to_string()),
                tool_call: Some(true),
                limit: Some(ModelLimit {
                    context: 65_536,
                    input: None,
                    output: 8_192,
                }),
                ..ProviderModelConfig::default()
            },
        );
        let mut configured = BTreeMap::new();
        configured.insert(
            "llama.cpp".to_string(),
            ProviderConfig {
                name: Some("llama-server (local)".to_string()),
                auth: Some(ProviderAuthMode::None),
                options: neoism_agent_core::ProviderConfigOptions {
                    base_url: Some("http://127.0.0.1:8080/v1/".to_string()),
                },
                models,
                ..ProviderConfig::default()
            },
        );
        let catalog = ProviderCatalog::from_env_with_config(configured);
        let providers = catalog.apply_configured(Vec::new()).await.unwrap();
        let provider = &providers[0];
        let model = &provider.models["qwen"];

        assert_eq!(provider.name, "llama-server (local)");
        assert_eq!(model.api.url, "http://127.0.0.1:8080/v1");
        assert_eq!(model.api.auth, ProviderAuthMode::None);
        assert_eq!(model.api.tool_call, Some(true));
        assert_eq!(model.api.stream_usage, Some(false));
        assert_eq!(model.limit.context, 65_536);
    }

    use super::*;

    #[test]
    fn generation_metadata_forwards_reasoning_capability_without_effort_compatibility() {
        let providers = parse_models(
            r#"{"openai":{"id":"openai","name":"OpenAI","models":{
            "future-reasoner":{"id":"future-reasoner","name":"Future", "reasoning":true, "release_date":"2026-01-01", "limit":{"context":128000,"output":4096}},
            "future-chat":{"id":"future-chat","name":"Chat", "reasoning":false, "release_date":"2026-01-01", "limit":{"context":128000,"output":4096}}
        }}}"#,
        )
        .unwrap();
        for (id, capability) in [("future-reasoner", true), ("future-chat", false)] {
            let model = UserModel {
                provider_id: "openai".into(),
                model_id: id.into(),
                connection_id: None,
                variant: None,
            };
            let metadata = metadata_for_auth(&providers, &model, false);
            let api = metadata.api.unwrap();
            assert_eq!(api.reasoning, Some(capability));
            assert_eq!(api.reasoning_effort, None);
        }
    }

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
            connection_id: None,
            variant: None,
        };

        let metadata = metadata_for_auth(&providers, &model, false);

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
    fn codex_account_maximum_is_default_and_explicit_config_only_lowers_it() {
        let mut raw = parse_codex_limit_fixture();
        let openai = raw.iter_mut().find(|p| p.id == "openai").unwrap();
        let mut sol = openai.models["gpt-5.6-sol"].clone();
        sol.id = "sol-alias".into();
        sol.api.id = "gpt-6.1-sol".into();
        openai.models.insert("sol-alias".into(), sol);
        let access = OpenAiModelAccess::Codex(BTreeMap::from([(
            "gpt-6.1-sol".into(),
            CodexModelMetadata {
                context_window: Some(272_000),
                max_context_window: Some(872_000),
                effective_context_window_percent: Some(95),
            },
        )]));
        for (requested, expected) in [
            (None, 872_000),
            (Some(800_000), 800_000),
            (Some(1_050_000), 872_000),
            (Some(128_000), 128_000),
        ] {
            let mut configured = BTreeMap::new();
            let mut providers = raw.clone();
            if let Some(context) = requested {
                // Exercise the actual config parser and apply_model_config path.
                let config: ProviderConfig = serde_json::from_value(serde_json::json!({
                    "models": {"sol-alias": {"id": "gpt-6.1-sol",
                        "limit": {"context": context, "input": if context == 128_000 { Some(100_000) } else { None }, "output": 4096}}}
                }))
                .unwrap();
                let model_config = &config.models["sol-alias"];
                let model = providers
                    .iter_mut()
                    .find(|p| p.id == "openai")
                    .unwrap()
                    .models
                    .get_mut("sol-alias")
                    .unwrap();
                apply_model_config(
                    model,
                    "openai",
                    "sol-alias",
                    "",
                    "@ai-sdk/openai",
                    ProviderAuthMode::Required,
                    true,
                    true,
                    model_config,
                );
                configured.insert("openai".into(), config);
            }
            let effective =
                effective_catalog_with_config(&providers, &access, &configured, true);
            let openai = effective.iter().find(|p| p.id == "openai").unwrap();
            assert_eq!(
                openai.models.len(),
                1,
                "wire ID must gate alias availability"
            );
            let selected = &openai.models["sol-alias"];
            assert_eq!(selected.limit.context, expected);
            assert_eq!(
                selected.limit.input,
                Some(if requested == Some(128_000) {
                    100_000
                } else {
                    (expected as u128 * 95 / 100) as u64
                })
            );
            assert_eq!(
                selected.limit.output,
                if requested.is_some() { 4096 } else { 128_000 }
            );
            let user = UserModel {
                provider_id: "openai".into(),
                model_id: "sol-alias".into(),
                connection_id: Some("selected-account".into()),
                variant: None,
            };
            let generation = generation_metadata(&effective, &user);
            assert_eq!(generation.api.unwrap().id, "gpt-6.1-sol");
            let wire_user = UserModel {
                model_id: "gpt-6.1-sol".into(),
                ..user.clone()
            };
            assert_eq!(
                generation_metadata(&effective, &wire_user)
                    .limit
                    .unwrap()
                    .context,
                expected
            );

            let limit = generation.limit.unwrap();
            assert_eq!(
                limit.context, expected,
                "generation must not double-clamp effective catalog"
            );
            assert_eq!(limit.input, selected.limit.input);
            let threshold = neoism_agent_core::CompactionConfig::default()
                .threshold(limit.context, limit.context);
            assert_eq!(threshold, (expected as f64 * 0.65) as u64);
        }
        let api = effective_catalog_with_config(
            &raw,
            &OpenAiModelAccess::Api,
            &BTreeMap::new(),
            true,
        );
        assert_eq!(
            api.iter().find(|p| p.id == "openai").unwrap().models["sol-alias"]
                .limit
                .context,
            1_050_000
        );
        assert_eq!(
            raw.iter().find(|p| p.id == "openai").unwrap().models["sol-alias"]
                .limit
                .context,
            1_050_000,
            "account resolution must not mutate the shared API cache"
        );
    }

    #[test]
    fn codex_account_model_maxima_agree_in_picker_generation_and_compaction() {
        let raw = parse_codex_limit_fixture();
        for (sol_max, terra_max, terra_context) in
            [(872_000, Some(400_000), 200_000), (512_000, None, 128_000)]
        {
            let access = OpenAiModelAccess::Codex(BTreeMap::from([
                (
                    "gpt-5.6-sol".into(),
                    CodexModelMetadata {
                        context_window: Some(272_000),
                        max_context_window: Some(sol_max),
                        effective_context_window_percent: Some(95),
                    },
                ),
                (
                    "gpt-5.6-terra".into(),
                    CodexModelMetadata {
                        context_window: Some(terra_context),
                        max_context_window: terra_max,
                        effective_context_window_percent: Some(95),
                    },
                ),
            ]));
            let picker = effective_provider_catalog(&raw, &access);
            let generation =
                effective_catalog_with_config(&raw, &access, &BTreeMap::new(), false);
            for (id, expected) in [
                ("gpt-5.6-sol", sol_max),
                ("gpt-5.6-terra", terra_max.unwrap_or(terra_context)),
            ] {
                let selected =
                    &picker.iter().find(|p| p.id == "openai").unwrap().models[id];
                let model = UserModel {
                    provider_id: "openai".into(),
                    model_id: id.into(),
                    connection_id: Some("selected-account".into()),
                    variant: None,
                };
                let limit = generation_metadata(&generation, &model).limit.unwrap();
                assert_eq!(selected.limit.context, expected);
                assert_eq!(limit.context, expected);
                assert_eq!(limit.input, Some((expected as u128 * 95 / 100) as u64));
                assert_eq!(selected.limit.input, limit.input);
                assert_eq!(
                    neoism_agent_core::CompactionConfig::default().threshold(
                        limit.context,
                        limit.input.unwrap().saturating_sub(20_000)
                    ),
                    (expected as f64 * 0.65) as u64
                );
            }
        }
    }

    #[test]
    fn codex_uses_each_models_own_maximum_then_context_then_fallback() {
        for (metadata, expected) in [
            (
                CodexModelMetadata {
                    context_window: Some(272_000),
                    max_context_window: Some(872_000),
                    effective_context_window_percent: Some(95),
                },
                872_000,
            ),
            (
                CodexModelMetadata {
                    context_window: Some(200_000),
                    max_context_window: Some(400_000),
                    effective_context_window_percent: None,
                },
                400_000,
            ),
            (
                CodexModelMetadata {
                    context_window: Some(128_000),
                    max_context_window: None,
                    effective_context_window_percent: None,
                },
                128_000,
            ),
            (
                CodexModelMetadata {
                    context_window: Some(128_000),
                    max_context_window: Some(0),
                    effective_context_window_percent: None,
                },
                128_000,
            ),
            (CodexModelMetadata::default(), 272_000),
        ] {
            for api_context in [64_000, 1_050_000] {
                let mut limit = ModelLimit {
                    context: api_context,
                    input: Some(32_000),
                    output: 4096,
                };
                apply_codex_limit(&mut limit, Some(&metadata), None);
                assert_eq!(
                    limit.context, expected,
                    "API context is not a Codex authority"
                );
                assert_eq!(
                    limit.input,
                    Some((expected as u128 * 95 / 100) as u64),
                    "input comes from the upstream allowance, not API input"
                );
                assert_eq!(limit.output, 4096);
            }
        }
    }

    #[test]
    fn codex_catalog_mode_cannot_bypass_lower_base_model_limits() {
        let mut raw = parse_codex_limit_fixture();
        let openai = raw.iter_mut().find(|p| p.id == "openai").unwrap();
        let mut high = openai.models["gpt-5.6-sol"].clone();
        high.id = "gpt-5.6-sol-high".into();
        openai.models.insert(high.id.clone(), high);
        let configured = BTreeMap::from([(
            "openai".into(),
            serde_json::from_value(serde_json::json!({"models": {"gpt-5.6-sol": {
                "limit": {"context":800000,"input":600000,"output":4096}
            }}}))
            .unwrap(),
        )]);
        let access = OpenAiModelAccess::Codex(BTreeMap::from([(
            "gpt-5.6-sol".into(),
            CodexModelMetadata {
                context_window: Some(272_000),
                max_context_window: Some(872_000),
                effective_context_window_percent: Some(95),
            },
        )]));
        let resolved = effective_catalog_with_config(&raw, &access, &configured, true);
        let model = UserModel {
            provider_id: "openai".into(),
            model_id: "gpt-5.6-sol".into(),
            connection_id: None,
            variant: Some("high".into()),
        };
        let limit = generation_metadata(&resolved, &model).limit.unwrap();
        assert_eq!(
            (limit.context, limit.input, limit.output),
            (800_000, Some(600_000), 4096)
        );
    }

    #[test]
    fn codex_percent_bounds_and_large_context_arithmetic_are_safe() {
        for (percent, effective_percent) in [
            (None, 95),
            (Some(0), 95),
            (Some(101), 95),
            (Some(u64::MAX), 95),
            (Some(1), 1),
            (Some(95), 95),
            (Some(100), 100),
        ] {
            for context in [101, 272_000, 872_000, u64::MAX] {
                let mut limit = ModelLimit {
                    context: 1_050_000,
                    input: None,
                    output: 4096,
                };
                apply_codex_limit(
                    &mut limit,
                    Some(&CodexModelMetadata {
                        context_window: Some(272_000),
                        max_context_window: Some(context),
                        effective_context_window_percent: percent,
                    }),
                    None,
                );
                assert_eq!(
                    limit.context, context,
                    "even max < default remains authoritative"
                );
                assert_eq!(
                    limit.input,
                    Some((context as u128 * effective_percent as u128 / 100) as u64)
                );
                assert!(limit.input.unwrap() <= context);
            }
        }
    }

    #[test]
    fn codex_percent_applies_to_lower_context_but_not_twice_to_lower_input() {
        let metadata = CodexModelMetadata {
            context_window: Some(272_000),
            max_context_window: Some(872_000),
            effective_context_window_percent: Some(95),
        };
        for (context, input, expected_context, expected_input) in [
            (800_000, None, 800_000, 760_000),
            (800_000, Some(600_000), 800_000, 600_000),
            (800_000, Some(790_000), 800_000, 760_000),
            (1_050_000, Some(900_000), 872_000, 828_400),
            (128_000, Some(100_000), 128_000, 100_000),
        ] {
            let explicit = ModelLimit {
                context,
                input,
                output: 4096,
            };
            let mut limit = explicit.clone();
            apply_codex_limit(&mut limit, Some(&metadata), Some(&explicit));
            assert_eq!(
                (limit.context, limit.input, limit.output),
                (expected_context, Some(expected_input), 4096)
            );
        }
    }

    #[test]
    fn codex_invalid_default_or_maximum_falls_back_field_by_field() {
        for (json, expected_context) in [
            (
                serde_json::json!({"context_window":272000,"max_context_window":128000}),
                128_000,
            ),
            (
                serde_json::json!({"context_window":0,"max_context_window":872000}),
                872_000,
            ),
            (
                serde_json::json!({"context_window":"bad","max_context_window":872000}),
                872_000,
            ),
            (
                serde_json::json!({"context_window":128000,"max_context_window":0}),
                128_000,
            ),
            (
                serde_json::json!({"context_window":128000,"max_context_window":-1}),
                128_000,
            ),
            (
                serde_json::json!({"context_window":128000,"max_context_window":"bad"}),
                128_000,
            ),
            (
                serde_json::json!({"context_window":0,"max_context_window":0}),
                272_000,
            ),
            (
                serde_json::json!({"context_window":{},"max_context_window":null}),
                272_000,
            ),
        ] {
            let metadata: CodexModelMetadata = serde_json::from_value(json).unwrap();
            let mut limit = ModelLimit {
                context: 1_050_000,
                input: Some(922_000),
                output: 0,
            };
            apply_codex_limit(&mut limit, Some(&metadata), None);
            assert_eq!(limit.context, expected_context);
            assert_eq!(limit.input, Some(expected_context * 95 / 100));
            assert_eq!(limit.output, 0, "no output metadata is invented");
        }
    }

    #[test]
    fn codex_missing_metadata_cannot_authorize_larger_override_or_invent_output() {
        let explicit = ModelLimit {
            context: 800_000,
            input: None,
            output: 0,
        };
        for metadata in [
            None,
            Some(CodexModelMetadata::default()),
            Some(CodexModelMetadata {
                context_window: None,
                max_context_window: Some(872_000),
                effective_context_window_percent: Some(200),
            }),
        ] {
            let mut limit = explicit.clone();
            apply_codex_limit(&mut limit, metadata.as_ref(), None);
            assert_eq!(
                limit.context,
                if metadata
                    .as_ref()
                    .and_then(|m| m.max_context_window)
                    .is_some()
                {
                    872_000
                } else {
                    272_000
                }
            );
            assert_eq!(
                limit.input,
                Some((limit.context as u128 * 95 / 100) as u64),
                "fallback percentage remains conservative"
            );
            assert_eq!(limit.output, 0, "unknown output must remain unknown");
            let mut limit = explicit.clone();
            apply_codex_limit(&mut limit, metadata.as_ref(), Some(&explicit));
            assert_eq!(
                limit.context,
                if metadata
                    .as_ref()
                    .and_then(|m| m.max_context_window)
                    .is_some()
                {
                    800_000
                } else {
                    272_000
                }
            );
        }
        // Smaller explicit user limits are never enlarged by account metadata.
        let mut small = ModelLimit {
            context: 64_000,
            input: Some(32_000),
            output: 2048,
        };
        apply_codex_limit(
            &mut small,
            Some(&CodexModelMetadata {
                context_window: Some(272_000),
                max_context_window: Some(872_000),
                effective_context_window_percent: Some(95),
            }),
            Some(&ModelLimit {
                context: 64_000,
                input: Some(32_000),
                output: 2048,
            }),
        );
        assert_eq!(
            (small.context, small.input, small.output),
            (64_000, Some(32_000), 2048)
        );
    }

    #[test]
    fn generation_metadata_uses_conservative_codex_limits_for_openai_oauth_models() {
        let providers = parse_codex_limit_fixture();
        let model = UserModel {
            provider_id: "openai".to_string(),
            model_id: "gpt-5.5".to_string(),
            connection_id: None,
            variant: None,
        };

        let metadata = metadata_for_auth(&providers, &model, true);
        let limit = metadata.limit.expect("limit");
        let cost = metadata.cost.expect("cost");

        assert_eq!(limit.context, 272_000);
        assert_eq!(limit.input, Some(258_400));
        assert_eq!(limit.output, 128_000);
        assert_eq!(cost.input, 0.0);
        assert_eq!(cost.output, 0.0);
        assert_eq!(cost.cache.read, 0.0);
        assert_eq!(cost.cache.write, 0.0);
    }

    #[test]
    fn effective_provider_catalog_uses_conservative_codex_limits_for_ui_models() {
        let fixture = parse_codex_limit_fixture();
        let providers = effective_provider_catalog(&fixture, &codex_access(&fixture));
        let openai = providers
            .iter()
            .find(|provider| provider.id == "openai")
            .expect("openai provider");
        let model = openai.models.get("gpt-5.5").expect("gpt-5.5 model");

        assert_eq!(model.limit.context, 272_000);
        assert_eq!(model.limit.input, Some(258_400));
        assert_eq!(model.limit.output, 128_000);
        assert_eq!(model.cost.input, 0.0);
        assert_eq!(model.cost.output, 0.0);
    }

    #[test]
    fn effective_provider_catalog_treats_gpt_5_6_family_as_codex_subscription_models() {
        let fixture = parse_codex_limit_fixture();
        let providers = effective_provider_catalog(&fixture, &codex_access(&fixture));
        let openai = providers
            .iter()
            .find(|provider| provider.id == "openai")
            .expect("openai provider");

        for model_id in ["gpt-5.6", "gpt-5.6-luna", "gpt-5.6-terra", "gpt-5.6-sol"] {
            let model = openai.models.get(model_id).expect("gpt-5.6 family model");
            assert_eq!(model.limit.context, 272_000, "{model_id}");
            assert_eq!(model.limit.input, Some(258_400), "{model_id}");
            assert_eq!(model.limit.output, 128_000, "{model_id}");
            assert_eq!(model.cost.input, 0.0, "{model_id}");
            assert_eq!(model.cost.output, 0.0, "{model_id}");
        }
    }

    #[test]
    fn sol_and_astra_limits_and_compaction_follow_subscription_auth() {
        let mut providers = parse_codex_limit_fixture();
        let openai = providers
            .iter_mut()
            .find(|provider| provider.id == "openai")
            .unwrap();
        let future_ids = [
            "gpt-6",
            "gpt-6-astra",
            "gpt-6-sol",
            "gpt-6-luna",
            "gpt-6.1-sol",
            "openai/gpt-6.1-sol",
            "gpt-42.7-next",
            "future-reasoner",
        ];
        for id in future_ids {
            let mut model = openai.models["gpt-5.6-sol"].clone();
            model.id = id.into();
            model.api.id = id.into();
            model.name = id.into();
            openai.models.insert(id.into(), model);
        }
        for id in std::iter::once("gpt-5.6-sol").chain(future_ids) {
            for oauth in [true, false] {
                let model = UserModel {
                    provider_id: "openai".into(),
                    model_id: id.into(),
                    connection_id: None,
                    variant: None,
                };
                let metadata = metadata_for_auth(&providers, &model, oauth);
                let limit = metadata.limit.unwrap();
                let access = if oauth {
                    codex_access(&providers)
                } else {
                    OpenAiModelAccess::Api
                };
                let visible = effective_provider_catalog(&providers, &access);
                let visible = &visible
                    .iter()
                    .find(|provider| provider.id == "openai")
                    .unwrap()
                    .models[id];
                assert_eq!(visible.limit.context, limit.context, "{id}, oauth={oauth}");
                assert_eq!(limit.context, if oauth { 272_000 } else { 1_050_000 });
                assert_eq!(limit.input, Some(if oauth { 258_400 } else { 922_000 }));
                assert_eq!(metadata.cost.unwrap().input == 0.0, oauth);
                let trigger = neoism_agent_core::CompactionConfig::default().threshold(
                    limit.context,
                    limit.input.unwrap().saturating_sub(20_000),
                );
                // Default 65% compaction follows Codex's 272k context,
                // never the API's million-token context.
                assert_eq!(trigger, if oauth { 176_800 } else { 682_500 });
            }
        }
    }

    #[test]
    fn codex_service_ceilings_never_increase_smaller_model_limits() {
        for input in [None, Some(96_000)] {
            let mut limit = ModelLimit {
                context: 128_000,
                input,
                output: 4_096,
            };
            let explicit = limit.clone();
            apply_codex_limit(&mut limit, None, Some(&explicit));
            assert_eq!(limit.context, 128_000);
            assert_eq!(limit.input, Some(input.unwrap_or(121_600)));
            assert_eq!(limit.output, 4_096);
        }
    }

    #[test]
    fn effective_provider_catalog_keeps_api_limits_without_codex_oauth() {
        let providers = effective_provider_catalog(
            &parse_codex_limit_fixture(),
            &OpenAiModelAccess::Api,
        );
        let openai = providers
            .iter()
            .find(|provider| provider.id == "openai")
            .expect("openai provider");
        let model = openai.models.get("gpt-5.6-sol").expect("gpt-5.6-sol model");

        // API-key requests ride the platform Responses API, which honors the
        // full advertised window and bills per token — no codex clamp.
        assert_eq!(model.limit.context, 1_050_000);
        assert_eq!(model.limit.input, Some(922_000));
        assert_eq!(model.limit.output, 128_000);
        assert_eq!(model.cost.input, 5.0);
        assert_eq!(model.cost.output, 30.0);
    }

    #[test]
    fn usable_provider_catalog_requires_opencode_connection() {
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
            &OpenAiModelAccess::Api,
        );

        assert!(usable.iter().all(|provider| provider.id != "opencode"));
        let openai = usable
            .iter()
            .find(|provider| provider.id == "openai")
            .expect("connected openai provider");
        assert!(openai.models.contains_key("gpt"));
        assert!(usable.iter().all(|provider| provider.id != "anthropic"));
        assert!(usable.iter().all(|provider| provider.id != "google"));

        let connected = usable_provider_catalog(
            &providers,
            &["opencode".to_string()],
            &OpenAiModelAccess::Api,
        );
        let opencode = connected
            .iter()
            .find(|provider| provider.id == "opencode")
            .expect("connected opencode provider");
        assert_eq!(
            opencode.models.keys().cloned().collect::<Vec<_>>(),
            vec!["free".to_string(), "paid".to_string()]
        );
    }

    #[test]
    fn codex_catalog_is_restricted_to_account_model_ids() {
        let providers = parse_codex_limit_fixture();
        let visible = effective_provider_catalog(
            &providers,
            &OpenAiModelAccess::Codex(BTreeMap::from([(
                "gpt-5.6-sol".to_string(),
                CodexModelMetadata::default(),
            )])),
        );
        let openai = visible
            .iter()
            .find(|provider| provider.id == "openai")
            .expect("openai provider");

        assert_eq!(
            openai.models.keys().cloned().collect::<Vec<_>>(),
            vec!["gpt-5.6-sol".to_string()]
        );
        assert_eq!(openai.models["gpt-5.6-sol"].cost.input, 0.0);
    }

    #[test]
    fn connect_catalog_keeps_disconnected_opencode_without_models() {
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
                  }
                }
              }
            }"#,
        )
        .unwrap();

        let disconnected =
            connect_provider_catalog(&providers, &[], &OpenAiModelAccess::Api);
        let opencode = disconnected
            .iter()
            .find(|provider| provider.id == "opencode")
            .expect("opencode remains connectable");
        assert!(opencode.models.is_empty());
        assert!(!default_model_ids(&disconnected).contains_key("opencode"));

        let connected = connect_provider_catalog(
            &providers,
            &["opencode".to_string()],
            &OpenAiModelAccess::Api,
        );
        assert!(connected
            .iter()
            .find(|provider| provider.id == "opencode")
            .is_some_and(|provider| provider.models.contains_key("free")));
    }

    fn codex_access(providers: &[ProviderInfo]) -> OpenAiModelAccess {
        let model_ids = providers
            .iter()
            .find(|provider| provider.id == "openai")
            .into_iter()
            .flat_map(|provider| {
                provider
                    .models
                    .keys()
                    .cloned()
                    .map(|id| (id, CodexModelMetadata::default()))
            })
            .collect();
        OpenAiModelAccess::Codex(model_ids)
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
                  },
                  "gpt-5.6": {
                    "id": "gpt-5.6",
                    "name": "GPT-5.6",
                    "release_date": "2026-07-01",
                    "limit": { "context": 1050000, "input": 922000, "output": 128000 },
                    "cost": { "input": 5.0, "output": 30.0, "cache_read": 0.5, "cache_write": 6.25 }
                  },
                  "gpt-5.6-luna": {
                    "id": "gpt-5.6-luna",
                    "name": "GPT-5.6 Luna",
                    "release_date": "2026-07-01",
                    "limit": { "context": 1050000, "input": 922000, "output": 128000 },
                    "cost": { "input": 1.0, "output": 6.0, "cache_read": 0.1, "cache_write": 1.25 }
                  },
                  "gpt-5.6-terra": {
                    "id": "gpt-5.6-terra",
                    "name": "GPT-5.6 Terra",
                    "release_date": "2026-07-01",
                    "limit": { "context": 1050000, "input": 922000, "output": 128000 },
                    "cost": { "input": 2.5, "output": 15.0, "cache_read": 0.25, "cache_write": 3.125 }
                  },
                  "gpt-5.6-sol": {
                    "id": "gpt-5.6-sol",
                    "name": "GPT-5.6 Sol",
                    "release_date": "2026-07-01",
                    "limit": { "context": 1050000, "input": 922000, "output": 128000 },
                    "cost": { "input": 5.0, "output": 30.0, "cache_read": 0.5, "cache_write": 6.25 }
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
                  },
                  "gpt-5.6-sol": {
                    "id": "gpt-5.6-sol",
                    "name": "GPT-5.6 Sol",
                    "release_date": "2026-07-01",
                    "limit": { "context": 1050000, "input": 922000, "output": 128000 },
                    "cost": { "input": 5.0, "output": 30.0, "cache_read": 0.5 }
                  }
                }
              }
            }"#,
        )
        .unwrap()
    }
}
