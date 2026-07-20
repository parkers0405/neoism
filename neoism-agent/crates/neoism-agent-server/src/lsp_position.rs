use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Map, Value};

/// The coordinate encoding negotiated for every LSP `Position.character`.
///
/// Neoism's editor-facing convention is a zero-based UTF-8 byte column (the
/// same convention used by Neovim). Conversion therefore happens only at the
/// protocol boundary. LSP requires UTF-16 support and defaults to it when a
/// server omits `capabilities.positionEncoding`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum PositionEncoding {
    Utf8,
    #[default]
    Utf16,
    Utf32,
}

impl PositionEncoding {
    pub(super) fn from_server_name(name: Option<&str>) -> Self {
        match name.unwrap_or("utf-16").to_ascii_lowercase().as_str() {
            "utf-8" | "utf8" => Self::Utf8,
            "utf-32" | "utf32" => Self::Utf32,
            // UTF-16 is the protocol default. Treat an unknown value as that
            // mandatory baseline rather than silently interpreting it as
            // Neoism's UTF-8 byte convention.
            _ => Self::Utf16,
        }
    }

    pub(super) const fn protocol_name(self) -> &'static str {
        match self {
            Self::Utf8 => "utf-8",
            Self::Utf16 => "utf-16",
            Self::Utf32 => "utf-32",
        }
    }
}

/// Convert Neoism's UTF-8 byte column into the units selected by the server.
/// Oversized and mid-codepoint columns clamp to the preceding valid boundary,
/// matching LSP's requirement that positions denote boundaries between code
/// points and that positions beyond a line clamp to its end.
pub(super) fn byte_column_to_protocol(
    text: &str,
    line: u32,
    byte_column: u32,
    encoding: PositionEncoding,
) -> u32 {
    let Some(line) = line_text(text, line) else {
        return byte_column;
    };
    let boundary = clamp_byte_boundary(line, byte_column as usize);
    let prefix = &line[..boundary];
    match encoding {
        PositionEncoding::Utf8 => boundary as u32,
        PositionEncoding::Utf16 => prefix.encode_utf16().count() as u32,
        PositionEncoding::Utf32 => prefix.chars().count() as u32,
    }
}

/// Convert a server-selected character offset back to Neoism's UTF-8 byte
/// convention. Invalid offsets that split a multi-unit code point clamp to the
/// preceding code-point boundary; oversized offsets clamp to line end.
pub(super) fn protocol_column_to_byte(
    text: &str,
    line: u32,
    protocol_column: u32,
    encoding: PositionEncoding,
) -> u32 {
    let Some(line) = line_text(text, line) else {
        return protocol_column;
    };
    if encoding == PositionEncoding::Utf8 {
        return clamp_byte_boundary(line, protocol_column as usize) as u32;
    }

    let target = protocol_column as usize;
    let mut consumed = 0usize;
    for (byte_index, ch) in line.char_indices() {
        if consumed >= target {
            return byte_index as u32;
        }
        let width = match encoding {
            PositionEncoding::Utf8 => ch.len_utf8(),
            PositionEncoding::Utf16 => ch.len_utf16(),
            PositionEncoding::Utf32 => 1,
        };
        if consumed + width > target {
            return byte_index as u32;
        }
        consumed += width;
    }
    line.len() as u32
}

/// Return the final zero-based line and UTF-8 byte column in a document. This
/// is the end position used when an incremental-sync server receives a valid
/// whole-document replacement (a legal incremental change with one range).
pub(super) fn document_end_byte_position(text: &str) -> (u32, u32) {
    let bytes = text.as_bytes();
    let mut line = 0u32;
    let mut line_start = 0usize;
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        let width = match bytes[cursor] {
            b'\n' => 1,
            b'\r' if bytes.get(cursor + 1) == Some(&b'\n') => 2,
            b'\r' => 1,
            _ => {
                cursor += 1;
                continue;
            }
        };
        line = line.saturating_add(1);
        cursor += width;
        line_start = cursor;
    }
    (line, text.len().saturating_sub(line_start) as u32)
}

fn clamp_byte_boundary(line: &str, requested: usize) -> usize {
    let mut boundary = requested.min(line.len());
    while !line.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

/// LSP recognizes LF, CRLF, and lone CR as line terminators. `str::lines()`
/// does not treat a lone CR as a separator, so use a tiny byte scanner here.
fn line_text(text: &str, target: u32) -> Option<&str> {
    let bytes = text.as_bytes();
    let mut line = 0u32;
    let mut start = 0usize;
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        let width = match bytes[cursor] {
            b'\n' => 1,
            b'\r' if bytes.get(cursor + 1) == Some(&b'\n') => 2,
            b'\r' => 1,
            _ => {
                cursor += 1;
                continue;
            }
        };
        if line == target {
            return Some(&text[start..cursor]);
        }
        line = line.saturating_add(1);
        cursor += width;
        start = cursor;
    }
    (line == target).then(|| &text[start..])
}

/// Convert every LSP Position nested in a client request from Neoism's byte
/// columns to the negotiated protocol encoding. URI-bearing containers keep
/// ranges for edits/locations in the correct document even when a response
/// spans multiple files.
pub(super) fn client_value_to_protocol(
    value: &mut Value,
    documents: &mut HashMap<PathBuf, String>,
    encoding: PositionEncoding,
) {
    client_value_to_protocol_for_file(value, None, documents, encoding);
}

/// Variant for document-scoped requests whose params do not contain a URI
/// (notably `completionItem/resolve`).
pub(super) fn client_value_to_protocol_for_file(
    value: &mut Value,
    default_file: Option<&Path>,
    documents: &mut HashMap<PathBuf, String>,
    encoding: PositionEncoding,
) {
    let default_path = value_document_path(value);
    convert_value(
        value,
        default_path.as_deref().or(default_file),
        documents,
        encoding,
        Direction::ToProtocol,
    );
}

/// Convert every LSP Position nested in a server result/notification back to
/// Neoism's UTF-8 byte convention before parsers or the diagnostics cache see
/// it. `default_file` covers document-scoped results such as formatting edits;
/// URI-bearing locations and workspace edits override it recursively.
pub(super) fn server_value_to_bytes(
    value: &mut Value,
    default_file: Option<&Path>,
    documents: &mut HashMap<PathBuf, String>,
    encoding: PositionEncoding,
) {
    let value_path = value_document_path(value);
    convert_value(
        value,
        value_path.as_deref().or(default_file),
        documents,
        encoding,
        Direction::ToBytes,
    );
}

#[derive(Clone, Copy)]
enum Direction {
    ToProtocol,
    ToBytes,
}

fn convert_value(
    value: &mut Value,
    inherited_path: Option<&Path>,
    documents: &mut HashMap<PathBuf, String>,
    encoding: PositionEncoding,
    direction: Direction,
) {
    match value {
        Value::Array(items) => {
            for item in items {
                convert_value(item, inherited_path, documents, encoding, direction);
            }
        }
        Value::Object(object) => {
            let own_path = object_document_path(object)
                .or_else(|| inherited_path.map(Path::to_path_buf));
            if let (Some(line), Some(character)) = (
                object.get("line").and_then(Value::as_u64),
                object.get("character").and_then(Value::as_u64),
            ) {
                if let (Ok(line), Ok(character)) =
                    (u32::try_from(line), u32::try_from(character))
                {
                    if let Some(path) = own_path.as_deref() {
                        if let Some(text) = document_text(documents, path) {
                            let converted = match direction {
                                Direction::ToProtocol => byte_column_to_protocol(
                                    text, line, character, encoding,
                                ),
                                Direction::ToBytes => protocol_column_to_byte(
                                    text, line, character, encoding,
                                ),
                            };
                            object
                                .insert("character".to_string(), Value::from(converted));
                        }
                    }
                }
                return;
            }

            let target_path = object
                .get("targetUri")
                .and_then(Value::as_str)
                .and_then(super::lsp_uri::file_uri_to_path);
            for (key, child) in object.iter_mut() {
                if (key == "changes" || key == "relatedDocuments") && child.is_object() {
                    if let Some(uri_map) = child.as_object_mut() {
                        for (uri, nested) in uri_map {
                            let path = super::lsp_uri::file_uri_to_path(uri);
                            convert_value(
                                nested,
                                path.as_deref().or(own_path.as_deref()),
                                documents,
                                encoding,
                                direction,
                            );
                        }
                    }
                    continue;
                }
                let child_path = match key.as_str() {
                    "targetRange" | "targetSelectionRange" => {
                        target_path.as_deref().or(own_path.as_deref())
                    }
                    "originSelectionRange" => inherited_path.or(own_path.as_deref()),
                    _ => own_path.as_deref(),
                };
                convert_value(child, child_path, documents, encoding, direction);
            }
        }
        _ => {}
    }
}

pub(super) fn value_document_path(value: &Value) -> Option<PathBuf> {
    value
        .pointer("/textDocument/uri")
        .or_else(|| value.pointer("/item/uri"))
        .or_else(|| value.get("uri"))
        .and_then(Value::as_str)
        .and_then(super::lsp_uri::file_uri_to_path)
}

fn object_document_path(object: &Map<String, Value>) -> Option<PathBuf> {
    object
        .get("uri")
        .and_then(Value::as_str)
        .and_then(super::lsp_uri::file_uri_to_path)
        .or_else(|| {
            object
                .get("textDocument")
                .and_then(|document| document.get("uri"))
                .and_then(Value::as_str)
                .and_then(super::lsp_uri::file_uri_to_path)
        })
}

fn document_text<'a>(
    documents: &'a mut HashMap<PathBuf, String>,
    path: &Path,
) -> Option<&'a str> {
    if !documents.contains_key(path) {
        if let Ok(text) = fs::read_to_string(path) {
            documents.insert(path.to_path_buf(), text);
        }
    }
    documents.get(path).map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negotiated_names_include_mandatory_utf16_default() {
        assert_eq!(
            PositionEncoding::from_server_name(Some("utf-8")),
            PositionEncoding::Utf8
        );
        assert_eq!(
            PositionEncoding::from_server_name(Some("utf-32")),
            PositionEncoding::Utf32
        );
        assert_eq!(
            PositionEncoding::from_server_name(None),
            PositionEncoding::Utf16
        );
        assert_eq!(
            PositionEncoding::from_server_name(Some("future-encoding")),
            PositionEncoding::Utf16
        );
    }

    #[test]
    fn astral_and_multibyte_bmp_columns_round_trip_all_encodings() {
        // UTF-8 boundaries: 0, 1, 3, 7, 10, 11
        // UTF-16 units:    0, 1, 2, 4,  5,  6
        // UTF-32 units:    0, 1, 2, 3,  4,  5
        let text = "aé😀中z\n";
        let cases = [
            (PositionEncoding::Utf8, [0, 1, 3, 7, 10, 11]),
            (PositionEncoding::Utf16, [0, 1, 2, 4, 5, 6]),
            (PositionEncoding::Utf32, [0, 1, 2, 3, 4, 5]),
        ];
        for (encoding, protocol_columns) in cases {
            for (byte_column, protocol_column) in
                [0, 1, 3, 7, 10, 11].into_iter().zip(protocol_columns)
            {
                assert_eq!(
                    byte_column_to_protocol(text, 0, byte_column, encoding),
                    protocol_column,
                    "byte -> protocol for {encoding:?}"
                );
                assert_eq!(
                    protocol_column_to_byte(text, 0, protocol_column, encoding),
                    byte_column,
                    "protocol -> byte for {encoding:?}"
                );
            }
        }
    }

    #[test]
    fn uri_less_completion_item_uses_explicit_document_for_position_conversion() {
        let path = PathBuf::from("/workspace/main.ts");
        let mut documents = HashMap::from([(path.clone(), "a😀.de\n".to_string())]);
        let mut item = serde_json::json!({
            "label": "details",
            "textEdit": {
                "range": {
                    "start": {"line": 0, "character": 6},
                    "end": {"line": 0, "character": 8}
                },
                "newText": "details"
            }
        });

        client_value_to_protocol_for_file(
            &mut item,
            Some(&path),
            &mut documents,
            PositionEncoding::Utf16,
        );

        assert_eq!(
            item.pointer("/textEdit/range/start/character")
                .and_then(Value::as_u64),
            Some(4)
        );
        assert_eq!(
            item.pointer("/textEdit/range/end/character")
                .and_then(Value::as_u64),
            Some(6)
        );
    }

    #[test]
    fn invalid_columns_and_every_lsp_line_ending_clamp_safely() {
        let text = "é\r\n😀\r中\nz";
        assert_eq!(
            byte_column_to_protocol(text, 0, 1, PositionEncoding::Utf16),
            0,
            "mid-UTF-8 byte clamps backward"
        );
        assert_eq!(
            protocol_column_to_byte(text, 1, 1, PositionEncoding::Utf16),
            0,
            "mid-surrogate UTF-16 unit clamps backward"
        );
        assert_eq!(
            protocol_column_to_byte(text, 1, 99, PositionEncoding::Utf16),
            4,
            "oversized column clamps to astral line end"
        );
        assert_eq!(
            protocol_column_to_byte(text, 2, 1, PositionEncoding::Utf32),
            3,
            "lone CR separates the BMP line"
        );
        assert_eq!(
            protocol_column_to_byte(text, 3, 1, PositionEncoding::Utf8),
            1,
            "LF separates the final ASCII line"
        );
        assert_eq!(document_end_byte_position(text), (3, 1));
        assert_eq!(document_end_byte_position("a\n"), (1, 0));
        assert_eq!(document_end_byte_position(""), (0, 0));
    }
}
