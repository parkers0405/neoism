use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{lsp, resolve_directory, InstanceQuery};

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
) -> Json<Vec<lsp::LspStatus>> {
    let directory = resolve_directory(query.directory, &headers);
    Json(lsp::status(&state.workspace_runtime(&directory).await.lsp(), directory))
}

pub(crate) async fn lsp_hover(
    State(state): State<crate::state::AppState>,
    Query(query): Query<LspPositionQuery>,
    headers: HeaderMap,
) -> Json<Vec<lsp::LspHover>> {
    let directory = resolve_directory(query.directory, &headers);
    Json(lsp::hover(&state.workspace_runtime(&directory).await.lsp(),
        directory,
        query.file,
        query.line,
        query.character,
    ))
}

pub(crate) async fn lsp_signature_help(
    State(state): State<crate::state::AppState>,
    Query(query): Query<LspPositionQuery>,
    headers: HeaderMap,
) -> Json<Vec<lsp::LspSignatureHelp>> {
    let directory = resolve_directory(query.directory, &headers);
    Json(lsp::signature_help(&state.workspace_runtime(&directory).await.lsp(),
        directory,
        query.file,
        query.line,
        query.character,
    ))
}

pub(crate) async fn lsp_inlay_hints(
    State(state): State<crate::state::AppState>,
    Query(query): Query<LspLineRangeQuery>,
    headers: HeaderMap,
) -> Json<Vec<lsp::LspInlayHint>> {
    let directory = resolve_directory(query.directory, &headers);
    Json(lsp::inlay_hints(&state.workspace_runtime(&directory).await.lsp(),
        directory,
        query.file,
        query.start_line,
        query.end_line,
    ))
}

pub(crate) async fn lsp_document_highlights(
    State(state): State<crate::state::AppState>,
    Query(query): Query<LspPositionQuery>,
    headers: HeaderMap,
) -> Json<Vec<lsp::LspDocumentHighlight>> {
    let directory = resolve_directory(query.directory, &headers);
    Json(lsp::document_highlights(&state.workspace_runtime(&directory).await.lsp(),
        directory,
        query.file,
        query.line,
        query.character,
    ))
}

pub(crate) async fn lsp_definition(
    State(state): State<crate::state::AppState>,
    Query(query): Query<LspPositionQuery>,
    headers: HeaderMap,
) -> Json<Vec<lsp::LspLocation>> {
    let directory = resolve_directory(query.directory, &headers);
    Json(lsp::definitions(&state.workspace_runtime(&directory).await.lsp(),
        directory,
        query.file,
        query.line,
        query.character,
    ))
}

pub(crate) async fn lsp_references(
    State(state): State<crate::state::AppState>,
    Query(query): Query<LspPositionQuery>,
    headers: HeaderMap,
) -> Json<Vec<lsp::LspLocation>> {
    let directory = resolve_directory(query.directory, &headers);
    Json(lsp::references(&state.workspace_runtime(&directory).await.lsp(),
        directory,
        query.file,
        query.line,
        query.character,
    ))
}

pub(crate) async fn lsp_implementation(
    State(state): State<crate::state::AppState>,
    Query(query): Query<LspPositionQuery>,
    headers: HeaderMap,
) -> Json<Vec<lsp::LspLocation>> {
    let directory = resolve_directory(query.directory, &headers);
    Json(lsp::implementations(&state.workspace_runtime(&directory).await.lsp(),
        directory,
        query.file,
        query.line,
        query.character,
    ))
}

pub(crate) async fn lsp_prepare_call_hierarchy(
    State(state): State<crate::state::AppState>,
    Query(query): Query<LspPositionQuery>,
    headers: HeaderMap,
) -> Json<Vec<lsp::LspCallHierarchyItem>> {
    let directory = resolve_directory(query.directory, &headers);
    Json(lsp::prepare_call_hierarchy(&state.workspace_runtime(&directory).await.lsp(),
        directory,
        query.file,
        query.line,
        query.character,
    ))
}

pub(crate) async fn lsp_incoming_calls(
    State(state): State<crate::state::AppState>,
    Query(query): Query<LspPositionQuery>,
    headers: HeaderMap,
) -> Json<Vec<lsp::LspCallHierarchyCall>> {
    let directory = resolve_directory(query.directory, &headers);
    Json(lsp::incoming_calls(&state.workspace_runtime(&directory).await.lsp(),
        directory,
        query.file,
        query.line,
        query.character,
    ))
}

pub(crate) async fn lsp_outgoing_calls(
    State(state): State<crate::state::AppState>,
    Query(query): Query<LspPositionQuery>,
    headers: HeaderMap,
) -> Json<Vec<lsp::LspCallHierarchyCall>> {
    let directory = resolve_directory(query.directory, &headers);
    Json(lsp::outgoing_calls(&state.workspace_runtime(&directory).await.lsp(),
        directory,
        query.file,
        query.line,
        query.character,
    ))
}

pub(crate) async fn lsp_diagnostics(
    State(state): State<crate::state::AppState>,
    Query(query): Query<LspDocumentQuery>,
    headers: HeaderMap,
) -> Json<Vec<lsp::LspDiagnostic>> {
    let directory = resolve_directory(query.directory, &headers);
    Json(lsp::diagnostics(&state.workspace_runtime(&directory).await.lsp(), directory, query.file))
}

pub(crate) async fn lsp_document_symbols(
    State(state): State<crate::state::AppState>,
    Query(query): Query<LspDocumentQuery>,
    headers: HeaderMap,
) -> Json<Vec<lsp::LspDocumentSymbol>> {
    let directory = resolve_directory(query.directory, &headers);
    Json(lsp::document_symbols(&state.workspace_runtime(&directory).await.lsp(), directory, query.file))
}

pub(crate) async fn lsp_formatting(
    State(state): State<crate::state::AppState>,
    Query(query): Query<LspDocumentQuery>,
    headers: HeaderMap,
) -> Json<Vec<Value>> {
    let directory = resolve_directory(query.directory, &headers);
    Json(lsp::formatting(&state.workspace_runtime(&directory).await.lsp(), directory, query.file))
}

pub(crate) async fn lsp_code_actions(
    State(state): State<crate::state::AppState>,
    Query(query): Query<LspPositionQuery>,
    headers: HeaderMap,
) -> Json<Vec<Value>> {
    let directory = resolve_directory(query.directory, &headers);
    Json(lsp::code_actions(&state.workspace_runtime(&directory).await.lsp(),
        directory,
        query.file,
        query.line,
        query.character,
    ))
}

pub(crate) async fn lsp_touch(
    State(state): State<crate::state::AppState>,
    headers: HeaderMap,
    Json(request): Json<LspTouchRequest>,
) -> Json<Vec<Value>> {
    let directory = resolve_directory(request.directory, &headers);
    Json(lsp::touch_document(&state.workspace_runtime(&directory).await.lsp(),
        directory,
        request.file,
        request.text.as_deref(),
    ))
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
