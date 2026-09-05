use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::ApiError;
use crate::{lsp, resolve_directory, InstanceQuery};

async fn runtime_lsp(
    state: &crate::state::AppState,
    directory: &str,
) -> Result<crate::workspace_runtime::LeasedResource<crate::lsp::LspRuntime>, ApiError> {
    state
        .workspace_runtime(directory)
        .await
        .map_err(ApiError::gone)?
        .lsp()
        .map_err(|error| ApiError::gone(error.to_string()))
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct LspPositionQuery {
    pub directory: Option<String>,
    pub file: String,
    pub line: u32,
    pub character: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct LspDocumentQuery {
    pub directory: Option<String>,
    pub file: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct LspLineRangeQuery {
    pub directory: Option<String>,
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct LspTouchRequest {
    pub directory: Option<String>,
    pub file: String,
    pub text: Option<String>,
}

pub(crate) async fn lsp_status(
    State(state): State<crate::state::AppState>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<lsp::LspStatus>>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    Ok(Json(lsp::status(
        &*runtime_lsp(&state, &directory).await?,
        directory,
    )))
}

pub(crate) async fn lsp_hover(
    State(state): State<crate::state::AppState>,
    Query(query): Query<LspPositionQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<lsp::LspHover>>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    Ok(Json(lsp::hover(
        &*runtime_lsp(&state, &directory).await?,
        directory,
        query.file,
        query.line,
        query.character,
    )))
}

pub(crate) async fn lsp_signature_help(
    State(state): State<crate::state::AppState>,
    Query(query): Query<LspPositionQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<lsp::LspSignatureHelp>>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    Ok(Json(lsp::signature_help(
        &*runtime_lsp(&state, &directory).await?,
        directory,
        query.file,
        query.line,
        query.character,
    )))
}

pub(crate) async fn lsp_inlay_hints(
    State(state): State<crate::state::AppState>,
    Query(query): Query<LspLineRangeQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<lsp::LspInlayHint>>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    Ok(Json(lsp::inlay_hints(
        &*runtime_lsp(&state, &directory).await?,
        directory,
        query.file,
        query.start_line,
        query.end_line,
    )))
}

pub(crate) async fn lsp_document_highlights(
    State(state): State<crate::state::AppState>,
    Query(query): Query<LspPositionQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<lsp::LspDocumentHighlight>>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    Ok(Json(lsp::document_highlights(
        &*runtime_lsp(&state, &directory).await?,
        directory,
        query.file,
        query.line,
        query.character,
    )))
}

pub(crate) async fn lsp_definition(
    State(state): State<crate::state::AppState>,
    Query(query): Query<LspPositionQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<lsp::LspLocation>>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    Ok(Json(lsp::definitions(
        &*runtime_lsp(&state, &directory).await?,
        directory,
        query.file,
        query.line,
        query.character,
    )))
}

pub(crate) async fn lsp_references(
    State(state): State<crate::state::AppState>,
    Query(query): Query<LspPositionQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<lsp::LspLocation>>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    Ok(Json(lsp::references(
        &*runtime_lsp(&state, &directory).await?,
        directory,
        query.file,
        query.line,
        query.character,
    )))
}

pub(crate) async fn lsp_implementation(
    State(state): State<crate::state::AppState>,
    Query(query): Query<LspPositionQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<lsp::LspLocation>>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    Ok(Json(lsp::implementations(
        &*runtime_lsp(&state, &directory).await?,
        directory,
        query.file,
        query.line,
        query.character,
    )))
}

pub(crate) async fn lsp_prepare_call_hierarchy(
    State(state): State<crate::state::AppState>,
    Query(query): Query<LspPositionQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<lsp::LspCallHierarchyItem>>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    Ok(Json(lsp::prepare_call_hierarchy(
        &*runtime_lsp(&state, &directory).await?,
        directory,
        query.file,
        query.line,
        query.character,
    )))
}

pub(crate) async fn lsp_incoming_calls(
    State(state): State<crate::state::AppState>,
    Query(query): Query<LspPositionQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<lsp::LspCallHierarchyCall>>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    Ok(Json(lsp::incoming_calls(
        &*runtime_lsp(&state, &directory).await?,
        directory,
        query.file,
        query.line,
        query.character,
    )))
}

pub(crate) async fn lsp_outgoing_calls(
    State(state): State<crate::state::AppState>,
    Query(query): Query<LspPositionQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<lsp::LspCallHierarchyCall>>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    Ok(Json(lsp::outgoing_calls(
        &*runtime_lsp(&state, &directory).await?,
        directory,
        query.file,
        query.line,
        query.character,
    )))
}

pub(crate) async fn lsp_diagnostics(
    State(state): State<crate::state::AppState>,
    Query(query): Query<LspDocumentQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<lsp::LspDiagnostic>>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    Ok(Json(lsp::diagnostics(
        &*runtime_lsp(&state, &directory).await?,
        directory,
        query.file,
    )))
}

pub(crate) async fn lsp_document_symbols(
    State(state): State<crate::state::AppState>,
    Query(query): Query<LspDocumentQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<lsp::LspDocumentSymbol>>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    Ok(Json(lsp::document_symbols(
        &*runtime_lsp(&state, &directory).await?,
        directory,
        query.file,
    )))
}

pub(crate) async fn lsp_formatting(
    State(state): State<crate::state::AppState>,
    Query(query): Query<LspDocumentQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<Value>>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    Ok(Json(lsp::formatting(
        &*runtime_lsp(&state, &directory).await?,
        directory,
        query.file,
    )))
}

pub(crate) async fn lsp_code_actions(
    State(state): State<crate::state::AppState>,
    Query(query): Query<LspPositionQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<Value>>, ApiError> {
    let directory = resolve_directory(query.directory, &headers);
    Ok(Json(lsp::code_actions(
        &*runtime_lsp(&state, &directory).await?,
        directory,
        query.file,
        query.line,
        query.character,
    )))
}

pub(crate) async fn lsp_touch(
    State(state): State<crate::state::AppState>,
    headers: HeaderMap,
    Json(request): Json<LspTouchRequest>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let directory = resolve_directory(request.directory, &headers);
    Ok(Json(lsp::touch_document(
        &*runtime_lsp(&state, &directory).await?,
        directory,
        request.file,
        request.text.as_deref(),
    )))
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct LspShutdownResponse {
    shutdown: bool,
}

pub(crate) async fn lsp_shutdown(
    State(state): State<crate::state::AppState>,
) -> Json<LspShutdownResponse> {
    for runtime in state.inner.workspace_runtimes.runtimes().await {
        if let Some(lsp_runtime) = runtime.lsp_if_allocated() {
            lsp::shutdown_all(&lsp_runtime);
        }
    }
    Json(LspShutdownResponse { shutdown: true })
}
