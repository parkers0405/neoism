//! The focused view of a conversation: a turn reads as its prompt, one quiet work row and
//! the reply (t3code's "Worked for ..." row, `TimelineWorkGroupRows.tsx`).
//!
//! A turn has two states, running or done, and the work row's arrow says which it is in:
//!
//! - folded (the arrow points right): none of its calls show. It is the work row, which says
//!   how much there was (`14 steps · 3 files changed`), everything the agent wrote (its notes
//!   on the way and the reply, in order) and, while it runs, the thought it is having now, one
//!   at a time. While it runs the row's `Working` shimmers and its dots count up;
//! - open (the arrow points down, after a press on the row): the whole trace, every call as
//!   its card with its output and diffs. Another press folds it.
//!
//! What a folded turn also keeps:
//!
//! - sub-agents still at work, each as its own card until it finishes;
//! - checklists, the reader's own shell commands, notices and compaction dividers.
//!
//! The running turn's activity line ("Crafting") names the step it is on (`step`), so the
//! view says what is happening without a row for each call. `appearance.agent-focused-view`
//! turns the whole thing off: every turn is then the full trace, as before.

use super::timeline::RowItem;
use super::tools;
use crate::app::App;
use crate::ui::theme::rgb;
use neoism_protocol::agent::{HistoryMessage, HistoryMessageKind as Kind};
use rune::{El, Tx, row, text};
use std::collections::HashSet;

/// Rows a folded turn keeps: a checklist, a command the reader ran themselves, and a
/// sub-agent still at work (t3code keeps live sub-agent cards out of its fold too,
/// `liveSubagentCardEntryIds`); a finished one folds with the rest.
fn kept(message: &HistoryMessage) -> bool {
    tools::live_subagent(message) || (message.kind == Kind::Tool && (tools::is_todos(message) || message.tool == "user_shell"))
}

/// A call the agent made: what the work row counts.
pub(super) fn step(message: &HistoryMessage, index: usize) -> bool {
    matches!(message.kind, Kind::Tool | Kind::Subtask) && !kept(message) && tools::visible(message, index, 0)
}

pub(super) fn last_turn_folded(session: &super::Session, index: usize) -> bool {
    let start = session.messages.iter().rposition(|m| m.kind == Kind::User).unwrap_or(0);
    index >= start && session.messages.get(start).is_some_and(|m| !session.open_turns.contains(&m.id))
}

/// Something that folds under the work row: a call, or a thought.
fn folds(message: &HistoryMessage, index: usize) -> bool {
    step(message, index) || (message.kind == Kind::Reasoning && tools::visible(message, index, 0))
}

fn written(message: &HistoryMessage) -> bool {
    message.kind == Kind::Assistant && !(message.text.trim().is_empty() && message.attachments.is_empty())
}

/// The transcript's rows with each turn folded, except those in `open` (by the id of the
/// turn's first message). `live_from` and `activity` are as for `timeline::row_items`.
pub fn row_items(messages: &[HistoryMessage], live_from: usize, activity: bool, running: bool, open: &HashSet<String>) -> Vec<RowItem> {
    let mut items = Vec::with_capacity(messages.len() / 2 + 2);
    let mut start = 0;
    while start < messages.len() {
        // A turn: a prompt (or a page's first rows, whose prompt is not loaded) and what
        // follows it up to the next prompt.
        let body = start + usize::from(messages[start].kind == Kind::User);
        let end = messages[body..].iter().position(|m| m.kind == Kind::User).map_or(messages.len(), |n| body + n);
        if body > start && tools::visible(&messages[start], start, live_from) { items.push(RowItem::Message(start)); }
        if !(body..end).any(|i| folds(&messages[i], i)) {
            super::timeline::rows_in(messages, body..end, live_from, &mut items);
        } else {
            items.push(RowItem::Work(start, end));
            // Two states, and the arrow says which: folded shows none of the calls, whether
            // the turn is running or done; pressed open, it is the whole trace.
            if open.contains(&messages[start].id) {
                super::timeline::rows_in(messages, body..end, body.min(live_from), &mut items);
            } else {
                folded(messages, body, end, live_from, running && end == messages.len(), &mut items);
            }
        }
        start = end;
    }
    if activity { items.push(RowItem::Activity); }
    items
}

fn folded(messages: &[HistoryMessage], body: usize, end: usize, live_from: usize, live: bool, items: &mut Vec<RowItem>) {
    // Hidden calls don't replace visible content. Keep the latest thought through the calls
    // it led to, until the next visible message takes its place or the turn ends.
    let thought = live.then(|| (body..end).rev().find(|&i| {
        let message = &messages[i];
        match message.kind {
            Kind::Assistant => written(message),
            Kind::Tool | Kind::Subtask => kept(message) && tools::visible(message, i, 0),
            _ => tools::visible(message, i, live_from.min(i)),
        }
    })).flatten().filter(|&i| messages[i].kind == Kind::Reasoning);
    for i in body..end {
        let message = &messages[i];
        match message.kind {
            // Everything the agent wrote stays, in order: only its calls and thoughts fold.
            Kind::Assistant => if tools::visible(message, i, live_from) { items.push(RowItem::Message(i)); },
            Kind::Reasoning => if thought == Some(i) { items.push(RowItem::Thought(i)); },
            Kind::Tool | Kind::Subtask => if kept(message) && tools::visible(message, i, 0) { items.push(RowItem::Message(i)); },
            _ => if tools::visible(message, i, live_from) { items.push(RowItem::Message(i)); },
        }
    }
}

/// What a turn's work came to.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub steps: usize,
    pub thoughts: usize,
    pub files: usize,
    pub failed: usize,
    pub running: bool,
}

impl Summary {
    /// Counted from the rows' own fields: no output or diff is read.
    pub fn of(messages: &[HistoryMessage]) -> Summary {
        let mut summary = Summary::default();
        let mut files = HashSet::new();
        for (index, message) in messages.iter().enumerate() {
            if message.kind == Kind::Reasoning && tools::visible(message, index, 0) { summary.thoughts += 1; }
            if !step(message, index) { continue; }
            summary.steps += 1;
            let status = message.status.trim().to_ascii_lowercase();
            summary.running |= matches!(status.as_str(), "pending" | "running" | "streaming");
            if matches!(status.as_str(), "error" | "failed") { summary.failed += 1; }
            else if tools::is_edit_tool(&message.tool) { files.insert(message.title.trim()); }
        }
        summary.files = files.len();
        summary
    }

    /// `14 steps · 3 files changed`; a turn that only thought reads `Thought`.
    pub fn label(&self) -> String {
        let count = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let mut parts = Vec::new();
        if self.steps > 0 { parts.push(count(self.steps, "step", "steps")); }
        if self.files > 0 { parts.push(count(self.files, "file changed", "files changed")); }
        if parts.is_empty() { parts.push(if self.thoughts > 1 { count(self.thoughts, "thought", "thoughts") } else { "Thought".into() }); }
        parts.join(" \u{b7} ")
    }
}

/// The step a running turn is on, for its activity line: the newest call's name and target.
pub fn current_step(messages: &[HistoryMessage]) -> Option<String> {
    const MAX: usize = 72;
    let message = messages.iter().rev().find(|m| m.kind != Kind::Assistant || written(m))?;
    if !matches!(message.kind, Kind::Tool | Kind::Subtask) || kept(message) { return None; }
    let label = tools::step_label(message);
    let label = label.trim();
    if label.is_empty() { return None; }
    Some(if label.chars().count() > MAX { format!("{}\u{2026}", label.chars().take(MAX - 1).collect::<String>().trim_end()) } else { label.to_owned() })
}

/// The work row of the turn `messages[start..end]`: what it came to, and a press away from
/// the trace. Cached until the turn's rows change.
pub fn work_row(app: &App, session_id: &str, session: &super::Session, start: usize, end: usize) -> El {
    let Some(turn) = session.messages.get(start..end).filter(|turn| !turn.is_empty()) else { return rune::el() };
    let id = turn[0].id.clone();
    let key = format!("work:{id}");
    let live = end == session.messages.len() && app.session_live(session_id);
    let open = session.open_turns.contains(&id);
    let version = (session.structure, end - start, open, live, app.pal as *const _ as usize, app.ty.size.to_bits());
    rune::memo(key.clone(), version, || {
        let p = app.pal;
        let s = app.ty.scale();
        let summary = Summary::of(turn);
        let lead = if live { "Working" } else if summary.steps > 0 { "Worked" } else { "" };
        let arrow = crate::ui::feather::view(app, if open { crate::ui::feather::Icon::Down } else { crate::ui::feather::Icon::Right }, 12.0 * s, p.muted).key("work-chevron");
        let arrow = if live && !open {
            let offset = session.work_completion.get_or_init(|| rune::Live::new([0.0, 0.0]));
            arrow.live_offset(offset.smooth(tools::HOP_SPRING))
        } else { arrow };
        let mut line = row().key("work-line").h(26.0 * s).px(8.0 * s).ml(-8.0 * s).gap(7.0 * s).radius(7.0 * s).center_y()
            .hover(rgb(p.fg).alpha(0.06)).cursor(rune::Cursor::Pointer)
            .child(arrow);
        if live {
            // Live values, stepped with the activity line: the cached row never rebuilds for them.
            line = line.child(row().items_end()
                .child(text(lead).font(app.ty.sans(13.0, 600.0)).live_text_color(&app.work_ink))
                .child(text("...").font(app.ty.sans(13.0, 600.0)).live_text(&app.work_dots).live_text_color(&app.work_ink)));
        } else if !lead.is_empty() { line = line.child(text(lead).font(app.ty.sans(13.0, 600.0)).color(rgb(p.dim))); }
        line = line.child(text(summary.label()).font(app.ty.sans(13.0, 400.0)).color(rgb(p.muted)));
        if summary.failed > 0 {
            line = line.child(text(format!("\u{b7} {} failed", summary.failed)).font(app.ty.sans(13.0, 400.0)).color(rgb(p.red).alpha(0.9)));
        }
        let (sid, turn_id, held) = (session_id.to_owned(), id.clone(), key.clone());
        // Padding, not margin: a virtual row's top margin is dropped from its measured height.
        row().key(key.clone()).w_full().pt(14.0 * s)
            .child(line.on_press_tx(move |a: &mut App, tx: &mut Tx| {
                let Some(session) = a.sessions.get_mut(&sid) else { return };
                if !session.open_turns.remove(&turn_id) { session.open_turns.insert(turn_id.clone()); }
                session.structure += 1;
                session.turn_released();
                // The row stays under the pointer while its turn opens or folds around it.
                tx.keep_on_screen(held.clone());
            }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use RowItem::{Activity, Message, Thought, Work};

    fn message(id: &str, kind: &str, text: &str) -> HistoryMessage {
        let tool = match kind { "Tool" => "bash", _ => "" };
        serde_json::from_value(serde_json::json!({
            "id": id, "role": if kind == "User" { "User" } else { "Assistant" }, "kind": kind, "title": id,
            "text": text, "tool": tool, "status": if kind == "Tool" { "completed" } else { "" }
        })).unwrap()
    }

    fn turn() -> Vec<HistoryMessage> {
        vec![
            message("ask", "User", "Fix it"),
            message("note", "Assistant", "Looking at the tests."),
            message("run", "Tool", ""),
            message("think", "Reasoning", "The failure is in the parser."),
            message("edit", "Tool", ""),
            message("done", "Assistant", "Fixed."),
            message("more", "Assistant", "And tidied."),
        ]
    }

    #[test]
    fn a_settled_turn_is_its_prompt_a_work_row_and_the_reply() {
        let rows = row_items(&turn(), usize::MAX, false, false, &HashSet::new());
        assert_eq!(rows, [Message(0), Work(0, 7), Message(1), Message(5), Message(6)]);
    }

    #[test]
    fn an_open_turn_is_the_whole_trace() {
        let rows = row_items(&turn(), usize::MAX, false, false, &HashSet::from(["ask".to_owned()]));
        assert_eq!(rows, [Message(0), Work(0, 7), Message(1), Message(2), Message(3), Message(4), Message(5), Message(6)]);
    }

    #[test]
    fn a_running_turn_shows_no_calls_until_it_is_opened() {
        let messages = turn();
        let rows = |n: usize| row_items(&messages[..n], 0, true, true, &HashSet::new());
        // Thinking: the note written before the call stays, with the thought.
        assert_eq!(rows(4), [Message(0), Work(0, 4), Message(1), Thought(3), Activity]);
        // A hidden call doesn't displace the thought.
        assert_eq!(rows(5), [Message(0), Work(0, 5), Message(1), Thought(3), Activity]);
        // The reply follows the note: both stay.
        assert_eq!(rows(6), [Message(0), Work(0, 6), Message(1), Message(5), Activity]);
        // Pressed open while it runs: every call.
        let open = row_items(&messages[..5], 0, true, true, &HashSet::from(["ask".to_owned()]));
        assert_eq!(open, [Message(0), Work(0, 5), Message(1), Message(2), Message(3), Message(4), Activity]);
    }

    #[test]
    fn a_folded_tool_completion_hops_only_the_chevron() {
        use neoism_protocol::agent::AgentServerMessage::MessageUpdated;

        let mut app = App::new();
        app.focused_view = true;
        app.load_fixture("work", "Work", r#"[{"id":"ask","kind":"User","role":"User","text":"Check it"}]"#);
        app.set_streaming("work", true);
        let update = |message| MessageUpdated { session_id: "work".into(), message, parent_message_id: None };
        let mut call = message("run", "Tool", "");
        call.status = "running".into();
        app.apply_agent(update(call.clone()));
        app.tick_activity();
        assert_eq!(app.activity.as_ref().unwrap().state.label(), "Crafting");
        let mut rt = rune::Runtime::new(app, |a: &App, _: &mut rune::Cx<App>| {
            let session = &a.sessions["work"];
            rune::col().w_full()
                .child(work_row(a, "work", session, 0, session.messages.len()))
                .child(a.activity.as_ref().unwrap().view(a))
        }, (1000.0, 180.0), rune::Env::new(true, 1.0));
        for _ in 0..30 { rt.frame(1.0 / 60.0); }
        let activity = rt.presented("activity").unwrap().0;
        let work = rt.presented("work:ask").unwrap().0;
        let y = rt.presented("work-chevron").unwrap().0[1];
        call.status = "completed".into();
        rt.update(|a, _| {
            a.apply_agent(update(call));
            assert!(!a.tick_activity().1, "completion must not restart the activity word");
        });
        let mut min_y = y;
        for _ in 0..90 {
            rt.frame(1.0 / 60.0);
            min_y = min_y.min(rt.presented("work-chevron").unwrap().0[1]);
            assert_eq!(rt.presented("activity").unwrap().0, activity);
            assert_eq!(rt.presented("work:ask").unwrap().0, work);
        }
        assert!(min_y < y - 0.5, "the chevron should hop: {min_y}");
        assert!((rt.presented("work-chevron").unwrap().0[1] - y).abs() < 0.1);
    }

    #[test]
    fn a_sub_agent_shows_while_it_works_and_folds_when_it_finishes() {
        let mut messages = turn();
        messages.truncate(5);
        messages[4].tool = "task".into();
        messages[4].status = "running".into();
        assert_eq!(row_items(&messages, 0, true, true, &HashSet::new()), [Message(0), Work(0, 5), Message(1), Message(4), Activity]);
        assert_eq!(Summary::of(&messages).steps, 1, "it is counted once it is done");
        messages[4].status = "completed".into();
        assert_eq!(row_items(&messages, 0, false, false, &HashSet::new()), [Message(0), Work(0, 5), Message(1)]);
        assert_eq!(Summary::of(&messages).steps, 2);
    }

    #[test]
    fn a_turn_without_work_has_no_work_row() {
        let messages = vec![message("ask", "User", "Hi"), message("hello", "Assistant", "Hello.")];
        assert_eq!(row_items(&messages, usize::MAX, false, false, &HashSet::new()), [Message(0), Message(1)]);
    }

    #[test]
    fn a_turn_that_ends_on_a_call_keeps_the_text_before_it() {
        let mut messages = turn();
        messages.truncate(5);
        assert_eq!(row_items(&messages, usize::MAX, false, false, &HashSet::new()), [Message(0), Work(0, 5), Message(1)]);
    }

    #[test]
    fn the_work_row_counts_calls_edits_and_failures() {
        let mut messages = turn();
        messages[4].tool = "apply_patch".into();
        messages[2].status = "error".into();
        let summary = Summary::of(&messages);
        assert_eq!(summary, Summary { steps: 2, thoughts: 1, files: 1, failed: 1, running: false });
        assert_eq!(summary.label(), "2 steps \u{b7} 1 file changed");
        assert_eq!(Summary::of(&[message("think", "Reasoning", "Hm.")]).label(), "Thought");
    }

    #[test]
    fn the_activity_line_names_the_newest_call() {
        let messages = turn();
        assert_eq!(current_step(&messages[..3]).as_deref(), Some("run"));
        assert_eq!(current_step(&messages[..4]), None, "a thought is shown as itself");
        assert_eq!(current_step(&messages), None, "the reply is being written");
    }
}
