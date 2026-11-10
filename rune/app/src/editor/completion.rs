//! Apply byte-coordinate completion edits atomically, preserving the document's undo history.
use super::document::Document;
use neoism_protocol::editor::EditorLspCompletionItem;
use serde_json::Value;

fn offset(lines: &[String], pos: &Value) -> Option<usize> {
    let line = pos.get("line")?.as_u64()? as usize;
    let col = pos.get("character")?.as_u64()? as usize;
    let text = lines.get(line)?;
    if !text.is_char_boundary(col) {
        return None;
    }
    Some(lines[..line].iter().map(|l| l.len() + 1).sum::<usize>() + col)
}

pub fn apply(doc: &mut Document, start: usize, item: &EditorLspCompletionItem) -> bool {
    let line = doc.cursor_line;
    let base: usize = doc.lines[..line].iter().map(|l| l.len() + 1).sum();
    let mut range = (base + start, base + doc.cursor_col);
    let raw = if item.insert_text.is_empty() {
        &item.label
    } else {
        &item.insert_text
    };
    let payload = item.payload.as_ref().unwrap_or(&Value::Null);
    let edit = &payload["textEdit"];
    let raw = edit["newText"].as_str().unwrap_or(raw);
    let text = if payload["insertTextFormat"].as_u64() == Some(2) {
        super::lsp::strip_snippet(raw)
    } else {
        raw.to_owned()
    };
    if let Some(r) = edit
        .get("range")
        .or_else(|| edit.get("replace"))
        .or_else(|| edit.get("insert"))
    {
        let (Some(a), Some(b)) = (
            offset(&doc.lines, &r["start"]),
            offset(&doc.lines, &r["end"]),
        ) else {
            return false;
        };
        range = (a, b);
    }
    let mut edits = vec![(range.0, range.1, text.clone())];
    if let Some(extra) = payload["additionalTextEdits"].as_array() {
        for e in extra {
            let (Some(a), Some(b), Some(t)) = (
                offset(&doc.lines, &e["range"]["start"]),
                offset(&doc.lines, &e["range"]["end"]),
                e["newText"].as_str(),
            ) else {
                return false;
            };
            edits.push((a, b, t.to_owned()));
        }
    }
    edits.sort_by_key(|e| (e.0, e.1));
    if edits.iter().any(|e| e.0 > e.1)
        || edits
            .windows(2)
            .any(|w| w[0].1 > w[1].0 || w[0].0 == w[1].0)
    {
        return false;
    }
    let shift: isize = edits
        .iter()
        .filter(|e| e.1 <= range.0 && e.0 < range.0)
        .map(|e| e.2.len() as isize - (e.1 - e.0) as isize)
        .sum();
    let caret = (range.0 as isize + shift) as usize + text.len();
    let mut source = doc.lines.join("\n");
    for (a, b, text) in edits.into_iter().rev() {
        source.replace_range(a..b, &text);
    }
    doc.begin_insert_group();
    doc.lines = source.split('\n').map(str::to_owned).collect();
    doc.cursor_line = source[..caret].bytes().filter(|b| *b == b'\n').count();
    doc.cursor_col = source[..caret].rsplit('\n').next().unwrap_or("").len();
    doc.mark_edited();
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn item(payload: Value) -> EditorLspCompletionItem {
        serde_json::from_value(json!({"label":"push", "kind":"method", "insert_text":"push", "payload":payload})).unwrap()
    }
    #[test]
    fn multiline_auto_import_and_undo_are_one_operation() {
        let mut doc = Document::from_text("let café = 1;\nitems.pu");
        doc.mode = super::super::document::Mode::Insert;
        doc.cursor_line = 1;
        doc.cursor_col = 8;
        let i = item(
            json!({"textEdit":{"range":{"start":{"line":1,"character":6},"end":{"line":1,"character":8}},"newText":"push(\n    café\n)"},
            "additionalTextEdits":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"newText":"use Thing;\n"}]}),
        );
        assert!(apply(&mut doc, 6, &i));
        assert_eq!(
            doc.lines,
            [
                "use Thing;",
                "let café = 1;",
                "items.push(",
                "    café",
                ")"
            ]
        );
        assert_eq!((doc.cursor_line, doc.cursor_col), (4, 1));
        doc.end_insert_group();
        doc.undo();
        assert_eq!(doc.lines, ["let café = 1;", "items.pu"]);
    }
    #[test]
    fn invalid_unicode_boundary_leaves_document_untouched() {
        let mut doc = Document::from_text("éx");
        doc.cursor_col = 3;
        let i = item(
            json!({"textEdit":{"range":{"start":{"line":0,"character":1},"end":{"line":0,"character":3}},"newText":"x"}}),
        );
        assert!(!apply(&mut doc, 2, &i));
        assert_eq!(doc.lines, ["éx"]);
    }
    #[test]
    fn plain_dollars_are_not_snippets() {
        let mut doc = Document::from_text("pu");
        doc.cursor_col = 2;
        let mut i = item(Value::Null);
        i.insert_text = "$HOME".into();
        assert!(apply(&mut doc, 0, &i));
        assert_eq!(doc.lines, ["$HOME"]);
    }
}
