//! The focused view of a conversation: a turn reads as its prompt, one quiet work row and
//! the reply (t3code's "Worked for ..." row, `TimelineWorkGroupRows.tsx`).
//!
//! As t3code does (`deriveTurnFolds`: the running turn is never folded), a turn shows its work
//! while it runs and folds when it is done:
//!
//! - running, it is its notes as they are written and, between them, its calls as one line
//!   each (`steps_row`: the newest few of a run, the rest counted), then the thought it is
//!   having now, one at a time;
//! - done, everything it did on the way (calls, sub-agents, thinking, the notes between them)
//!   folds under the work row, which says how much there was: `14 steps · 3 files changed`.
//!   The reply stays. Pressing the row opens that turn as the full trace, cards and diffs,
//!   and pressing it again folds it.
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
    let running = matches!(message.status.trim().to_ascii_lowercase().as_str(), "pending" | "running" | "streaming");
    match message.kind {
        Kind::Subtask => running,
        Kind::Tool => tools::is_todos(message) || message.tool == "user_shell" || (message.tool == "task" && running),
        _ => false,
    }
}

/// A call the agent made: what the work row counts.
fn step(message: &HistoryMessage, index: usize) -> bool {
    matches!(message.kind, Kind::Tool | Kind::Subtask) && !kept(message) && tools::visible(message, index, 0)
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
            if open.contains(&messages[start].id) {
                // The whole trace, whenever the turn ran.
                super::timeline::rows_in(messages, body..end, body.min(live_from), &mut items);
            } else if running && end == messages.len() {
                unfolding(messages, body, end, live_from, &mut items);
            } else {
                folded(messages, body, end, live_from, &mut items);
            }
        }
        start = end;
    }
    if activity { items.push(RowItem::Activity); }
    items
}

/// The running turn: its notes, each run of calls between them as one row of lines, and the
/// thought being had.
fn unfolding(messages: &[HistoryMessage], body: usize, end: usize, live_from: usize, items: &mut Vec<RowItem>) {
    // The thought being had: the turn's newest row.
    let thought = (body..end).rev().find(|&i| messages[i].kind != Kind::Assistant || written(&messages[i]))
        .filter(|&i| messages[i].kind == Kind::Reasoning && tools::visible(&messages[i], i, 0));
    let mut run: Option<usize> = None;
    let mut close = |run: &mut Option<usize>, at: usize, items: &mut Vec<RowItem>| if let Some(from) = run.take() { items.push(RowItem::Steps(from, at)); };
    for i in body..end {
        let message = &messages[i];
        if step(message, i) { run.get_or_insert(i); continue; }
        // Thoughts and unwritten replies sit inside a run of calls without ending it.
        let shown = match message.kind {
            Kind::Assistant => written(message),
            Kind::Reasoning => thought == Some(i),
            Kind::Tool | Kind::Subtask => kept(message) && tools::visible(message, i, 0),
            _ => tools::visible(message, i, live_from),
        };
        if !shown { continue; }
        close(&mut run, i, items);
        items.push(if message.kind == Kind::Reasoning { RowItem::Thought(i) } else { RowItem::Message(i) });
    }
    close(&mut run, end, items);
}

fn folded(messages: &[HistoryMessage], body: usize, end: usize, live_from: usize, items: &mut Vec<RowItem>) {
    // The newest text and what was written with it: back to the call before it.
    let newest = (body..end).rev().find(|&i| written(&messages[i]));
    let text_from = newest.map(|newest| {
        let mut from = newest;
        for i in (body..newest).rev() {
            match messages[i].kind {
                Kind::Tool | Kind::Subtask => break,
                Kind::Assistant if written(&messages[i]) => from = i,
                _ => {}
            }
        }
        from
    });
    for i in body..end {
        let message = &messages[i];
        match message.kind {
            Kind::Assistant => if text_from.is_some_and(|from| i >= from) { items.push(RowItem::Message(i)); },
            Kind::Reasoning => {}
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

/// How many of a run's calls are listed; the ones before them are counted.
const LISTED: usize = 5;

/// The calls among `messages[start..end]` of a running turn, a line each: what it did, the
/// newest last. Cached until the turn's rows change.
pub fn steps_row(app: &App, session: &super::Session, start: usize, end: usize) -> El {
    let Some(first) = session.messages.get(start) else { return rune::el() };
    let key = format!("steps:{}", first.id);
    let version = (session.structure, end - start, app.pal as *const _ as usize, app.ty.size.to_bits());
    rune::memo(key.clone(), version, || {
        let p = app.pal;
        let s = app.ty.scale();
        let calls: Vec<&HistoryMessage> = (start..end.min(session.messages.len())).filter(|&i| step(&session.messages[i], i)).map(|i| &session.messages[i]).collect();
        let earlier = calls.len().saturating_sub(LISTED);
        let mut lines = rune::col().key(key.clone()).w_full().pt(10.0 * s).gap(2.0 * s);
        if earlier > 0 {
            lines = lines.child(text(format!("{earlier} earlier {}", if earlier == 1 { "step" } else { "steps" }))
                .font(app.ty.sans(12.0, 400.0)).color(rgb(p.muted).alpha(0.7)).h(20.0 * s).pl(21.0 * s));
        }
        for call in &calls[earlier..] {
            let status = call.status.trim().to_ascii_lowercase();
            let failed = matches!(status.as_str(), "error" | "failed");
            let running = matches!(status.as_str(), "pending" | "running" | "streaming");
            let ink = if failed { p.red } else if running { p.dim } else { p.muted };
            let label = tools::step_label(call);
            lines = lines.child(row().key(format!("step:{}", call.id)).w_full().h(22.0 * s).gap(8.0 * s).center_y()
                .child(crate::ui::feather::view(app, tools::tool_symbol(&call.tool), 13.0 * s, ink))
                .child(text(if running { format!("{label}\u{2026}") } else { label }).font(app.ty.sans(13.0, 400.0)).color(rgb(ink)).truncate().shrink()));
        }
        lines
    })
}

/// The work row of the turn `messages[start..end]`: what it came to, and a press away from
/// the trace. Cached until the turn's rows change.
pub fn work_row(app: &App, session_id: &str, session: &super::Session, start: usize, end: usize) -> El {
    let Some(turn) = session.messages.get(start..end).filter(|turn| !turn.is_empty()) else { return rune::el() };
    let id = turn[0].id.clone();
    let key = format!("work:{id}");
    let open = session.open_turns.contains(&id);
    let live = end == session.messages.len() && app.session_live(session_id);
    let version = (session.structure, end - start, open, live, app.pal as *const _ as usize, app.ty.size.to_bits());
    rune::memo(key.clone(), version, || {
        let p = app.pal;
        let s = app.ty.scale();
        let summary = Summary::of(turn);
        let lead = if live { "Working" } else if summary.steps > 0 { "Worked" } else { "" };
        let mut line = row().key("work-line").h(26.0 * s).px(8.0 * s).ml(-8.0 * s).gap(7.0 * s).radius(7.0 * s).center_y()
            .hover(rgb(p.fg).alpha(0.06)).cursor(rune::Cursor::Pointer)
            .child(crate::ui::feather::view(app, if open { crate::ui::feather::Icon::Down } else { crate::ui::feather::Icon::Right }, 12.0 * s, p.muted));
        if !lead.is_empty() { line = line.child(text(lead).font(app.ty.sans(13.0, 600.0)).color(rgb(p.dim))); }
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
    use RowItem::{Activity, Message, Steps, Thought, Work};

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
        assert_eq!(rows, [Message(0), Work(0, 7), Message(5), Message(6)]);
    }

    #[test]
    fn an_open_turn_is_the_whole_trace() {
        let rows = row_items(&turn(), usize::MAX, false, false, &HashSet::from(["ask".to_owned()]));
        assert_eq!(rows, [Message(0), Work(0, 7), Message(1), Message(2), Message(3), Message(4), Message(5), Message(6)]);
    }

    #[test]
    fn a_running_turn_shows_its_notes_its_calls_a_line_each_and_its_thought() {
        let messages = turn();
        let rows = |n: usize| row_items(&messages[..n], 0, true, true, &HashSet::new());
        // A call after the note, then the thought being had.
        assert_eq!(rows(4), [Message(0), Work(0, 4), Message(1), Steps(2, 3), Thought(3), Activity]);
        // The next call joins the run: the thought before it is no longer shown, so nothing parts them.
        assert_eq!(rows(5), [Message(0), Work(0, 5), Message(1), Steps(2, 5), Activity]);
        // The reply follows the calls.
        assert_eq!(rows(6), [Message(0), Work(0, 6), Message(1), Steps(2, 5), Message(5), Activity]);
        // Done, it folds to the work row and the reply.
        assert_eq!(row_items(&messages, 0, false, false, &HashSet::new()), [Message(0), Work(0, 7), Message(5), Message(6)]);
    }

    #[test]
    fn a_sub_agent_shows_while_it_works_and_folds_when_it_finishes() {
        let mut messages = turn();
        messages.truncate(5);
        messages[4].tool = "task".into();
        messages[4].status = "running".into();
        assert_eq!(row_items(&messages, 0, true, true, &HashSet::new()), [Message(0), Work(0, 5), Message(1), Steps(2, 4), Message(4), Activity]);
        assert_eq!(Summary::of(&messages).steps, 1, "it is counted once it is done");
        // The turn ended with the sub-agent still at work in the background: its card stays.
        assert_eq!(row_items(&messages, 0, true, false, &HashSet::new()), [Message(0), Work(0, 5), Message(1), Message(4), Activity]);
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
