//! A scripted workload the app runs on itself (`NEOISM_RUNE_DRIVE`), for frame-pacing checks
//! of a real window against the real daemon that never take keyboard or pointer focus: the
//! window can sit on a spare (headless) output while someone keeps working.
//!
//! Steps are separated by `;` and run in order; each logs `[drive] <step> mono_us=<time>`
//! on stderr as it starts (CLOCK_MONOTONIC, the clock presentation feedback uses):
//!
//! - `ready`: wait for the active conversation's first history page (at most 10 s)
//! - `system:<saved id>` / `server:<saved id>`: join a real remote workspace
//! - `fixture:<file>`: load a transcript fixture for matched local/remote comparisons
//! - `sidebar`: show the conversation sidebar
//! - `wait:<seconds>`
//! - `wheel:<notches per second>:<seconds>`: mouse-wheel notches over the active
//!   conversation or file; a negative rate scrolls down
//! - `touch:<points per second>:<seconds>`: continuous trackpad motion sampled on polls; negative scrolls down
//! - `tablewheel:<notches per second>:<seconds>` / `tabletouch:<points per second>:<seconds>`:
//!   horizontal wheel/trackpad motion over the active Markdown file's table at source line 0;
//!   negative scrolls right (the `md-interactions` fixture starts with this table)
//! - `hand:<seconds>[:<flick|read|fight>[:<seed>]]`: on macOS, scroll as a hand on the trackpad
//!   does (`native/HumanScroll.swift`): real scroll events with finger and momentum phases,
//!   posted to the app's own event queue on their own clock, through AppKit and winit
//! - `keys:<text>:<per second>`: type `text` into the focused surface, a character at a time
//!   (`jjjj` walks a file's cursor, as a held key does)
//! - `alt:<up|down|left|right>`: dispatch an app-wide focus chord and log its target/material
//! - `prompt:<text>`: send `text` from the active composer (a real prompt)
//! - `stream:<file>:<seconds>`: stream the file's text into the active conversation as a
//!   reply, through the reducer the daemon's deltas take (nothing is sent to the daemon)
//! - `live`: show the active conversation's whole loaded history as the live turn, so every
//!   tool call, edit and thought in it is a row (a long agent run's transcript)
//! - `hold`: remember the active conversation; `stream` goes to it whatever tab shows
//! - `tab:<index>`: show the tab at this place in the strip
//! - `hover:<index>`: rest the pointer on the tab at this place in the strip (its title slides);
//!   `hover:chat` rests it on the sidebar row with the longest title
//! - `child`: open the first sub-agent at work under the active conversation (waits a minute for one)
//! - `treemenu:<row>`: open the right-click menu on that row of the showing file or notes tree
//! - `threads`: log the listed conversations (id, pinned, title)
//! - `dump:<file>`: write the active conversation's loaded rows to a file as text
//! - `rows`: log the active conversation's id and rows (kind, id, text length, text hash)
//! - `watch:<seconds>`: log the held (or active) conversation's scroll offset every frame
//! - `mark:<name>`
//! - `quit`
//!
//! The deltas enter the real host wheel handlers (`App::timeline_wheel`, `App::editor_wheel`,
//! or `editor::markdown::table_wheel`), so everything
//! after input dispatch is the real path.

use crate::app::App;
use crate::tabs::TabKind;
use neoism_protocol::agent::{AgentServerMessage, ContentKind, StreamingState};
use rune::{Tx, WheelEvent};
use std::collections::VecDeque;
use std::time::Duration;

enum Step {
    Ready,
    TreeMenu(usize),
    Wait(f64),
    Wheel { rate: f64, seconds: f64 },
    Touch { speed: f64, seconds: f64 },
    TableWheel { rate: f64, seconds: f64 },
    TableTouch { speed: f64, seconds: f64 },
    Hand { seconds: f64, profile: i32, seed: u64 },
    Keys { text: Vec<char>, rate: f64 },
    Alt(rune::Key),
    Prompt(String),
    /// Host a server on a folder, as submitting the Host form does.
    Host(String),
    JoinSystem(String),
    JoinServer(String),
    Fixture(String),
    Sidebar,
    Rail(crate::app::Sidebar),
    Stream { text: String, bounds: Vec<usize>, seconds: f64 },
    Live,
    Hold,
    Tab(usize),
    Hover(Option<usize>),
    Child,
    Rows,
    Threads,
    Dump(String),
    Watch(f64),
    Mark(String),
    Quit,
}

pub struct Drive {
    steps: VecDeque<Step>,
    /// When the current step started (frame clock) and how many deltas it has sent.
    started: Option<f64>,
    sent: u64,
    /// Elapsed precision-scroll time already delivered, independent of timer/display phase.
    sampled: f64,
    /// The conversation `hold` remembered.
    held: Option<String>,
}

/// Request samples while the scripted touch is active; the display owns actual frame timing.
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
                "tablewheel" => { let (rate, seconds) = pair()?; Step::TableWheel { rate, seconds } }
                "tabletouch" => { let (speed, seconds) = pair()?; Step::TableTouch { speed, seconds } }
                "hand" => {
                    let mut parts = rest.split(':');
                    let seconds = parts.next()?.parse().ok()?;
                    let profile = match parts.next() { None | Some("mixed") => 0, Some("flick") => 1, Some("read") => 2, Some("fight") => 3, _ => return None };
                    Step::Hand { seconds, profile, seed: parts.next().map_or(Some(7), |seed| seed.parse().ok())? }
                }
                "keys" => { let (text, rate) = rest.rsplit_once(':')?; Step::Keys { text: text.chars().collect(), rate: rate.parse().ok()? } }
                "alt" => Step::Alt(match rest { "up" => rune::Key::Up, "down" => rune::Key::Down, "left" => rune::Key::Left, "right" => rune::Key::Right, "e" | "n" | "c" => rune::Key::Text(rest.into()), _ => return None }),
                "prompt" => Step::Prompt(rest.to_owned()),
                "host" => Step::Host(rest.to_owned()),
                "system" => Step::JoinSystem(rest.to_owned()),
                "server" => Step::JoinServer(rest.to_owned()),
                "fixture" => Step::Fixture(std::fs::read_to_string(rest).ok()?),
                "sidebar" => Step::Sidebar,
                "rail" => Step::Rail(match rest { "files" => crate::app::Sidebar::Files, "notes" => crate::app::Sidebar::Notes, "agent" => crate::app::Sidebar::Conversations, _ => return None }),
                "stream" => {
                    let (path, seconds) = rest.rsplit_once(':')?;
                    let text = std::fs::read_to_string(path).ok()?;
                    let bounds = text.char_indices().map(|(i, _)| i).chain([text.len()]).collect();
                    Step::Stream { text, bounds, seconds: seconds.parse().ok()? }
                }
                "live" => Step::Live,
                "hold" => Step::Hold,
                "tab" => Step::Tab(rest.parse().ok()?),
                "hover" => Step::Hover(rest.parse().ok()),
                "watch" => Step::Watch(rest.parse().ok()?),
                "child" => Step::Child,
                "rows" => Step::Rows,
                "threads" => Step::Threads,
                "dump" => Step::Dump(rest.to_owned()),
                "treemenu" => Step::TreeMenu(rest.parse().ok()?),
                "mark" => Step::Mark(rest.to_owned()),
                "quit" => Step::Quit,
                _ => return None,
            })
        }).collect();
        (!steps.is_empty()).then_some(Drive { steps, started: None, sent: 0, sampled: 0.0, held: None })
    }
}

#[cfg(target_os = "macos")]
fn human_scroll(seconds: f64, seed: u64, profile: i32) {
    unsafe extern "C" { fn neoism_human_scroll(seconds: f64, seed: u64, profile: i32); }
    // SAFETY: called on the main thread (the poll); the Swift side only reads its arguments.
    unsafe { neoism_human_scroll(seconds, seed, profile) }
}

#[cfg(not(target_os = "macos"))]
fn human_scroll(_: f64, _: u64, _: i32) {}

fn touch_delta(speed: f64, seconds: f64, elapsed: f64, sampled: &mut f64) -> f32 {
    let through = elapsed.clamp(0.0, seconds.max(0.0));
    let delta = (through - *sampled).max(0.0) * speed;
    *sampled = sampled.max(through);
    delta as f32
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
                Step::TableWheel { rate, seconds } => format!("tablewheel:{rate}:{seconds}"),
                Step::TableTouch { speed, seconds } => format!("tabletouch:{speed}:{seconds}"),
                Step::Hand { seconds, profile, seed } => format!("hand:{seconds}:{profile}:{seed}"),
                Step::Keys { text, rate } => format!("keys:{}:{rate}", text.len()),
                Step::Alt(key) => format!("alt:{key:?}"),
                Step::Prompt(_) => "prompt".to_owned(),
                Step::Host(_) => "host".to_owned(),
                Step::JoinSystem(_) => "join-system".to_owned(),
                Step::JoinServer(_) => "join-server".to_owned(),
                Step::Fixture(_) => "fixture".to_owned(),
                Step::Sidebar => "sidebar".to_owned(),
                Step::Rail(panel) => format!("rail:{panel:?}"),
                Step::Stream { seconds, .. } => format!("stream:{seconds}"),
                Step::Live => "live".to_owned(),
                Step::Hold => "hold".to_owned(),
                Step::Tab(index) => format!("tab:{index}"),
                Step::Hover(index) => format!("hover:{index:?}"),
                Step::Watch(s) => format!("watch:{s}"),
                Step::Child => "child".to_owned(),
                Step::Rows => "rows".to_owned(),
                Step::Threads => "threads".to_owned(),
                Step::Dump(_) => "dump".to_owned(),
                Step::TreeMenu(row) => format!("treemenu:{row}"),
                Step::Mark(name) => format!("mark:{name}"),
                Step::Quit => "quit".to_owned(),
            };
            // How much history is loaded says what a span was measured against.
            let (messages, (offset, end)) = active_session(app).and_then(|id| app.sessions.get(&id).map(|s| (s.messages.len(), s.scroll.get()))).unwrap_or_default();
            eprintln!("[drive] {name} mono_us={} messages={messages} offset={offset:.1} end={end:.1} threads={}", crate::ui::fps::monotonic_us(std::time::Instant::now()), app.threads.len());
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
            Step::Alt(key) => {
                let event = rune::KeyEvent { key: key.clone(), mods: rune::Mods { alt: true, ..Default::default() } };
                let handled = crate::keys::on_key(app, &event, tx);
                changed |= handled;
                eprintln!("[drive] focus={:?} handled={handled} target={:?} glass={} native={}", app.focus, app.cursor_target(), app.cursor.liquid_glass, app.cursor.native_glass);
                None
            }
            Step::Wait(seconds) => (elapsed < *seconds).then(|| seconds - elapsed),
            Step::Hand { seconds, profile, seed } => {
                if fresh { human_scroll(*seconds, *seed, *profile); }
                (elapsed < *seconds).then(|| seconds - elapsed)
            }
            Step::Rail(panel) => {
                match panel {
                    crate::app::Sidebar::Files | crate::app::Sidebar::Notes => app.click_tree(*panel),
                    crate::app::Sidebar::Conversations => { app.toggle_conversations(); }
                }
                changed = true;
                None
            }
            Step::Wheel { rate, seconds } | Step::Touch { speed: rate, seconds }
            | Step::TableWheel { rate, seconds } | Step::TableTouch { speed: rate, seconds } => {
                let precise = matches!(step, Step::Touch { .. } | Step::TableTouch { .. });
                let table = matches!(step, Step::TableWheel { .. } | Step::TableTouch { .. });
                // Quantizing a continuous gesture to floor(elapsed * 120) causes zero/double
                // steps when the poll clock crosses the synthetic clock's phase. Preserve
                // distance at the actual sample time instead; real input is unchanged.
                let (dy, count) = if precise {
                    (touch_delta(*rate, *seconds, elapsed, &mut drive.sampled), 1)
                } else {
                    let due = (elapsed.min(*seconds) * rate.abs()) as u64;
                    let count = due.saturating_sub(drive.sent);
                    drive.sent = due;
                    (40.0 * rate.signum() as f32, count)
                };
                let line_h = crate::editor::view::line_height(app);
                let file = match &app.active_tab().kind { TabKind::Editor { path } => Some(path.clone()), _ => None };
                let chat = active_session(app);
                for _ in 0..count {
                    let mut e = if table && precise { WheelEvent::new(dy, 0.0) } else { WheelEvent::new(0.0, dy) }.precise(precise);
                    if table && !precise { e.mods.shift = true; }
                    match (&file, &chat) {
                        _ if table => { crate::editor::markdown::table_wheel(app, 0, e, tx); }
                        _ if app.active_tab().kind == TabKind::GitDiff => { app.git_review_wheel(&e, tx); }
                        (Some(path), _) => { app.editor_wheel(path, e, tx); }
                        (_, Some(id)) => { app.timeline_wheel(id, e, line_h, tx); }
                        _ => {}
                    }
                }
                (elapsed < *seconds).then(|| if precise { (seconds - elapsed).min(1.0 / TOUCH_HZ) }
                    else { ((drive.sent + 1) as f64 / rate.abs() - elapsed).max(0.0) })
            }
            Step::Prompt(text) => {
                let index = app.active;
                app.tabs[index].draft.replace_text(text.clone());
                app.submit();
                changed = true;
                None
            }
            Step::Host(directory) => {
                app.form = Some(crate::servers::form::Form::server(crate::servers::form::Draft { host: true, directory: directory.clone(), ..Default::default() }));
                app.submit_form(tx);
                changed = true;
                None
            }
            Step::Stream { text, bounds, seconds } => {
                // Deltas land a hundred times a second, as a fast model's do.
                const DELTA_HZ: f64 = 100.0;
                let Some(id) = drive.held.clone().or_else(|| active_session(app)) else { drive.steps.pop_front(); drive.started = None; continue };
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
            Step::JoinSystem(id) | Step::JoinServer(id) => {
                if fresh {
                    match step {
                        Step::JoinSystem(_) => app.connect_system(id, "Pacing check"),
                        _ => app.select_server(id),
                    }
                    changed = true;
                }
                if app.server.remote() && app.workspace_ready { None }
                else if elapsed >= 30.0 { eprintln!("[drive] join failed or timed out"); app.quit_requested = true; None }
                else { Some(0.05) }
            }
            Step::Sidebar => { app.sidebar = Some(crate::app::Sidebar::Conversations); changed = true; None }
            Step::Fixture(json) => {
                app.load_fixture("pacing-fixture", "Pacing fixture", json);
                changed = true;
                None
            }
            Step::Live => {
                if let Some(session) = active_session(app).and_then(|id| app.sessions.get_mut(&id)) { session.live_from = Some(0); }
                changed = true;
                None
            }
            Step::Hold => { drive.held = active_session(app); None }
            Step::Tab(index) => {
                if let Some(id) = app.tabs.get(*index).map(|t| t.id) { app.select_tab(id); }
                changed = true;
                None
            }
            Step::Hover(index) => {
                let title = match index {
                    Some(index) => app.tabs.get(*index).map(|tab| (format!("pane-tab:{}", tab.id), crate::ui::marquee::natural(&app.tab_title(tab), app.ty.tab()))),
                    None => app.threads.iter().max_by_key(|t| t.title.chars().count())
                        .map(|t| (format!("thread:{}", t.session_id), crate::ui::marquee::natural(&t.title, app.ty.body()))),
                };
                if let Some((key, width)) = title {
                    app.title_hover(&key, width, true);
                }
                changed = true;
                None
            }
            Step::Child => {
                let found = active_session(app).and_then(|root| {
                    let child = app.families.roster(&root).iter().find(|m| app.families.working_now(&m.id)).map(|m| m.id.clone())?;
                    Some((root, child))
                });
                match found {
                    Some((root, child)) => { app.open_child(&root, &child); changed = true; None }
                    None if elapsed < 60.0 => Some(0.1),
                    None => None,
                }
            }
            Step::Rows => {
                if let Some((id, session)) = active_session(app).and_then(|id| app.sessions.get(&id).map(|s| (id, s))) {
                    // Each row's kind, id, text length and a hash of its text: two windows on
                    // the same conversation can be compared row for row (`check-sleep-resume.py`).
                    let hash = |text: &str| { use std::hash::{Hash, Hasher}; let mut h = std::collections::hash_map::DefaultHasher::new(); text.hash(&mut h); h.finish() & 0xffff_ffff };
                    let rows: Vec<String> = session.messages.iter().map(|m| format!("{:?}:{}:{}:{:08x}", m.kind, m.id, m.text.len(), hash(&m.text))).collect();
                    eprintln!("[drive] rows session={id} streaming={} live_from={:?} connection={:?} {}", session.streaming, session.live_from, app.connection, rows.join(" "));
                }
                None
            }
            Step::Threads => {
                for thread in &app.threads { eprintln!("[drive] thread {} pinned={} {}", thread.session_id, thread.pinned, thread.title); }
                None
            }
            Step::Dump(path) => {
                if let Some(session) = active_session(app).and_then(|id| app.sessions.get(&id)) {
                    let text: String = session.messages.iter().map(|m| format!("=== {:?} {} {}\n{}\n\n", m.kind, m.id, m.title, m.text)).collect();
                    if let Err(error) = std::fs::write(path, text) { eprintln!("[drive] dump failed: {error}"); }
                }
                None
            }
            Step::Watch(seconds) => {
                let (offset, end) = drive.held.clone().or_else(|| active_session(app)).and_then(|id| app.sessions.get(&id).map(|s| s.scroll.get())).unwrap_or_default();
                eprintln!("[drive] watching t={elapsed:.4} offset={offset:.1} end={end:.1}");
                (elapsed < *seconds).then_some(1.0 / 240.0)
            }
            Step::TreeMenu(index) => {
                // Where the row is drawn: beside the rail, under the sidebar header.
                let y = 96.0 + 26.0 * *index as f32;
                app.open_tree_context(Some(*index), [150.0, y * app.ty.scale()]);
                changed = true;
                None
            }
            Step::Mark(_) => None,
            Step::Quit => { app.quit_requested = true; None }
        };
        match remaining {
            Some(wait) => { tx.wake_after(Duration::from_secs_f64(wait)); break; }
            None => { drive.steps.pop_front(); drive.started = None; drive.sent = 0; drive.sampled = 0.0; }
        }
    }
    if !drive.steps.is_empty() { app.drive = Some(drive); }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuous_touch_does_not_alternate_zero_and_double_steps_at_clock_boundaries() {
        let mut sampled = 0.0;
        let mut distance = 0.0;
        for frame in 1..=120 {
            let elapsed = frame as f64 / 120.0 + if frame % 2 == 0 { 0.0001 } else { -0.0001 };
            let delta = touch_delta(700.0, 1.0, elapsed, &mut sampled);
            assert!((5.6..6.1).contains(&delta), "uneven synthetic motion: {delta}");
            distance += delta;
        }
        assert!((distance - 700.0).abs() < 0.01);
        assert_eq!(touch_delta(700.0, 1.0, 1.5, &mut sampled), 0.0);
        // A delayed poll preserves the elapsed distance; the end does not overshoot.
        sampled = 0.0;
        assert_eq!(touch_delta(-700.0, 1.0, 0.5, &mut sampled), -350.0);
        assert_eq!(touch_delta(-700.0, 1.0, 1.5, &mut sampled), -350.0);
    }

    #[test]
    fn a_script_parses_into_its_steps_and_skips_what_it_cannot_read() {
        let parse = |script: &str| {
            // SAFETY: tests in this module are the only readers of this variable.
            unsafe { std::env::set_var("NEOISM_RUNE_DRIVE", script) };
            let drive = Drive::from_env();
            unsafe { std::env::remove_var("NEOISM_RUNE_DRIVE") };
            drive
        };
        let drive = parse("ready; wait:1.5; wheel:-30:2; touch:900:1; prompt:say hi: now; live; bogus; alt:up; alt:invalid; mark:end; tablewheel:-12:2; tabletouch:700:3; hand:2:flick:42; quit").unwrap();
        let names: Vec<&str> = drive.steps.iter().map(|step| match step {
            Step::Rail(_) => "rail",
            Step::Ready => "ready", Step::Wait(_) => "wait", Step::Wheel { .. } => "wheel", Step::Touch { .. } => "touch", Step::Keys { .. } => "keys",
            Step::TableWheel { .. } => "tablewheel", Step::TableTouch { .. } => "tabletouch",
            Step::Hand { .. } => "hand",
            Step::Prompt(_) => "prompt", Step::Host(_) => "host",
            Step::Alt(_) => "alt",
            Step::JoinSystem(_) => "system", Step::JoinServer(_) => "server", Step::Fixture(_) => "fixture", Step::Sidebar => "sidebar", Step::Stream { .. } => "stream", Step::Live => "live",
            Step::Hold => "hold", Step::Tab(_) => "tab", Step::Hover(_) => "hover", Step::Watch(_) => "watch", Step::Child => "child", Step::Rows => "rows", Step::Threads => "threads", Step::Dump(_) => "dump", Step::TreeMenu(_) => "treemenu", Step::Mark(_) => "mark", Step::Quit => "quit",
        }).collect();
        assert_eq!(names, ["ready", "wait", "wheel", "touch", "prompt", "live", "alt", "mark", "tablewheel", "tabletouch", "hand", "quit"]);
        assert!(matches!(&drive.steps[8], Step::TableWheel { rate, seconds } if *rate == -12.0 && *seconds == 2.0));
        assert!(matches!(&drive.steps[9], Step::TableTouch { speed, seconds } if *speed == 700.0 && *seconds == 3.0));
        assert!(matches!(&drive.steps[10], Step::Hand { seconds, profile, seed } if *seconds == 2.0 && *profile == 1 && *seed == 42));
        assert!(matches!(&drive.steps[6], Step::Alt(rune::Key::Up)));
        assert!(matches!(&drive.steps[2], Step::Wheel { rate, seconds } if *rate == -30.0 && *seconds == 2.0));
        // A prompt keeps its colons.
        assert!(matches!(&drive.steps[4], Step::Prompt(text) if text == "say hi: now"));
        assert!(parse("nothing here").is_none());
    }
}
