use axum::extract::Query;
use axum::http::HeaderMap;
use axum::Json;
use serde::Deserialize;
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
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Json<Vec<lsp::LspStatus>> {
    let directory = resolve_directory(query.directory, &headers);
    Json(lsp::status(directory))
}

pub(crate) async fn lsp_hover(
    Query(query): Query<LspPositionQuery>,
    headers: HeaderMap,
) -> Json<Vec<lsp::LspHover>> {
    let directory = resolve_directory(query.directory, &headers);
    Json(lsp::hover(
        directory,
        query.file,
        query.line,
        query.character,
    ))
}

pub(crate) async fn lsp_signature_help(
    Query(query): Query<LspPositionQuery>,
    headers: HeaderMap,
) -> Json<Vec<lsp::LspSignatureHelp>> {
    let directory = resolve_directory(query.directory, &headers);
    Json(lsp::signature_help(
        directory,
        query.file,
        query.line,
        query.character,
    ))
}

pub(crate) async fn lsp_inlay_hints(
    Query(query): Query<LspLineRangeQuery>,
    headers: HeaderMap,
) -> Json<Vec<lsp::LspInlayHint>> {
    let directory = resolve_directory(query.directory, &headers);
    Json(lsp::inlay_hints(
        directory,
        query.file,
        query.start_line,
        query.end_line,
    ))
}

pub(crate) async fn lsp_document_highlights(
    Query(query): Query<LspPositionQuery>,
    headers: HeaderMap,
) -> Json<Vec<lsp::LspDocumentHighlight>> {
    let directory = resolve_directory(query.directory, &headers);
    Json(lsp::document_highlights(
        directory,
        query.file,
        query.line,
        query.character,
    ))
}

pub(crate) async fn lsp_definition(
    Query(query): Query<LspPositionQuery>,
    headers: HeaderMap,
) -> Json<Vec<lsp::LspLocation>> {
    let directory = resolve_directory(query.directory, &headers);
    Json(lsp::definitions(
        directory,
        query.file,
        query.line,
        query.character,
    ))
}

pub(crate) async fn lsp_references(
    Query(query): Query<LspPositionQuery>,
    headers: HeaderMap,
) -> Json<Vec<lsp::LspLocation>> {
    let directory = resolve_directory(query.directory, &headers);
    Json(lsp::references(
        directory,
        query.file,
        query.line,
        query.character,
    ))
}

pub(crate) async fn lsp_implementation(
    Query(query): Query<LspPositionQuery>,
    headers: HeaderMap,
) -> Json<Vec<lsp::LspLocation>> {
    let directory = resolve_directory(query.directory, &headers);
    Json(lsp::implementations(
        directory,
        query.file,
        query.line,
        query.character,
    ))
}

pub(crate) async fn lsp_prepare_call_hierarchy(
    Query(query): Query<LspPositionQuery>,
    headers: HeaderMap,
) -> Json<Vec<lsp::LspCallHierarchyItem>> {
    let directory = resolve_directory(query.directory, &headers);
    Json(lsp::prepare_call_hierarchy(
        directory,
        query.file,
        query.line,
        query.character,
    ))
}

pub(crate) async fn lsp_incoming_calls(
    Query(query): Query<LspPositionQuery>,
    headers: HeaderMap,
) -> Json<Vec<lsp::LspCallHierarchyCall>> {
    let directory = resolve_directory(query.directory, &headers);
    Json(lsp::incoming_calls(
        directory,
        query.file,
        query.line,
        query.character,
    ))
}

pub(crate) async fn lsp_outgoing_calls(
    Query(query): Query<LspPositionQuery>,
    headers: HeaderMap,
) -> Json<Vec<lsp::LspCallHierarchyCall>> {
    let directory = resolve_directory(query.directory, &headers);
    Json(lsp::outgoing_calls(
        directory,
        query.file,
        query.line,
        query.character,
    ))
}

pub(crate) async fn lsp_diagnostics(
    Query(query): Query<LspDocumentQuery>,
    headers: HeaderMap,
) -> Json<Vec<lsp::LspDiagnostic>> {
    let directory = resolve_directory(query.directory, &headers);
    Json(lsp::diagnostics(directory, query.file))
}

pub(crate) async fn lsp_document_symbols(
    Query(query): Query<LspDocumentQuery>,
    headers: HeaderMap,
) -> Json<Vec<lsp::LspDocumentSymbol>> {
    let directory = resolve_directory(query.directory, &headers);
    Json(lsp::document_symbols(directory, query.file))
}

pub(crate) async fn lsp_formatting(
    Query(query): Query<LspDocumentQuery>,
    headers: HeaderMap,
) -> Json<Vec<Value>> {
    let directory = resolve_directory(query.directory, &headers);
    Json(lsp::formatting(directory, query.file))
}

pub(crate) async fn lsp_code_actions(
    Query(query): Query<LspPositionQuery>,
    headers: HeaderMap,
) -> Json<Vec<Value>> {
    let directory = resolve_directory(query.directory, &headers);
    Json(lsp::code_actions(
        directory,
        query.file,
        query.line,
        query.character,
    ))
}

pub(crate) async fn lsp_touch(
    headers: HeaderMap,
    Json(request): Json<LspTouchRequest>,
) -> Json<Vec<Value>> {
    let directory = resolve_directory(request.directory, &headers);
    Json(lsp::touch_document(
        directory,
        request.file,
        request.text.as_deref(),
    ))
}

pub(crate) async fn lsp_shutdown() -> Json<Value> {
    lsp::shutdown_all();
    Json(serde_json::json!({ "shutdown": true }))
}
