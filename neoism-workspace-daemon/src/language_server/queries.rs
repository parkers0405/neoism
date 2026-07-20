use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
};

use neoism_agent_server::language_server;
use neoism_protocol::editor::{EditorLspCompletionItem, EditorServerMessage};

use crate::nvim::NvimSessionHandle;

use super::active_buffer::read_active_file_buffer;

/// Completion at the active buffer's cursor, served by Neoism's LSP engine.
/// The engine request is blocking (spawns/queries the language server), so it
/// runs on the blocking pool — the daemon's async loop (and therefore nvim
/// keystroke dispatch) never stalls. `seq` is echoed so the client can drop a
/// response that a newer keystroke already superseded.
pub(crate) async fn completion(
    session: &NvimSessionHandle,
    workspace_root: &Path,
    seq: u64,
    trigger_character: Option<&str>,
) -> EditorServerMessage {
    let empty = EditorServerMessage::LspCompletions {
        surface_id: None,
        seq,
        replace_prefix: String::new(),
        items: Vec::new(),
    };
    let log = std::env::var_os("NEOISM_LSP_LOG").is_some();
    let buffer = match read_active_file_buffer(session).await {
        Ok(buffer) => buffer,
        Err(error) => {
            if log {
                eprintln!(
                    "neoism::lsp completion seq={seq}: no active file buffer ({error})"
                );
            }
            return empty;
        }
    };
    let replace_prefix = identifier_prefix(&buffer);
    let root = workspace_root.to_path_buf();
    let path = buffer.path.clone();
    let line = buffer.cursor_line;
    let character = buffer.cursor_col;
    if log {
        eprintln!(
            "neoism::lsp completion seq={seq}: requesting file={} line={line} char={character} prefix={replace_prefix:?}",
            path.display()
        );
    }
    let query_path = path.clone();
    let trigger_character = trigger_character.map(str::to_string);
    let items = tokio::task::spawn_blocking(move || {
        // Preserve the editor event order. Sending this query's full snapshot
        // directly from another worker could overtake a queued on_lines edit
        // and assign newer LSP versions to older text.
        super::flush_document_sync(&root, &query_path);
        language_server::completion_with_trigger(
            &root,
            &query_path,
            line,
            character,
            None,
            trigger_character.as_deref(),
        )
    })
    .await
    .unwrap_or_default();
    if log {
        eprintln!(
            "neoism::lsp completion seq={seq}: engine returned {} items (first={:?})",
            items.len(),
            items.first().map(|item| item.label.clone())
        );
    }
    let mut items = rank_completion_items(
        items.into_iter().map(map_completion_item).collect(),
        &replace_prefix,
        &buffer.text,
    );
    let revision = super::actions::document_revision(&buffer.text);
    for item in &mut items {
        item.file_path = buffer.path.clone();
        item.document_revision = revision.clone();
    }
    EditorServerMessage::LspCompletions {
        surface_id: None,
        seq,
        replace_prefix,
        items,
    }
}

/// Hover docs at an explicit Neovim UI grid cell, without moving the cursor.
/// Neovim resolves the rendered cell to a byte-oriented buffer position before
/// we sync the live text and query the language server.
pub(crate) async fn hover_at(
    session: &NvimSessionHandle,
    workspace_root: &Path,
    seq: u64,
    grid: i64,
    row: i64,
    col: i64,
) -> EditorServerMessage {
    let Some(position) = session
        .buffer_position_at_grid_cell(grid, row, col)
        .await
        .ok()
        .flatten()
    else {
        return EditorServerMessage::LspHoverResult {
            surface_id: None,
            seq,
            line: 0,
            character: 0,
            contents: String::new(),
        };
    };
    let line = position.line;
    let character = position.character;
    let empty = EditorServerMessage::LspHoverResult {
        surface_id: None,
        seq,
        line,
        character,
        contents: String::new(),
    };
    let buffer = match read_active_file_buffer(session).await {
        Ok(buffer) => buffer,
        Err(_) => return empty,
    };
    let root = workspace_root.to_path_buf();
    let path = buffer.path.clone();
    let contents = tokio::task::spawn_blocking(move || {
        // The event-driven on_lines path owns document synchronization. Wait
        // for its FIFO barrier rather than racing it with an ad-hoc didChange.
        super::flush_document_sync(&root, &path);
        language_server::hover(&root, &path, line, character)
            .first()
            .map(|item| item.contents.clone())
            .unwrap_or_default()
    })
    .await
    .unwrap_or_default();
    EditorServerMessage::LspHoverResult {
        surface_id: None,
        seq,
        line,
        character,
        contents,
    }
}

/// The identifier already typed immediately before the cursor (the trailing
/// run of `[A-Za-z0-9_]`), which the client backspaces before inserting a
/// chosen item so prefix/member completion replaces cleanly.
fn identifier_prefix(buffer: &crate::nvim::BufferText) -> String {
    let line = buffer
        .text
        .lines()
        .nth(buffer.cursor_line as usize)
        .unwrap_or("");
    // nvim_win_get_cursor returns a UTF-8 byte column, not a Unicode scalar
    // count. Treating it as `chars().take(col)` moves the logical cursor to the
    // right whenever earlier text contains a multibyte character.
    let mut col = (buffer.cursor_col as usize).min(line.len());
    while !line.is_char_boundary(col) {
        col -= 1;
    }
    let mut prefix = line[..col]
        .chars()
        .rev()
        .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
        .collect::<Vec<_>>();
    prefix.reverse();
    prefix.into_iter().collect()
}

/// Rank one completion response against the text that is actually before the
/// cursor. Language servers are allowed to return broad, only loosely ordered
/// lists; blindly painting them makes an exact local match lose to unrelated
/// alphabetic entries. This applies one language-neutral fuzzy score while
/// retaining the server's `preselect` and `sortText` as tie breakers.
///
/// Buffer identifiers are blended as a final source (the same useful fallback
/// users get from editor completion engines) once at least two characters have
/// been typed. They never replace or cap server results, and a server label
/// always wins a case-insensitive duplicate.
fn rank_completion_items(
    server_items: Vec<EditorLspCompletionItem>,
    prefix: &str,
    buffer_text: &str,
) -> Vec<EditorLspCompletionItem> {
    #[derive(Debug)]
    struct Ranked {
        item: EditorLspCompletionItem,
        score: CompletionMatchScore,
        server: bool,
        frequency: usize,
    }

    let mut ranked = Vec::with_capacity(server_items.len());
    let mut exact_items = HashSet::new();
    let mut server_labels = HashSet::new();
    for item in server_items {
        let filter = item.filter_text.as_deref().unwrap_or(&item.label);
        let Some(score) = completion_match_score(filter, prefix) else {
            continue;
        };
        // Keep overloads with distinct signatures, but collapse byte-for-byte
        // duplicates some servers emit through multiple completion routes.
        let dedupe_key = (
            item.label.to_lowercase(),
            item.insert_text.clone(),
            item.kind.clone(),
            item.detail.clone(),
        );
        if !exact_items.insert(dedupe_key) {
            continue;
        }
        server_labels.insert(item.label.to_lowercase());
        ranked.push(Ranked {
            item,
            score,
            server: true,
            frequency: 0,
        });
    }

    // A one-character prefix commonly matches most identifiers in a large
    // file. Waiting for two characters keeps buffer fallback useful without
    // flooding out a language server's semantically aware candidates.
    if prefix.chars().count() >= 2 {
        let mut identifiers = buffer_identifier_frequencies(buffer_text)
            .into_iter()
            .filter_map(|(label, frequency)| {
                if label == prefix || server_labels.contains(&label.to_lowercase()) {
                    return None;
                }
                let score = completion_match_score(&label, prefix)?;
                Some((label, frequency, score))
            })
            .collect::<Vec<_>>();
        identifiers.sort_by(|a, b| {
            a.2.cmp(&b.2)
                .then_with(|| b.1.cmp(&a.1))
                .then_with(|| a.0.cmp(&b.0))
        });
        // Buffer fallback is bounded independently; every matching LSP item is
        // retained even when a source file contains thousands of identifiers.
        for (label, frequency, score) in identifiers.into_iter().take(96) {
            ranked.push(Ranked {
                item: EditorLspCompletionItem {
                    server_id: None,
                    file_path: Default::default(),
                    document_revision: String::new(),
                    insert_text: label.clone(),
                    label,
                    kind: "text".to_string(),
                    detail: Some("Buffer".to_string()),
                    documentation: None,
                    filter_text: None,
                    sort_text: None,
                    preselect: false,
                    payload: None,
                },
                score,
                server: false,
                frequency,
            });
        }
    }

    ranked.sort_by(|a, b| {
        a.score
            .cmp(&b.score)
            .then_with(|| b.item.preselect.cmp(&a.item.preselect))
            .then_with(|| b.server.cmp(&a.server))
            .then_with(|| b.frequency.cmp(&a.frequency))
            .then_with(|| {
                a.item
                    .sort_text
                    .as_deref()
                    .unwrap_or(&a.item.label)
                    .cmp(b.item.sort_text.as_deref().unwrap_or(&b.item.label))
            })
            .then_with(|| a.item.label.cmp(&b.item.label))
    });
    ranked.into_iter().map(|ranked| ranked.item).collect()
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct CompletionMatchScore {
    /// 0 exact case-sensitive prefix, 1 case-insensitive prefix, 2 ordered
    /// subsequence. Lower is better.
    tier: u8,
    /// Number of skipped characters for subsequence matches.
    gaps: usize,
    /// Prefer the shorter completion when all other relevance is equal.
    tail: usize,
}

fn completion_match_score(candidate: &str, prefix: &str) -> Option<CompletionMatchScore> {
    if prefix.is_empty() {
        return Some(CompletionMatchScore {
            tier: 0,
            gaps: 0,
            tail: candidate.chars().count(),
        });
    }
    let candidate_chars = candidate.chars().count();
    let prefix_chars = prefix.chars().count();
    if candidate.starts_with(prefix) {
        return Some(CompletionMatchScore {
            tier: 0,
            gaps: 0,
            tail: candidate_chars.saturating_sub(prefix_chars),
        });
    }
    let folded_candidate = candidate.to_lowercase();
    let folded_prefix = prefix.to_lowercase();
    if folded_candidate.starts_with(&folded_prefix) {
        return Some(CompletionMatchScore {
            tier: 1,
            gaps: 0,
            tail: candidate_chars.saturating_sub(prefix_chars),
        });
    }

    let mut wanted = folded_prefix.chars();
    let mut next = wanted.next()?;
    let mut matched = 0usize;
    let mut last_match = None;
    let mut gaps = 0usize;
    for (index, ch) in folded_candidate.chars().enumerate() {
        if ch != next {
            continue;
        }
        if let Some(previous) = last_match {
            gaps += index.saturating_sub(previous + 1);
        } else {
            gaps += index;
        }
        last_match = Some(index);
        matched += 1;
        match wanted.next() {
            Some(ch) => next = ch,
            None => {
                return Some(CompletionMatchScore {
                    tier: 2,
                    gaps,
                    tail: candidate_chars.saturating_sub(matched),
                });
            }
        }
    }
    None
}

fn buffer_identifier_frequencies(text: &str) -> BTreeMap<String, usize> {
    let mut identifiers = BTreeMap::new();
    let mut current = String::new();
    let flush = |current: &mut String, identifiers: &mut BTreeMap<String, usize>| {
        if current.chars().count() >= 2
            && current
                .chars()
                .next()
                .is_some_and(|ch| ch == '_' || ch == '$' || ch.is_alphabetic())
        {
            *identifiers.entry(std::mem::take(current)).or_default() += 1;
        } else {
            current.clear();
        }
    };
    for ch in text.chars() {
        if ch == '_' || ch == '$' || ch.is_alphanumeric() {
            current.push(ch);
        } else {
            flush(&mut current, &mut identifiers);
        }
    }
    flush(&mut current, &mut identifiers);
    identifiers
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(text: &str, cursor_col: u32) -> crate::nvim::BufferText {
        crate::nvim::BufferText {
            path: "unicode.rs".into(),
            text: text.to_string(),
            cursor_line: 0,
            cursor_col,
        }
    }

    #[test]
    fn identifier_prefix_uses_nvim_utf8_byte_columns() {
        assert_eq!(identifier_prefix(&buffer("😀 alpha_suffix", 10)), "alpha");
        assert_eq!(identifier_prefix(&buffer("é beta_tail", 7)), "beta");
        assert_eq!(identifier_prefix(&buffer("中 café_tail", 9)), "café");
    }

    #[test]
    fn identifier_prefix_clamps_a_mid_codepoint_column_backward() {
        assert_eq!(identifier_prefix(&buffer("éclair", 1)), "");
    }

    fn item(label: &str) -> EditorLspCompletionItem {
        EditorLspCompletionItem {
            server_id: None,
            file_path: Default::default(),
            document_revision: String::new(),
            label: label.to_string(),
            kind: "function".to_string(),
            detail: None,
            documentation: None,
            insert_text: label.to_string(),
            filter_text: None,
            sort_text: None,
            preselect: false,
            payload: None,
        }
    }

    #[test]
    fn completion_ranking_filters_and_uses_filter_text() {
        let mut aliased = item("display label");
        aliased.filter_text = Some("dateMDY".to_string());
        let result = rank_completion_items(
            vec![item("zebra"), item("details"), aliased],
            "dM",
            "",
        );
        assert_eq!(
            result
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            vec!["display label"]
        );
    }

    #[test]
    fn completion_ranking_prefers_prefix_then_server_tie_breakers() {
        let mut preselected = item("details");
        preselected.preselect = true;
        preselected.sort_text = Some("9".to_string());
        let mut first_by_server = item("debugOne");
        first_by_server.sort_text = Some("0".to_string());
        let mut second_by_server = item("debugTwo");
        second_by_server.sort_text = Some("1".to_string());
        let result = rank_completion_items(
            vec![
                second_by_server,
                first_by_server,
                item("deepMatch"),
                preselected,
            ],
            "de",
            "",
        );
        assert_eq!(result[0].label, "details");
        assert_eq!(result[1].label, "debugOne");
        assert_eq!(result[2].label, "debugTwo");
        assert_eq!(result[3].label, "deepMatch");
    }

    #[test]
    fn completion_ranking_blends_buffer_words_without_server_duplicates() {
        let result = rank_completion_items(
            vec![item("details")],
            "de",
            "const details = defineModule(details);\nconst defineModuleRoute = defineModule;",
        );
        assert_eq!(
            result.iter().filter(|item| item.label == "details").count(),
            1
        );
        assert!(result.iter().any(|item| {
            item.label == "defineModule"
                && item.kind == "text"
                && item.detail.as_deref() == Some("Buffer")
        }));
        assert!(result.iter().any(|item| item.label == "defineModuleRoute"));
    }
}

fn map_completion_item(
    item: language_server::LspCompletionItem,
) -> EditorLspCompletionItem {
    EditorLspCompletionItem {
        server_id: item.server_id,
        file_path: Default::default(),
        document_revision: String::new(),
        label: item.label,
        kind: item.kind,
        detail: item.detail,
        documentation: item.documentation,
        insert_text: item.insert_text,
        filter_text: item.filter_text,
        sort_text: item.sort_text,
        preselect: item.preselect,
        payload: Some(item.payload),
    }
}
