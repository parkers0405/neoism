use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::lsp_uri::file_uri_to_path;
use super::{
    LspCallHierarchyCall, LspCallHierarchyItem, LspCompletionItem, LspDiagnostic,
    LspDocumentHighlight, LspDocumentSymbol, LspHover, LspInlayHint, LspLocation,
    LspParameterInfo, LspPosition, LspRange, LspSignatureHelp, LspSignatureInfo,
    WorkspaceSymbol, MAX_COMPLETIONS, MAX_SYMBOLS,
};

const MAX_HOVER_CONTENT_CHARS: usize = 8_192;

pub(crate) fn parse_workspace_symbols(
    root: &Path,
    language: &str,
    result: Value,
) -> Vec<WorkspaceSymbol> {
    let Value::Array(items) = result else {
        return Vec::new();
    };

    items
        .into_iter()
        .filter_map(|item| parse_workspace_symbol(root, language, &item))
        .take(MAX_SYMBOLS)
        .collect()
}

fn parse_workspace_symbol(
    root: &Path,
    language: &str,
    item: &Value,
) -> Option<WorkspaceSymbol> {
    let name = item.get("name")?.as_str()?.to_string();
    let kind = item
        .get("kind")
        .and_then(Value::as_u64)
        .map(symbol_kind_name)
        .unwrap_or("unknown")
        .to_string();
    let location = item.get("location")?;
    let uri = location
        .get("uri")
        .or_else(|| location.get("targetUri"))?
        .as_str()?;
    let path = path_for_lsp_uri(root, uri);
    let line = location
        .pointer("/range/start/line")
        .or_else(|| location.pointer("/targetSelectionRange/start/line"))
        .and_then(Value::as_u64)
        .and_then(|line| u32::try_from(line + 1).ok());

    Some(WorkspaceSymbol {
        name,
        kind,
        path,
        line,
        language: Some(language.to_string()),
    })
}

pub(crate) fn parse_hover(
    root: &Path,
    file: &Path,
    language: &str,
    result: Value,
) -> Option<LspHover> {
    if result.is_null() {
        return None;
    }
    let contents_value = result.get("contents")?;
    let contents = truncate_hover_contents(hover_contents_to_string(contents_value)?);
    if contents.trim().is_empty() {
        return None;
    }
    let kind = contents_value
        .get("kind")
        .and_then(Value::as_str)
        .map(str::to_string);
    let range = result.get("range").and_then(parse_lsp_range);

    Some(LspHover {
        path: relative_path(root, file),
        contents,
        kind,
        range,
        language: Some(language.to_string()),
    })
}

fn truncate_hover_contents(contents: String) -> String {
    let Some((byte, _)) = contents.char_indices().nth(MAX_HOVER_CONTENT_CHARS) else {
        return contents;
    };
    format!(
        "{}\n\n[hover documentation truncated]",
        contents[..byte].trim_end()
    )
}

fn hover_contents_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => {
            let parts = items
                .iter()
                .filter_map(hover_contents_to_string)
                .filter(|text| !text.trim().is_empty())
                .collect::<Vec<_>>();
            (!parts.is_empty()).then(|| parts.join("\n\n"))
        }
        Value::Object(_) => {
            let text = value.get("value").and_then(Value::as_str)?;
            if let Some(language) = value.get("language").and_then(Value::as_str) {
                Some(format!("```{language}\n{text}\n```"))
            } else {
                Some(text.to_string())
            }
        }
        _ => None,
    }
}

pub(crate) fn parse_signature_help(
    root: &Path,
    file: &Path,
    language: &str,
    result: Value,
) -> Option<LspSignatureHelp> {
    if result.is_null() {
        return None;
    }
    let signatures = result
        .get("signatures")?
        .as_array()?
        .iter()
        .filter_map(parse_signature_info)
        .collect::<Vec<_>>();
    if signatures.is_empty() {
        return None;
    }

    Some(LspSignatureHelp {
        path: relative_path(root, file),
        signatures,
        active_signature: optional_u32(result.get("activeSignature")),
        active_parameter: optional_u32(result.get("activeParameter")),
        language: Some(language.to_string()),
    })
}

fn parse_signature_info(item: &Value) -> Option<LspSignatureInfo> {
    let label = item.get("label")?.as_str()?.to_string();
    let parameters = item
        .get("parameters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|parameter| {
            Some(LspParameterInfo {
                label: parameter_label_text(&label, parameter.get("label")?)?,
                documentation: parameter
                    .get("documentation")
                    .and_then(hover_contents_to_string)
                    .filter(|doc| !doc.trim().is_empty()),
            })
        })
        .collect();

    Some(LspSignatureInfo {
        documentation: item
            .get("documentation")
            .and_then(hover_contents_to_string)
            .filter(|doc| !doc.trim().is_empty()),
        parameters,
        active_parameter: optional_u32(item.get("activeParameter")),
        label,
    })
}

/// A ParameterInformation label is either the literal substring or a
/// `[start, end)` pair of UTF-16 code-unit offsets into the signature label.
/// Both forms normalize to the substring; offsets that split a surrogate pair
/// or overrun the label are rejected.
fn parameter_label_text(signature_label: &str, label: &Value) -> Option<String> {
    match label {
        Value::String(text) => Some(text.clone()),
        Value::Array(offsets) => {
            let start = offsets.first()?.as_u64()?;
            let end = offsets.get(1)?.as_u64()?;
            if end < start {
                return None;
            }
            let start = utf16_offset_to_byte(signature_label, start)?;
            let end = utf16_offset_to_byte(signature_label, end)?;
            signature_label.get(start..end).map(str::to_string)
        }
        _ => None,
    }
}

fn utf16_offset_to_byte(label: &str, offset: u64) -> Option<usize> {
    let mut utf16 = 0u64;
    for (byte_index, ch) in label.char_indices() {
        match utf16.cmp(&offset) {
            std::cmp::Ordering::Equal => return Some(byte_index),
            std::cmp::Ordering::Greater => return None,
            std::cmp::Ordering::Less => utf16 += ch.len_utf16() as u64,
        }
    }
    (utf16 == offset).then_some(label.len())
}

fn optional_u32(value: Option<&Value>) -> Option<u32> {
    value
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
}

pub(crate) fn parse_inlay_hints(
    root: &Path,
    file: &Path,
    language: &str,
    result: Value,
) -> Vec<LspInlayHint> {
    let Value::Array(items) = result else {
        return Vec::new();
    };
    items
        .into_iter()
        .filter_map(|item| parse_inlay_hint(root, file, language, &item))
        // A viewport of hints can legitimately exceed the symbol cap; bound by
        // the larger completion budget instead.
        .take(MAX_COMPLETIONS)
        .collect()
}

fn parse_inlay_hint(
    root: &Path,
    file: &Path,
    language: &str,
    item: &Value,
) -> Option<LspInlayHint> {
    let position = parse_lsp_position(item.get("position")?)?;
    let label = inlay_hint_label_text(item.get("label")?)?;

    Some(LspInlayHint {
        path: relative_path(root, file),
        line: position.line,
        character: position.character,
        label,
        kind: match item.get("kind").and_then(Value::as_u64) {
            Some(1) => Some("type".to_string()),
            Some(2) => Some("parameter".to_string()),
            _ => None,
        },
        padding_left: item
            .get("paddingLeft")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        padding_right: item
            .get("paddingRight")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        language: Some(language.to_string()),
    })
}

/// An InlayHint label is either one string or a list of InlayHintLabelPart
/// objects; the parts concatenate into the displayed text.
fn inlay_hint_label_text(label: &Value) -> Option<String> {
    match label {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => {
            let text = parts
                .iter()
                .filter_map(|part| part.get("value").and_then(Value::as_str))
                .collect::<String>();
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    }
}

pub(crate) fn parse_document_highlights(
    root: &Path,
    file: &Path,
    language: &str,
    result: Value,
) -> Vec<LspDocumentHighlight> {
    let Value::Array(items) = result else {
        return Vec::new();
    };
    items
        .into_iter()
        .filter_map(|item| parse_document_highlight(root, file, language, &item))
        .take(MAX_SYMBOLS)
        .collect()
}

fn parse_document_highlight(
    root: &Path,
    file: &Path,
    language: &str,
    item: &Value,
) -> Option<LspDocumentHighlight> {
    let range = item.get("range").and_then(parse_lsp_range)?;

    Some(LspDocumentHighlight {
        path: relative_path(root, file),
        range: Some(range),
        kind: match item.get("kind").and_then(Value::as_u64) {
            Some(1) => Some("text".to_string()),
            Some(2) => Some("read".to_string()),
            Some(3) => Some("write".to_string()),
            _ => None,
        },
        language: Some(language.to_string()),
    })
}

pub(crate) fn parse_locations(
    root: &Path,
    language: &str,
    result: Value,
) -> Vec<LspLocation> {
    match result {
        Value::Array(items) => items
            .into_iter()
            .filter_map(|item| parse_location(root, language, &item))
            .take(MAX_SYMBOLS)
            .collect(),
        Value::Object(_) => parse_location(root, language, &result)
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

pub(crate) fn parse_diagnostics(
    root: &Path,
    file: &Path,
    language: &str,
    params: Value,
) -> Vec<LspDiagnostic> {
    let path = params
        .get("uri")
        .and_then(Value::as_str)
        .map(|uri| path_for_lsp_uri(root, uri))
        .unwrap_or_else(|| relative_path(root, file));
    let Some(items) = params.get("diagnostics").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| parse_diagnostic(root, &path, language, item))
        .take(MAX_SYMBOLS)
        .collect()
}

fn parse_diagnostic(
    root: &Path,
    path: &str,
    language: &str,
    item: &Value,
) -> Option<LspDiagnostic> {
    let message = item.get("message")?.as_str()?.to_string();
    Some(LspDiagnostic {
        path: path.to_string(),
        range: item.get("range").and_then(parse_lsp_range),
        severity: diagnostic_severity(item.get("severity").and_then(Value::as_u64)),
        code: item.get("code").and_then(diagnostic_code),
        code_description: item
            .pointer("/codeDescription/href")
            .and_then(Value::as_str)
            .map(str::to_string),
        source: item
            .get("source")
            .and_then(Value::as_str)
            .map(str::to_string),
        message,
        tags: item
            .get("tags")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|tag| match tag.as_u64() {
                Some(1) => Some("unnecessary".to_string()),
                Some(2) => Some("deprecated".to_string()),
                _ => None,
            })
            .collect(),
        related_information: item
            .get("relatedInformation")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|related| {
                let location = related.get("location")?;
                let uri = location.get("uri")?.as_str()?;
                let related_path = path_for_lsp_uri(root, uri);
                Some(super::LspDiagnosticRelatedInformation {
                    path: related_path,
                    range: location.get("range").and_then(parse_lsp_range),
                    message: related.get("message")?.as_str()?.to_string(),
                })
            })
            .collect(),
        data: item.get("data").cloned(),
        language: Some(language.to_string()),
    })
}

fn diagnostic_severity(severity: Option<u64>) -> String {
    match severity {
        Some(1) => "error",
        Some(2) => "warning",
        Some(3) => "information",
        Some(4) => "hint",
        _ => "unknown",
    }
    .to_string()
}

fn diagnostic_code(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_i64().map(|value| value.to_string()))
        .or_else(|| value.as_u64().map(|value| value.to_string()))
}

pub(crate) fn parse_call_hierarchy_items(
    root: &Path,
    language: &str,
    result: Value,
) -> Vec<LspCallHierarchyItem> {
    let Value::Array(items) = result else {
        return Vec::new();
    };
    items
        .into_iter()
        .filter_map(|item| parse_call_hierarchy_item(root, language, &item))
        .take(MAX_SYMBOLS)
        .collect()
}

pub(crate) fn parse_call_hierarchy_calls(
    root: &Path,
    language: &str,
    result: Value,
    incoming: bool,
) -> Vec<LspCallHierarchyCall> {
    let Value::Array(items) = result else {
        return Vec::new();
    };
    items
        .into_iter()
        .filter_map(|item| parse_call_hierarchy_call(root, language, &item, incoming))
        .take(MAX_SYMBOLS)
        .collect()
}

pub(crate) fn parse_call_hierarchy_item(
    root: &Path,
    language: &str,
    item: &Value,
) -> Option<LspCallHierarchyItem> {
    let name = item.get("name")?.as_str()?.to_string();
    let kind = item
        .get("kind")
        .and_then(Value::as_u64)
        .map(symbol_kind_name)
        .unwrap_or("unknown")
        .to_string();
    let uri = item.get("uri")?.as_str()?;
    let range = item.get("range").and_then(parse_lsp_range);
    let selection_range = item.get("selectionRange").and_then(parse_lsp_range);
    Some(LspCallHierarchyItem {
        name,
        kind,
        detail: item
            .get("detail")
            .and_then(Value::as_str)
            .map(str::to_string),
        path: path_for_lsp_uri(root, uri),
        range,
        selection_range,
        language: Some(language.to_string()),
    })
}

fn parse_call_hierarchy_call(
    root: &Path,
    language: &str,
    item: &Value,
    incoming: bool,
) -> Option<LspCallHierarchyCall> {
    let target = if incoming {
        item.get("from")?
    } else {
        item.get("to")?
    };
    let ranges = item
        .get("fromRanges")
        .and_then(Value::as_array)
        .map(|ranges| ranges.iter().filter_map(parse_lsp_range).collect())
        .unwrap_or_default();
    Some(LspCallHierarchyCall {
        item: parse_call_hierarchy_item(root, language, target)?,
        ranges,
        direction: if incoming { "incoming" } else { "outgoing" }.to_string(),
        language: Some(language.to_string()),
    })
}

fn parse_location(root: &Path, language: &str, item: &Value) -> Option<LspLocation> {
    let uri = item
        .get("uri")
        .or_else(|| item.get("targetUri"))?
        .as_str()?;
    let range = item
        .get("range")
        .or_else(|| item.get("targetSelectionRange"))
        .or_else(|| item.get("targetRange"))
        .and_then(parse_lsp_range);

    Some(LspLocation {
        path: path_for_lsp_uri(root, uri),
        range,
        language: Some(language.to_string()),
    })
}

/// Parse a `textDocument/completion` response: either a bare
/// `CompletionItem[]` or a `CompletionList { items }`. CompletionList
/// `itemDefaults` are expanded before the item crosses the engine boundary so
/// resolve and acceptance see the same effective text edit/data the server
/// described. Snippet items are rejected because the client advertises
/// `snippetSupport: false`; inserting placeholder syntax as plain text would
/// corrupt the document.
pub(crate) fn parse_completion(result: Value) -> Vec<LspCompletionItem> {
    let (items, defaults) = match result {
        Value::Array(items) => (items, None),
        Value::Object(mut map) => {
            let defaults = map.remove("itemDefaults");
            match map.remove("items") {
                Some(Value::Array(items)) => (items, defaults),
                _ => return Vec::new(),
            }
        }
        _ => return Vec::new(),
    };
    items
        .into_iter()
        .filter_map(|mut item| {
            expand_completion_item_defaults(&mut item, defaults.as_ref());
            parse_completion_item(&item)
        })
        .take(MAX_COMPLETIONS)
        .collect()
}

fn expand_completion_item_defaults(item: &mut Value, defaults: Option<&Value>) {
    let (Some(item), Some(defaults)) =
        (item.as_object_mut(), defaults.and_then(Value::as_object))
    else {
        return;
    };
    for field in [
        "commitCharacters",
        "insertTextFormat",
        "insertTextMode",
        "data",
    ] {
        if !item.contains_key(field) {
            if let Some(value) = defaults.get(field) {
                item.insert(field.to_string(), value.clone());
            }
        }
    }

    // LSP 3.17 permits a list-wide editRange paired with per-item
    // textEditText. Materialize the ordinary TextEdit/InsertReplaceEdit so the
    // rest of Neoism has one canonical acceptance shape.
    if !item.contains_key("textEdit") {
        if let (Some(new_text), Some(edit_range)) = (
            item.get("textEditText")
                .and_then(Value::as_str)
                .or_else(|| item.get("insertText").and_then(Value::as_str))
                .or_else(|| item.get("label").and_then(Value::as_str))
                .map(str::to_string),
            defaults.get("editRange"),
        ) {
            let text_edit = if edit_range.get("insert").is_some()
                && edit_range.get("replace").is_some()
            {
                json!({
                    "insert": edit_range.get("insert").cloned().unwrap_or(Value::Null),
                    "replace": edit_range.get("replace").cloned().unwrap_or(Value::Null),
                    "newText": new_text,
                })
            } else {
                json!({ "range": edit_range, "newText": new_text })
            };
            item.insert("textEdit".to_string(), text_edit);
        }
    }
}

fn parse_completion_item(item: &Value) -> Option<LspCompletionItem> {
    let label = item.get("label")?.as_str()?.trim().to_string();
    if label.is_empty() {
        return None;
    }
    if item
        .get("insertTextFormat")
        .and_then(Value::as_u64)
        .is_some_and(|format| format == 2)
    {
        return None;
    }
    let kind = item
        .get("kind")
        .and_then(Value::as_u64)
        .map(completion_kind_name)
        .unwrap_or("text")
        .to_string();
    let detail = item
        .get("detail")
        .and_then(Value::as_str)
        .filter(|detail| !detail.trim().is_empty())
        .map(str::to_string)
        .or_else(|| {
            item.pointer("/labelDetails/detail")
                .and_then(Value::as_str)
                .filter(|detail| !detail.trim().is_empty())
                .map(str::to_string)
        });
    let documentation = item
        .get("documentation")
        .and_then(hover_contents_to_string)
        .filter(|doc| !doc.trim().is_empty());
    // Insert-text precedence: textEdit.newText > insertText > label.
    let insert_text = item
        .pointer("/textEdit/newText")
        .and_then(Value::as_str)
        .or_else(|| item.get("insertText").and_then(Value::as_str))
        .unwrap_or(label.as_str())
        .to_string();
    let filter_text = item
        .get("filterText")
        .and_then(Value::as_str)
        .map(str::to_string);
    let sort_text = item
        .get("sortText")
        .and_then(Value::as_str)
        .map(str::to_string);
    let preselect = item
        .get("preselect")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Some(LspCompletionItem {
        server_id: None,
        label,
        kind,
        detail,
        documentation,
        insert_text,
        filter_text,
        sort_text,
        preselect,
        payload: item.clone(),
    })
}

/// LSP `CompletionItemKind` (1..=25) → lowercase word for the popup tag/icon.
fn completion_kind_name(kind: u64) -> &'static str {
    match kind {
        1 => "text",
        2 => "method",
        3 => "function",
        4 => "constructor",
        5 => "field",
        6 => "variable",
        7 => "class",
        8 => "interface",
        9 => "module",
        10 => "property",
        11 => "unit",
        12 => "value",
        13 => "enum",
        14 => "keyword",
        15 => "snippet",
        16 => "color",
        17 => "file",
        18 => "reference",
        19 => "folder",
        20 => "enum_member",
        21 => "constant",
        22 => "struct",
        23 => "event",
        24 => "operator",
        25 => "type_parameter",
        _ => "text",
    }
}

pub(crate) fn parse_document_symbols(
    root: &Path,
    file: &Path,
    language: &str,
    result: Value,
) -> Vec<LspDocumentSymbol> {
    let Value::Array(items) = result else {
        return Vec::new();
    };

    items
        .into_iter()
        .filter_map(|item| parse_document_symbol(root, file, language, &item))
        .take(MAX_SYMBOLS)
        .collect()
}

fn parse_document_symbol(
    root: &Path,
    file: &Path,
    language: &str,
    item: &Value,
) -> Option<LspDocumentSymbol> {
    if item.get("selectionRange").is_none() {
        return parse_symbol_information_as_document_symbol(root, language, item);
    }

    let name = item.get("name")?.as_str()?.to_string();
    let kind = item
        .get("kind")
        .and_then(Value::as_u64)
        .map(symbol_kind_name)
        .unwrap_or("unknown")
        .to_string();
    let detail = item
        .get("detail")
        .and_then(Value::as_str)
        .map(str::to_string);
    let range = item.get("range").and_then(parse_lsp_range);
    let selection_range = item.get("selectionRange").and_then(parse_lsp_range);
    let children = item
        .get("children")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|child| parse_document_symbol(root, file, language, child))
                .take(MAX_SYMBOLS)
                .collect()
        })
        .unwrap_or_default();

    Some(LspDocumentSymbol {
        name,
        kind,
        detail,
        path: relative_path(root, file),
        range,
        selection_range,
        children,
        language: Some(language.to_string()),
    })
}

fn parse_symbol_information_as_document_symbol(
    root: &Path,
    language: &str,
    item: &Value,
) -> Option<LspDocumentSymbol> {
    let name = item.get("name")?.as_str()?.to_string();
    let kind = item
        .get("kind")
        .and_then(Value::as_u64)
        .map(symbol_kind_name)
        .unwrap_or("unknown")
        .to_string();
    let location = item.get("location")?;
    let uri = location
        .get("uri")
        .or_else(|| location.get("targetUri"))?
        .as_str()?;
    let range = location
        .get("range")
        .or_else(|| location.get("targetSelectionRange"))
        .and_then(parse_lsp_range);

    Some(LspDocumentSymbol {
        name,
        kind,
        detail: item
            .get("containerName")
            .and_then(Value::as_str)
            .map(str::to_string),
        path: path_for_lsp_uri(root, uri),
        range: range.clone(),
        selection_range: range,
        children: Vec::new(),
        language: Some(language.to_string()),
    })
}

pub(crate) fn parse_lsp_range(value: &Value) -> Option<LspRange> {
    Some(LspRange {
        start: parse_lsp_position(value.get("start")?)?,
        end: parse_lsp_position(value.get("end")?)?,
    })
}

fn parse_lsp_position(value: &Value) -> Option<LspPosition> {
    Some(LspPosition {
        line: u32::try_from(value.get("line")?.as_u64()? + 1).ok()?,
        character: u32::try_from(value.get("character")?.as_u64()? + 1).ok()?,
    })
}
fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn path_for_lsp_uri(root: &Path, uri: &str) -> String {
    let path = file_uri_to_path(uri).unwrap_or_else(|| PathBuf::from(uri));
    relative_path(root, &path)
}

fn symbol_kind_name(kind: u64) -> &'static str {
    match kind {
        1 => "file",
        2 => "module",
        3 => "namespace",
        4 => "package",
        5 => "class",
        6 => "method",
        7 => "property",
        8 => "field",
        9 => "constructor",
        10 => "enum",
        11 => "interface",
        12 => "function",
        13 => "variable",
        14 => "constant",
        15 => "string",
        16 => "number",
        17 => "boolean",
        18 => "array",
        19 => "object",
        20 => "key",
        21 => "null",
        22 => "enum_member",
        23 => "struct",
        24 => "event",
        25 => "operator",
        26 => "type_parameter",
        _ => "unknown",
    }
}

#[cfg(test)]
mod signature_label_tests {
    use super::*;

    #[test]
    fn parameter_labels_normalize_strings_and_utf16_offsets() {
        let signature = "fn greet(name: &str, wave: 😀) -> ()";
        assert_eq!(
            parameter_label_text(signature, &json!("name: &str")).as_deref(),
            Some("name: &str")
        );
        // "wave: 😀" spans UTF-16 units 21..29 (the emoji is a surrogate pair).
        assert_eq!(
            parameter_label_text(signature, &json!([21, 29])).as_deref(),
            Some("wave: 😀")
        );
        // Mid-surrogate and reversed offsets are rejected, not mis-sliced.
        assert_eq!(parameter_label_text(signature, &json!([21, 28])), None);
        assert_eq!(parameter_label_text(signature, &json!([9, 3])), None);
        assert_eq!(parameter_label_text(signature, &json!([0, 999])), None);
    }
}
