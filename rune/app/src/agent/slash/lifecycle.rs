//! Authentication is a deliberate workflow, not a dismiss-on-background-click menu.
use super::{Kind, catalog};
use crate::app::App;

impl App {
    pub(crate) fn authentication_picker_open(&self) -> bool {
        self.composer.picker.as_ref().is_some_and(|p| matches!(p.kind,
            Kind::Connect | Kind::ConnectAccount | Kind::ConnectAccountActions |
            Kind::ConnectAuth | Kind::ConnectSecret | Kind::ConnectLabel |
            Kind::Mcp | Kind::McpActions))
    }

    pub(crate) fn dismiss_prompt_picker(&mut self) {
        if !self.authentication_picker_open() { self.close_prompt_picker(); }
    }

    pub(super) fn current_mcp_picker(&self, name: Option<&str>) -> bool {
        self.composer.picker.as_ref().is_some_and(|p| matches!(p.kind, Kind::Mcp | Kind::McpActions))
            && name.is_none_or(|name| self.composer.mcp_server.as_deref() == Some(name))
    }

    pub(super) fn refresh_mcp_activity(&mut self) {
        if !std::mem::take(&mut self.composer.mcp_refresh_requested) { return; }
        let Some(name) = self.composer.mcp_server.as_deref() else { return };
        let Some(picker) = self.composer.picker.as_mut().filter(|p| p.kind == Kind::McpActions) else { return };
        if !picker.activity.as_ref().is_some_and(|a| a.busy) { return; }
        let Some(row) = catalog::mcp_servers(&self.composer.mcp_status).into_iter()
            .find(|r| serde_json::from_str::<serde_json::Value>(&r.value).ok().is_some_and(|v| v["name"] == name)) else { return };
        let Ok(entry) = serde_json::from_str::<serde_json::Value>(&row.value) else { return };
        let state = &self.composer.mcp_status[name];
        let state = state.get("status").filter(|s| s.is_object()).unwrap_or(state);
        let connected = entry["connected"].as_bool() == Some(true);
        self.composer.mcp_authorizing = false;
        picker.query.clear();
        let mut rows = catalog::mcp_actions(&entry);
        rows.insert(0, super::picker::Row::new("Done", "", "", catalog::DONE));
        picker.replace(rows);
        if let Some(activity) = &mut picker.activity {
            let (status, body) = if connected {
                ("Connected", format!("{name} connected. You can use its tools now."))
            } else {
                match state["status"].as_str() {
                    Some("failed") => ("Connection failed", state["error"].as_str().unwrap_or("Unable to connect to the MCP server.").to_owned()),
                    Some("needs_auth" | "needs_client_registration") => ("Authorization required", format!("{name} still needs authorization. Try connecting again.")),
                    Some("disabled") => ("Disabled", format!("{name} is disabled. Enable it to connect.")),
                    _ => ("Not connected", format!("{name} is not connected. Choose Connect to retry.")),
                }
            };
            activity.body = body;
            activity.status = status.into();
            activity.busy = false;
            activity.url = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::slash::{ConnectFlow, Picker};
    use crate::agent::slash::picker::{Activity, Row};
    use neoism_protocol::agent::AgentServerMessage as Server;
    use rune::{Env, Key, KeyEvent, Runtime, Tx};
    use serde_json::json;

    fn app(kind: Kind) -> App {
        let mut app = App::new();
        app.load_fixture("auth", "Authentication", "[]");
        app.restore_composer_selection();
        let mut picker = Picker::new(kind, "Authentication", Vec::new());
        picker.query = rune::TextEdit::new("pending-input");
        picker.activity = Some(Activity { body: "Continue authorization in your browser".into(), status: "Waiting".into(), busy: true, show_input: true, url: None });
        app.composer.picker = Some(picker);
        if kind == Kind::ConnectSecret { app.composer.connect = Some(ConnectFlow { waiting: true, attempt_id: Some("pending-attempt".into()), ..Default::default() }); }
        else { app.composer.mcp_server = Some("pending-mcp".into()); app.composer.mcp_authorizing = true; }
        app
    }

    #[test]
    fn authentication_survives_background_click_focus_resize_and_selection_restore() {
        for kind in [Kind::ConnectSecret, Kind::McpActions] {
            let app = app(kind);
            let mut rt = Runtime::new(app, crate::ui::chrome::root, (1252.0, 674.0), Env::new(true, 1.0));
            for _ in 0..4 { rt.frame(1.0 / 60.0); }
            assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("prompt-picker")));
            rt.pointer_down(10.0, 10.0);
            rt.pointer_up(10.0, 10.0);
            rt.frame(1.0 / 60.0);
            assert!(rt.state.composer.picker.is_some(), "background click must not dismiss authentication");
            rt.pointer_down(10.0, 10.0);
            rt.window_focus(false);
            rt.set_viewport((1100.0, 700.0), 1.0);
            rt.window_focus(true);
            rt.pointer_up(10.0, 10.0);
            for _ in 0..4 { rt.frame(1.0 / 60.0); }
            rt.update(|a, _| { a.composer.selection_owner = None; a.restore_composer_selection(); });
            let picker = rt.state.composer.picker.as_ref().expect("authentication must remain open");
            assert_eq!(picker.kind, kind);
            assert_eq!(picker.query.text, "pending-input");
            assert!(picker.activity.as_ref().unwrap().busy);
            if kind == Kind::ConnectSecret { assert_eq!(rt.state.composer.connect.as_ref().unwrap().attempt_id.as_deref(), Some("pending-attempt")); }
            // Explicit Escape still backs out/cancels; a late response must not reopen it.
            rt.update(|a, tx| { a.picker_key(&KeyEvent { key: Key::Escape, mods: Default::default() }, tx); });
            rt.update(|a, tx| { if a.composer.picker.is_some() { a.picker_key(&KeyEvent { key: Key::Escape, mods: Default::default() }, tx); } });
            assert!(rt.state.composer.picker.is_none());
            rt.state.apply_slash_event(&Server::McpFailed { name: Some("pending-mcp".into()), error: "late failure".into() });
            assert!(rt.state.composer.picker.is_none());
        }
    }

    #[test]
    fn ordinary_model_picker_still_dismisses_on_background_click() {
        let mut app = app(Kind::Model);
        app.dismiss_prompt_picker();
        assert!(app.composer.picker.is_none());
    }

    #[test]
    fn mcp_connect_keeps_progress_open_until_the_connected_catalog_arrives() {
        let mut app = app(Kind::McpActions);
        app.composer.picker = Some(Picker::new(Kind::McpActions, "Server actions", vec![Row::new("Connect", "", "", json!({"name": "pending-mcp", "action": "connect"}).to_string())]));
        app.accept_picker_row(Some(0), &mut Tx::default());
        assert!(app.composer.mcp_authorizing);
        app.apply_slash_event(&Server::McpChanged { name: "pending-mcp".into() });
        assert_eq!(app.composer.picker.as_ref().unwrap().kind, Kind::McpActions);
        app.apply_slash_event(&Server::McpCatalog { status: json!({"pending-mcp": {"status": {"status": "connected"}, "runtimeConnected": true}}), directory: None });
        assert!(!app.composer.mcp_authorizing);
        assert_eq!(app.composer.picker.as_ref().unwrap().activity.as_ref().unwrap().status, "Connected");
        if let Some(path) = std::env::var_os("NEOISM_RUNE_MCP_CONNECTED_PNG") {
            use rune::render::{Gpu, Offscreen, Renderer, OFFSCREEN_FORMAT};
            let mut rt = Runtime::new(app, crate::ui::chrome::root, (1252.0, 674.0), Env::new(true, 1.0));
            for _ in 0..30 { rt.frame(1.0 / 60.0); }
            let gpu = Gpu::headless();
            let target = Offscreen::with_format(&gpu.device, 1252, 674, OFFSCREEN_FORMAT);
            let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
            renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (1252, 674));
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            renderer.draw(&mut enc, &target.view, [0.0, 0.0, 0.0, 1.0]);
            target.copy(&mut enc);
            renderer.submit(&gpu.queue, enc);
            rune::render::write_png(std::path::Path::new(&path), 1252, 674, &target.read(&gpu.device)).unwrap();
            rt.state.accept_picker_row(Some(0), &mut Tx::default());
            assert!(rt.state.composer.picker.is_none());
            return;
        }
        app.accept_picker_row(Some(0), &mut Tx::default());
        assert!(app.composer.picker.is_none());
    }

    #[test]
    fn unrelated_mcp_updates_do_not_replace_waiting_auth_or_other_pickers() {
        let mut app = app(Kind::McpActions);
        app.apply_slash_event(&Server::McpCatalog { status: json!({"pending-mcp": {"status": {"status": "connected"}, "runtimeConnected": true}}), directory: None });
        assert!(app.composer.picker.as_ref().unwrap().activity.as_ref().unwrap().busy, "a prior catalog snapshot must not finish a new authorization");
        app.apply_slash_event(&Server::McpChanged { name: "other-server".into() });
        app.apply_slash_event(&Server::McpFailed { name: Some("other-server".into()), error: "unrelated failure".into() });
        assert_eq!(app.composer.picker.as_ref().unwrap().activity.as_ref().unwrap().status, "Waiting");
        app.apply_slash_event(&Server::McpChanged { name: "pending-mcp".into() });
        assert_eq!(app.composer.picker.as_ref().unwrap().kind, Kind::McpActions);
        app.apply_slash_event(&Server::McpCatalog { status: json!({"pending-mcp": {"status": {"status": "connected"}, "runtimeConnected": true}}), directory: None });
        let picker = app.composer.picker.as_ref().unwrap();
        assert_eq!(picker.activity.as_ref().unwrap().status, "Connected");
        assert!(!picker.activity.as_ref().unwrap().busy);
        app.close_prompt_picker();
        app.composer.picker = Some(Picker::new(Kind::Model, "Models", vec![Row::new("model", "", "", "provider/model")]));
        app.apply_slash_event(&Server::McpOauthUrl { name: "pending-mcp".into(), url: "neoism-test://late-callback".into() });
        app.apply_slash_event(&Server::McpFailed { name: Some("pending-mcp".into()), error: "late failure".into() });
        assert!(app.composer.picker.as_ref().unwrap().activity.is_none());
        assert!(!app.picker_key(&KeyEvent { key: Key::Backspace, mods: Default::default() }, &mut Tx::default()));
    }

    #[test]
    fn mcp_completion_stops_waiting_on_runtime_failure_and_can_be_dismissed() {
        let mut app = app(Kind::McpActions);
        app.apply_slash_event(&Server::McpChanged { name: "pending-mcp".into() });
        app.apply_slash_event(&Server::McpCatalog { status: json!({"pending-mcp": {
            "status": {"status": "failed", "error": "Server unavailable"}, "runtimeConnected": false
        }}), directory: None });
        let activity = app.composer.picker.as_ref().unwrap().activity.as_ref().unwrap();
        assert!(!activity.busy);
        assert_eq!(activity.status, "Connection failed");
        assert_eq!(activity.body, "Server unavailable");
        app.accept_picker_row(Some(0), &mut Tx::default());
        assert!(app.composer.picker.is_none());
    }

    #[test]
    fn provider_success_is_not_overwritten_by_late_or_unrelated_failures() {
        let mut app = app(Kind::Connect);
        app.composer.connect = Some(ConnectFlow {
            waiting: true,
            provider: Some(catalog::ConnectProvider { id: "openai".into(), name: "OpenAI".into(), connected: false }),
            method: Some(catalog::AuthMethod { index: 0, label: "OAuth".into(), api: false }),
            ..Default::default()
        });
        app.apply_slash_event(&Server::ConnectFailed { provider: "other".into(), error: "Unrelated failure".into() });
        assert!(app.composer.connect.as_ref().unwrap().waiting);
        app.apply_slash_event(&Server::ConnectFinished { provider: "openai".into(), connection_id: None });
        app.apply_slash_event(&Server::ConnectFailed { provider: "openai".into(), error: "Late failure".into() });
        let activity = app.composer.picker.as_ref().unwrap().activity.as_ref().unwrap();
        assert_eq!(activity.status, "Connected");
        assert!(!activity.busy);
    }
}