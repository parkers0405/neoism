//! Workspace workflow catalog and inspection over the daemon's Agent v2 HTTP proxy.
use crate::{app::App, daemon, ui::theme::rgb};
use rune::{Color, El, Key, KeyEvent, ScrollSpec, TextEdit, Tx, col, el, row, text};
use serde::Deserialize;
use std::time::Duration;
use tokio::sync::oneshot;

const PLUGIN: &str = "dev.neoism.workflows";
const LIST: &str = "workflow-list";
const MAX_BODY: usize = 8 * 1024 * 1024;

#[derive(Default, Deserialize)]
struct Catalog {
    workflows: Vec<Workflow>,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Diagnostic {
    source_path: String,
    message: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Workflow {
    definition: Definition,
    source_path: String,
    active: bool,
    source_hash: Option<String>,
    source_origin: Option<String>,
    #[serde(default)]
    can_activate: bool,
    #[serde(rename = "activationID")]
    activation_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Operation { Activate, History, Pause, Run, Source }

impl Operation {
    fn label(self) -> &'static str {
        match self { Self::Activate => "Activate", Self::History => "History", Self::Pause => "Pause", Self::Run => "Run now", Self::Source => "Open source" }
    }
}

#[derive(Clone, serde::Serialize)]
struct Command { operation: Operation, id: String, revision: String, source: String, origin: Option<String> }

#[derive(Deserialize)]
struct RunSummary { id: String, status: String, created: u64 }

enum Reply { Catalog(Catalog), History(String, Vec<RunSummary>), Activated(String, String), Paused(String), Accepted(String), Source(String) }

#[derive(Deserialize)]
struct Definition {
    id: String,
    name: String,
    prompt: String,
    directory: Option<String>,
    agent: Option<String>,
    skill: Option<String>,
    model: Option<Model>,
    schedule: Schedule,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Model { provider_id: String, id: String }

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Schedule {
    frequency: String,
    interval: u32,
    timezone: String,
    cron: Option<String>,
    at: Option<String>,
    date: Option<String>,
    time: Option<String>,
    weekdays: Option<Vec<String>>,
    month_day: Option<u32>,
    minute: Option<u32>,
}

impl Schedule {
    fn label(&self) -> String {
        let mut parts = vec![if self.interval > 1 {
            format!("{} (every {})", self.frequency, self.interval)
        } else { self.frequency.clone() }];
        parts.extend([&self.cron, &self.at, &self.date, &self.time].into_iter().flatten().cloned());
        if let Some(days) = &self.weekdays { if !days.is_empty() { parts.push(days.join(", ")); } }
        if let Some(day) = self.month_day { parts.push(format!("day {day}")); }
        if let Some(minute) = self.minute { parts.push(format!("minute {minute:02}")); }
        parts.push(self.timezone.clone());
        parts.join(" · ")
    }
}

impl Workflow {
    fn status(&self) -> &'static str {
        if self.active { "Active" }
        else if self.activation_id.is_some() { "Paused" }
        else { "Inactive" }
    }
}

#[cfg(test)]
async fn fetch_catalog(base: (url::Url, Option<String>)) -> Result<Catalog, String> {
    match fetch(base, None).await? { Reply::Catalog(catalog) => Ok(catalog), _ => unreachable!() }
}

async fn fetch(base: (url::Url, Option<String>), command: Option<Command>) -> Result<Reply, String> {
        let (mut url, token) = base;
        if !matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "localhost")) {
            return Err("Workflows are available only on the local daemon.".into());
        }
        let mut path = url.path_segments_mut().map_err(|_| "Invalid workflow endpoint")?;
        path.extend(["v2", "plugins", PLUGIN, if command.is_some() { "local-prompt" } else { "local-catalog" }]);
        drop(path);
        let client = reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(125)).build().map_err(|_| "Could not start the workflow client")?;
        let mut request = if let Some(command) = &command {
            client.post(url).header("content-type", "application/json").body(serde_json::to_vec(command).map_err(|_| "Invalid workflow request")?)
        } else { client.get(url) };
        if let Some(token) = token { request = request.bearer_auth(token); }
        let mut response = request.header("accept", "application/json").send().await
            .map_err(|e| if e.is_timeout() { "Workflow request timed out." } else { "Could not reach the daemon's workflow API." })?;
        let status = response.status();
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| "Could not read the workflow response.")? {
            if chunk.len() > MAX_BODY - body.len() { return Err("The workflow response is too large.".into()); }
            body.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            let parsed = serde_json::from_slice::<serde_json::Value>(&body).ok();
            if status == reqwest::StatusCode::FORBIDDEN
                && parsed.as_ref().and_then(|v| v["code"].as_str()) == Some("auth.hosted_route_forbidden")
            {
                return Err("403: This Agent does not allow workflows through the workspace daemon. The workspace proxy needs backend support for Workflow access.".into());
            }
            let message = parsed.as_ref().and_then(|v| v["message"].as_str().or_else(|| v["error"]["message"].as_str()))
                .or_else(|| std::str::from_utf8(&body).ok().filter(|s| !s.trim_start().starts_with(['<', '{'])))
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(if status.as_u16() == 404 { "Workspace or Workflows API not available" } else { "Workflow request rejected" });
            return Err(format!("{}: {}", status.as_u16(), message.trim().chars().take(350).collect::<String>()));
        }
        let invalid = || "Invalid response from the Workflows API.".to_owned();
        let Some(command) = command else { return serde_json::from_slice(&body).map(Reply::Catalog).map_err(|_| invalid()); };
        let value: serde_json::Value = serde_json::from_slice(&body).map_err(|_| invalid())?;
        match command.operation {
            Operation::Activate if value["active"] == true => value["activationID"].as_str().map(|id| Reply::Activated(command.id, id.into())).ok_or_else(invalid),
            Operation::History => serde_json::from_value(value["runs"].clone()).map(|runs| Reply::History(command.id, runs)).map_err(|_| invalid()),
            Operation::Pause if value["active"] == false => Ok(Reply::Paused(command.id)),
            Operation::Run => value["runID"].as_str().map(|id| Reply::Accepted(id.to_owned())).ok_or_else(invalid),
            Operation::Source => value["path"].as_str().map(|path| Reply::Source(path.to_owned())).ok_or_else(invalid),
            _ => Err(invalid()),
        }
}

pub struct Panel {
    workspace: String,
    root: String,
    pub query: TextEdit,
    catalog: Option<Catalog>,
    selected: usize,
    pending: Option<oneshot::Receiver<Result<Reply, String>>>,
    mutation_pending: bool,
    confirmation: Option<Command>,
    history: Option<(String, Vec<RunSummary>)>,
    notice: Option<String>,
    error: Option<String>,
}

impl Panel {
    fn new(app: &App) -> Self {
        Self { workspace: app.workspace_id.clone(), root: app.cwd.clone(), query: TextEdit::default(),
            catalog: None, selected: 0, pending: None, mutation_pending: false, confirmation: None, history: None, notice: None, error: None }
    }

    fn matching(&self) -> Vec<&Workflow> {
        let query = self.query.text.trim().to_lowercase();
        self.catalog.iter().flat_map(|c| &c.workflows).filter(|w| {
            query.is_empty() || w.definition.name.to_lowercase().contains(&query) || w.definition.id.to_lowercase().contains(&query)
        }).collect()
    }

    fn apply(&mut self, reply: Result<Catalog, String>) {
        match reply {
            Ok(catalog) => {
                let previous = self.matching().get(self.selected).map(|w| w.definition.id.clone());
                self.catalog = Some(catalog);
                self.selected = self.matching().iter().position(|w| Some(&w.definition.id) == previous.as_ref()).unwrap_or(0);
            }
            Err(error) => self.error = Some(error),
        }
    }

    fn command(&self, operation: Operation) -> Option<Command> {
        let matching = self.matching();
        let workflow = matching.get(self.selected)?;
        Some(Command { operation, id: workflow.definition.id.clone(), revision: format!("sha256:{}", workflow.source_hash.as_deref()?), source: workflow.source_path.clone(), origin: workflow.source_origin.clone() })
    }
}

impl App {
    pub fn open_workflows(&mut self) {
        self.workflows = Some(Panel::new(self));
        self.workflow_request();
    }

    fn workflow_request(&mut self) {
        self.workflow_send(None);
    }

    fn workflow_send(&mut self, command: Option<Command>) {
        let Some(panel) = &mut self.workflows else { return };
        if panel.pending.is_some() || panel.workspace != self.workspace_id || panel.root != self.cwd { return; }
        panel.error = None;
        panel.notice = None;
        if self.server.remote() {
            panel.error = Some("Workflows are available only on the local daemon.".into());
            return;
        }
        let Some(daemon) = self.daemon.as_ref().filter(|_| self.workspace_ready && self.pending_cd.is_none()) else {
            panel.error = Some("Workspace connection is not ready. Refresh after it connects.".into());
            return;
        };
        let target = daemon.http_target();
        let workspace = panel.workspace.clone();
        let wake = self.waker.clone();
        let mutation = command.as_ref().is_some_and(|command| matches!(command.operation, Operation::Activate | Operation::Pause | Operation::Run));
        let (mut send, receive) = oneshot::channel();
        match std::thread::Builder::new().name("rune-workflows".into()).spawn(move || {
            let result = daemon::agent_http_endpoint(target.as_ref(), &workspace).and_then(|base| {
                let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|_| "Could not start the workflow client")?;
                runtime.block_on(async {
                    if mutation { return fetch(base, command).await; }
                    tokio::select! {
                        result = fetch(base, command) => result,
                        () = send.closed() => Err("Workflow request cancelled.".into()),
                    }
                })
            });
            if send.send(result).is_ok() && let Some(wake) = wake { wake(); }
        }) {
            Ok(_) => { panel.pending = Some(receive); panel.mutation_pending = mutation; },
            Err(_) => panel.error = Some("Could not start the workflow worker.".into()),
        }
    }

    pub fn poll_workflows(&mut self) -> bool {
        let Some(panel) = &mut self.workflows else { return false };
        if panel.workspace != self.workspace_id || panel.root != self.cwd {
            self.workflows = None;
            return true;
        }
        let Some(pending) = &mut panel.pending else { return false };
        let reply = match pending.try_recv() {
            Ok(reply) => reply,
            Err(oneshot::error::TryRecvError::Empty) => return false,
            Err(oneshot::error::TryRecvError::Closed) => Err("The workflow worker stopped unexpectedly.".into()),
        };
        panel.pending = None;
        let mutation = std::mem::take(&mut panel.mutation_pending);
        match reply {
            Ok(Reply::Catalog(catalog)) => { panel.history = None; panel.apply(Ok(catalog)); }
            Ok(Reply::History(id, runs)) => panel.history = Some((id, runs)),
            Ok(Reply::Activated(id, activation)) => {
                if let Some(workflow) = panel.catalog.as_mut().and_then(|c| c.workflows.iter_mut().find(|w| w.definition.id == id)) {
                    workflow.active = true;
                    workflow.activation_id = Some(activation);
                }
                panel.notice = Some("Workflow activated for this source revision".into());
            }
            Ok(Reply::Paused(id)) => {
                if let Some(workflow) = panel.catalog.as_mut().and_then(|c| c.workflows.iter_mut().find(|w| w.definition.id == id)) { workflow.active = false; }
                panel.notice = Some("Workflow paused".into());
            }
            Ok(Reply::Accepted(id)) => panel.notice = Some(format!("Run accepted · {id}")),
            Ok(Reply::Source(path)) => {
                if !path.is_empty() && std::path::Path::new(&path).components().all(|p| matches!(p, std::path::Component::Normal(_))) {
                    self.workflows = None;
                    self.open_file(&path, None);
                } else { panel.error = Some("Invalid workspace source path".into()); }
            }
            Err(error) => panel.error = Some(if mutation { format!("{error} Refresh or check History before retrying.") } else { error }),
        }
        true
    }

    fn close_workflows(&mut self) {
        if self.workflows.as_ref().is_none_or(|panel| !panel.mutation_pending) { self.workflows = None; }
    }

    fn workflow_operation(&mut self, operation: Operation) {
        let Some(panel) = self.workflows.as_mut().filter(|panel| panel.pending.is_none()) else { return };
        let Some(command) = panel.command(operation) else { return };
        if !matches!(operation, Operation::Activate | Operation::Pause | Operation::Run) { self.workflow_send(Some(command)); return; }
        let label = operation.label();
        let name = panel.matching()[panel.selected].definition.name.clone();
        let effect = match operation {
            Operation::Activate => "Enables the schedule for this exact prompt revision. A source change requires confirmation again.",
            Operation::Pause => "Stops future scheduled runs. Already-started runs are unaffected.",
            _ => "Runs this exact prompt revision once without enabling a schedule.",
        };
        panel.confirmation = Some(command);
        use crate::ui::modal::{Action, Button, Modal};
        self.modal = Some(Modal { title: format!("{label}: {name}?"), body: effect.into(), selected: 0,
            buttons: vec![Button { label: "Cancel".into(), key: "Esc".into(), action: Action::Close },
                Button { label: label.into(), key: String::new(), action: Action::Workflow(operation) }] });
    }

    pub fn confirm_workflow_operation(&mut self, operation: Operation) {
        let command = self.workflows.as_mut().and_then(|panel| panel.confirmation.take()).filter(|command| command.operation == operation);
        if let Some(command) = command { self.workflow_send(Some(command)); }
    }

    pub fn workflows_key(&mut self, event: &KeyEvent, tx: &mut Tx) -> bool {
        if event.mods.ctrl || event.mods.alt || event.mods.logo || event.mods.cmd { return false; }
        let Some(panel) = &mut self.workflows else { return false };
        match event.key {
            Key::Escape => self.close_workflows(),
            Key::Up | Key::Down => {
                let count = panel.matching().len();
                panel.selected = if event.key == Key::Up { panel.selected.saturating_sub(1) }
                    else { (panel.selected + 1).min(count.saturating_sub(1)) };
                if count > 0 { tx.scroll_into_view(format!("workflow-row:{}", panel.selected)); }
            }
            _ => return false,
        }
        true
    }
}

fn button(app: &App, label: &'static str, enabled: bool, press: impl Fn(&mut App) + 'static) -> El {
    let (p, s) = (app.pal, app.ty.scale());
    let button = el().key(format!("workflow-button:{label}")).h(28.0 * s).px(9.0 * s).center().radius(6.0 * s).fill(rgb(p.surface))
        .child(text(label).font(app.ty.sans(12.0, 500.0)).color(rgb(if enabled { p.fg } else { p.muted })));
    if enabled { button.hover(rgb(p.hover)).cursor(rune::Cursor::Pointer).on_press(press) } else { button }
}

fn display_source<'a>(root: &str, source: &'a str) -> &'a str {
    std::path::Path::new(source).strip_prefix(root).ok()
        .and_then(std::path::Path::to_str).filter(|path| !path.is_empty()).unwrap_or(source)
}

pub fn view(app: &App) -> El {
    let Some(panel) = &app.workflows else { return el() };
    let (p, s) = (app.pal, app.ty.scale());
    let width = (820.0 * s).min((app.window[0] - 24.0).max(1.0));
    let height = (434.0 * s).min((app.window[1] - 36.0).max(1.0));
    let busy = panel.pending.is_some();
    let matching = panel.matching();
    let caption = |value: String| text(value).font(app.ty.chat_sized(12.0)).color(rgb(p.dim)).wrap().w_full();
    let header = row().w_full().px(14.0 * s).py(10.0 * s).gap(8.0 * s).center_y()
        .border_sides([0.0, 0.0, 1.0, 0.0], rgb(p.border))
        .child(col().grow(1.0).shrink().min_w(0.0).gap(2.0 * s)
            .child(text("Workflows").font(app.ty.sans(16.0, 600.0)).color(rgb(p.fg)))
            .child(text(panel.root.clone()).font(app.ty.sized(11.0)).color(rgb(p.dim)).truncate()))
        .child(button(app, "Refresh", !busy, App::workflow_request))
        .child(button(app, "Close", !panel.mutation_pending, App::close_workflows));

    let filter = rune::text_input(&panel.query, app.ty.chat_sized(13.0)).key("workflow-filter").autofocus()
        .placeholder("Filter workflows…", rgb(p.dim)).color(rgb(p.fg)).w_full()
        .on_edit(|a: &mut App| &mut a.workflows.as_mut().expect("workflow panel open").query)
        .on_input_change(|a: &mut App, tx: &mut Tx| {
            if let Some(panel) = &mut a.workflows { panel.selected = 0; }
            tx.scroll_to(LIST, 0.0, 0.0);
        })
        .on_input_key(|a: &mut App, e, tx| {
            if a.modal.is_some() { return crate::ui::modal::keys(a, e, tx); }
            a.workflows_key(e, tx)
        });
    let mut list = col().key(LIST).w_full().grow(1.0).shrink().min_h(0.0).padding(6.0 * s).gap(3.0 * s)
        .scroll(ScrollSpec { stick_bottom: false });
    if let Some(catalog) = &panel.catalog {
        if matching.is_empty() {
            let label = if catalog.workflows.is_empty() { "No workflows in this workspace" } else { "No matching workflows" };
            list = list.child(caption(label.into()).padding(5.0 * s));
        }
        for (i, w) in matching.iter().enumerate() {
            list = list.child(col().key(format!("workflow-row:{i}")).w_full().h(42.0 * s).px(9.0 * s).justify_center().radius(6.0 * s)
                .fill(if i == panel.selected { rgb(p.hover) } else { Color::TRANSPARENT })
                .hover(rgb(p.hover)).cursor(rune::Cursor::Pointer)
                .child(text(w.definition.name.clone()).font(app.ty.sans(13.0, 600.0)).color(rgb(p.fg)).truncate())
                .child(text(w.status()).font(app.ty.chat_sized(11.0)).color(rgb(if w.active { p.green } else { p.dim })))
                .on_press(move |a: &mut App| { if let Some(panel) = &mut a.workflows { panel.selected = i; } }));
        }
        for d in &catalog.diagnostics {
            list = list.child(col().w_full().min_w(0.0).padding(5.0 * s)
                .child(text(display_source(&panel.root, &d.source_path)).font(app.ty.chat_sized(12.0)).color(rgb(p.yellow)).truncate())
                .child(caption(d.message.clone()).color(rgb(p.yellow))));
        }
    }
    let left = col().w((290.0 * s).min(width * 0.38)).h_full().border_sides([0.0, 1.0, 0.0, 0.0], rgb(p.border))
        .child(row().mx(8.0 * s).mt(8.0 * s).h(30.0 * s).px(8.0 * s).center_y().radius(6.0 * s).fill(rgb(p.surface)).child(filter))
        .child(list);

    let mut detail = col().h_full().grow(1.0).shrink().min_w(0.0).padding(12.0 * s);
    if let Some(w) = matching.get(panel.selected) {
        let d = &w.definition;
        let mut info = col().key(format!("workflow-detail:{}", d.id)).w_full().grow(1.0).shrink().min_h(0.0).gap(8.0 * s)
            .padding(12.0 * s).radius(8.0 * s).border(1.0, rgb(p.border)).scroll(ScrollSpec { stick_bottom: false })
            .child(text(d.name.clone()).font(app.ty.sans(17.0, 600.0)).color(rgb(p.fg)).wrap().w_full())
            .child(caption(format!("{} · {}", d.id, w.status())))
            .child(caption(format!("Schedule   {}", d.schedule.label())))
            .child(caption(format!("Source   {}", display_source(&panel.root, &w.source_path))))
            .child(caption(format!("Directory   {}", d.directory.as_deref().unwrap_or("Workspace root"))));
        if let Some(agent) = &d.agent { info = info.child(caption(format!("Agent   {agent}"))); }
        if let Some(skill) = &d.skill { info = info.child(caption(format!("Skill   {skill}"))); }
        if let Some(model) = &d.model { info = info.child(caption(format!("Model   {}/{}", model.provider_id, model.id))); }
        info = info.child(col().w_full().gap(4.0 * s).pt(8.0 * s).border_sides([1.0, 0.0, 0.0, 0.0], rgb(p.border))
            .child(text("Prompt").font(app.ty.sans(12.0, 600.0)).color(rgb(p.dim)))
            .child(text(d.prompt.clone()).font(app.ty.chat_sized(13.0)).color(rgb(p.fg)).wrap().w_full()));
        if let Some((_, runs)) = panel.history.as_ref().filter(|(id, _)| id == &d.id) {
            info = info.child(caption("Rune run history".into()));
            if runs.is_empty() { info = info.child(caption("No Rune runs recorded.".into())); }
            for run in runs {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
                let minutes = now.saturating_sub(run.created as u128) / 60_000;
                let time = match minutes { 0 => "just now".into(), 1..=59 => format!("{minutes}m ago"), 60..=1439 => format!("{}h ago", minutes / 60), _ => format!("{}d ago", minutes / 1440) };
                info = info.child(caption(format!("{} · {}\n{}", run.status, time, run.id)));
            }
        }
        detail = detail.child(info);
        if w.source_hash.is_some() {
            let action = |operation: Operation, enabled| button(app, operation.label(), !busy && enabled, move |a| a.workflow_operation(operation));
            detail = detail.child(row().gap(6.0 * s).mt(8.0 * s)
                .child(action(Operation::History, true)).child(action(Operation::Source, true))
                .child(if w.active && w.activation_id.is_some() { action(Operation::Pause, true) }
                    else { action(Operation::Activate, w.can_activate && w.source_origin.is_some()) })
                .child(action(Operation::Run, d.directory.is_none() && d.skill.is_none())));
        }
    } else {
        detail = detail.child(caption(if panel.catalog.is_some() { "Select a workflow to inspect it." } else { "Workflow details will appear here." }.into()));
    }
    let (message, color) = match (busy, &panel.error) {
        (true, _) => ("Loading workflows…".into(), p.dim),
        (_, Some(message)) => (message.clone(), p.red),
        _ => (panel.notice.clone().unwrap_or_else(|| "Workspace prompt workflows   ·   ↑↓ Inspect   ·   Esc Close".into()), p.dim),
    };
    let card = col().key("workflows-panel").w(width).h(height).radius(12.0 * s).clip_rounded(12.0 * s)
        .fill(rgb(p.bg)).border(1.0, rgb(p.border)).preserve_focus_on_press(true).on_press(|_: &mut App| {})
        .child(header)
        .child(row().w_full().grow(1.0).shrink().min_h(0.0).child(left).child(detail))
        .child(col().w_full().px(14.0 * s).py(7.0 * s).border_sides([1.0, 0.0, 0.0, 0.0], rgb(p.border))
            .child(caption(message).color(rgb(color))));
    let card = if app.modal.is_none() && app.attach.preview.is_none() {
        card.modal(true, App::close_workflows).on_key(App::workflows_key)
    } else { card.disabled(true) };
    col().key("workflows-scrim").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).center()
        .fill(Color::BLACK.alpha(0.35)).on_press(|a: &mut App| a.close_workflows()).child(card)
}

const FIXTURE: &str = r#"{
    "workflows": [
    {"definition":{"id":"daily-summary","name":"Daily summary","agent":"build","prompt":"Summarize yesterday’s changes and draft a short handoff for the team. Do not modify files.","schedule":{"frequency":"daily","interval":1,"time":"09:00","timezone":"America/New_York"}},"active":true,"activationID":"daily-summary-activation","sourcePath":".neoism/workflows/daily-summary.md","sourceHash":"fixture-daily"},
        {"definition":{"id":"weekly-review","name":"Weekly review","prompt":"Summarize this week’s merged changes and list follow-ups.","schedule":{"frequency":"weekly","interval":1,"weekdays":["fri"],"time":"14:30","timezone":"UTC"}},"active":false,"activationID":"weekly-review-activation","sourcePath":".neoism/workflows/weekly-review.md"}
    ], "diagnostics": []
}"#;

pub fn scene(app: &mut App) {
    let mut panel = Panel::new(app);
    panel.apply(Ok(serde_json::from_str(FIXTURE).expect("workflow scene")));
    app.workflows = Some(panel);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn workspace_proxy_contract() {
        use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = daemon::Target {
            url: format!("ws://{}/prefix/session/?ignored=yes", listener.local_addr().unwrap()),
            auth_token: Some("master".into()), pairing_token: None,
        };
        let mut app = App::new();
        scene(&mut app);
        let panel = app.workflows.as_mut().unwrap();
        for (status, body) in [
            ("200 OK", FIXTURE),
            ("403 Forbidden", r#"{"code":"auth.hosted_route_forbidden","message":"This global credential or configuration route is unavailable in hosted mode"}"#),
        ] {
            let server = async {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut raw = Vec::new();
                while !raw.ends_with(b"\r\n\r\n") { raw.push(stream.read_u8().await.unwrap()); }
                let raw = String::from_utf8(raw).unwrap();
                assert_eq!(raw.lines().next().unwrap(), format!("GET /prefix/agent/workspaces/my%2Fworkspace/v2/plugins/{PLUGIN}/local-catalog HTTP/1.1"));
                assert!(raw.to_lowercase().contains("\r\nauthorization: bearer master\r\n"));
                assert!(!raw.to_lowercase().contains("x-neoism-directory:"));
                let response = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                stream.write_all(response.as_bytes()).await.unwrap();
            };
            let base = daemon::agent_http_endpoint(Some(&target), "my/workspace").unwrap();
            let (_, reply) = tokio::time::timeout(Duration::from_secs(3), async { tokio::join!(server, fetch_catalog(base)) }).await.unwrap();
            panel.apply(reply);
            match status {
                "403 Forbidden" => assert!(panel.error.as_ref().unwrap().contains("does not allow workflows through the workspace daemon")),
                _ => assert_eq!(panel.matching().len(), 2),
            }
        }
    }

    #[test]
    fn palette_filter_inspect_and_dismiss() {
        use rune::{Cx, Env, Runtime};
        let mut app = App::new();
        app.window = [1000.0, 650.0];
        app.open_picker(crate::palette::Mode::Commands);
        let mut rt = Runtime::new(app, |a: &App, _: &mut Cx<App>| col().w_full().h_full()
            .child(crate::palette::view(a)).child(view(a)).child(crate::ui::modal::view(a)), (1000.0, 650.0), Env::new(true, 1.0));
        rt.on_key = Some(crate::keys::on_key);
        fn settle(rt: &mut Runtime<App>) { for _ in 0..30 { rt.frame(1.0 / 120.0); } }
        fn key(rt: &mut Runtime<App>, key: Key) {
            rt.key(KeyEvent { key, mods: Default::default() });
            settle(rt);
        }
        settle(&mut rt);
        key(&mut rt, Key::Text("workflows".into()));
        key(&mut rt, Key::Enter);
        assert!(rt.state.workflows.as_ref().unwrap().error.is_some(), "opening offline must be recoverable, not an empty catalog");
        rt.update(|a, _| scene(a));
        settle(&mut rt);
        key(&mut rt, Key::Down);
        assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("workflow-detail:weekly-review") && !e.exiting));
        key(&mut rt, Key::Text("daily".into()));
        assert_eq!(rt.state.workflows.as_ref().unwrap().matching().len(), 1);
        let [x, y, w, h] = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("workflow-button:Run now") && !e.exiting).unwrap().rect;
        rt.pointer_down(x + w * 0.5, y + h * 0.5);
        rt.pointer_up(x + w * 0.5, y + h * 0.5);
        settle(&mut rt);
        assert_eq!(rt.state.modal.as_ref().unwrap().selected, 0);
        assert_eq!(rt.state.workflows.as_ref().unwrap().confirmation.as_ref().unwrap().revision, "sha256:fixture-daily");
        key(&mut rt, Key::Escape);
        assert!(rt.state.workflows.as_ref().unwrap().pending.is_none());
        key(&mut rt, Key::Escape);
        assert!(rt.state.workflows.is_none());
    }
}