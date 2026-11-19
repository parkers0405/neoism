//! Frozen fixtures; no daemon, random IDs or wall clock in screenshot baselines.
use super::*;
use neochat::protocol::Task;
pub fn load(app: &mut App, dense: bool) {
    app.fixture = true;
    app.local_name = "parker".into();
    app.profiles = profile::Profiles::default();
    app.now = 1_790_000_060;
    app.status = "fixture".into();
    app.page = Page::Tasks;
    let titles = ["Plan the desktop preview", "Verify reconnect and replay after a daemon restart",
        "Implement the native Rune shell", "Review task dependency handling", "Test two simultaneous claims",
        "Capture the narrow-window layout", "Ship authenticated board discovery", "Document CLI workflows",
        "Review thread composer", "Validate lease renewal", "Prepare the release checklist", "Run persistence checks"];
    let states = [TaskState::Ready, TaskState::Ready, TaskState::Claimed, TaskState::Waiting, TaskState::Blocked,
        TaskState::Ready, TaskState::Completed, TaskState::Failed, TaskState::Canceled];
    let messages = titles.iter().take(if dense { 12 } else { 2 }).enumerate().map(|(i, title)| Message {
        id: i as u64 + 1, channel: if i % 3 == 0 { "dev/rune" } else { "dev" }.into(), parent: None,
        author: if i % 2 == 0 { "parker" } else { "codex" }.into(), text: title.to_string(),
        labels: [("priority".into(), if i % 3 == 0 { "high" } else { "normal" }.into())].into(),
        created_at: 1_790_000_000,
        task: Some(Task { state: states[i % states.len()].clone(), needs: if i == 3 { vec![1, 3] } else { vec![] },
             assignee: (i == 2).then(|| "codex".into()), owner: (i == 2).then(|| "codex".into()), expires_at: (i == 2).then_some(app.now + 300),finished_by:(i==6).then(||"parker".into()),result:(i==6).then(||"Verified authenticated discovery.".into()) }),
    }).chain(std::iter::once(Message { id: 100, channel: "dev/rune".into(), parent: Some(1), author: "codex".into(),
        text: "Rune window is ready for a layout pass. Can someone check the narrow-window view?".into(), labels: BTreeMap::new(), created_at: 1_790_000_010, task: None })).collect();
    app.snapshot = Snapshot { cursor: 100, channels: vec!["dev".into(), "dev/rune".into(), "releases".into()], messages };
    app.channel = edit("dev");
    app.reactions = vec![Reaction { id: 1, channel: "dev/rune".into(), actor: "codex".into(), reaction: "+1".into() }];
    app.schedules = vec![ScheduledPost { id: 1, channel: "releases".into(), author: "parker".into(), text: "Desktop preview is ready for review.".into(), labels: BTreeMap::new(), publish_at: app.now + 3600 }];
    app.attachments.insert(1, vec![Attachment { id: 1, message_id: 1, name: "layout-notes.txt".into(), size: 128 }]);
    for name in &mut app.snapshot.channels {*name=projects::channel(app.project,name);}
    for m in &mut app.snapshot.messages {m.channel=projects::channel(app.project,&m.channel);}
    for s in &mut app.schedules {s.channel=projects::channel(app.project,&s.channel);}
    for r in &mut app.reactions {r.channel=projects::channel(app.project,&r.channel);}
    app.channel=edit(&projects::channel(app.project,"dev"));
    app.positions=app.snapshot.messages.iter().enumerate().map(|(i,m)|(m.id,i)).collect();
}