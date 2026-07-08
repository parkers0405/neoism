use std::collections::BTreeMap;

use axum::extract::{Path, State};
use axum::Json;
use neoism_agent_core::{
    AuthInfo, ConfigProvidersResult, ProviderAuthAuthorization, ProviderAuthMethod,
    ProviderListResult,
};
use serde::Deserialize;
use serde_json::Value;

use crate::error::ApiError;
use crate::provider_auth;
use crate::provider_catalog::{
    default_model_ids, effective_provider_catalog, provider_connectable,
    usable_provider_catalog,
};
use crate::state::AppState;

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ProviderAuthorizeRequest {
    pub method: Value,
    #[serde(default)]
    pub inputs: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ProviderCallbackRequest {
    pub method: Value,
    pub code: Option<String>,
}

pub(crate) async fn provider_list(
    State(state): State<AppState>,
) -> Result<Json<ProviderListResult>, ApiError> {
    let providers = state.inner.provider_catalog.providers().await?;
    let connected = state.inner.providers.connected_ids(&providers)?;
    let mut providers = effective_provider_catalog(&providers);
    // Only surface providers neoism can actually stream through — there's no
    // point offering to connect one (e.g. Gemini/`google`, `amazon-bedrock`)
    // that could never appear in `/model`.
    providers.retain(provider_connectable);
    Ok(Json(ProviderListResult {
        default: default_model_ids(&providers),
        connected,
        all: providers,
    }))
}

pub(crate) async fn config_providers(
    State(state): State<AppState>,
) -> Result<Json<ConfigProvidersResult>, ApiError> {
    let raw_providers = state.inner.provider_catalog.providers().await?;
    let connected = state.inner.providers.connected_ids(&raw_providers)?;
    let providers = usable_provider_catalog(&raw_providers, &connected);
    Ok(Json(ConfigProvidersResult {
        default: default_model_ids(&providers),
        providers,
    }))
}

pub(crate) async fn provider_auth_methods(
    State(state): State<AppState>,
) -> Result<Json<BTreeMap<String, Vec<ProviderAuthMethod>>>, ApiError> {
    let providers = state.inner.provider_catalog.providers().await?;
    Ok(Json(provider_auth::methods(&providers)))
}

pub(crate) async fn auth_get(
    State(state): State<AppState>,
    Path(provider_id): Path<String>,
) -> Result<Json<Option<AuthInfo>>, ApiError> {
    Ok(Json(state.inner.auth_store.get(&provider_id)?))
}

pub(crate) async fn auth_set(
    State(state): State<AppState>,
    Path(provider_id): Path<String>,
    Json(info): Json<AuthInfo>,
) -> Result<Json<bool>, ApiError> {
    state.inner.auth_store.set(&provider_id, info)?;
    Ok(Json(true))
}

pub(crate) async fn auth_remove(
    State(state): State<AppState>,
    Path(provider_id): Path<String>,
) -> Result<Json<bool>, ApiError> {
    state.inner.auth_store.remove(&provider_id)?;
    Ok(Json(true))
}

pub(crate) async fn provider_oauth_authorize(
    State(state): State<AppState>,
    Path(provider_id): Path<String>,
    Json(request): Json<ProviderAuthorizeRequest>,
) -> Result<Json<Option<ProviderAuthAuthorization>>, ApiError> {
    let providers = state.inner.provider_catalog.providers().await?;
    Ok(Json(
        provider_auth::authorize(
            &provider_id,
            &request.method,
            &request.inputs,
            &providers,
            &state.inner.auth_store,
            &state.inner.provider_oauth,
        )
        .await
        .map_err(|error| ApiError::bad_request(error.to_string()))?,
    ))
}

pub(crate) async fn provider_oauth_callback(
    State(state): State<AppState>,
    Path(provider_id): Path<String>,
    Json(request): Json<ProviderCallbackRequest>,
) -> Result<Json<bool>, ApiError> {
    let providers = state.inner.provider_catalog.providers().await?;
    provider_auth::callback(
        &provider_id,
        &request.method,
        request.code.as_deref(),
        &providers,
        &state.inner.auth_store,
        &state.inner.provider_oauth,
    )
    .await
    .map_err(|error| ApiError::bad_request(error.to_string()))?;
    Ok(Json(true))
}
