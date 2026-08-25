use std::pin::Pin;

use futures_core::Stream;
use neoism_agent_core::{
    AuthInfo, ProviderApiInfo, ProviderGenerationRequest, ProviderInfo, ProviderStreamEvent,
};

use crate::auth_store::AuthStore;

#[path = "provider_anthropic.rs"]
mod provider_anthropic;
#[path = "provider_chat_completion.rs"]
mod provider_chat_completion;
#[path = "provider_openai.rs"]
mod provider_openai;
#[path = "provider_openai_stream.rs"]
mod provider_openai_stream;
#[path = "provider_stub.rs"]
mod provider_stub;
use provider_anthropic::{AnthropicClient, AnthropicRuntime};
pub(crate) use provider_chat_completion::reasoning_effort;
use provider_openai::{OpenAiClient, OpenAiRuntime};
pub(crate) use provider_openai_stream::estimate_tokens;
use provider_stub::StubRuntime;

pub(crate) type ProviderEventStream =
    Pin<Box<dyn Stream<Item = anyhow::Result<ProviderStreamEvent>> + Send>>;

pub(crate) trait ProviderRuntime: Send + Sync {
    fn stream(&self, request: ProviderGenerationRequest) -> ProviderEventStream;
}

pub(crate) struct ProviderStream {
    pub(crate) provider_id: String,
    pub(crate) model_id: String,
    pub(crate) events: ProviderEventStream,
}

#[derive(Clone)]
pub(crate) struct ProviderRegistry {
    default_provider: Option<String>,
    auth_store: AuthStore,
    openai: OpenAiClient,
}

impl ProviderRegistry {
    pub(crate) fn from_env(auth_store: AuthStore) -> Self {
        Self {
            default_provider: std::env::var("NEOISM_AGENT_PROVIDER").ok(),
            auth_store,
            openai: OpenAiClient::from_env(),
        }
    }

    pub(crate) fn connected_ids(
        &self,
        providers: &[ProviderInfo],
    ) -> anyhow::Result<Vec<String>> {
        let mut connected = self.auth_store.all()?.keys().cloned().collect::<Vec<_>>();
        for provider in providers {
            if provider.env.iter().any(|key| std::env::var(key).is_ok()) {
                connected.push(provider.id.clone());
            }
        }
        if std::env::var("NEOISM_AGENT_OPENAI_API_KEY").is_ok()
            || std::env::var("OPENAI_API_KEY").is_ok()
        {
            connected.push("openai".to_string());
        }
        connected.sort();
        connected.dedup();
        Ok(connected)
    }

    pub(crate) fn stream(
        &self,
        mut request: ProviderGenerationRequest,
    ) -> anyhow::Result<ProviderStream> {
        let requested_provider = request.provider_id.clone();
        let provider_id = if request.provider_id == "neoism" && request.model_id != "stub"
        {
            self.default_provider
                .clone()
                .unwrap_or_else(|| request.provider_id.clone())
        } else {
            request.provider_id.clone()
        };
        if provider_id == "openai" {
            let auth = self.auth_store.get("openai")?;
            request.provider_id = "openai".to_string();
            if request.model_id == "stub" {
                request.model_id = std::env::var("NEOISM_AGENT_OPENAI_MODEL")
                    .or_else(|_| std::env::var("OPENAI_MODEL"))
                    .unwrap_or_else(|_| "gpt-4.1-mini".to_string());
            }
            let model_id = request.model_id.clone();
            let runtime = OpenAiRuntime {
                client: self.openai.clone(),
                auth,
                auth_store: self.auth_store.clone(),
                use_oauth_responses: true,
                allow_openai_env_fallback: true,
            };
            return Ok(ProviderStream {
                provider_id: "openai".to_string(),
                model_id,
                events: runtime.stream(request),
            });
        }

        if let Some(api) = request.api.clone() {
            if let Some(adapter) = ProviderAdapter::from_api(&api) {
                let auth = self.provider_auth(&provider_id, &request.auth_env)?;
                let model_id = request.model_id.clone();
                request.provider_id = provider_id.clone();
                return match adapter.kind {
                    ProviderAdapterKind::OpenAiCompatible => {
                        let runtime = OpenAiRuntime {
                            client: OpenAiClient::with_base_url(adapter.base_url),
                            auth,
                            auth_store: self.auth_store.clone(),
                            use_oauth_responses: false,
                            allow_openai_env_fallback: false,
                        };
                        Ok(ProviderStream {
                            provider_id,
                            model_id,
                            events: runtime.stream(request),
                        })
                    }
                    ProviderAdapterKind::Anthropic => {
                        let runtime = AnthropicRuntime {
                            client: AnthropicClient::with_base_url(adapter.base_url),
                            auth,
                        };
                        Ok(ProviderStream {
                            provider_id,
                            model_id,
                            events: runtime.stream(request),
                        })
                    }
                };
            }
            anyhow::bail!(
                "provider {} uses unsupported adapter {} for model {}",
                provider_id,
                api.npm,
                request.model_id
            );
        }

        if requested_provider != "neoism" {
            anyhow::bail!("provider {requested_provider} is not configured")
        }

        let model_id = request.model_id.clone();
        Ok(ProviderStream {
            provider_id: "neoism".to_string(),
            model_id,
            events: StubRuntime.stream(request),
        })
    }

    fn provider_auth(
        &self,
        provider_id: &str,
        env_keys: &[String],
    ) -> anyhow::Result<Option<AuthInfo>> {
        if let Some(auth) = self.auth_store.get(provider_id)? {
            return Ok(Some(auth));
        }
        for key in env_keys {
            if let Ok(value) = std::env::var(key) {
                if !value.trim().is_empty() {
                    return Ok(Some(AuthInfo::Api {
                        key: value,
                        metadata: None,
                    }));
                }
            }
        }
        if provider_id == "opencode" {
            return Ok(Some(AuthInfo::Api {
                key: "public".to_string(),
                metadata: None,
            }));
        }
        Ok(None)
    }
}

struct ProviderAdapter {
    base_url: String,
    kind: ProviderAdapterKind,
}

enum ProviderAdapterKind {
    OpenAiCompatible,
    Anthropic,
}

impl ProviderAdapter {
    fn from_api(api: &ProviderApiInfo) -> Option<Self> {
        let base_url = api.url.trim_end_matches('/').to_string();
        if base_url.is_empty() {
            return None;
        }
        let kind = if is_openai_compatible_npm(&api.npm) {
            ProviderAdapterKind::OpenAiCompatible
        } else if is_anthropic_npm(&api.npm) {
            ProviderAdapterKind::Anthropic
        } else {
            return None;
        };
        Some(Self { base_url, kind })
    }
}

pub(crate) fn provider_api_supported(api: &ProviderApiInfo) -> bool {
    ProviderAdapter::from_api(api).is_some()
}

fn is_openai_compatible_npm(npm: &str) -> bool {
    matches!(
        npm,
        "@ai-sdk/openai-compatible"
            | "@openrouter/ai-sdk-provider"
            | "@llmgateway/ai-sdk-provider"
            | "@ai-sdk/openai"
            | "@ai-sdk/xai"
            | "@ai-sdk/azure"
            | "@ai-sdk/mistral"
            | "@ai-sdk/groq"
            | "@ai-sdk/deepinfra"
            | "@ai-sdk/cerebras"
            | "@ai-sdk/togetherai"
            | "@ai-sdk/perplexity"
            | "@ai-sdk/vercel"
            | "@ai-sdk/gateway"
            | "@ai-sdk/alibaba"
            | "venice-ai-sdk-provider"
    )
}

fn is_anthropic_npm(npm: &str) -> bool {
    matches!(npm, "@ai-sdk/anthropic")
}

#[cfg(test)]
#[path = "provider_tests.rs"]
mod tests;
