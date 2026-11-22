//! A sent message opens its turn at the top of the view (t3code's new-turn anchor,
//! `useTimelineFollow.ts`): the transcript slides until the message rests just under the
//! viewport's top, and the reply streams into the room below it instead of pushing the
//! message up a line at a time.
//!
//! The room is a floor under the rows' height (`Session::turn_floor`), not padding that
//! shrinks as the reply grows: the content's height does not change until the turn has
//! filled the view, so nothing moves while it streams. The view is on its end when the
//! message is sent, so `stick_bottom` slides it into that room on Rune's own spring, with no
//! commit per frame. Past that:
//!
//! - a turn that runs tools or reasons is followed as before, once it has filled the view
//!   (t3code releases its anchor for tool activity);
//! - a reply of text alone stays where it is, its start in view, and runs on under the
//!   composer: the view is taken a point off its end so it is not followed, and the
//!   jump-to-latest button goes to the end.
//!
//! Wheel or trackpad input takes over at once; nothing here moves the view afterwards.

use crate::app::App;
use neoism_protocol::agent::HistoryMessageKind;
use rune::Tx;
use std::time::{Duration, Instant};

/// How far under the viewport's top the sent message rests (t3code's `CHAT_LIST_ANCHOR_OFFSET`).
const TOP: f32 = 16.0;
/// Room past the resting place: a view on its end slides this far past it, and stepping back
/// by it leaves a `stick_bottom` view off its end, where it is not followed.
const SLACK: f32 = 1.0;
/// How long after a send the slide is watched for on every frame.
const WATCH: Duration = Duration::from_secs(2);

/// The turn a sent message opened.
pub struct Turn {
    message: String,
    /// The message's row top in the transcript, from its last layout.
    top: Option<f32>,
    opened: Instant,
    /// Not yet seen by a frame.
    fresh: bool,
    /// The view rested at the top and was taken off its end.
    held: bool,
    /// Nothing more to decide: the turn runs tools, it ended, or the reader took over.
    settled: bool,
    /// The transcript structure tool activity was last looked for at.
    looked: Option<u64>,
    /// Where the view was at the last frame.
    last: f32,
}

impl super::Session {
    /// A message was sent into an idle conversation: it opens a turn at the top of the view.
    pub fn open_turn(&mut self, message_id: &str) {
        self.turn = Some(Turn {
            message: message_id.to_owned(), top: None, opened: Instant::now(),
            fresh: true, held: false, settled: false, looked: None, last: 0.0,
        });
    }

    /// The sent message's row was laid out with its top `top` points into the transcript.
    /// Returns whether the turn's room changes with it (the view re-runs for that alone).
    pub fn turn_row_at(&mut self, message_id: &str, top: f32) -> bool {
        let Some(turn) = self.turn.as_mut().filter(|t| t.message == message_id) else { return false };
        if turn.top.is_some_and(|was| (was - top).abs() <= 0.5) { return false; }
        turn.top = Some(top);
        true
    }

    /// The least the transcript's rows may be tall: enough for the open turn's message to rest
    /// `TOP` under the viewport's top. `None` once another message leads the newest turn.
    pub fn turn_floor(&self) -> Option<f32> {
        let turn = self.turn.as_ref()?;
        let top = turn.top.filter(|_| self.viewport_h > 0.0)?;
        let newest = self.messages.iter().rev().find(|m| m.kind == HistoryMessageKind::User)?;
        (newest.id == turn.message).then_some(top + self.viewport_h - TOP + SLACK)
    }

    /// The reader moved the transcript: the turn's view is theirs.
    pub fn turn_released(&mut self) {
        if let Some(turn) = &mut self.turn { turn.settled = true; }
    }

    /// Whether the open turn has run a tool or reasoned (looked for once per structure change).
    fn turn_works(&mut self) -> bool {
        let Some(turn) = self.turn.as_mut().filter(|t| t.looked != Some(self.structure)) else { return false };
        turn.looked = Some(self.structure);
        self.messages.iter().rev().take_while(|m| m.id != turn.message)
            .any(|m| matches!(m.kind, HistoryMessageKind::Tool | HistoryMessageKind::Subtask | HistoryMessageKind::Reasoning))
    }
}

impl App {
    /// Per frame, for a turn still open: take a reply of text alone off the view's end once
    /// its message rests at the top, and put a turn that runs tools back on it. Returns
    /// whether the slide to the top is still being watched for.
    pub fn turn_tick(&mut self, tx: &mut Tx) -> bool {
        let mut busy = false;
        let open = |s: &super::Session| s.shown && s.turn.as_ref().is_some_and(|t| !t.settled);
        let ids: Vec<String> = self.sessions.iter().filter(|(_, s)| open(s)).map(|(id, _)| id.clone()).collect();
        for id in ids {
            let key = format!("conversation:{id}");
            let Some(session) = self.sessions.get_mut(&id) else { continue };
            let (offset, max) = session.scroll.get();
            let works = session.turn_works();
            let live = session.streaming || session.activity_reveal.is_some();
            let Some(turn) = session.turn.as_mut() else { continue };
            let rested = (offset - turn.last).abs() < 0.01;
            turn.last = offset;
            // On its end, the message where the floor puts it.
            let at_top = max - offset <= 2.0 * SLACK && turn.top.is_some_and(|top| (top - TOP - offset).abs() <= 2.0 * SLACK);
            if std::mem::take(&mut turn.fresh) {
                busy = true;
                // Sent from up in the history: the turn opens at the end, as any other.
                if max - offset > 2.0 { self.jump_to_latest(&id, tx); }
            } else if works {
                turn.settled = true;
                if turn.held && at_top { tx.scroll_to_bottom(key); }
            } else if !turn.held && rested && at_top && max - offset <= 0.5 {
                turn.held = true;
                tx.scroll_to_instant(key, 0.0, max - SLACK);
            } else if !live {
                turn.settled = true;
            } else {
                busy |= !turn.held && turn.opened.elapsed() < WATCH;
            }
        }
        busy
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::timeline::pane;
    use neoism_protocol::agent::HistoryMessage;

    const VIEW: (f32, f32) = (1252.0, 674.0);

    fn frames(rt: &mut rune::Runtime<App>, n: usize) {
        for frame in 0..n {
            rt.update(|a, tx| {
                a.turn_tick(tx);
                a.scrollers.step(frame as f64 / 120.0 + 100.0, tx);
                a.place_conversations(tx);
            });
            rt.frame(1.0 / 120.0);
        }
    }

    fn runtime() -> rune::Runtime<App> {
        let mut app = App::new();
        app.load_fixture("send", "Send", include_str!("../../fixtures/tools-timeline.json"));
        let mut rt = rune::Runtime::new(app, |a: &App, cx: &mut rune::Cx<App>| pane(a, Some("send"), cx), VIEW, rune::Env::new(true, 1.0));
        frames(&mut rt, 120);
        rt.update(|a, _| {
            a.sessions.get_mut("send").unwrap().shown = true;
            a.turn_at_top = true;
            a.submit_prompt("send", "A message that opens its turn".into());
        });
        frames(&mut rt, 240);
        rt
    }

    /// The sent bubble's row top, below the transcript viewport's top.
    fn row_top(rt: &rune::Runtime<App>) -> f32 {
        let elements = rt.elements();
        let view = elements.iter().find(|e| e.key.as_deref() == Some("conversation:send")).unwrap().rect;
        let id = rt.state.sessions["send"].turn.as_ref().unwrap().message.clone();
        let row = elements.iter().find(|e| e.key.as_deref() == Some(&format!("msg:{id}"))).unwrap().rect;
        row[1] - view[1]
    }

    fn reply(rt: &mut rune::Runtime<App>, kind: &str, lines: usize) {
        rt.update(|a, _| {
            let session = a.sessions.get_mut("send").unwrap();
            let text = (0..lines).map(|i| format!("Line {i} of the reply.")).collect::<Vec<_>>().join("\n\n");
            let message: HistoryMessage = serde_json::from_value(serde_json::json!({
                "id": format!("reply-{kind}"), "role": "Assistant", "kind": kind, "title": "", "text": text,
                "tool": if kind == "Tool" { "bash" } else { "" }, "status": if kind == "Tool" { "completed" } else { "" }
            })).unwrap();
            match session.messages.iter().position(|m| m.id == message.id) {
                Some(i) => session.messages[i] = message,
                None => session.messages.push(message),
            }
            session.structure += 1;
            *session.message_versions.entry(format!("reply-{kind}")).or_default() += 1;
        });
    }

    #[test]
    fn a_sent_message_rests_at_the_top_of_the_view() {
        let rt = runtime();
        assert!((row_top(&rt) - TOP).abs() < 1.5, "the sent message rests {TOP} under the top, not at {}", row_top(&rt));
    }

    #[test]
    fn a_reply_fills_the_room_under_it_without_moving_it() {
        let mut rt = runtime();
        let before = row_top(&rt);
        for lines in [1, 3, 6] {
            reply(&mut rt, "Assistant", lines);
            frames(&mut rt, 30);
            assert!((row_top(&rt) - before).abs() < 1.0, "a reply of {lines} lines moved the message from {before} to {}", row_top(&rt));
        }
    }

    #[test]
    fn a_long_reply_of_text_alone_is_not_followed() {
        let mut rt = runtime();
        let before = row_top(&rt);
        reply(&mut rt, "Assistant", 80);
        frames(&mut rt, 120);
        assert!((row_top(&rt) - before).abs() < 1.0, "the start of a text reply stays in view: {before} -> {}", row_top(&rt));
        let (offset, max) = rt.state.sessions["send"].scroll.get();
        assert!(max - offset > 100.0, "the reply runs on below the view");
    }

    #[test]
    fn a_turn_that_runs_tools_is_followed_once_it_fills_the_view() {
        let mut rt = runtime();
        let before = row_top(&rt);
        reply(&mut rt, "Tool", 1);
        frames(&mut rt, 120);
        assert!((row_top(&rt) - before).abs() <= 2.0 * SLACK, "a short turn stays under the top: {before} -> {}", row_top(&rt));
        reply(&mut rt, "Assistant", 80);
        frames(&mut rt, 240);
        let (offset, max) = rt.state.sessions["send"].scroll.get();
        assert!(max - offset <= 2.0, "the view follows the turn's end: {offset} of {max}");
    }

    #[test]
    fn the_wheel_takes_the_view_back() {
        let mut rt = runtime();
        rt.update(|a, tx| { a.timeline_wheel("send", rune::WheelEvent::new(0.0, 40.0), 20.0, tx); });
        frames(&mut rt, 120);
        assert!(row_top(&rt) > TOP + 30.0, "a notch up moved the transcript: {}", row_top(&rt));
        let mut busy = true;
        rt.update(|a, tx| busy = a.turn_tick(tx));
        assert!(!busy, "nothing is placed after the reader took over");
    }
}
