//! Pending interactions retain their source session. Presentation follows family membership,
//! never sidebar expansion, loaded history, or which child happens to have an open tab.
use super::question_policy::{self, NeoismAgentPendingQuestion, QuestionCommit};
use crate::{app::App, tabs::TabKind, ui::theme::rgb};
use neoism_protocol::agent::{AgentClientMessage as C, AgentServerMessage as M, PermissionDecision as D};
use rune::{el, text, El, Key, KeyEvent, TextEdit, Tx};
use serde_json::Value;

pub struct Prompt {
    session: String,
    id: String,
    permission: Option<(String, Vec<String>)>,
    question: Option<NeoismAgentPendingQuestion>,
    edit: TextEdit,
    selected: usize,
    responding: bool,
    error: Option<String>,
}

#[derive(Default)]
pub struct Prompts { pending: Vec<Prompt> }

impl Prompts {
    fn insert(&mut self, prompt: Prompt) {
        // Stream reattach replays pending requests. Do not erase typed answers or progress.
        if !self.pending.iter().any(|p| p.session == prompt.session && p.id == prompt.id && p.permission.is_some() == prompt.permission.is_some()) {
            self.pending.push(prompt);
        }
    }
    fn question(&mut self, session: &str, request: &Value) {
        let mut q = question_policy::question_request_from_event(request);
        if q.id.is_empty() || q.questions.is_empty() { return; }
        q.session_id = session.into();
        let mut edit = TextEdit::new("");
        edit.focused = true;
        self.insert(Prompt { session: session.into(), id: q.id.clone(), permission: None, question: Some(q), edit, selected: 0, responding: false, error: None });
    }
    fn permission(&mut self, session: &str, id: &str, title: &str, patterns: Vec<String>) {
        if id.is_empty() { return; }
        self.insert(Prompt { session: session.into(), id: id.into(), permission: Some((title.into(), patterns)), question: None,
            edit: TextEdit::new(""), selected: 1, responding: false, error: None });
    }
    pub fn reconnect(&mut self) {
        // Replies outstanding on a dead socket will never receive an ack. Keep the answer
        // available for retry until the authoritative hydration removes or confirms it.
        for p in &mut self.pending { p.responding = false; }
    }
}

impl App {
    fn prompt_index(&self) -> Option<usize> {
        let TabKind::Agent { session: Some(viewed) } = &self.active_tab().kind else { return None };
        let root = self.families.root_of(viewed);
        self.composer.prompts.pending.iter().enumerate()
            .filter(|(_, p)| p.session == *viewed || (viewed == root && self.families.root_of(&p.session) == root))
            .min_by_key(|(i, p)| (p.permission.is_none(), *i)).map(|(i, _)| i)
    }
    pub(crate) fn has_prompt(&self) -> bool { self.prompt_index().is_some() }

    pub(crate) fn sync_prompt_focus(&mut self, enabled: bool) -> bool {
        let active = self.prompt_index();
        let mut changed = false;
        for (i, p) in self.composer.prompts.pending.iter_mut().enumerate() {
            let focused = enabled && Some(i) == active && p.question.is_some() && !p.responding;
            if p.edit.focused != focused { p.edit.focused = focused; changed = true; }
        }
        changed
    }

    pub(crate) fn question_prompt(&self) -> bool {
        self.prompt_index().is_some_and(|i| self.composer.prompts.pending[i].question.is_some())
    }

    pub(crate) fn submit_pending_prompt(&mut self) -> bool {
        let Some(i) = self.prompt_index() else { return false };
        let p = &self.composer.prompts.pending[i];
        self.reply_prompt((p.session.clone(), p.id.clone()), None, false);
        true
    }

    /// The underlying composer is not an alternate prompt buffer. If clicked while a card
    /// is up, edits/paste/IME still go to that card's native field, leaving the draft intact.
    pub(crate) fn composer_edit(&mut self) -> &mut TextEdit {
        if let Some(i) = self.prompt_index() { return &mut self.composer.prompts.pending[i].edit; }
        let i = self.active;
        &mut self.tabs[i].draft
    }

    pub(crate) fn pending_prompt_changed(&mut self) -> bool {
        if !self.has_prompt() { return false; }
        self.prompt_changed();
        true
    }

    pub(crate) fn apply_prompt_event(&mut self, event: &M) -> bool {
        let prompts = &mut self.composer.prompts;
        match event {
            M::ToolUseRequest { session_id, request_id, title, tool, patterns, .. } => {
                prompts.permission(session_id, request_id, &format!("{tool} · {title}"), patterns.clone());
            }
            M::QuestionAsked { session_id, request } => prompts.question(session_id, request),
            M::QuestionsUpdated { session_id, requests } | M::PermissionsUpdated { session_id, requests } => {
                let permission = matches!(event, M::PermissionsUpdated { .. });
                prompts.pending.retain(|p| p.session != *session_id || p.permission.is_some() != permission
                    || requests.iter().any(|r| r["id"].as_str() == Some(&p.id)));
                for r in requests {
                    if permission {
                        let patterns = r["patterns"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();
                        prompts.permission(session_id, r["id"].as_str().unwrap_or_default(), r["title"].as_str().unwrap_or("Allow tool?"), patterns);
                    } else { prompts.question(session_id, r); }
                }
            }
            M::PermissionRemoved { session_id, request_id } | M::QuestionRemoved { session_id, request_id } => {
                let permission = matches!(event, M::PermissionRemoved { .. });
                prompts.pending.retain(|p| p.session != *session_id || p.id != *request_id || p.permission.is_some() != permission);
            }
            M::PermissionReplyFailed { request_id, error } | M::QuestionReplyFailed { request_id, error } => {
                let permission = matches!(event, M::PermissionReplyFailed { .. });
                for p in prompts.pending.iter_mut().filter(|p| p.id == *request_id && p.permission.is_some() == permission) {
                    p.responding = false;
                    p.error = Some(error.clone());
                }
            }
            _ => return false,
        }
        true
    }

    fn prompt_changed(&mut self) {
        let Some(i) = self.prompt_index() else { return };
        let p = &mut self.composer.prompts.pending[i];
        if let Some(q) = &mut p.question {
            q.typed = p.edit.text.clone();
            q.selected = 0;
            q.scroll_offset = 0;
        }
    }

    /// A click carries its request identity: a delayed click must never approve the next card.
    fn reply_prompt(&mut self, identity: (String, String), choice: Option<usize>, reject: bool) {
        let Some(i) = self.prompt_index() else { return };
        let p = &mut self.composer.prompts.pending[i];
        if (p.session.clone(), p.id.clone()) != identity || p.responding { return; }
        let message = if p.permission.is_some() {
            let selected = if reject { 2 } else { choice.unwrap_or(p.selected) };
            C::ApproveTool { session_id: p.session.clone(), request_id: p.id.clone(), decision: match selected { 0 => D::Always, 1 => D::Yes, _ => D::No } }
        } else if reject {
            C::RejectQuestion { session_id: p.session.clone(), request_id: p.id.clone() }
        } else {
            let q = p.question.as_mut().unwrap();
            if let Some(choice) = choice { q.selected = choice; }
            // A failed final POST retries the SAME full answer, not an extra empty question.
            let answers = if q.current >= q.questions.len() { q.answers.clone() } else {
                match q.commit_selected() {
                    QuestionCommit::Nothing => return,
                    QuestionCommit::Advanced => { p.edit.clear(); return; }
                    QuestionCommit::Finished(answers) => answers,
                }
            };
            C::AnswerQuestion { session_id: p.session.clone(), request_id: p.id.clone(), answers }
        };
        p.responding = true;
        p.error = None;
        self.send_agent(message);
    }

    pub(crate) fn prompt_key(&mut self, e: &KeyEvent) -> bool {
        let Some(i) = self.prompt_index() else { return false };
        if e.mods.alt || e.mods.logo || e.mods.ctrl { return false; }
        let p = &mut self.composer.prompts.pending[i];
        if p.responding { return true; }
        let identity = (p.session.clone(), p.id.clone());
        match &e.key {
            Key::Up | Key::Down => {
                let delta = if e.key == Key::Up { -1 } else { 1 };
                if let Some(q) = &mut p.question { q.move_selection(delta); }
                else { p.selected = (p.selected as isize + delta).rem_euclid(3) as usize; }
            }
            Key::Escape => self.reply_prompt(identity, None, true),
            Key::Enter => self.reply_prompt(identity, None, false),
            Key::Text(t) if p.permission.is_some() => {
                match t.to_lowercase().as_str() {
                    "a" => self.reply_prompt(identity, Some(0), false),
                    "y" => self.reply_prompt(identity, Some(1), false),
                    "n" => self.reply_prompt(identity, Some(2), false),
                    _ => {}
                }
            }
            _ => return false, // The native question TextEdit owns typing, selection, IME/paste.
        }
        true
    }
}

/// Same input-anchored card geometry/tokens as slash::view and the shared prompt picker.
pub fn card(app: &App) -> El {
    let Some(i) = app.prompt_index() else { return el() };
    let p = &app.composer.prompts.pending[i];
    let Some(&[x, y, width, _]) = app.rects.get(&format!("composer-card:{}", app.active_tab().id)) else { return el() };
    let s = app.ty.scale();
    let palette = app.pal;
    let identity = (p.session.clone(), p.id.clone());
    let source = if app.families.is_child(&p.session) {
        format!("Subagent {} · ", app.families.member(&p.session).map_or(p.session.as_str(), |m| m.title.as_str()))
    } else { String::new() };
    let (title, rows, selected) = if let Some((title, patterns)) = &p.permission {
        (format!("{source}Permission required · {title}"), vec![("Always".into(), patterns.join(" · ")), ("Yes".into(), "Allow once".into()), ("No".into(), "Reject this request".into())], p.selected)
    } else {
        let q = p.question.as_ref().unwrap();
        let item = q.current_item().or_else(|| q.questions.last()).unwrap();
        (format!("{source}({}/{}) {}", (q.current + 1).min(q.questions.len()), q.questions.len(), item.text),
            q.visible_rows().into_iter().map(|r| (r.label, r.description)).collect(), q.selected)
    };
    let top = app.rects.get(&format!("agent-area:{}", app.active_tab().id)).map_or(0.0, |r| r[1]) + 8.0 * s;
    let row_h = 46.0 * s;
    let fit = (((y - top - 90.0 * s) / row_h).floor().max(1.0) as usize).min(7);
    let first = selected.saturating_sub(fit - 1).min(rows.len().saturating_sub(fit));
    let visible = rows.len().min(fit);
    let height = (82.0 * s + visible as f32 * row_h).min((y - top - 6.0 * s).max(0.0));
    let font = |size| rune::tok::TextStyle::new(size * s, 400.0).mono();
    let mut card = el().key(format!("agent-prompt:{}:{}", p.session, p.id)).w(width).h(height)
        .fill(rgb(palette.bg)).radius(14.0 * s).border(s, rgb(palette.border)).clip_rounded(14.0 * s)
        .on_press(|_: &mut App| {})
        .child(text(title).font(font(13.0)).color(rgb(palette.fg)).truncate().w(width - 70.0 * s).absolute(14.0 * s, 9.0 * s));
    let reject_id = identity.clone();
    card = card.child(text("esc").font(font(12.0)).color(rgb(palette.muted)).absolute(width - 42.0 * s, 9.0 * s)
        .on_press(move |a: &mut App| a.reply_prompt(reject_id.clone(), None, true)));
    if p.question.is_some() && !p.responding {
        card = card.child(rune::text_input(&p.edit, font(13.0)).key("agent-prompt-answer").w(width - 28.0 * s)
            .placeholder("Type an answer or choose an option", rgb(palette.muted)).color(rgb(palette.fg))
            .on_edit(|a: &mut App| { let i = a.prompt_index().unwrap(); &mut a.composer.prompts.pending[i].edit })
            .on_input_change(|a: &mut App, _: &mut Tx| a.prompt_changed())
            .on_input_key(|a: &mut App, e: &KeyEvent, _: &mut Tx| a.prompt_key(e))
            .absolute(14.0 * s, 31.0 * s));
    }
    for (index, (label, description)) in rows.into_iter().enumerate().skip(first).take(visible) {
        let id = identity.clone();
        let row = el().w(width).h(row_h).fill(rgb(if index == selected { palette.hover } else { palette.bg }))
            .child(text(label).font(font(14.0)).color(rgb(palette.fg)).truncate().w(width - 28.0 * s).absolute(14.0 * s, 4.0 * s))
            .child(text(description).font(font(12.0)).color(rgb(palette.muted)).truncate().w(width - 28.0 * s).absolute(14.0 * s, 25.0 * s))
            .on_press(move |a: &mut App| a.reply_prompt(id.clone(), Some(index), false));
        card = card.child(row.absolute(0.0, 54.0 * s + (index - first) as f32 * row_h));
    }
    let hint = p.error.clone().unwrap_or_else(|| if p.responding { "sending…".into() } else { "↑↓ choose · enter answer · esc reject".into() });
    card = card.child(text(hint).font(font(12.0)).color(rgb(if p.error.is_some() { palette.red } else { palette.muted }))
        .truncate().w(width - 28.0 * s).absolute(14.0 * s, height - 22.0 * s));
    el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).child(card.absolute(x, (y - height - 6.0 * s).max(top)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn app() -> App { let mut a = App::new(); a.load_fixture("root", "Main", "[]"); a.agent_sent.borrow_mut().clear(); a }
    fn family(a: &mut App) {
        a.apply_agent(M::CatalogFamily { session_id: "root".into(), snapshot: json!({"rootSessionId":"root","familyRevision":1,"branches":[
            {"sessionId":"child","parentSessionId":"root","title":"Worker","status":"outstanding"},
            {"sessionId":"grandchild","parentSessionId":"child","status":"outstanding"}
        ]}) });
        a.agent_sent.borrow_mut().clear();
    }
    fn permission(a: &mut App, session: &str, id: &str) {
        a.apply_agent(M::ToolUseRequest { session_id: session.into(), request_id: id.into(), tool: "bash".into(), title: "Allow command?".into(), patterns: vec!["cargo *".into()], args: Value::Null, source_agent: None });
    }
    fn question(a: &mut App, session: &str, id: &str, multi: bool) {
        let mut questions = vec![json!({"question":"Deploy?","options":["Yes","No"]})];
        if multi { questions.push(json!({"question":"Which environment?"})); }
        a.apply_agent(M::QuestionAsked { session_id: session.into(), request: json!({"id":id,"questions":questions}) });
    }
    fn key(a: &mut App, key: Key) { assert!(a.prompt_key(&KeyEvent { key, mods: Default::default() })); }
    fn last(a: &App) -> Value { serde_json::from_str(a.agent_sent.borrow().last().unwrap()).unwrap() }

    #[test]
    fn unopened_children_and_nested_children_route_to_main_even_when_family_arrives_late() {
        let mut a = app();
        permission(&mut a, "grandchild", "p");
        assert!(!a.has_prompt());
        family(&mut a);
        assert!(a.has_prompt());
        assert!(!a.tabs.iter().any(|t| matches!(&t.kind, TabKind::Agent {session: Some(s)} if s == "grandchild")));
        key(&mut a, Key::Enter);
        assert_eq!(last(&a), json!({"ApproveTool":{"session_id":"grandchild","request_id":"p","decision":"Yes"}}));
    }

    #[test]
    fn permission_queue_has_priority_deduplication_safe_default_and_ack_removal() {
        let mut a = app(); family(&mut a);
        question(&mut a, "root", "q", false);
        permission(&mut a, "child", "p"); permission(&mut a, "child", "p");
        permission(&mut a, "root", "p2");
        assert_eq!(a.composer.prompts.pending.len(), 3);
        key(&mut a, Key::Text("a".into()));
        assert_eq!(last(&a)["ApproveTool"]["decision"], "Always");
        key(&mut a, Key::Enter);
        assert_eq!(a.agent_sent.borrow().len(), 1, "no duplicate while POST is in flight");
        a.apply_agent(M::PermissionRemoved { session_id: "root".into(), request_id: "p".into() });
        assert_eq!(a.composer.prompts.pending.len(), 3, "wrong owner cannot remove child request");
        a.apply_agent(M::PermissionRemoved { session_id: "child".into(), request_id: "p".into() });
        key(&mut a, Key::Escape);
        assert_eq!(last(&a)["ApproveTool"]["decision"], "No");
    }

    #[test]
    fn multi_question_options_custom_answer_and_failed_final_reply_retry_same_payload() {
        let mut a = app(); family(&mut a); question(&mut a, "child", "q", true);
        a.tabs[a.active].draft.replace_text("Keep my draft");
        key(&mut a, Key::Down); key(&mut a, Key::Enter);
        assert!(a.agent_sent.borrow().is_empty());
        a.composer_edit().replace_text("staging"); a.pending_prompt_changed();
        key(&mut a, Key::Enter);
        let expected = json!({"AnswerQuestion":{"session_id":"child","request_id":"q","answers":[["No"],["staging"]]}});
        assert_eq!(last(&a), expected);
        a.apply_agent(M::QuestionReplyFailed { request_id: "q".into(), error: "offline".into() });
        key(&mut a, Key::Enter);
        assert_eq!(last(&a), expected);
        assert_eq!(a.active_tab().draft.text, "Keep my draft");
        a.apply_agent(M::QuestionRemoved { session_id: "child".into(), request_id: "q".into() });
        assert!(!a.has_prompt());
    }

    #[test]
    fn snapshots_are_session_scoped_preserve_progress_and_clear_stale_requests() {
        let mut a = app(); family(&mut a); question(&mut a, "child", "q", true);
        key(&mut a, Key::Enter);
        a.apply_agent(M::QuestionsUpdated { session_id:"child".into(), requests:vec![json!({"id":"q","questions":[{"question":"Deploy?"},{"question":"Env?"}]})] });
        assert_eq!(a.composer.prompts.pending[0].question.as_ref().unwrap().current, 1);
        permission(&mut a, "root", "p"); permission(&mut a, "child", "pc");
        key(&mut a, Key::Enter);
        a.composer.prompts.reconnect();
        assert!(!a.composer.prompts.pending[1].responding);
        a.apply_agent(M::PermissionsUpdated { session_id:"root".into(), requests:vec![] });
        assert_eq!(a.composer.prompts.pending.len(), 2);
        a.apply_agent(M::QuestionsUpdated { session_id:"child".into(), requests:vec![] });
        assert_eq!(a.composer.prompts.pending.len(), 1);
        a.apply_agent(M::PermissionReplyFailed { request_id:"pc".into(), error:"retry".into() });
        key(&mut a, Key::Enter);
        assert_eq!(last(&a)["ApproveTool"]["session_id"], "child");
    }

    #[test]
    fn unrelated_sessions_do_not_leak_and_reject_targets_specific_question() {
        let mut a = app(); family(&mut a);
        question(&mut a, "unrelated", "other", false);
        assert!(!a.has_prompt());
        question(&mut a, "grandchild", "q", false);
        key(&mut a, Key::Escape);
        assert_eq!(last(&a), json!({"RejectQuestion":{"session_id":"grandchild","request_id":"q"}}));
    }

    #[test]
    fn stale_click_cannot_approve_next_prompt() {
        let mut a = app(); permission(&mut a, "root", "p"); permission(&mut a, "root", "next");
        a.apply_agent(M::PermissionRemoved { session_id:"root".into(), request_id:"p".into() });
        a.reply_prompt(("root".into(), "p".into()), Some(0), false);
        assert!(a.agent_sent.borrow().is_empty());
    }
}