//! Semantic transcript search: a background indexer embeds message text via
//! an OpenAI-compatible `/embeddings` endpoint into the `message_embeddings`
//! vector table (turso backend only), and `semantic_search` ranks messages by
//! cosine distance to an embedded query. Indexing is fully out of the prompt
//! path: nothing here ever blocks message persistence or streaming.

use std::time::Duration;

use anyhow::Context;
use axum::extract::{Query, State};
use axum::Json;

use crate::auth_store::AuthStore;
use crate::error::ApiError;
use crate::state::{AppState, SemanticSearchHit};
use neoism_agent_core::AuthInfo;

const DEFAULT_MODEL: &str = "openai/text-embedding-3-small";
/// Keep embedding inputs comfortably under model token limits.
const MAX_INPUT_CHARS: usize = 8_000;
const BATCH_SIZE: usize = 32;
const IDLE_POLL: Duration = Duration::from_secs(5);
const ERROR_BACKOFF: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub(crate) struct EmbeddingsClient {
    base_url: String,
    api_key: String,
    /// Model id as sent to the API (without the provider prefix).
    model_id: String,
    /// Full `provider/model` spec; stored with each embedding so a model
    /// switch re-indexes instead of comparing incompatible vectors.
    pub(crate) model_spec: String,
    http: reqwest::Client,
}

impl EmbeddingsClient {
    /// Resolve the embeddings provider from env + the auth store. Returns
    /// `None` (semantic search disabled) when embeddings are turned off or no
    /// API key is available for the configured provider.
    pub(crate) fn from_env(auth: &AuthStore) -> Option<Self> {
        if std::env::var_os("NEOISM_AGENT_DISABLE_EMBEDDINGS").is_some() {
            return None;
        }
        let spec = std::env::var("NEOISM_AGENT_EMBEDDINGS_MODEL")
            .ok()
            .filter(|spec| !spec.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());
        let (provider_id, model_id) = match spec.split_once('/') {
            Some((provider, model)) if !provider.is_empty() && !model.is_empty() => {
                (provider.to_string(), model.to_string())
            }
            _ => {
                tracing::warn!(
                    spec,
                    "invalid NEOISM_AGENT_EMBEDDINGS_MODEL (expected provider/model); semantic search disabled"
                );
                return None;
            }
        };

        let env_key = std::env::var(format!(
            "{}_API_KEY",
            provider_id.to_ascii_uppercase().replace('-', "_")
        ))
        .ok()
        .filter(|key| !key.trim().is_empty());
        let auth_key = match auth.get(&provider_id) {
            Ok(Some(AuthInfo::Api { key, .. })) => Some(key),
            _ => None,
        };
        let Some(api_key) = env_key.or(auth_key) else {
            tracing::info!(
                provider = provider_id,
                "semantic search disabled: no API key for embeddings provider \
                 (set {}_API_KEY or `neoism-agent auth set-api {} <key>`)",
                provider_id.to_ascii_uppercase(),
                provider_id
            );
            return None;
        };

        let base_url = std::env::var("NEOISM_AGENT_EMBEDDINGS_URL")
            .ok()
            .filter(|url| !url.trim().is_empty())
            .or_else(|| match provider_id.as_str() {
                "openai" => std::env::var("NEOISM_AGENT_OPENAI_BASE_URL")
                    .ok()
                    .filter(|url| !url.trim().is_empty())
                    .or_else(|| Some("https://api.openai.com/v1".to_string())),
                "xai" => Some("https://api.x.ai/v1".to_string()),
                _ => None,
            });
        let Some(base_url) = base_url else {
            tracing::warn!(
                provider = provider_id,
                "semantic search disabled: unknown embeddings provider base URL \
                 (set NEOISM_AGENT_EMBEDDINGS_URL)"
            );
            return None;
        };

        Some(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            model_id,
            model_spec: spec,
            http: reqwest::Client::new(),
        })
    }

    pub(crate) async fn embed(&self, inputs: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        #[derive(serde::Deserialize)]
        struct EmbeddingsResponse {
            data: Vec<EmbeddingsDatum>,
        }
        #[derive(serde::Deserialize)]
        struct EmbeddingsDatum {
            index: usize,
            embedding: Vec<f32>,
        }

        let response = self
            .http
            .post(format!("{}/embeddings", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&serde_json::json!({
                "model": self.model_id,
                "input": inputs,
            }))
            .send()
            .await
            .context("embeddings request failed")?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "embeddings request returned {status}: {}",
                body.chars().take(300).collect::<String>()
            );
        }
        let mut parsed: EmbeddingsResponse = response
            .json()
            .await
            .context("failed to decode embeddings response")?;
        parsed.data.sort_by_key(|datum| datum.index);
        if parsed.data.len() != inputs.len() {
            anyhow::bail!(
                "embeddings response returned {} vectors for {} inputs",
                parsed.data.len(),
                inputs.len()
            );
        }
        Ok(parsed
            .data
            .into_iter()
            .map(|datum| datum.embedding)
            .collect())
    }
}

/// Render a float vector as the JSON-array text `vector32()` accepts.
pub(crate) fn vector_json(vector: &[f32]) -> String {
    let mut out = String::with_capacity(vector.len() * 10 + 2);
    out.push('[');
    for (index, value) in vector.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        // f32 round-trip formatting; NaN/inf would corrupt the literal.
        if value.is_finite() {
            out.push_str(&value.to_string());
        } else {
            out.push('0');
        }
    }
    out.push(']');
    out
}

/// Truncate embedding input on a char boundary.
fn clamp_input(content: &str) -> String {
    content.chars().take(MAX_INPUT_CHARS).collect()
}

#[derive(Debug, serde::Deserialize)]
pub(crate) struct SemanticSearchQuery {
    q: String,
    limit: Option<usize>,
    #[serde(rename = "sessionID")]
    session_id: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct SemanticSearchResponse {
    /// False when the backend has no vector support or no embeddings
    /// provider is configured — clients should quietly fall back.
    available: bool,
    hits: Vec<SemanticSearchHit>,
}

/// `GET /search/semantic?q=…&limit=…&sessionID=…` — embed the query and rank
/// indexed transcript messages by cosine distance.
pub(crate) async fn semantic_search_route(
    State(state): State<AppState>,
    Query(query): Query<SemanticSearchQuery>,
) -> Result<Json<SemanticSearchResponse>, ApiError> {
    let store = &state.inner.store;
    let Some(client) = state.inner.semantic.as_ref() else {
        return Ok(Json(SemanticSearchResponse {
            available: false,
            hits: Vec::new(),
        }));
    };
    if !store.semantic_search_supported() {
        return Ok(Json(SemanticSearchResponse {
            available: false,
            hits: Vec::new(),
        }));
    }
    let needle = query.q.trim();
    if needle.is_empty() {
        return Ok(Json(SemanticSearchResponse {
            available: true,
            hits: Vec::new(),
        }));
    }
    let vectors = client.embed(&[needle.to_string()]).await?;
    let vector = vectors
        .into_iter()
        .next()
        .context("embeddings response was empty")?;
    let hits = store
        .semantic_search(
            &vector_json(&vector),
            &client.model_spec,
            query.session_id.as_deref(),
            query.limit.unwrap_or(20),
        )
        .await?;
    Ok(Json(SemanticSearchResponse {
        available: true,
        hits,
    }))
}

/// Start the background embedding indexer if the backend supports vector
/// search and an embeddings provider is configured. Safe to call always.
pub(crate) fn spawn_indexer(state: AppState) {
    if !state.inner.store.semantic_search_supported() {
        tracing::info!(
            "semantic search unavailable: database backend has no vector support"
        );
        return;
    }
    let Some(client) = state.inner.semantic.clone() else {
        return;
    };
    tokio::spawn(async move {
        tracing::info!(model = client.model_spec, "semantic search indexer started");
        loop {
            match index_batch(&state, &client).await {
                Ok(0) => tokio::time::sleep(IDLE_POLL).await,
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(%error, "semantic indexing batch failed; backing off");
                    tokio::time::sleep(ERROR_BACKOFF).await;
                }
            }
        }
    });
}

/// Embed one batch of not-yet-indexed messages. Returns how many rows were
/// processed (embedded or tombstoned); 0 means fully caught up.
async fn index_batch(
    state: &AppState,
    client: &EmbeddingsClient,
) -> anyhow::Result<usize> {
    let pending = state
        .inner
        .store
        .messages_missing_embeddings(&client.model_spec, BATCH_SIZE)
        .await?;
    if pending.is_empty() {
        return Ok(0);
    }

    let mut to_embed: Vec<(String, String, i64, String)> = Vec::new();
    let mut processed = 0usize;
    for row in pending {
        let Ok(message) = serde_json::from_str::<neoism_agent_core::MessageWithParts>(
            &row.message_json,
        ) else {
            // Undecodable rows get a tombstone so they aren't retried forever.
            state
                .inner
                .store
                .tombstone_message_embedding(
                    &row.message_id,
                    &row.session_id,
                    row.created,
                )
                .await?;
            processed += 1;
            continue;
        };
        let (_, _, content) = crate::state::fts_document(&message);
        if content.trim().is_empty() {
            state
                .inner
                .store
                .tombstone_message_embedding(
                    &row.message_id,
                    &row.session_id,
                    row.created,
                )
                .await?;
            processed += 1;
            continue;
        }
        to_embed.push((
            row.message_id,
            row.session_id,
            row.created,
            clamp_input(&content),
        ));
    }

    if to_embed.is_empty() {
        return Ok(processed);
    }
    let inputs: Vec<String> = to_embed
        .iter()
        .map(|(_, _, _, text)| text.clone())
        .collect();
    let vectors = client.embed(&inputs).await?;
    for ((message_id, session_id, created, _), vector) in to_embed.iter().zip(vectors) {
        state
            .inner
            .store
            .upsert_message_embedding(
                message_id,
                session_id,
                *created,
                &client.model_spec,
                &vector_json(&vector),
            )
            .await?;
        processed += 1;
    }
    Ok(processed)
}
