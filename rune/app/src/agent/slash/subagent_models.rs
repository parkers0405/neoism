use super::{agent_connections as harnesses, picker::{Kind, Picker, Row}, view::sans};
use crate::{App, ui::theme::{rgb, glyph, NERD}};
use crate::ui::ChildIf;
use neoism_protocol::agent::{AgentClientMessage as Client, AgentServerMessage as Server, SubagentModel};
use neoism_protocol::pty::HarnessAuth;
use rune::{el, text, El};

#[derive(Default)]
pub(super) struct Selection {
    current: Option<(String, Option<String>, Option<String>)>,
    default_request: Option<String>,
    catalog_request: Option<String>,
    provider: String,
    models: Vec<SubagentModel>,
    reasoning: Vec<SubagentModel>,
    catalog_error: Option<String>,
    save_request: Option<String>,
    save_error: Option<String>,
    returning: bool,
    query: rune::TextEdit,
}

impl App {
    fn send_subagent_picker_request(&self, message: Client) {
        if let (Some(daemon), Ok(value)) = (&self.daemon, serde_json::to_value(message)) {
            daemon.send_scoped(crate::daemon::Family::Agent, value, &self.cwd);
        }
    }

    pub(super) fn open_subagent_models(&mut self, provider: Option<&str>) {
        let modal = self.composer.card_modal;
        let provider = provider.or_else(|| self.composer.subagent_models.current.as_ref().map(|entry| entry.0.as_str()))
            .filter(|id| self.subagent_harness_visible(id)).unwrap_or("neoism").to_owned();
        self.composer.subagent_models.returning = false;
        self.focus_picker(Picker::new(Kind::SubagentModels, "Subagents", self.subagent_harness_rows()).select_value(&provider));
        self.composer.card_modal = modal;
        self.check_harness_status();
        if self.daemon.is_some() {
            let request_id = self.composer.request_id("subagent-default");
            self.composer.subagent_models.default_request = Some(request_id.clone());
            self.send_subagent_picker_request(Client::GetSubagentDefault { request_id });
        }
        self.load_subagent_models(&provider);
    }

    fn subagent_harness_visible(&self, id: &str) -> bool {
        id == "neoism" || self.composer.harness_status.get(id).is_some_and(|row| row.installed && row.auth == HarnessAuth::SignedIn)
    }

    fn subagent_harness_rows(&self) -> Vec<Row> {
        std::iter::once(Row::new("Neoism", "", "", "neoism"))
            .chain(harnesses::rows().into_iter().filter(|row| self.subagent_harness_visible(&row.value))).collect()
    }

    pub(super) fn refresh_subagent_harness_rows(&mut self) {
        if !self.composer.picker.as_ref().is_some_and(|picker| picker.kind == Kind::SubagentModels) { return; }
        let rows = self.subagent_harness_rows();
        if let Some(picker) = self.composer.picker.as_mut() { picker.replace(rows); }
        self.sync_subagent_models();
    }

    pub(super) fn load_subagent_models(&mut self, provider: &str) {
        let state = &mut self.composer.subagent_models;
        state.provider = provider.to_owned();
        state.models.clear();
        state.reasoning.clear();
        state.catalog_error = None;
        state.catalog_request = None;
        state.query = rune::TextEdit::default();
        state.query.focused = true;
        if provider == "neoism" || self.daemon.is_none() { return; }
        if !self.subagent_harness_visible(provider) { return; }
        let request_id = self.composer.request_id("subagent-models");
        self.composer.subagent_models.catalog_request = Some(request_id.clone());
        self.send_subagent_picker_request(Client::GetSubagentModels { request_id, provider: provider.to_owned() });
    }

    pub(super) fn sync_subagent_models(&mut self) {
        let provider = self.composer.picker.as_ref().filter(|picker| picker.kind == Kind::SubagentModels)
            .and_then(Picker::selected_row).map(|row| row.value.clone());
        if let Some(provider) = provider.filter(|provider| *provider != self.composer.subagent_models.provider) {
            self.load_subagent_models(&provider);
        }
    }

    fn choose_subagent_model(&mut self, provider: &str, model: Option<String>, reasoning: Option<String>) {
        if self.daemon.is_none() || self.composer.subagent_models.save_request.is_some() { return; }
        let request_id = self.composer.request_id("set-subagent-default");
        let state = &mut self.composer.subagent_models;
        state.save_request = Some(request_id.clone());
        state.save_error = None;
        self.send_subagent_picker_request(Client::SetSubagentDefault { request_id, provider: provider.to_owned(), model, reasoning });
    }

    pub(super) fn apply_subagent_model_event(&mut self, event: &Server) -> bool {
        let state = &mut self.composer.subagent_models;
        match event {
            Server::SubagentModels { request_id, provider, models, reasoning, error } => {
                if state.catalog_request.as_ref() == Some(request_id) && state.provider == *provider {
                    state.catalog_request = None;
                    state.models = models.clone();
                    state.reasoning = reasoning.clone();
                    state.catalog_error = error.clone();
                }
            }
            Server::SubagentDefault { request_id, provider, model, reasoning, error } => {
                if state.save_request.as_ref() == Some(request_id) {
                    state.default_request = None;
                    state.save_request = None;
                    state.save_error = error.clone();
                    if error.is_none() { state.current = Some((provider.clone(), model.clone(), reasoning.clone())); }
                } else if state.default_request.as_ref() == Some(request_id) {
                    state.default_request = None;
                    if error.is_none() { state.current = Some((provider.clone(), model.clone(), reasoning.clone())); }
                }
            }
            _ => return false,
        }
        true
    }

    pub(super) fn connect_from_subagent_models(&mut self, provider: Option<&str>) {
        self.open_agent_connections(provider);
        self.composer.subagent_models.returning = true;
    }

    pub(super) fn return_to_subagent_models(&mut self) -> bool {
        if !self.composer.subagent_models.returning { return false; }
        let provider = self.composer.picker.as_ref().and_then(|p| p.selected_row()).map(|row| row.value.clone());
        self.open_subagent_models(provider.as_deref());
        true
    }

    pub(super) fn reset_subagent_setup_return(&mut self) { self.composer.subagent_models.returning = false; }

    pub(crate) fn fixture_subagent_models(&mut self) {
        self.open_subagent_models(None);
        if let Some(picker) = &mut self.composer.picker {
            picker.replace(vec![Row::new("Neoism", "", "", "neoism"), Row::new("Antigravity", "", "", "antigravity")]);
            if let Some(index) = picker.rows().iter().position(|row| row.value == "antigravity") { picker.selected = index; }
        }
        let state = &mut self.composer.subagent_models;
        state.provider = "antigravity".into();
        state.current = Some(("antigravity".into(), Some("gemini-3.8-flash".into()), None));
        state.reasoning = ["Low", "Medium", "High"].into_iter().map(|label| SubagentModel { id: label.to_lowercase(), label: label.into() }).collect();
        state.models = [
            ("gemini-3.8-flash", "Gemini 3.8 Flash"),
            ("claude-sonnet-4-6", "Claude Sonnet 4.6 (Thinking)"),
            ("claude-opus-4-6-thinking", "Claude Opus 4.6 (Thinking)"),
            ("gpt-oss-120b-medium", "GPT-OSS 120B (Medium)"),
        ].into_iter().map(|(id, label)| SubagentModel { id: id.into(), label: label.into() }).collect();
    }
}

pub(super) const RAIL_WIDTH: f32 = 56.0;
const RAIL_BUTTON_SIZE: f32 = 34.0;
const RAIL_ACTION_GAP: f32 = 4.0;
pub(super) const RAIL_ACTION_HEIGHT: f32 = 2.0 * RAIL_BUTTON_SIZE + RAIL_ACTION_GAP;

pub(super) fn rail_actions(app: &App, subagents: bool) -> El {
    use crate::ui::feather::{self, Icon};
    let (p, s) = (app.pal, app.ty.scale());
    let button = |key| el().key(key).w(RAIL_BUTTON_SIZE * s).h(RAIL_BUTTON_SIZE * s).center().radius(8.0 * s)
        .hover(rgb(p.fg).alpha(0.12));
    rune::col().w_full().h(RAIL_ACTION_HEIGHT * s).items_center().gap(RAIL_ACTION_GAP * s)
        .child(button("model-subagents-toggle")
            .role("button").label("Subagents").checked(subagents)
            .help(if subagents { "Back to Main Agent models" } else { "Subagents" }).help_above()
            .fill(rgb(p.blue).alpha(if subagents { 0.18 } else { 0.0 }))
            .child(feather::view(app, Icon::Hierarchy, 16.0 * s, if subagents { p.blue } else { p.dim }))
            .on_press(move |a: &mut App| {
                if subagents {
                    let modal = a.composer.card_modal;
                    a.open_selection(Kind::Model);
                    a.composer.card_modal = modal;
                } else { a.open_subagent_models(None); }
            }))
        .child(button(if subagents { "subagent-connect" } else { "model-connect" })
            .role("button").label(if subagents { "Connect an agent" } else { "Connect a provider" })
            .border(1.0, rgb(p.fg).alpha(0.12))
            .child(text("\u{f0415}").font(rune::tok::TextStyle::new(16.0 * s, 400.0).family(NERD)).color(rgb(p.dim)))
            .on_press(move |a: &mut App| {
                if subagents { a.connect_from_subagent_models(None); } else { a.connect_from_model_card(); }
            }))
}

pub(super) fn is_current_harness(app: &App, provider: &str) -> bool {
    app.composer.subagent_models.current.as_ref().is_some_and(|(active, _, _)| active == provider)
}

pub(super) fn details(app: &App, provider: &str, pane: El) -> El {
    let (p, s) = (app.pal, app.ty.scale());
    let state = &app.composer.subagent_models;
    let search = rune::row().h(31.0 * s).gap(8.0 * s).center_y()
        .child(text(glyph::SEARCH_LINE).font(rune::tok::TextStyle::new(14.0 * s, 400.0).family(NERD)).color(rgb(p.muted)))
        .child(rune::text_input(&state.query, sans(13.5 * s, 400.0)).key("subagent-model-query")
            .placeholder("Search models", rgb(p.muted)).color(rgb(p.fg)).grow(1.0)
            .on_edit(|a: &mut App| &mut a.composer.subagent_models.query)
            .on_input_key(|a: &mut App, e: &rune::KeyEvent, tx: &mut rune::Tx| e.key == rune::Key::Escape && a.picker_key(e, tx)));
    let scope = state.provider == provider;
    let loaded = scope && state.catalog_request.is_none() && state.catalog_error.is_none();
    let query = state.query.text.to_lowercase();
    let matches = |label: &str, id: &str| query.split_whitespace().all(|word| label.to_lowercase().contains(word) || id.to_lowercase().contains(word));
    let mut list = rune::col().key(format!("subagent-model-list:{provider}:{query}")).w_full().grow(1.0).min_h(0.0)
        .scroll(rune::ScrollSpec { stick_bottom: false }).blur_after_touch_scroll();
    let row = |id: Option<&str>, label: &str, mark: El| {
        let selected = state.current.as_ref().is_some_and(|(active, model, _)| active == provider && model.as_deref() == id);
        let key = id.map_or_else(|| "subagent-model-default".into(), |id| format!("subagent-model:{id}"));
        let value = id.map(str::to_owned);
        let provider = provider.to_owned();
        rune::row().key(key).min_h(32.0 * s).w_full().px(8.0 * s).gap(6.0 * s).center_y().radius(6.0 * s)
            .fill(rgb(p.fg).alpha(if selected { 0.08 } else { 0.0 })).hover(rgb(p.fg).alpha(0.1))
            .child(el().w(18.0 * s).h(18.0 * s).center().child(mark))
            .child(text(label.to_owned()).font(sans(13.0 * s, 500.0)).color(rgb(p.fg)).grow(1.0).truncate())
            .child_if(selected, || text(glyph::CHECK).font(rune::tok::TextStyle::new(12.0 * s, 400.0).family(NERD)).color(rgb(p.green)))
            .touch_scroll().on_press(move |a: &mut App| {
                if !selected { a.choose_subagent_model(&provider, value.clone(), None); }
            })
    };
    let default = if provider == "neoism" { "Built-in default" } else { "Harness default" };
    if matches(default, "") && (provider == "neoism" || (loaded && app.daemon.is_some()) || !state.models.is_empty()) {
        list = list.child(row(None, default, el()));
    }
    if provider == "neoism" {
        for model in super::catalog::models(&app.composer.providers).into_iter().filter(|row| !row.header && matches(&row.title, &row.value)) {
            let (source, _) = model.value.split_once('/').unwrap_or_default();
            list = list.child(row(Some(&model.value), &model.title, super::view::model_mark(app, source, &model.section, 16.0)));
        }
    } else if scope && state.catalog_request.is_some() {
        for width in [120.0, 164.0, 142.0, 177.0, 110.0] {
            list = list.child(rune::row().h(32.0 * s).px(8.0 * s).gap(6.0 * s).center_y()
                .child(el().w(16.0 * s).h(16.0 * s).radius(4.0 * s).fill(rgb(p.fg).alpha(0.08)))
                .child(el().w(width * s).h(10.0 * s).radius(4.0 * s).fill(rgb(p.fg).alpha(0.08))));
        }
    } else if scope {
        for model in state.models.iter().filter(|model| matches(&model.label, &model.id)) {
            list = list.child(row(Some(&model.id), &model.label, el()));
        }
        if let Some(status) = state.catalog_error.as_deref().or_else(|| state.models.is_empty().then_some("No models reported by this harness")) {
            list = list.child(text(status.to_owned()).font(sans(12.0 * s, 400.0)).color(rgb(p.dim)).wrap());
        }
    }
    let mut pane = pane.child(search);
    if let Some(error) = &state.save_error {
        pane = pane.child(text(error.clone()).font(sans(12.0 * s, 400.0)).color(rgb(p.red)).wrap());
    }
    pane.child(list)
}

pub(super) fn reasoning(app: &App, provider: &str, width: f32, height: f32) -> El {
    let (p, s) = (app.pal, app.ty.scale());
    let state = &app.composer.subagent_models;
    let chosen = state.current.as_ref().filter(|(id, _, _)| id == provider);
    let model = chosen.and_then(|(_, model, _)| model.as_deref());
    let selected = chosen.and_then(|(_, _, reasoning)| reasoning.as_deref());
    let levels = if provider == "neoism" {
        model.into_iter().flat_map(|model| super::catalog::thinking(model, selected.unwrap_or_default()))
            .filter(|row| !row.header && !row.value.is_empty() && row.value != "default")
            .map(|row| SubagentModel { id: row.value, label: row.title }).collect::<Vec<_>>()
    } else if state.provider == provider { state.reasoning.clone() } else { vec![] };
    let mut list = rune::col().key("subagent-reasoning-list").grow(1.0).min_h(0.0).scroll(rune::ScrollSpec { stick_bottom: false }).blur_after_touch_scroll();
    if chosen.is_some() && !levels.is_empty() {
        for (id, label) in std::iter::once((None, "Default")).chain(levels.iter().map(|level| (Some(level.id.as_str()), level.label.as_str()))) {
            let on = selected == id;
            let model = model.map(str::to_owned);
            let provider = provider.to_owned();
            let value = id.map(str::to_owned);
            list = list.child(rune::row().h(31.0 * s).px(6.0 * s).gap(4.0 * s).center_y().radius(6.0 * s)
                .fill(rgb(p.fg).alpha(if on { 0.08 } else { 0.0 })).hover(rgb(p.fg).alpha(0.1))
                .child(text(label.to_owned()).font(sans(12.5 * s, 500.0)).color(rgb(p.fg)).grow(1.0).truncate())
                .child_if(on, || text(glyph::CHECK).font(rune::tok::TextStyle::new(12.0 * s, 400.0).family(NERD)).color(rgb(p.green)))
                .touch_scroll().on_press(move |a: &mut App| if !on { a.choose_subagent_model(&provider, model.clone(), value.clone()); }));
        }
    } else if state.catalog_request.is_none() {
        list = list.child(text(if chosen.is_none() || (provider == "neoism" && model.is_none()) { "Pick a model first" } else { "No separate levels" })
            .font(sans(12.0 * s, 400.0)).color(rgb(p.muted)).wrap());
    }
    rune::col().w(width).h(height).px(8.0 * s).py(12.0 * s).gap(6.0 * s)
        .border_sides([0.0, 0.0, 0.0, 1.0], rgb(p.fg).alpha(0.08))
        .child(text("Reasoning").font(sans(13.0 * s, 550.0)).color(rgb(p.dim)))
        .child(list)
}