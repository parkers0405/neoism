//! A scripted workload the app runs on itself (`NEOISM_RUNE_DRIVE`), for frame-pacing checks
//! of a real window against the real daemon that never take keyboard or pointer focus: the
//! window can sit on a spare (headless) output while someone keeps working.
//!
//! Steps are separated by `;` and run in order; each logs `[drive] <step> mono_us=<time>`
//! on stderr as it starts (CLOCK_MONOTONIC, the clock presentation feedback uses):
//!
//! - `ready`: wait for the active conversation's first history page (at most 10 s)
//! - `wait:<seconds>`
//! - `wheel:<notches per second>:<seconds>`: mouse-wheel notches over the active
//!   conversation or file; a negative rate scrolls down
//! - `touch:<points per second>:<seconds>`: trackpad deltas at 120 Hz; negative scrolls down
//! - `keys:<text>:<per second>`: type `text` into the focused surface, a character at a time
//!   (`jjjj` walks a file's cursor, as a held key does)
//! - `prompt:<text>`: send `text` from the active composer (a real prompt)
//! - `stream:<file>:<seconds>`: stream the file's text into the active conversation as a
//!   reply, through the reducer the daemon's deltas take (nothing is sent to the daemon)
//! - `live`: show the active conversation's whole loaded history as the live turn, so every
//!   tool call, edit and thought in it is a row (a long agent run's transcript)
//! - `mark:<name>`
//! - `quit`
//!
//! The deltas enter where Rune's wheel handler does (`App::timeline_wheel`), so everything
//! after input dispatch is the real path.

use crate::app::App;
use crate::tabs::TabKind;
use neoism_protocol::agent::{AgentServerMessage, ContentKind, StreamingState};
use rune::{Tx, WheelEvent};
use std::collections::VecDeque;
use std::time::Duration;

enum Step {
    Ready,
    Wait(f64),
    Wheel { rate: f64, seconds: f64 },
    Touch { speed: f64, seconds: f64 },
    Keys { text: Vec<char>, rate: f64 },
    Prompt(String),
    Stream { text: String, bounds: Vec<usize>, seconds: f64 },
    Live,
    Mark(String),
    Quit,
}

pub struct Drive {
    steps: VecDeque<Step>,
    /// When the current step started (frame clock) and how many deltas it has sent.
    started: Option<f64>,
    sent: u64,
}

/// Trackpad deltas arrive at about this rate.
const TOUCH_HZ: f64 = 120.0;

impl Drive {
    pub fn from_env() -> Option<Drive> {
        let script = std::env::var("NEOISM_RUNE_DRIVE").ok()?;
        let steps: VecDeque<Step> = script.split(';').filter_map(|step| {
            let step = step.trim();
            let (name, rest) = step.split_once(':').unwrap_or((step, ""));
            let pair = || rest.split_once(':').and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)));
            Some(match name {
                "ready" => Step::Ready,
                "wait" => Step::Wait(rest.parse().ok()?),
                "wheel" => { let (rate, seconds) = pair()?; Step::Wheel { rate, seconds } }
                "touch" => { let (speed, seconds) = pair()?; Step::Touch { speed, seconds } }
                "keys" => { let (text, rate) = rest.rsplit_once(':')?; Step::Keys { text: text.chars().collect(), rate: rate.parse().ok()? } }
                "prompt" => Step::Prompt(rest.to_owned()),
                "stream" => {
                    let (path, seconds) = rest.rsplit_once(':')?;
                    let text = std::fs::read_to_string(path).ok()?;
                    let bounds = text.char_indices().map(|(i, _)| i).chain([text.len()]).collect();
                    Step::Stream { text, bounds, seconds: seconds.parse().ok()? }
                }
                "live" => Step::Live,
                "mark" => Step::Mark(rest.to_owned()),
                "quit" => Step::Quit,
                _ => return None,
            })
        }).collect();
        (!steps.is_empty()).then_some(Drive { steps, started: None, sent: 0 })
    }
}

fn active_session(app: &App) -> Option<String> {
    match &app.active_tab().kind { TabKind::Agent { session: Some(id) } => Some(id.clone()), _ => None }
}

/// Run the script up to `clock`. Returns whether the views must re-run.
pub fn tick(app: &mut App, clock: f64, tx: &mut Tx) -> bool {
    let Some(mut drive) = app.drive.take() else { return false };
    let mut changed = false;
    while let Some(step) = drive.steps.front() {
        let fresh = drive.started.is_none();
        let started = *drive.started.get_or_insert(clock);
        let elapsed = clock - started;
        if fresh {
            let name = match step {
                Step::Ready => "ready".to_owned(),
                Step::Wait(s) => format!("wait:{s}"),
                Step::Wheel { rate, seconds } => format!("wheel:{rate}:{seconds}"),
                Step::Touch { speed, seconds } => format!("touch:{speed}:{seconds}"),
                Step::Keys { text, rate } => format!("keys:{}:{rate}", text.len()),
                Step::Prompt(_) => "prompt".to_owned(),
                Step::Stream { seconds, .. } => format!("stream:{seconds}"),
                Step::Live => "live".to_owned(),
                Step::Mark(name) => format!("mark:{name}"),
                Step::Quit => "quit".to_owned(),
            };
            // How much history is loaded says what a span was measured against.
            let (messages, (offset, end)) = active_session(app).and_then(|id| app.sessions.get(&id).map(|s| (s.messages.len(), s.scroll.get()))).unwrap_or_default();
            eprintln!("[drive] {name} mono_us={} messages={messages} offset={offset:.1} end={end:.1}", crate::ui::fps::monotonic_us(std::time::Instant::now()));
        }
        // How long the step still runs; `None` once it is over.
        let remaining = match step {
            Step::Ready => {
                let loaded = match &app.active_tab().kind {
                    TabKind::Editor { path } => app.buffers.get(path).is_some_and(|b| b.loaded),
                    _ => active_session(app).and_then(|id| app.sessions.get(&id).map(|s| s.loaded)).unwrap_or(false),
                };
                (!loaded && elapsed < 10.0).then_some(0.05)
            }
            Step::Keys { text, rate } => {
                let due = ((elapsed * rate) as usize).min(text.len());
                while (drive.sent as usize) < due {
                    // `^` is Escape and `~` is Enter, so a script can leave insert mode and run `:w`.
                    let key = match text[drive.sent as usize] { '^' => rune::Key::Escape, '~' => rune::Key::Enter, c => rune::Key::Text(c.to_string()) };
                    let key = rune::KeyEvent { key, mods: Default::default() };
                    drive.sent += 1;
                    changed |= crate::keys::on_key(app, &key, tx);
                }
                (due < text.len()).then(|| ((drive.sent + 1) as f64 / rate - elapsed).max(0.0))
            }
            Step::Wait(seconds) => (elapsed < *seconds).then(|| seconds - elapsed),
            Step::Wheel { rate, seconds } | Step::Touch { speed: rate, seconds } => {
                let precise = matches!(step, Step::Touch { .. });
                let (hz, dy) = if precise { (TOUCH_HZ, (*rate / TOUCH_HZ) as f32) } else { (rate.abs(), 40.0 * rate.signum() as f32) };
                let due = (elapsed.min(*seconds) * hz) as u64;
                let line_h = crate::editor::view::line_height(app);
                let file = match &app.active_tab().kind { TabKind::Editor { path } => Some(path.clone()), _ => None };
                let chat = active_session(app);
                while drive.sent < due {
                    drive.sent += 1;
                    let e = WheelEvent::new(0.0, dy).precise(precise);
                    match (&file, &chat) {
                        (Some(path), _) => { app.editor_wheel(path, e, tx); }
                        (_, Some(id)) => { app.timeline_wheel(id, e, line_h, tx); }
                        _ => {}
                    }
                }
                (elapsed < *seconds).then(|| ((drive.sent + 1) as f64 / hz - elapsed).max(0.0))
            }
            Step::Prompt(text) => {
                let index = app.active;
                app.tabs[index].draft.replace_text(text.clone());
                app.submit();
                changed = true;
                None
            }
            Step::Stream { text, bounds, seconds } => {
                // Deltas land a hundred times a second, as a fast model's do.
                const DELTA_HZ: f64 = 100.0;
                let Some(id) = active_session(app) else { drive.steps.pop_front(); drive.started = None; continue };
                let message_id = format!("drive-stream-{started:.3}");
                if fresh {
                    app.apply_agent(AgentServerMessage::StreamingState { session_id: id.clone(), state: StreamingState::Generating, label: None });
                }
                let chars = bounds.len() - 1;
                let due = ((((elapsed / seconds).min(1.0) * DELTA_HZ * seconds).floor() / (DELTA_HZ * seconds)) * chars as f64) as usize;
                let sent = drive.sent as usize;
                if due > sent {
                    app.apply_agent(AgentServerMessage::ContentDelta {
                        session_id: id.clone(), message_id, parent_message_id: None, kind: ContentKind::Text,
                        text: text[bounds[sent]..bounds[due]].to_owned(),
                    });
                    drive.sent = due as u64;
                    // Shown on the stream's pace, as the daemon's deltas are.
                    app.stream_pace.pending = true;
                }
                if elapsed >= *seconds {
                    if let Some(session) = app.sessions.get_mut(&id) { session.streaming = false; session.status = None; }
                    changed = true;
                    None
                } else {
                    Some(1.0 / DELTA_HZ)
                }
            }
            Step::Live => {
                if let Some(session) = active_session(app).and_then(|id| app.sessions.get_mut(&id)) { session.live_from = Some(0); }
                changed = true;
                None
            }
            Step::Mark(_) => None,
            Step::Quit => { app.quit_requested = true; None }
        };
        match remaining {
            Some(wait) => { tx.wake_after(Duration::from_secs_f64(wait)); break; }
            None => { drive.steps.pop_front(); drive.started = None; drive.sent = 0; }
        }
    }
    if !drive.steps.is_empty() { app.drive = Some(drive); }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_script_parses_into_its_steps_and_skips_what_it_cannot_read() {
        let parse = |script: &str| {
            // SAFETY: tests in this module are the only readers of this variable.
            unsafe { std::env::set_var("NEOISM_RUNE_DRIVE", script) };
            let drive = Drive::from_env();
            unsafe { std::env::remove_var("NEOISM_RUNE_DRIVE") };
            drive
        };
        let drive = parse("ready; wait:1.5; wheel:-30:2; touch:900:1; prompt:say hi: now; live; bogus; mark:end; quit").unwrap();
        let names: Vec<&str> = drive.steps.iter().map(|step| match step {
            Step::Ready => "ready", Step::Wait(_) => "wait", Step::Wheel { .. } => "wheel", Step::Touch { .. } => "touch", Step::Keys { .. } => "keys",
            Step::Prompt(_) => "prompt", Step::Stream { .. } => "stream", Step::Live => "live", Step::Mark(_) => "mark", Step::Quit => "quit",
        }).collect();
        assert_eq!(names, ["ready", "wait", "wheel", "touch", "prompt", "live", "mark", "quit"]);
        assert!(matches!(&drive.steps[2], Step::Wheel { rate, seconds } if *rate == -30.0 && *seconds == 2.0));
        // A prompt keeps its colons.
        assert!(matches!(&drive.steps[4], Step::Prompt(text) if text == "say hi: now"));
        assert!(parse("nothing here").is_none());
    }
}
