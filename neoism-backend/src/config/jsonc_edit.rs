//! Small JSONC concrete-syntax editor.
//!
//! Unlike a serde round trip, this retains comments, whitespace, key order, and
//! trailing commas. Only the selected value/property/array element is changed.

use serde_json::Value;
use std::io::{Error, ErrorKind, Result};

#[derive(Clone, Debug)]
struct Node {
    start: usize,
    end: usize,
    kind: Kind,
}

#[derive(Clone, Debug)]
enum Kind {
    Object(Vec<Property>),
    Array(Vec<Node>),
    Scalar,
}

#[derive(Clone, Debug)]
struct Property {
    key: String,
    value: Node,
}

struct Parser<'a> {
    source: &'a [u8],
    cursor: usize,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source: source.as_bytes(),
            cursor: 0,
        }
    }

    fn trivia(&mut self) {
        loop {
            while self.cursor < self.source.len()
                && self.source[self.cursor].is_ascii_whitespace()
            {
                self.cursor += 1;
            }
            if self.source.get(self.cursor..self.cursor + 2) == Some(b"//") {
                self.cursor += 2;
                while self.cursor < self.source.len() && self.source[self.cursor] != b'\n'
                {
                    self.cursor += 1;
                }
            } else if self.source.get(self.cursor..self.cursor + 2) == Some(b"/*") {
                self.cursor += 2;
                while self.cursor + 1 < self.source.len()
                    && &self.source[self.cursor..self.cursor + 2] != b"*/"
                {
                    self.cursor += 1;
                }
                self.cursor = (self.cursor + 2).min(self.source.len());
            } else {
                break;
            }
        }
    }

    fn string(&mut self) -> Result<(usize, usize)> {
        let start = self.cursor;
        if self.source.get(self.cursor) != Some(&b'"') {
            return Err(invalid("expected a JSON string"));
        }
        self.cursor += 1;
        let mut escaped = false;
        while let Some(&byte) = self.source.get(self.cursor) {
            self.cursor += 1;
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                return Ok((start, self.cursor));
            }
        }
        Err(invalid("unterminated JSON string"))
    }

    fn value(&mut self) -> Result<Node> {
        self.trivia();
        let start = self.cursor;
        match self.source.get(self.cursor) {
            Some(b'{') => self.object(start),
            Some(b'[') => self.array(start),
            Some(b'"') => {
                let (_, end) = self.string()?;
                Ok(Node {
                    start,
                    end,
                    kind: Kind::Scalar,
                })
            }
            Some(_) => {
                while let Some(byte) = self.source.get(self.cursor) {
                    if matches!(byte, b',' | b'}' | b']') || byte.is_ascii_whitespace() {
                        break;
                    }
                    self.cursor += 1;
                }
                if start == self.cursor {
                    return Err(invalid("expected a JSON value"));
                }
                Ok(Node {
                    start,
                    end: self.cursor,
                    kind: Kind::Scalar,
                })
            }
            None => Err(invalid("expected a JSON value")),
        }
    }

    fn object(&mut self, start: usize) -> Result<Node> {
        self.cursor += 1;
        let mut properties = Vec::new();
        loop {
            self.trivia();
            if self.source.get(self.cursor) == Some(&b'}') {
                self.cursor += 1;
                return Ok(Node {
                    start,
                    end: self.cursor,
                    kind: Kind::Object(properties),
                });
            }
            let (key_start, key_end) = self.string()?;
            let key: String = serde_json::from_slice(&self.source[key_start..key_end])
                .map_err(|error| invalid(error.to_string()))?;
            self.trivia();
            if self.source.get(self.cursor) != Some(&b':') {
                return Err(invalid("expected ':' after object key"));
            }
            self.cursor += 1;
            let value = self.value()?;
            properties.push(Property { key, value });
            self.trivia();
            match self.source.get(self.cursor) {
                Some(b',') => self.cursor += 1,
                Some(b'}') => {}
                _ => return Err(invalid("expected ',' or '}' in object")),
            }
        }
    }

    fn array(&mut self, start: usize) -> Result<Node> {
        self.cursor += 1;
        let mut values = Vec::new();
        loop {
            self.trivia();
            if self.source.get(self.cursor) == Some(&b']') {
                self.cursor += 1;
                return Ok(Node {
                    start,
                    end: self.cursor,
                    kind: Kind::Array(values),
                });
            }
            values.push(self.value()?);
            self.trivia();
            match self.source.get(self.cursor) {
                Some(b',') => self.cursor += 1,
                Some(b']') => {}
                _ => return Err(invalid("expected ',' or ']' in array")),
            }
        }
    }
}

fn invalid(message: impl Into<String>) -> Error {
    Error::new(ErrorKind::InvalidData, message.into())
}

fn parse(source: &str) -> Result<Node> {
    let mut parser = Parser::new(source);
    let root = parser.value()?;
    parser.trivia();
    if parser.cursor != source.len() {
        return Err(invalid("unexpected content after root JSON value"));
    }
    Ok(root)
}

fn compact(value: &Value) -> Result<String> {
    serde_json::to_string(value).map_err(|error| invalid(error.to_string()))
}

fn nested_value(path: &[&str], value: Value) -> Value {
    path.iter().rev().fold(value, |child, key| {
        let mut map = serde_json::Map::new();
        map.insert((*key).to_string(), child);
        Value::Object(map)
    })
}

fn find<'a>(node: &'a Node, path: &[&str]) -> Option<&'a Node> {
    if path.is_empty() {
        return Some(node);
    }
    let Kind::Object(properties) = &node.kind else {
        return None;
    };
    let property = properties.iter().find(|property| property.key == path[0])?;
    find(&property.value, &path[1..])
}

fn line_indent(source: &str, at: usize) -> &str {
    let line = source[..at].rfind('\n').map_or(0, |index| index + 1);
    let prefix = &source[line..at];
    let indent = prefix
        .rfind(|character: char| !character.is_whitespace())
        .map_or(0, |index| {
            index + prefix[index..].chars().next().unwrap().len_utf8()
        });
    &prefix[indent..]
}

fn insert_property(
    source: &str,
    object: &Node,
    key: &str,
    value: Value,
) -> Result<String> {
    let Kind::Object(properties) = &object.kind else {
        return Err(invalid("config path parent is not an object"));
    };
    let close = object.end - 1;
    let base_indent = line_indent(source, close);
    let child_indent = format!("{base_indent}    ");
    let comma = properties
        .last()
        .filter(|last| !source[last.value.end..close].contains(','))
        .map_or("", |_| ",");
    let insertion = format!(
        "{comma}\n{child_indent}{}: {}\n{base_indent}",
        serde_json::to_string(key).map_err(|error| invalid(error.to_string()))?,
        compact(&value)?
    );
    let mut output = source.to_string();
    output.insert_str(close, &insertion);
    Ok(output)
}

/// Set a dotted path while retaining all unrelated concrete JSONC syntax.
pub(super) fn set_path(source: &str, path: &[&str], value: Value) -> Result<String> {
    if path.is_empty() || path.iter().any(|part| part.is_empty()) {
        return Err(invalid("config path must contain non-empty segments"));
    }
    let root = parse(source)?;
    let Kind::Object(_) = root.kind else {
        return Err(invalid("config root must be an object"));
    };

    let mut current = &root;
    for (index, part) in path.iter().enumerate() {
        let Kind::Object(properties) = &current.kind else {
            let replacement = compact(&nested_value(&path[index..], value))?;
            return replace(source, current.start, current.end, &replacement);
        };
        if let Some(property) = properties.iter().find(|property| property.key == *part) {
            if index + 1 == path.len() {
                return replace(
                    source,
                    property.value.start,
                    property.value.end,
                    &compact(&value)?,
                );
            }
            current = &property.value;
        } else {
            return insert_property(
                source,
                current,
                part,
                nested_value(&path[index + 1..], value),
            );
        }
    }
    unreachable!()
}

fn replace(source: &str, start: usize, end: usize, replacement: &str) -> Result<String> {
    if !source.is_char_boundary(start) || !source.is_char_boundary(end) {
        return Err(invalid("JSON token did not end on a UTF-8 boundary"));
    }
    let mut output = source.to_string();
    output.replace_range(start..end, replacement);
    Ok(output)
}

/// Upsert or remove one `keybinds.keys` entry, preserving all other syntax.
pub(super) fn edit_keybind(
    source: &str,
    action: &str,
    key: &str,
    with: &str,
) -> Result<String> {
    let root = parse(source)?;
    let Some(array) = find(&root, &["keybinds", "keys"]) else {
        if key.is_empty() {
            return Ok(source.to_string());
        }
        return set_path(
            source,
            &["keybinds", "keys"],
            Value::Array(vec![binding(action, key, with)]),
        );
    };
    let Kind::Array(elements) = &array.kind else {
        if key.is_empty() {
            return Ok(source.to_string());
        }
        return replace(
            source,
            array.start,
            array.end,
            &compact(&Value::Array(vec![binding(action, key, with)]))?,
        );
    };
    let matching = elements.iter().find(|element| {
        serde_json::from_str::<Value>(&source[element.start..element.end])
            .ok()
            .and_then(|value| {
                value
                    .get("action")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .as_deref()
            == Some(action)
    });
    if let Some(element) = matching {
        if !key.is_empty() {
            return replace(
                source,
                element.start,
                element.end,
                &compact(&binding(action, key, with))?,
            );
        }
        let index = elements
            .iter()
            .position(|candidate| candidate.start == element.start)
            .unwrap();
        let (start, end) = if let Some(next) = elements.get(index + 1) {
            (element.start, next.start)
        } else if let Some(previous) =
            index.checked_sub(1).and_then(|index| elements.get(index))
        {
            (previous.end, element.end)
        } else {
            let tail = &source[element.end..array.end - 1];
            let end = tail
                .find(',')
                .map_or(element.end, |comma| element.end + comma + 1);
            (element.start, end)
        };
        return replace(source, start, end, "");
    }
    if key.is_empty() {
        return Ok(source.to_string());
    }
    let close = array.end - 1;
    let comma = elements
        .last()
        .filter(|last| !source[last.end..close].contains(','))
        .map_or("", |_| ",");
    let insertion = format!("{comma}{}", compact(&binding(action, key, with))?);
    let mut output = source.to_string();
    output.insert_str(close, &insertion);
    Ok(output)
}

fn binding(action: &str, key: &str, with: &str) -> Value {
    let mut entry = serde_json::Map::new();
    entry.insert("key".into(), Value::String(key.into()));
    if !with.is_empty() {
        entry.insert("with".into(), Value::String(with.into()));
    }
    entry.insert("action".into(), Value::String(action.into()));
    Value::Object(entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targeted_set_retains_comments_trailing_commas_and_order() {
        let input = r#"{
    // keep this explanation
    "appearance": {
        "theme": "old", // inline survives
        "line-height": 1.2,
    },
    "unrelated": { "x": true },
}"#;
        let output =
            set_path(input, &["appearance", "theme"], Value::String("new".into()))
                .unwrap();
        assert!(output.contains("// keep this explanation"));
        assert!(output.contains("\"theme\": \"new\", // inline survives"));
        assert!(output.contains("\"line-height\": 1.2,"));
        assert!(output.contains("\"unrelated\": { \"x\": true }"));
    }

    #[test]
    fn targeted_set_adds_nested_path_without_reformatting_siblings() {
        let input = "{\n    // root comment\n    \"other\": [1, 2,],\n}\n";
        let output = set_path(input, &["editor", "vim-mode"], Value::Bool(true)).unwrap();
        assert!(output.contains("// root comment"));
        assert!(output.contains("\"other\": [1, 2,],"));
        assert!(output.contains("\"editor\": {\"vim-mode\":true}"));
    }

    #[test]
    fn keybind_edit_only_changes_matching_entry() {
        let input = r#"{
  "keybinds": { "keys": [
    { "key": "x", "action": "Other" }, // keep me
    { "key": "p", "with": "alt", "action": "Palette" },
  ] },
  // tail
}"#;
        let output = edit_keybind(input, "Palette", "k", "super").unwrap();
        assert!(output.contains("// keep me"));
        assert!(output.contains("{ \"key\": \"x\", \"action\": \"Other\" }"));
        assert!(output.contains(r#""key":"k""#));
        assert!(output.contains(r#""with":"super""#));
        assert!(output.contains(r#""action":"Palette""#));
        assert!(output.contains("// tail"));
    }

    #[test]
    fn insert_and_remove_remain_valid_with_trailing_commas() {
        let inserted = set_path(
            "{ \"other\": true, }",
            &["editor", "vim-mode"],
            Value::Bool(true),
        )
        .unwrap();
        parse(&inserted).unwrap();

        let removed = edit_keybind(
            "{ \"keybinds\": { \"keys\": [{ \"key\": \"p\", \"action\": \"Palette\" },] } }",
            "Palette",
            "",
            "",
        )
        .unwrap();
        parse(&removed).unwrap();
    }
}
