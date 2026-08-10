pub mod bridge_policy;
pub mod doc_sync;
pub mod helpers;
pub mod history;
pub mod input;
pub mod interactions;
pub mod links;
pub mod modes;
pub mod navigation;
pub mod pane;
pub mod render;
pub mod render_data;
pub mod roster;
pub mod scroll;
pub mod selection;
pub mod source_map;
pub mod spellcheck;
pub mod tables;
#[cfg(test)]
mod tests;
pub mod types;
pub mod vim;

pub use helpers::{
    is_markdown_path, parse_markdown_link_inner, parse_markdown_link_parts,
    parse_table_cell_bounds,
};
pub use links::{markdown_contact_value, markdown_link_open_action};
pub use source_map::InlineSourceMap;
pub use spellcheck::{
    bounded_levenshtein, is_misspelled_word, normalized_spellcheck_word,
    spellcheck_dictionary, spellcheck_words, spelling_suggestions, SpellcheckWord,
};
pub use types::*;

/// Strip inline Markdown syntax for text leaving the editor surface. Vim
/// registers intentionally retain source Markdown for correct paste semantics.
pub fn rendered_inline_text(text: &str) -> String {
    text.split('\n')
        .map(|line| {
            let marker_len = helpers::visible_marker_len(line).min(line.len());
            let body = &line[marker_len..];
            let body = helpers::heading_visible_end_col(line)
                .and_then(|end| line.get(marker_len..end))
                .unwrap_or(body);
            InlineSourceMap::new(body).visible_text()
        })
        .collect::<Vec<_>>()
        .join("\n")
}
