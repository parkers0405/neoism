//! Native atomic input pills. Payloads stay out of the visible draft and expand only on send.
use crate::{app::App, tabs::TabKind};
use rune::TextEdit;
use std::ops::Range;

/// The unfinished `$skill` at the caret, never a committed pill or a shell variable mid-word.
pub(super) fn skill_query(draft: &TextEdit) -> Option<(Range<usize>, &str)> {
    if draft.anchor != draft.focus { return None; }
    let caret = draft.focus.min(draft.text.len());
    let before = draft.text.get(..caret)?;
    let start = before.rfind(char::is_whitespace).map_or(0, |i| i + before[i..].chars().next().unwrap().len_utf8());
    let word = &before[start..];
    let query = word.strip_prefix('$')?;
    let skill_char = |c: char| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':');
    if !query.chars().all(skill_char) { return None; }
    if draft.pills().any(|(r, _)| r.start < caret && start < r.end) { return None; }
    let end = caret + draft.text[caret..].chars().take_while(|c| skill_char(*c)).map(char::len_utf8).sum::<usize>();
    Some((start..end, query))
}

impl App {
    /// Compact only substantial pastes; ordinary prose stays ordinary editable text.
    pub(crate) fn paste_composer(&mut self, content: &rune::ClipboardContent) -> bool {
        if self.has_prompt() || !matches!(self.tabs[self.active].kind, TabKind::Agent { .. }) { return false; }
        if self.paste_image(content) { self.composer_changed(); return true; }
        self.paste_composer_text(content.text())
    }

    pub(crate) fn paste_composer_text(&mut self, text: &str) -> bool {
        if self.has_prompt() || !matches!(self.tabs[self.active].kind, TabKind::Agent { .. }) { return false; }
        let count = text.chars().count();
        if count <= 1000 && text.lines().take(11).count() <= 10 { return false; }
        let tab = &mut self.tabs[self.active];
        let id = format!("paste:{}", uuid::Uuid::new_v4());
        tab.pastes.insert(id.clone(), text.to_owned());
        tab.draft.insert_pill(id, &format!("[pasted {count} chars]"));
        self.composer_changed();
        true
    }

    pub(crate) fn insert_skill_pill(&mut self, name: &str, range: Option<Range<usize>>) {
        let draft = &mut self.tabs[self.active].draft;
        let label = format!("${name}");
        let id = format!("skill:{name}");
        if let Some(range) = range { draft.replace_with_pill(range, id, &label); }
        else { draft.insert_pill(id, &label); }
        if !draft.text[draft.focus..].starts_with(char::is_whitespace) { draft.paste(" "); }
    }

    pub(crate) fn expanded_composer_text(&self, index: usize) -> String {
        let tab = &self.tabs[index];
        let mut out = String::new();
        let mut end = 0;
        for (range, id) in tab.draft.pills() {
            let Some(payload) = tab.pastes.get(id) else { continue; };
            out.push_str(&tab.draft.text[end..range.start]);
            out.push_str(payload);
            end = range.end;
        }
        out.push_str(&tab.draft.text[end..]);
        out
    }

    /// Deleted attachment pills stay available to Undo, but must never be sent invisibly.
    pub(crate) fn prepare_composer_submit(&mut self, index: usize) {
        let tab = &self.tabs[index];
        if let Some(list) = self.attach.pending.get_mut(&tab.id) {
            list.retain(|a| tab.draft.pills().any(|(_, id)| id.strip_prefix("attachment:") == Some(a.token.as_str())));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_pastes_expand_by_identity_and_preserve_surrounding_unicode() {
        let mut app = App::new();
        app.open_tab(TabKind::Agent { session: None });
        let i = app.active;
        app.tabs[i].draft.paste("Before λ ");
        let payload = "long pasted text λ\n".repeat(100);
        assert!(app.paste_composer_text(&payload));
        app.tabs[i].draft.paste(" after");
        assert!(app.tabs[i].draft.text.contains("[pasted "));
        assert!(!app.tabs[i].draft.text.contains(&payload));
        assert_eq!(app.expanded_composer_text(i), format!("Before λ {payload} after"));
        assert!(!app.paste_composer_text("short paste"));
    }

    #[test]
    fn skill_query_ignores_committed_pills_and_does_not_replace_other_words() {
        let mut draft = TextEdit::new("explain λ $rev");
        let (range, query) = skill_query(&draft).unwrap();
        assert_eq!(query, "rev");
        draft.replace_with_pill(range, "skill:review", "$review");
        assert!(skill_query(&draft).is_none());
        draft.paste(" $next");
        assert_eq!(skill_query(&draft).unwrap().1, "next");
        assert!(skill_query(&TextEdit::new("price$var")).is_none());
    }

    #[test]
    fn backspace_removes_the_whole_paste_and_undo_restores_its_payload() {
        let mut app = App::new();
        app.open_tab(TabKind::Agent { session: None });
        let i = app.active;
        app.tabs[i].draft.paste("prefix ");
        let payload = "λ".repeat(1200);
        app.paste_composer_text(&payload);
        app.tabs[i].draft.key(&rune::KeyEvent { key: rune::Key::Backspace, mods: Default::default() }, None);
        assert_eq!(app.tabs[i].draft.text, "prefix ");
        assert_eq!(app.expanded_composer_text(i), "prefix ");
        app.tabs[i].draft.undo();
        assert_eq!(app.expanded_composer_text(i), format!("prefix {payload}"));
    }

    #[test]
    fn removed_image_pills_are_not_sent_and_other_pills_survive_attachment_insertion() {
        let mut app = App::new();
        app.load_fixture("chips", "Chips", "[]");
        let i = app.active;
        app.insert_skill_pill("review", None);
        assert!(app.attach_bytes("image.png", "image/png", vec![1, 2, 3]));
        let tab = app.tabs[i].id;
        let token = app.attach.pending[&tab][0].token.clone();
        assert_eq!(app.tabs[i].draft.pills().count(), 2);
        app.remove_attachment(tab, &token);
        assert_eq!(app.tabs[i].draft.pills().count(), 1);
        app.prepare_composer_submit(i);
        assert!(app.take_attachments("chips").0.is_empty());
    }

    #[test]
    fn dollar_picker_filters_and_inserts_a_skill_at_the_caret() {
        let mut app = App::new();
        app.open_tab(TabKind::Agent { session: None });
        let i = app.active;
        app.tabs[i].draft.replace_text("Before $rev after");
        app.tabs[i].draft.anchor = "Before $rev".len();
        app.tabs[i].draft.focus = app.tabs[i].draft.anchor;
        app.composer_changed();
        assert!(app.tabs[i].draft.focused);
        app.apply_agent(neoism_protocol::agent::AgentServerMessage::SkillCatalog { skills: vec![
            neoism_protocol::agent::SkillInfo { name: "review".into(), description: "Review code".into(), path: None },
            neoism_protocol::agent::SkillInfo { name: "deploy".into(), description: "Deploy code".into(), path: None },
        ] });
        assert_eq!(app.composer.picker.as_ref().unwrap().rows().len(), 1);
        app.accept_picker_row(None, &mut rune::Tx::default());
        assert_eq!(app.tabs[i].draft.text, "Before $review after");
        assert_eq!(app.tabs[i].draft.pills().count(), 1);
        app.tabs[i].draft.key(&rune::KeyEvent { key: rune::Key::Backspace, mods: Default::default() }, None);
        assert_eq!(app.tabs[i].draft.text, "Before  after");
    }
}