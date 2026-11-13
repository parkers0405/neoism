//! The current persistent goal, with wall-clock age from its server-owned timestamps.
use super::{Kind, Picker};
use crate::{agent::timeline, app::App, tabs::TabKind, ui::theme::rgb};
use neoism_protocol::agent::{AgentClientMessage as Client, AgentServerMessage as Server};
use rune::{El, Tx, col, el, text};
use rune_kit::markdown::Markdown;
use serde::Deserialize;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub(crate) struct Goal {
    pub text: String,
    #[serde(default)]
    created: u64,
    #[serde(default)]
    updated: u64,
    #[serde(default)]
    paused: bool,
    #[serde(default = "active_status")]
    status: String,
    #[serde(default)]
    summary: String,
}

fn active_status() -> String { "active".into() }
fn now_ms() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis().min(u64::MAX as u128) as u64 }

impl Goal {
    fn terminal(&self) -> bool { matches!(self.status.as_str(), "complete" | "blocked") }
    fn label(&self) -> &str {
        match self.status.as_str() { "complete" => "Complete", "blocked" => "Blocked", _ if self.paused => "Paused", _ => "Active" }
    }
    fn elapsed(&self, now: u64) -> String {
        if self.created == 0 { return "Start time unavailable".into(); }
        let end = if self.terminal() { self.updated } else { now };
        let secs = end.saturating_sub(self.created) / 1000;
        let duration = if secs >= 86_400 { format!("{}d {}h {}m", secs / 86_400, secs / 3600 % 24, secs / 60 % 60) }
            else if secs >= 3600 { format!("{}h {}m {}s", secs / 3600, secs / 60 % 60, secs % 60) }
            else if secs >= 60 { format!("{}m {}s", secs / 60, secs % 60) }
            else { format!("{secs}s") };
        if self.terminal() { format!("Duration {duration}") }
        else if self.paused { format!("Set {duration} ago (includes pauses)") }
        else { format!("Elapsed {duration}") }
    }
}

pub(super) fn view(app: &App, composer: [f32; 4]) -> El {
    let picker = app.composer.picker.as_ref().expect("goals panel");
    let p = app.pal;
    let s = app.ty.scale();
    let goal = app.active_session().and_then(|id| app.sessions.get(&id)).and_then(|session| session.goal.as_ref());
    let mut content = col().w((composer[2] - 2.0 * s).max(0.0)).px(14.0 * s).py(12.0 * s).gap(14.0 * s);
    if let Some(error) = &picker.goal_error {
        content = content.child(text(error).font(app.ty.body()).color(rgb(p.red)).w_full().wrap());
    }
    if picker.loading && goal.is_none() && picker.goal_error.is_none() {
        let width = (composer[2] - 30.0 * s).max(0.0);
        for fraction in [0.34, 0.82, 0.65] {
            content = content.child(el().w(width * fraction).h(10.0 * s).radius(5.0 * s).fill(rgb(p.surface)));
        }
    } else if let Some(goal) = goal {
        let color = match goal.status.as_str() { "complete" => p.green, "blocked" => p.red, _ if goal.paused => p.muted, _ => p.blue };
        content = content.child(text(format!("{} · {}", goal.label(), goal.elapsed(now_ms())))
            .font(app.ty.small()).color(rgb(color)).w_full().wrap());
        for (name, source) in [("text", goal.text.as_str()), ("summary", goal.summary.as_str())] {
            if source.is_empty() { continue; }
            if name == "summary" { content = content.child(text(if goal.status == "blocked" { "Blocker" } else { "Summary" }).font(app.ty.small()).color(rgb(p.muted))); }
            let look = (app.pal as *const _ as usize, app.ty.size.to_bits(), app.ty.weight.to_bits(), composer[2].to_bits());
            content = content.child(rune::memo(format!("goal-{name}:{}", app.active_tab().id), (source, look), || {
                Markdown::new(source.to_owned()).highlights(crate::agent::prose::highlights(source, p))
                    .reveal(0.0).code_frame(timeline::code_frame(app)).selectable(true)
                    .on_link_tx(|a: &mut App, url: &str, _: &mut Tx| a.open_link(url))
                    .render(&timeline::markdown_style(app))
            }));
        }
    } else if picker.goal_error.is_none() {
        let message = if app.active_session().is_none() { "Open a conversation to view its goal." }
            else { "No goal set. Use /goal <description> to set one." };
        content = content.child(text(message).font(app.ty.body()).color(rgb(p.muted)).w_full().wrap());
    }
    super::panel::view(app, composer, "goals", "Current goal".into(), "Enter refresh · Esc close", content)
}

impl App {
    pub fn open_goals(&mut self) {
        let mut picker = Picker::new(Kind::Goals, "Current goal", Vec::new());
        if let Some(session_id) = self.active_session() {
            if self.daemon.is_some() {
                let request_id = self.composer.request_id("goal");
                picker.goal_request = Some(request_id.clone());
                picker.loading = true;
                self.agent_send(Client::GetGoal { session_id, request_id });
            } else { picker.goal_error = Some("Not connected to the workspace daemon.".into()); }
        }
        self.focus_picker(picker);
    }

    pub(crate) fn apply_goal_event(&mut self, message: &Server) -> Option<bool> {
        match message {
            Server::GoalSnapshot { session_id, request_id, goal, error } => {
                if self.active_session().as_deref() != Some(session_id.as_str()) { return Some(false); }
                let Some(picker) = self.composer.picker.as_mut().filter(|p| p.kind == Kind::Goals && p.goal_request.as_ref() == Some(request_id)) else { return Some(false) };
                picker.loading = false;
                picker.goal_request = None;
                picker.goal_error = error.clone();
                if error.is_none() {
                    match goal.clone().map(serde_json::from_value::<Goal>).transpose() {
                        Ok(goal) => self.sessions.entry(session_id.clone()).or_default().goal = goal,
                        Err(error) => picker.goal_error = Some(format!("Invalid goal response: {error}")),
                    }
                }
                Some(true)
            }
            Server::SessionEvent { session_id, kind, properties } if kind == "session.updated" => {
                let info = properties.get("info").filter(|v| v.is_object())?;
                let goal = info.get("goal").filter(|v| !v.is_null()).cloned().map(serde_json::from_value::<Goal>).transpose().ok()?;
                let session = self.sessions.entry(session_id.clone()).or_default();
                let changed = session.goal != goal;
                session.goal = goal;
                // A live mutation supersedes an in-flight HTTP snapshot, including a clear.
                if self.active_session().as_deref() == Some(session_id.as_str()) {
                    if let Some(picker) = self.composer.picker.as_mut().filter(|p| p.kind == Kind::Goals) {
                        picker.goal_request = None;
                        picker.goal_error = None;
                        picker.loading = false;
                        return Some(true);
                    }
                }
                Some(changed)
            }
            _ => None,
        }
    }

    /// One-second text updates only while this panel has a running wall-clock age.
    pub(crate) fn goals_tick(&mut self) -> (Option<Duration>, bool) {
        let running = self.active_session().and_then(|id| self.sessions.get(&id)).and_then(|s| s.goal.as_ref())
            .is_some_and(|goal| goal.created != 0 && !goal.terminal());
        let Some(picker) = self.composer.picker.as_mut().filter(|p| p.kind == Kind::Goals && running) else { return (None, false) };
        let now = now_ms();
        let changed = picker.goal_second != now / 1000;
        picker.goal_second = now / 1000;
        (Some(Duration::from_millis(1000 - now % 1000)), changed)
    }

    pub(crate) fn fixture_goals(&mut self, state: &str) {
        let id = "rune-goals-fixture".to_owned();
        self.open_tab(TabKind::Agent { session: Some(id.clone()) });
        if !matches!(state, "empty" | "loading") {
            let now = now_ms();
            self.sessions.entry(id).or_default().goal = Some(Goal {
                text: "Ship the **native Rune composer**\n\n- Match the timeline's Markdown styling\n- Keep task and goal panels above the input\n- Preserve `pasted text` without filling the draft".into(),
                created: now.saturating_sub(3_723_000), updated: now, paused: state == "paused",
                status: if state == "complete" { "complete" } else { "active" }.into(),
                summary: if state == "complete" { "Implemented the composer and verified the affected checks." } else { "" }.into(),
            });
        }
        let mut picker = Picker::new(Kind::Goals, "Current goal", Vec::new());
        picker.loading = state == "loading";
        self.focus_picker(picker);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn age_uses_persisted_creation_and_terminal_goals_stop_at_updated() {
        let mut goal: Goal = serde_json::from_value(serde_json::json!({"text":"Ship it", "created":1000, "updated":3_724_000})).unwrap();
        assert_eq!(goal.elapsed(62_000), "Elapsed 1m 1s");
        goal.paused = true;
        assert_eq!(goal.elapsed(62_000), "Set 1m 1s ago (includes pauses)");
        goal.status = "complete".into();
        assert_eq!(goal.elapsed(9_000_000), "Duration 1h 2m 3s");
        assert_eq!(goal.label(), "Complete");
        goal.created = 0;
        assert_eq!(goal.elapsed(9_000_000), "Start time unavailable");
    }

    #[test]
    fn live_clear_wins_over_an_older_inflight_snapshot() {
        let mut app = App::new();
        app.fixture_goals("active");
        let id = app.active_session().unwrap().to_owned();
        let picker = app.composer.picker.as_mut().unwrap();
        picker.goal_request = Some("old".into());
        picker.loading = true;
        assert!(app.apply_agent(Server::SessionEvent { session_id: id.clone(), kind:"session.updated".into(), properties: serde_json::json!({"info":{"id":id}}) }));
        assert!(app.sessions[&id].goal.is_none());
        assert!(!app.apply_agent(Server::GoalSnapshot { session_id:id.clone(),request_id:"old".into(),goal:Some(serde_json::json!({"text":"Stale goal"})),error:None }));
        assert!(app.sessions[&id].goal.is_none());
    }

    #[test]
    fn goal_clock_only_wakes_while_the_visible_goal_has_a_live_age() {
        let mut app = App::new();
        app.fixture_goals("active");
        let (wait, changed) = app.goals_tick();
        assert!(wait.is_some_and(|d| d > Duration::ZERO && d <= Duration::from_secs(1)));
        assert!(changed);
        app.close_prompt_picker();
        assert_eq!(app.goals_tick(), (None, false));
        app.fixture_goals("complete");
        assert_eq!(app.goals_tick(), (None, false));
    }
}