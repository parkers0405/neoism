#[derive(Debug, Clone)]
pub(crate) enum V4AHunk {
    Add {
        path: String,
        contents: String,
    },
    Delete {
        path: String,
    },
    Update {
        path: String,
        move_path: Option<String>,
        chunks: Vec<V4AChunk>,
    },
}

#[derive(Debug, Clone, Default)]
pub(crate) struct V4AChunk {
    pub(crate) old_lines: Vec<String>,
    pub(crate) new_lines: Vec<String>,
    pub(crate) change_context: Option<String>,
}

pub(crate) struct PatchedContent {
    pub(crate) text: String,
    pub(crate) bom: bool,
}

pub(crate) fn parse_v4a_patch(text: &str) -> anyhow::Result<Vec<V4AHunk>> {
    let cleaned = text.trim();
    let lines: Vec<&str> = cleaned.split('\n').collect();
    let begin_idx = lines
        .iter()
        .position(|l| l.trim() == "*** Begin Patch")
        .ok_or_else(|| anyhow::anyhow!("V4A patch missing `*** Begin Patch`"))?;
    let end_idx = lines
        .iter()
        .position(|l| l.trim() == "*** End Patch")
        .ok_or_else(|| anyhow::anyhow!("V4A patch missing `*** End Patch`"))?;
    if begin_idx >= end_idx {
        anyhow::bail!("V4A patch has malformed Begin/End markers");
    }

    let mut hunks = Vec::new();
    let mut i = begin_idx + 1;
    while i < end_idx {
        let line = lines[i];
        if let Some(rest) = line.strip_prefix("*** Add File:") {
            let path = rest.trim().to_string();
            ensure_path(&path, "Add File")?;
            i += 1;
            let mut contents = String::new();
            while i < end_idx && !lines[i].starts_with("***") {
                if let Some(rest) = lines[i].strip_prefix('+') {
                    contents.push_str(rest);
                    contents.push('\n');
                } else {
                    anyhow::bail!(
                        "invalid Add File line for {path}: expected lines to start with `+`, got `{}`",
                        lines[i]
                    );
                }
                i += 1;
            }
            if contents.ends_with('\n') {
                contents.pop();
            }
            hunks.push(V4AHunk::Add { path, contents });
        } else if let Some(rest) = line.strip_prefix("*** Delete File:") {
            let path = rest.trim().to_string();
            ensure_path(&path, "Delete File")?;
            hunks.push(V4AHunk::Delete { path });
            i += 1;
        } else if let Some(rest) = line.strip_prefix("*** Update File:") {
            let path = rest.trim().to_string();
            ensure_path(&path, "Update File")?;
            i += 1;
            let mut move_path = None;
            if i < end_idx {
                if let Some(rest) = lines[i].strip_prefix("*** Move to:") {
                    let path = rest.trim().to_string();
                    ensure_path(&path, "Move to")?;
                    move_path = Some(path);
                    i += 1;
                }
            }
            let mut chunks: Vec<V4AChunk> = Vec::new();
            while i < end_idx && !lines[i].starts_with("***") {
                if lines[i].starts_with("@@") {
                    let change_context = lines[i]
                        .strip_prefix("@@")
                        .map(str::trim)
                        .map(|line| line.strip_suffix("@@").unwrap_or(line).trim())
                        .map(str::trim)
                        .filter(|line| !line.is_empty())
                        .map(ToString::to_string);
                    i += 1;
                    let mut chunk = V4AChunk {
                        change_context,
                        ..V4AChunk::default()
                    };
                    while i < end_idx
                        && !lines[i].starts_with("@@")
                        && !lines[i].starts_with("***")
                    {
                        let l = lines[i];
                        if l == "*** End of File" {
                            i += 1;
                            break;
                        }
                        if let Some(rest) = l.strip_prefix(' ') {
                            chunk.old_lines.push(rest.to_string());
                            chunk.new_lines.push(rest.to_string());
                        } else if let Some(rest) = l.strip_prefix('-') {
                            chunk.old_lines.push(rest.to_string());
                        } else if let Some(rest) = l.strip_prefix('+') {
                            chunk.new_lines.push(rest.to_string());
                        } else if !l.is_empty() {
                            chunk.old_lines.push(l.to_string());
                            chunk.new_lines.push(l.to_string());
                        } else {
                            chunk.old_lines.push(String::new());
                            chunk.new_lines.push(String::new());
                        }
                        i += 1;
                    }
                    chunks.push(chunk);
                } else {
                    i += 1;
                }
            }
            if chunks.is_empty() && move_path.is_none() {
                anyhow::bail!("Update File {path} has no hunks");
            }
            hunks.push(V4AHunk::Update {
                path,
                move_path,
                chunks,
            });
        } else if line.starts_with("***") {
            anyhow::bail!("unknown V4A patch header `{line}` at line {}", i + 1);
        } else {
            i += 1;
        }
    }
    Ok(hunks)
}

fn ensure_path(path: &str, header: &str) -> anyhow::Result<()> {
    if path.is_empty() {
        anyhow::bail!("{header} header is missing a path");
    }
    Ok(())
}

pub(crate) fn apply_chunks(
    original: &str,
    chunks: &[V4AChunk],
) -> anyhow::Result<PatchedContent> {
    let (bom, text) = split_bom(original);
    let trailing_newline = text.ends_with('\n');
    let mut lines = text
        .split('\n')
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if trailing_newline {
        lines.pop();
    }
    let replacements = compute_replacements(&lines, chunks)?;
    let mut patched = apply_replacements(&lines, replacements).join("\n");
    if trailing_newline && !patched.ends_with('\n') {
        patched.push('\n');
    }
    Ok(PatchedContent { text: patched, bom })
}

fn compute_replacements(
    lines: &[String],
    chunks: &[V4AChunk],
) -> anyhow::Result<Vec<(usize, usize, Vec<String>)>> {
    let mut replacements = Vec::new();
    let mut cursor = 0;
    for chunk in chunks {
        if let Some(context) = &chunk.change_context {
            let context_line = context_line(context);
            if let Some(index) = seek_sequence(lines, &[context_line.to_string()], 0) {
                cursor = index + 1;
            } else if let Some(index) = seek_sequence_trimmed(lines, &[context_line], 0) {
                cursor = index + 1;
            } else {
                anyhow::bail!("patch context not found: {context}");
            }
        }

        if chunk.old_lines.is_empty() {
            let insertion = cursor.min(lines.len());
            replacements.push((insertion, 0, chunk.new_lines.clone()));
            cursor = insertion.saturating_add(chunk.new_lines.len());
            continue;
        }

        let start = seek_sequence(lines, &chunk.old_lines, cursor)
            .or_else(|| seek_sequence(lines, &chunk.old_lines, 0))
            .or_else(|| {
                seek_sequence_trimmed(
                    lines,
                    &chunk
                        .old_lines
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>(),
                    cursor,
                )
            })
            .or_else(|| {
                seek_sequence_trimmed(
                    lines,
                    &chunk
                        .old_lines
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>(),
                    0,
                )
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "patch context not found:\n{}",
                    chunk
                        .old_lines
                        .iter()
                        .take(8)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            })?;
        replacements.push((start, chunk.old_lines.len(), chunk.new_lines.clone()));
        cursor = start.saturating_add(chunk.new_lines.len());
    }
    replacements.sort_by_key(|(start, _, _)| *start);
    for pair in replacements.windows(2) {
        let (left_start, left_len, _) = &pair[0];
        let (right_start, _, _) = &pair[1];
        if left_start.saturating_add(*left_len) > *right_start {
            anyhow::bail!("patch chunks overlap; refusing to apply ambiguous update");
        }
    }
    Ok(replacements)
}

fn context_line(context: &str) -> &str {
    context
        .trim()
        .strip_prefix(|ch: char| ch == '-' || ch == '+')
        .unwrap_or(context.trim())
        .trim()
}

fn seek_sequence(lines: &[String], needle: &[String], start: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(start.min(lines.len()));
    }
    if needle.len() > lines.len() {
        return None;
    }
    (start.min(lines.len())..=lines.len().saturating_sub(needle.len()))
        .find(|index| sequence_matches(lines, needle, *index))
}

fn seek_sequence_trimmed(
    lines: &[String],
    needle: &[&str],
    start: usize,
) -> Option<usize> {
    if needle.is_empty() {
        return Some(start.min(lines.len()));
    }
    if needle.len() > lines.len() {
        return None;
    }
    (start.min(lines.len())..=lines.len().saturating_sub(needle.len())).find(|index| {
        needle.iter().enumerate().all(|(offset, wanted)| {
            lines
                .get(index + offset)
                .is_some_and(|line| line.trim_start() == wanted.trim_start())
        })
    })
}

fn sequence_matches(lines: &[String], needle: &[String], start: usize) -> bool {
    needle
        .iter()
        .enumerate()
        .all(|(offset, wanted)| lines.get(start + offset) == Some(wanted))
}

fn apply_replacements(
    lines: &[String],
    replacements: Vec<(usize, usize, Vec<String>)>,
) -> Vec<String> {
    let mut result = lines.to_vec();
    for (start, old_len, new_lines) in replacements.into_iter().rev() {
        result.splice(start..start + old_len, new_lines);
    }
    result
}

pub(crate) fn split_bom(text: &str) -> (bool, &str) {
    text.strip_prefix('\u{feff}')
        .map(|rest| (true, rest))
        .unwrap_or((false, text))
}

pub(crate) fn join_bom(text: &str, bom: bool) -> String {
    if bom {
        format!("\u{feff}{text}")
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_chunks_preserves_bom_and_trailing_newline() {
        let chunk = V4AChunk {
            old_lines: vec!["old".to_string()],
            new_lines: vec!["new".to_string()],
            change_context: None,
        };
        let patched = apply_chunks("\u{feff}old\n", &[chunk]).unwrap();
        assert!(patched.bom);
        assert_eq!(join_bom(&patched.text, patched.bom), "\u{feff}new\n");
    }

    #[test]
    fn apply_chunks_uses_context_cursor_for_empty_old_lines() {
        let chunks = vec![
            V4AChunk {
                old_lines: vec!["fn main() {".to_string()],
                new_lines: vec!["fn main() {".to_string()],
                change_context: None,
            },
            V4AChunk {
                old_lines: Vec::new(),
                new_lines: vec!["    println!(\"hi\");".to_string()],
                change_context: Some("fn main() {".to_string()),
            },
        ];
        let patched = apply_chunks("fn main() {\n}\n", &chunks).unwrap();
        assert_eq!(patched.text, "fn main() {\n    println!(\"hi\");\n}\n");
    }

    #[test]
    fn parse_rejects_unknown_headers() {
        let error =
            parse_v4a_patch("*** Begin Patch\n*** Rename File: old.rs\n*** End Patch")
                .unwrap_err();
        assert!(error.to_string().contains("unknown V4A patch header"));
    }

    #[test]
    fn parse_rejects_add_lines_without_plus_prefix() {
        let error = parse_v4a_patch(
            "*** Begin Patch\n*** Add File: added.rs\nmissing plus\n*** End Patch",
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("expected lines to start with `+`"));
    }

    #[test]
    fn parse_rejects_empty_update() {
        let error =
            parse_v4a_patch("*** Begin Patch\n*** Update File: file.rs\n*** End Patch")
                .unwrap_err();
        assert!(error.to_string().contains("has no hunks"));
    }
}
