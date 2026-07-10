use std::ffi::OsStr;
use std::path::Path;

use anyhow::Result;
use serde_json::{json, Value};

use super::lsp_client::StdioLspClient;
use super::lsp_languages::LanguageSpec;
use super::lsp_parse::{
    parse_call_hierarchy_calls, parse_call_hierarchy_items, parse_diagnostics,
    parse_document_symbols, parse_hover, parse_locations, parse_workspace_symbols,
};
use super::lsp_uri::path_to_file_uri;
use super::{
    LspDiagnostic, LspDocumentSymbol, LspHover, LspLocation, WorkspaceSymbol,
    DIAGNOSTIC_TIMEOUT, DOCUMENT_TIMEOUT, SYMBOL_TIMEOUT,
};

pub(super) fn query_workspace_symbols(
    root: &Path,
    query: &str,
    spec: &LanguageSpec,
) -> Result<Vec<WorkspaceSymbol>> {
    let command = spec
        .command
        .iter()
        .map(|part| (*part).to_string())
        .collect::<Vec<_>>();
    query_workspace_symbols_with_command(root, query, &command, spec.id)
}

pub(super) fn query_hover(
    root: &Path,
    file: &Path,
    line: u32,
    character: u32,
    spec: &LanguageSpec,
) -> Result<Vec<LspHover>> {
    let command = spec
        .command
        .iter()
        .map(|part| (*part).to_string())
        .collect::<Vec<_>>();
    query_hover_with_command(root, file, line, character, &command, spec.id)
}

fn query_hover_with_command(
    root: &Path,
    file: &Path,
    line: u32,
    character: u32,
    command: &[String],
    language: &str,
) -> Result<Vec<LspHover>> {
    let mut client = StdioLspClient::spawn(root, command)?;
    let initialized = client.initialize(root)?;
    if !initialized.hover_provider {
        let _ = client.shutdown();
        return Ok(Vec::new());
    }

    let language_id = language_id_for_path(language, file);
    client.open_document(file, language_id)?;
    let result = client.request(
        "textDocument/hover",
        text_document_position_params(file, line, character),
        DOCUMENT_TIMEOUT,
    )?;
    let _ = client.shutdown();
    Ok(parse_hover(root, file, language, result)
        .into_iter()
        .collect())
}

pub(super) fn query_definitions(
    root: &Path,
    file: &Path,
    line: u32,
    character: u32,
    spec: &LanguageSpec,
) -> Result<Vec<LspLocation>> {
    let command = spec
        .command
        .iter()
        .map(|part| (*part).to_string())
        .collect::<Vec<_>>();
    query_definitions_with_command(root, file, line, character, &command, spec.id)
}

fn query_definitions_with_command(
    root: &Path,
    file: &Path,
    line: u32,
    character: u32,
    command: &[String],
    language: &str,
) -> Result<Vec<LspLocation>> {
    let mut client = StdioLspClient::spawn(root, command)?;
    let initialized = client.initialize(root)?;
    if !initialized.definition_provider {
        let _ = client.shutdown();
        return Ok(Vec::new());
    }

    let language_id = language_id_for_path(language, file);
    client.open_document(file, language_id)?;
    let result = client.request(
        "textDocument/definition",
        text_document_position_params(file, line, character),
        DOCUMENT_TIMEOUT,
    )?;
    let _ = client.shutdown();
    Ok(parse_locations(root, language, result))
}

pub(super) fn query_references(
    root: &Path,
    file: &Path,
    line: u32,
    character: u32,
    spec: &LanguageSpec,
) -> Result<Vec<LspLocation>> {
    let command = spec
        .command
        .iter()
        .map(|part| (*part).to_string())
        .collect::<Vec<_>>();
    query_references_with_command(root, file, line, character, &command, spec.id)
}

fn query_references_with_command(
    root: &Path,
    file: &Path,
    line: u32,
    character: u32,
    command: &[String],
    language: &str,
) -> Result<Vec<LspLocation>> {
    let mut client = StdioLspClient::spawn(root, command)?;
    let initialized = client.initialize(root)?;
    if !initialized.references_provider {
        let _ = client.shutdown();
        return Ok(Vec::new());
    }

    let language_id = language_id_for_path(language, file);
    client.open_document(file, language_id)?;
    let mut params = text_document_position_params(file, line, character);
    params["context"] = json!({ "includeDeclaration": true });
    let result = client.request("textDocument/references", params, DOCUMENT_TIMEOUT)?;
    let _ = client.shutdown();
    Ok(parse_locations(root, language, result))
}

pub(super) fn query_implementations(
    root: &Path,
    file: &Path,
    line: u32,
    character: u32,
    spec: &LanguageSpec,
) -> Result<Vec<LspLocation>> {
    let command = spec
        .command
        .iter()
        .map(|part| (*part).to_string())
        .collect::<Vec<_>>();
    query_implementations_with_command(root, file, line, character, &command, spec.id)
}

fn query_implementations_with_command(
    root: &Path,
    file: &Path,
    line: u32,
    character: u32,
    command: &[String],
    language: &str,
) -> Result<Vec<LspLocation>> {
    let mut client = StdioLspClient::spawn(root, command)?;
    let initialized = client.initialize(root)?;
    if !initialized.implementation_provider {
        let _ = client.shutdown();
        return Ok(Vec::new());
    }

    let language_id = language_id_for_path(language, file);
    client.open_document(file, language_id)?;
    let result = client.request(
        "textDocument/implementation",
        text_document_position_params(file, line, character),
        DOCUMENT_TIMEOUT,
    )?;
    let _ = client.shutdown();
    Ok(parse_locations(root, language, result))
}

pub(super) fn query_prepare_call_hierarchy(
    root: &Path,
    file: &Path,
    line: u32,
    character: u32,
    spec: &LanguageSpec,
) -> Result<Vec<super::LspCallHierarchyItem>> {
    let command = spec
        .command
        .iter()
        .map(|part| (*part).to_string())
        .collect::<Vec<_>>();
    query_prepare_call_hierarchy_with_command(
        root, file, line, character, &command, spec.id,
    )
}

fn query_prepare_call_hierarchy_with_command(
    root: &Path,
    file: &Path,
    line: u32,
    character: u32,
    command: &[String],
    language: &str,
) -> Result<Vec<super::LspCallHierarchyItem>> {
    let mut client = StdioLspClient::spawn(root, command)?;
    let initialized = client.initialize(root)?;
    if !initialized.call_hierarchy_provider {
        let _ = client.shutdown();
        return Ok(Vec::new());
    }

    let language_id = language_id_for_path(language, file);
    client.open_document(file, language_id)?;
    let result = client.request(
        "textDocument/prepareCallHierarchy",
        text_document_position_params(file, line, character),
        DOCUMENT_TIMEOUT,
    )?;
    let _ = client.shutdown();
    Ok(parse_call_hierarchy_items(root, language, result))
}

pub(super) fn query_incoming_calls(
    root: &Path,
    file: &Path,
    line: u32,
    character: u32,
    spec: &LanguageSpec,
) -> Result<Vec<super::LspCallHierarchyCall>> {
    query_call_hierarchy_calls(root, file, line, character, spec, true)
}

pub(super) fn query_outgoing_calls(
    root: &Path,
    file: &Path,
    line: u32,
    character: u32,
    spec: &LanguageSpec,
) -> Result<Vec<super::LspCallHierarchyCall>> {
    query_call_hierarchy_calls(root, file, line, character, spec, false)
}

fn query_call_hierarchy_calls(
    root: &Path,
    file: &Path,
    line: u32,
    character: u32,
    spec: &LanguageSpec,
    incoming: bool,
) -> Result<Vec<super::LspCallHierarchyCall>> {
    let command = spec
        .command
        .iter()
        .map(|part| (*part).to_string())
        .collect::<Vec<_>>();
    let mut client = StdioLspClient::spawn(root, &command)?;
    let initialized = client.initialize(root)?;
    if !initialized.call_hierarchy_provider {
        let _ = client.shutdown();
        return Ok(Vec::new());
    }

    let language_id = language_id_for_path(spec.id, file);
    client.open_document(file, language_id)?;
    let prepared = client.request(
        "textDocument/prepareCallHierarchy",
        text_document_position_params(file, line, character),
        DOCUMENT_TIMEOUT,
    )?;
    let method = if incoming {
        "callHierarchy/incomingCalls"
    } else {
        "callHierarchy/outgoingCalls"
    };
    let mut calls = Vec::new();
    if let Value::Array(items) = prepared {
        for item in items {
            let result =
                client.request(method, json!({ "item": item }), DOCUMENT_TIMEOUT)?;
            calls.extend(parse_call_hierarchy_calls(root, spec.id, result, incoming));
            if calls.len() >= super::MAX_SYMBOLS {
                calls.truncate(super::MAX_SYMBOLS);
                break;
            }
        }
    }
    let _ = client.shutdown();
    Ok(calls)
}

pub(super) fn query_diagnostics(
    root: &Path,
    file: &Path,
    spec: &LanguageSpec,
) -> Result<Vec<LspDiagnostic>> {
    let command = spec
        .command
        .iter()
        .map(|part| (*part).to_string())
        .collect::<Vec<_>>();
    query_diagnostics_with_command(root, file, &command, spec.id)
}

fn query_diagnostics_with_command(
    root: &Path,
    file: &Path,
    command: &[String],
    language: &str,
) -> Result<Vec<LspDiagnostic>> {
    let mut client = StdioLspClient::spawn(root, command)?;
    let _ = client.initialize(root)?;
    let language_id = language_id_for_path(language, file);
    client.open_document(file, language_id)?;
    let result = client
        .wait_for_notification("textDocument/publishDiagnostics", DIAGNOSTIC_TIMEOUT)?
        .unwrap_or(Value::Null);
    let _ = client.shutdown();
    Ok(parse_diagnostics(root, file, language, result))
}

pub(super) fn query_document_symbols(
    root: &Path,
    file: &Path,
    spec: &LanguageSpec,
) -> Result<Vec<LspDocumentSymbol>> {
    let command = spec
        .command
        .iter()
        .map(|part| (*part).to_string())
        .collect::<Vec<_>>();
    query_document_symbols_with_command(root, file, &command, spec.id)
}

fn query_document_symbols_with_command(
    root: &Path,
    file: &Path,
    command: &[String],
    language: &str,
) -> Result<Vec<LspDocumentSymbol>> {
    let mut client = StdioLspClient::spawn(root, command)?;
    let initialized = client.initialize(root)?;
    if !initialized.document_symbol_provider {
        let _ = client.shutdown();
        return Ok(Vec::new());
    }

    let language_id = language_id_for_path(language, file);
    client.open_document(file, language_id)?;
    let result = client.request(
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": path_to_file_uri(file) } }),
        DOCUMENT_TIMEOUT,
    )?;
    let _ = client.shutdown();
    Ok(parse_document_symbols(root, file, language, result))
}

pub(super) fn query_document_formatting(
    root: &Path,
    file: &Path,
    spec: &LanguageSpec,
) -> Result<Value> {
    let command = spec
        .command
        .iter()
        .map(|part| (*part).to_string())
        .collect::<Vec<_>>();
    let mut client = StdioLspClient::spawn(root, &command)?;
    let initialized = client.initialize(root)?;
    if !initialized.formatting_provider {
        let _ = client.shutdown();
        return Ok(Value::Array(Vec::new()));
    }

    let language_id = language_id_for_path(spec.id, file);
    client.open_document(file, language_id)?;
    let result = client.request(
        "textDocument/formatting",
        json!({
            "textDocument": { "uri": path_to_file_uri(file) },
            "options": {
                "tabSize": 4,
                "insertSpaces": true,
                "trimTrailingWhitespace": true,
                "insertFinalNewline": true,
                "trimFinalNewlines": true
            }
        }),
        DOCUMENT_TIMEOUT,
    )?;
    let _ = client.shutdown();
    Ok(result)
}

pub(super) fn query_code_actions(
    root: &Path,
    file: &Path,
    line: u32,
    character: u32,
    spec: &LanguageSpec,
) -> Result<Value> {
    let command = spec
        .command
        .iter()
        .map(|part| (*part).to_string())
        .collect::<Vec<_>>();
    let mut client = StdioLspClient::spawn(root, &command)?;
    let initialized = client.initialize(root)?;
    if !initialized.code_action_provider {
        let _ = client.shutdown();
        return Ok(Value::Array(Vec::new()));
    }

    let language_id = language_id_for_path(spec.id, file);
    client.open_document(file, language_id)?;
    let result = client.request(
        "textDocument/codeAction",
        json!({
            "textDocument": { "uri": path_to_file_uri(file) },
            "range": {
                "start": { "line": line, "character": character },
                "end": { "line": line, "character": character }
            },
            "context": { "diagnostics": [] }
        }),
        DOCUMENT_TIMEOUT,
    )?;
    let _ = client.shutdown();
    Ok(result)
}

pub(super) fn notify_document_touched(
    root: &Path,
    file: &Path,
    text: Option<&str>,
    spec: &LanguageSpec,
) -> Result<()> {
    let command = spec
        .command
        .iter()
        .map(|part| (*part).to_string())
        .collect::<Vec<_>>();
    let mut client = StdioLspClient::spawn(root, &command)?;
    let _ = client.initialize(root)?;
    let language_id = language_id_for_path(spec.id, file);
    match text {
        Some(text) => {
            client.open_document_with_text(file, language_id, text)?;
            client.change_document(file, 1, text)?;
        }
        None => {
            client.open_document(file, language_id)?;
        }
    }
    client.save_document(file)?;
    client.close_document(file)?;
    let _ = client.shutdown();
    Ok(())
}

pub(super) fn query_workspace_symbols_with_command(
    root: &Path,
    query: &str,
    command: &[String],
    language: &str,
) -> Result<Vec<WorkspaceSymbol>> {
    let mut client = StdioLspClient::spawn(root, command)?;
    let initialized = client.initialize(root)?;
    if !initialized.workspace_symbol_provider {
        let _ = client.shutdown();
        return Ok(Vec::new());
    }

    let result = client.request(
        "workspace/symbol",
        json!({ "query": query }),
        SYMBOL_TIMEOUT,
    )?;
    let _ = client.shutdown();
    Ok(parse_workspace_symbols(root, language, result))
}

pub(super) fn language_id_for_path(language: &str, file: &Path) -> &'static str {
    match language {
        "c_cpp" => match file.extension().and_then(OsStr::to_str) {
            Some("c") | Some("h") => "c",
            _ => "cpp",
        },
        "csharp" => "csharp",
        "typescript" => match file.extension().and_then(OsStr::to_str) {
            Some("js") | Some("jsx") | Some("mjs") | Some("cjs") => "javascript",
            _ => "typescript",
        },
        "javascript" => "javascript",
        "python" => "python",
        "rust" => "rust",
        "go" => "go",
        "java" => "java",
        "ruby" => "ruby",
        "lua" => "lua",
        _ => "plaintext",
    }
}

fn text_document_position_params(file: &Path, line: u32, character: u32) -> Value {
    json!({
        "textDocument": {
            "uri": path_to_file_uri(file)
        },
        "position": {
            "line": line,
            "character": character
        }
    })
}
