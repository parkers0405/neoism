//! External harness setup, deliberately separate from Neoism's API model accounts.
use super::picker::{Kind, Picker, Row};
use super::view::{query_row, sans};
use crate::ui::theme::{glyph, rgb, NERD};
use crate::ui::ChildIf;
use crate::App;
use neoism_protocol::pty::{ClientMessage, HarnessAuth, HarnessStatus, ServerMessage};
use rune::{el, text, Color, El, Tx};
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct Readiness {
    pending: Option<(u64, Instant)>,
    statuses: Vec<HarnessStatus>,
    host_os: Option<String>,
    next_check: Option<Instant>,
}

impl Readiness {
    pub(super) fn get(&self, id: &str) -> Option<&HarnessStatus> {
        self.statuses.iter().find(|item| item.id == id)
    }
}

struct Harness {
    id: &'static str,
    name: &'static str,
    mark: &'static str,
    account: &'static str,
    program: &'static str,
    args: &'static [&'static str],
}

const HARNESSES: &[Harness] = &[
    Harness { id: "antigravity", name: "Antigravity", mark: "antigravity", account: "Google account", program: "agy", args: &["acp", "--login"] },
    Harness { id: "claude", name: "Claude Code", mark: "anthropic", account: "Claude account", program: "claude", args: &["auth", "login"] },
    Harness { id: "codex", name: "Codex", mark: "openai", account: "ChatGPT account", program: "codex", args: &["login"] },
    Harness { id: "cursor", name: "Cursor", mark: "cursor", account: "Cursor account", program: "cursor-agent", args: &["login"] },
    Harness { id: "opencode", name: "OpenCode", mark: "opencode", account: "Provider credentials", program: "opencode", args: &["auth", "login"] },
    Harness { id: "grok", name: "Grok Build", mark: "xai", account: "xAI account", program: "grok", args: &["login"] },
    Harness { id: "kiro", name: "Kiro", mark: "amazon-bedrock", account: "Kiro account", program: "kiro-cli", args: &["login"] },
    Harness { id: "fx", name: "fx", mark: "fx", account: "Harness credentials", program: "fx", args: &["login"] },
];

fn harness(id: &str) -> Option<&'static Harness> {
    HARNESSES.iter().find(|item| item.id == id)
}

pub(super) fn rows() -> Vec<Row> {
    HARNESSES.iter().map(|item| Row::new(item.name, item.account, "", item.id)).collect()
}

pub(crate) fn harness_name(id: &str) -> Option<&'static str> { harness(id).map(|item| item.name) }

pub(crate) fn harness_mark(app: &App, id: &str, size: f32) -> Option<El> {
    let item = harness(id)?;
    if let Some(mark) = super::provider_mark_for(app, item.mark, size) { return Some(mark); }
    // Monograms are intentional for harnesses without artwork in the shipped atlas.
    Some(el().w(size).h(size).radius(size * 0.23).center().fill(rgb(app.pal.blue).alpha(0.16))
        .child(text(if id == "fx" { "fx" } else { &item.name[..1] })
            .font(sans(size * 0.63, 650.0)).color(rgb(app.pal.blue))))
}

impl App {
    pub(crate) fn open_agent_connections(&mut self, provider: Option<&str>) {
        let mut picker = Picker::new(Kind::AgentConnections, "Connect an agent", rows()).placeholder("Search harnesses");
        if let Some(index) = HARNESSES.iter().position(|item| Some(item.id) == provider) {
            picker.selected = index;
        }
        self.focus_picker(picker);
        self.composer.harness_status = Readiness::default();
        self.check_harness_status();
    }

    pub(super) fn check_harness_status(&mut self) {
        if self.composer.harness_status.pending.is_some() { return; }
        if self.connection != crate::app::Connection::Connected || !self.workspace_ready { return; }
        if let Some(daemon) = &self.daemon {
            let message = serde_json::to_value(ClientMessage::GetHarnessStatus {}).expect("harness request serializes");
            let id = daemon.request(crate::daemon::Family::Pty, message, None);
            self.composer.harness_status.pending = Some((id, Instant::now() + Duration::from_secs(8)));
            self.composer.harness_status.next_check = Some(Instant::now() + Duration::from_secs(20));
        }
    }

    pub(crate) fn apply_harness_status(&mut self, request_id: u64, message: &serde_json::Value) -> bool {
        let is_status = message.get("HarnessStatuses").is_some();
        if self.composer.harness_status.pending.is_none_or(|(id, _)| id != request_id) { return is_status; }
        self.composer.harness_status.pending = None;
        let readiness = &mut self.composer.harness_status;
        if let Ok(ServerMessage::HarnessStatuses { statuses, host_os }) = serde_json::from_value(message.clone()) {
            readiness.statuses = statuses;
            readiness.host_os = host_os;
        }
        self.refresh_subagent_harness_rows();
        true
    }

    pub(crate) fn poll_harness_status(&mut self) -> bool {
        if !self.composer.picker.as_ref().is_some_and(|picker| matches!(picker.kind, Kind::AgentConnections | Kind::SubagentModels)) { return false; }
        let now = Instant::now();
        let readiness = &mut self.composer.harness_status;
        if self.connection != crate::app::Connection::Connected || !self.workspace_ready || self.daemon.is_none() {
            let changed = readiness.pending.is_some() || !readiness.statuses.is_empty();
            *readiness = Readiness::default();
            if changed { self.refresh_subagent_harness_rows(); }
            return changed;
        }
        let expired = readiness.pending.is_some_and(|(_, deadline)| now >= deadline);
        if expired { readiness.pending = None; }
        if readiness.pending.is_none() && readiness.next_check.is_none_or(|at| now >= at) {
            self.check_harness_status();
            return true;
        }
        expired
    }
}

pub(super) fn start_sign_in(app: &mut App, id: &str) {
    let Some(item) = harness(id) else { return };
    let readiness = &app.composer.harness_status;
    if !readiness.get(id).is_some_and(|status| status.installed && status.auth != HarnessAuth::SignedIn) { return; }
    if app.daemon.is_none() || app.connection != crate::app::Connection::Connected || !app.workspace_ready {
        app.modal = Some(crate::ui::modal::Modal::notice("Workspace not connected", "Connect to the agent host before starting harness sign-in."));
        return;
    }
    let args = if id == "codex" && app.server.link.current.as_ref().is_some_and(|server| !server.is_local()) {
        &["login", "--device-auth"][..]
    } else {
        item.args
    };
    app.close_prompt_picker();
    app.open_harness_setup(item.id, false, (item.program, args));
}

type SetupCommand = (&'static str, &'static [&'static str]);

fn installer(id: &str, host_os: &str) -> Option<SetupCommand> {
    // Fixed vendor endpoints and arguments. No workspace or user input becomes shell text.
    let script: &'static [&'static str] = match (host_os, id) {
        ("macos" | "linux", "antigravity") => &["-o", "pipefail", "-c", "command -v agy >/dev/null || curl -fsSL https://antigravity.google/cli/install.sh | /bin/bash"],
        ("macos" | "linux", "claude") => &["-o", "pipefail", "-c", "command -v claude >/dev/null || curl -fsSL https://claude.ai/install.sh | /bin/bash"],
        ("macos" | "linux", "codex") => &["-o", "pipefail", "-c", "command -v codex >/dev/null || curl -fsSL https://chatgpt.com/codex/install.sh | /bin/sh"],
        ("macos" | "linux", "cursor") => &["-o", "pipefail", "-c", "command -v cursor-agent >/dev/null || curl -fsSL https://cursor.com/install | /bin/bash"],
        ("macos" | "linux", "opencode") => &["-o", "pipefail", "-c", "command -v opencode >/dev/null || curl -fsSL https://opencode.ai/install | /bin/bash"],
        ("macos" | "linux", "fx") => &["-o", "pipefail", "-c", "command -v fx >/dev/null || curl -fsSL https://fx.sh/setup.sh | /bin/bash"],
        ("windows", _) => return windows_installer(id),
        _ => return None,
    };
    Some(("/bin/bash", script))
}

fn windows_installer(id: &str) -> Option<SetupCommand> {
    let args: &'static [&'static str] = match id {
        "antigravity" => &["-NoProfile", "-Command", "$ErrorActionPreference = 'Stop'; if (!(Get-Command agy -ErrorAction SilentlyContinue)) { irm 'https://antigravity.google/cli/install.ps1' | iex }"],
        "claude" => &["-NoProfile", "-Command", "$ErrorActionPreference = 'Stop'; if (!(Get-Command claude -ErrorAction SilentlyContinue)) { irm 'https://claude.ai/install.ps1' | iex }"],
        "codex" => &["-NoProfile", "-Command", "$ErrorActionPreference = 'Stop'; if (!(Get-Command codex -ErrorAction SilentlyContinue)) { irm 'https://chatgpt.com/codex/install.ps1' | iex }"],
        _ => return None,
    };
    Some(("powershell.exe", args))
}

fn start_install(app: &mut App, item: &'static Harness) {
    let readiness = &app.composer.harness_status;
    if !readiness.get(item.id).is_some_and(|status| !status.installed)
        || app.connection != crate::app::Connection::Connected || !app.workspace_ready || app.daemon.is_none() { return; }
    let Some(command) = readiness.host_os.as_deref().and_then(|os| installer(item.id, os)) else { return };
    app.close_prompt_picker();
    app.open_harness_setup(item.id, true, command);
}

fn install_guide(id: &str) -> Option<&'static str> {
    Some(match id {
        "antigravity" => "https://antigravity.google/docs/getting-started",
        "claude" => "https://code.claude.com/docs/en/setup",
        "codex" => "https://learn.chatgpt.com/docs/codex/cli",
        "cursor" => "https://cursor.com/docs/cli/installation",
        "opencode" => "https://opencode.ai/docs/",
        "kiro" => "https://kiro.dev/docs/cli/setup/",
        "grok" => "https://x.ai/cli",
        "fx" => "https://fx.sh",
        _ => return None,
    })
}

fn step(app: &App, number: &str, label: &str, done: bool, enabled: bool, action: El) -> El {
    let (p, s) = (app.pal, app.ty.scale());
    let ink = if done { p.green } else if enabled { p.blue } else { p.muted };
    rune::row().w_full().min_h(33.0 * s).gap(8.0 * s).center_y()
        .child(el().key(format!("connect-agent-step-{number}-{}", if done { "done" } else { "pending" }))
            .w(21.0 * s).h(21.0 * s).radius(11.0 * s).center().fill(rgb(ink).alpha(0.14))
            .child(text(if done { "\u{2713}" } else { number }).font(sans(12.0 * s, 600.0)).color(rgb(ink))))
        .child(text(label).font(sans(13.0 * s, 500.0)).color(rgb(if enabled || done { p.fg } else { p.muted })).grow(1.0).truncate())
        .child(action)
}

pub(super) fn view(app: &App, picker: &Picker, composer: [f32; 4]) -> El {
    let (p, s) = (app.pal, app.ty.scale());
    let modal = app.composer.card_modal;
    let [cx, cy, cw, _] = composer;
    let top = app.rects.get(&format!("agent-area:{}", app.active_tab().id)).map_or(0.0, |r| r[1]) + 8.0 * s;
    let gap = 6.0 * s;
    let (x, y, width, height) = if modal {
        let width = (760.0 * s).min((app.window[0] - 48.0).max(1.0));
        let height = (412.0 * s).min((app.window[1] - 48.0).max(1.0));
        ((app.window[0] - width) * 0.5, (app.window[1] - height) * 0.5, width, height)
    } else {
        let space = cy - top - gap;
        if space < 48.0 * s { return el(); }
        let height = (320.0 * s).min(space);
        (cx, cy - gap - height, cw, height)
    };
    let subagents = picker.kind == Kind::SubagentModels;
    let side = if subagents { super::subagent_models::RAIL_WIDTH * s } else { (214.0 * s).min(width * 0.41) };
    let header = (if subagents { 36.0 } else { 48.0 }) * s;
    let body = (height - header).max(1.0);
    let reasoning_w = if subagents { (125.0 * s).min(width * 0.28) } else { 0.0 };
    let detail_w = (width - side - reasoning_w).max(1.0);
    let rail_overhead = if subagents { 23.0 + super::subagent_models::RAIL_ACTION_HEIGHT } else { 52.0 };
    picker.limit.set(((body - rail_overhead * s) / (36.0 * s)).floor().max(1.0) as usize);
    let rows = picker.rows();
    let visible = picker.limit.get();
    let first = picker.top.min(picker.selected).max((picker.selected + 1).saturating_sub(visible))
        .min(rows.len().saturating_sub(visible));

    let title = rune::row().h(header).px(14.0 * s).gap(8.0 * s).center_y()
        .border_sides([0.0, 0.0, 1.0, 0.0], rgb(p.fg).alpha(0.1))
        .child(text(if subagents { "Subagents" } else { "Connect an agent" })
            .font(sans(15.0 * s, 600.0)).color(rgb(p.fg)).grow(1.0))
        .child(el().key("connect-agent-expand").w(24.0 * s).h(24.0 * s).center().radius(6.0 * s).hover(rgb(p.fg).alpha(0.1))
            .child(text(if modal { "\u{f0615}" } else { "\u{f0616}" }).font(rune::tok::TextStyle::new(14.0 * s, 400.0).family(NERD)).color(rgb(p.dim)))
            .on_press_tx(App::toggle_picker_modal))
        .child(el().key("connect-agent-close").w(24.0 * s).h(24.0 * s).center().radius(6.0 * s).hover(rgb(p.red).alpha(0.15))
            .child(text("\u{f0156}").font(rune::tok::TextStyle::new(14.0 * s, 400.0).family(NERD)).color(rgb(p.red)))
            .on_press(|a: &mut App| { if !a.return_to_subagent_models() { a.close_prompt_picker(); } }));

    let mut list = rune::col().w_full().gap(2.0 * s).px(7.0 * s);
    for (index, row) in rows.iter().enumerate().skip(first).take(picker.limit.get()) {
        let id = row.value.clone();
        let selected = picker.selected == index;
        list = list.child(rune::row().key(format!("connect-agent:{id}")).h(34.0 * s).w_full().px(8.0 * s).gap(8.0 * s).center_y().radius(8.0 * s)
            .fill(rgb(p.fg).alpha(if selected { 0.09 } else { 0.0 })).hover(rgb(p.fg).alpha(0.1))
            .child_if(subagents && super::subagent_models::is_current_harness(app, &id), || el().w(2.0 * s).h(15.0 * s).radius(1.0 * s).fill(rgb(p.green)).absolute(2.0 * s, 9.0 * s))
            .child(el().w(21.0 * s).h(21.0 * s).center().child(if id == "neoism" { crate::ui::neoism_mark(app, 17.0 * s, "subagent-neoism-mark") } else { harness_mark(app, &id, 17.0 * s).unwrap_or_else(el) }))
            .child_if(!subagents, || text(row.title.clone()).font(sans(13.0 * s, if selected { 600.0 } else { 500.0 })).color(rgb(p.fg)).grow(1.0).truncate())
            .child_if(!subagents, || el().w(3.0 * s).h(16.0 * s).radius(1.5 * s).fill(rgb(p.blue).alpha(if selected { 1.0 } else { 0.0 })))
            .on_press(move |a: &mut App| {
                if let Some(picker) = a.composer.picker.as_mut().filter(|picker| matches!(picker.kind, Kind::AgentConnections | Kind::SubagentModels)) {
                    picker.selected = index;
                }
                if subagents { a.load_subagent_models(&id); }
            }));
    }
    if rows.is_empty() {
        list = list.child(text("No matching harnesses").font(sans(12.0 * s, 400.0)).color(rgb(p.muted)).px(7.0 * s).wrap());
    }
    let rail = rune::col().w(side).h(body).gap(7.0 * s)
        .py(if subagents { 8.0 * s } else { 0.0 })
        .border_sides([0.0, 1.0, 0.0, 0.0], rgb(p.fg).alpha(0.1))
        .on_wheel(move |a: &mut App, event: rune::WheelEvent, _: &mut Tx| {
            if let Some(picker) = a.composer.picker.as_mut() {
                let rows = if event.precise {
                    picker.wheel += event.dy;
                    let steps = (picker.wheel / (36.0 * s)).trunc();
                    picker.wheel -= steps * 36.0 * s;
                    -steps as isize
                } else { if event.dy < 0.0 { 1 } else { -1 } };
                if rows != 0 { picker.scroll(rows); }
            }
            if subagents { a.sync_subagent_models(); }
            true
        })
        .child_if(!subagents, || el().w(side).h(42.0 * s)
            .child(text(glyph::SEARCH_LINE).font(rune::tok::TextStyle::new(14.0 * s, 400.0).family(NERD)).color(rgb(p.muted)).absolute(13.0 * s, 13.0 * s))
            .child(query_row(app, picker, (side - 42.0 * s).max(1.0)).absolute(33.0 * s, 11.0 * s))
            .child(el().w((side - 24.0 * s).max(1.0)).h(1.5 * s).fill(rgb(p.blue).alpha(0.65)).absolute(12.0 * s, 40.0 * s)))
        .child(if subagents { el().w_full().grow(1.0).min_h(0.0).clip_rounded(0.0).child(list) } else { list })
        .child_if(subagents, || super::subagent_models::rail_actions(app, true));

    let mut details = rune::col().key("connect-agent-details").w(detail_w).h(body).px(17.0 * s).py(12.0 * s).gap(4.0 * s);
    if subagents {
        if let Some(row) = picker.selected_row() { details = super::subagent_models::details(app, &row.value, details); }
    } else if let Some(item) = picker.selected_row().and_then(|row| harness(&row.value)) {
        let readiness = &app.composer.harness_status;
        let status = readiness.get(item.id);
        let installed = status.is_some_and(|status| status.installed);
        let signed_in = installed && status.is_some_and(|status| status.auth == HarnessAuth::SignedIn);
        let checking = status.is_none() && readiness.pending.is_some();
        let missing = status.is_some_and(|status| !status.installed);
        let blocked = !installed || signed_in;
        let managed = missing && readiness.host_os.as_deref().and_then(|os| installer(item.id, os)).is_some();
        let mut install = el().key("connect-agent-install").px(8.0 * s).py(5.0 * s).radius(6.0 * s).fill(rgb(p.fg).alpha(0.06))
            .child(text(if installed { "Installed" } else if checking { "Checking..." } else if managed { "Install" } else { "Install guide" }).font(sans(12.0 * s, 600.0)).color(rgb(if installed || checking { p.muted } else { p.fg })).truncate());
        if !installed && !checking {
            if managed {
                install = install.hover(rgb(p.fg).alpha(0.12)).on_press(move |a: &mut App| start_install(a, item));
            } else if let Some(url) = install_guide(item.id) {
                install = install.hover(rgb(p.fg).alpha(0.12)).on_press(move |_: &mut App| { let _ = crate::cli::open_external(url); });
            } else {
                install = el();
            }
        }
        let mut sign_in = el().key("connect-agent-sign-in").px(8.0 * s).py(5.0 * s).radius(6.0 * s)
            .fill(rgb(if blocked { p.fg } else { p.blue }).alpha(if blocked { 0.06 } else { 0.14 }))
            .child(text(if signed_in { "Signed in" } else if !installed { "Install first" } else { match item.id { "codex" => "ChatGPT", "antigravity" => "Google", _ => "Sign in" } })
                .font(sans(12.0 * s, 600.0)).color(rgb(if blocked { p.muted } else { p.blue })).truncate());
        if !blocked { sign_in = sign_in.hover(rgb(p.blue).alpha(0.22)).on_press(move |a: &mut App| start_sign_in(a, item.id)); }
        details = details.scroll(rune::ScrollSpec { stick_bottom: false })
            .child(rune::row().gap(9.0 * s).center_y()
                .child(el().w(32.0 * s).h(32.0 * s).radius(8.0 * s).fill(rgb(p.fg).alpha(0.07)).center().child(harness_mark(app, item.id, 20.0 * s).unwrap_or_else(el)))
                .child(text(item.name).font(sans(18.0 * s, 600.0)).color(rgb(p.fg)).truncate()))
            .child(step(app, "1", "Install", installed, !checking, install))
            .child(step(app, "2", "Sign in", signed_in, !blocked, sign_in));
    }

    let card = rune::col().key("prompt-picker").w(width).h(height).fill(rgb(p.bg)).radius(12.0 * s)
        .border(1.0, rgb(p.fg).alpha(0.12)).shadow(8.0 * s, 28.0 * s, Color::BLACK.alpha(0.45)).clip_rounded(12.0 * s)
        .transition(rune::Transition::scale(0.94).combined(rune::Transition::offset(0.0, 14.0 * s)))
        .on_press(|_: &mut App| {})
        .child(title).child(rune::row().w_full().h(body).child(rail).child(details)
            .child_if(subagents, || super::subagent_models::reasoning(app, picker.selected_row().map_or("neoism", |row| row.value.as_str()), reasoning_w, body)));
    el().key("prompt-picker-layer").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .child(el().key("prompt-picker-scrim").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).fill(if modal { Color::BLACK.alpha(0.4) } else { Color::TRANSPARENT })
            .on_press(|a: &mut App| a.dismiss_prompt_picker()))
        .child(card.absolute(x, y))
}