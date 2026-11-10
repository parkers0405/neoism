//! A bounded fixture replay through the normal ContentDelta reducer. It makes window
//! performance checks repeatable without sending a prompt or charging an account.
use crate::app::App;
use neoism_protocol::agent::{AgentServerMessage, ContentKind};

pub struct Replay {
    source: String,
    boundaries: Vec<usize>,
    sent: usize,
    start: Option<f64>,
}

pub fn start(app: &mut App) {
    app.load_fixture(
        "fixture-stream",
        "Streaming replay",
        include_str!("../fixtures/code-timeline.json"),
    );
    let session = app.sessions.get_mut("fixture-stream").unwrap();
    let message = session.messages.last_mut().unwrap();
    message.status.clear();
    let source = std::mem::take(&mut message.text);
    let boundaries = source
        .char_indices()
        .map(|(i, _)| i)
        .chain([source.len()])
        .collect();
    session.streaming = true;
    session.live_from = Some(1);
    app.replay = Some(Replay {
        source,
        boundaries,
        sent: 0,
        start: None,
    });
}

pub fn tick(app: &mut App, clock: f64, tx: &mut rune::Tx) -> bool {
    let Some(replay) = &mut app.replay else {
        return false;
    };
    let start = *replay.start.get_or_insert(clock);
    let progress = ((clock - start) / 5.0).clamp(0.0, 1.0);
    let chars = replay.boundaries.len() - 1;
    let target = (progress * chars as f64) as usize;
    tx.wake_after(std::time::Duration::from_millis(33));
    if target <= replay.sent {
        return false;
    }
    let text = replay.source[replay.boundaries[replay.sent]..replay.boundaries[target]]
        .to_owned();
    replay.sent = target;
    let done = target == chars;
    app.apply_agent(AgentServerMessage::ContentDelta {
        session_id: "fixture-stream".into(),
        message_id: "a1".into(),
        parent_message_id: None,
        kind: ContentKind::Text,
        text,
    });
    if done {
        app.replay = None;
        app.sessions.get_mut("fixture-stream").unwrap().streaming = false;
    }
    true
}
