use super::*;
use neoism_ui::widgets::conversation_navigator::{
    ConversationNavigatorAction as Action, ConversationNavigatorMode as Mode,
    ConversationNavigatorRow as Row,
};

#[wasm_bindgen]
impl ChromeBridge {
    pub fn conversation_navigator_set_dashboard_root(&mut self, root: Option<String>) {
        self.chrome.set_conversation_dashboard_root(root);
    }

    pub fn drain_catalog_header_action(&mut self) -> Option<String> {
        use neoism_ui::panels::agent_pane::state::side_panel::CatalogHeaderAction;
        self.chrome.take_catalog_header_action().map(|action| {
            match action {
                CatalogHeaderAction::Search => "search",
                CatalogHeaderAction::Projects => "projects",
                CatalogHeaderAction::AddWorkspace => "add_workspace",
            }
            .to_owned()
        })
    }

    pub fn conversation_navigator_open(&mut self, mode: &str, rows_json: &str) {
        let mode = if mode == "projects" {
            Mode::Projects
        } else {
            Mode::Search
        };
        let rows = navigator_rows(rows_json);
        let anchor = self
            .chrome
            .agent_pane
            .as_ref()
            .and_then(|pane| pane.side_panel().project_button_rect());
        self.chrome.conversation_navigator.open(mode, rows);
        self.chrome.conversation_navigator.set_anchor(anchor);
    }
    pub fn conversation_navigator_close(&mut self) {
        self.chrome.conversation_navigator.close();
    }
    pub fn conversation_navigator_active(&self) -> bool {
        self.chrome.conversation_navigator.is_active()
    }
    pub fn conversation_navigator_query(&self) -> String {
        self.chrome.conversation_navigator.query().to_owned()
    }
    pub fn conversation_navigator_set_rows(&mut self, json: &str) {
        self.chrome
            .conversation_navigator
            .set_rows(navigator_rows(json));
    }
    pub fn conversation_navigator_set_loading(&mut self, loading: bool) {
        self.chrome.conversation_navigator.set_loading(loading);
    }
    pub fn conversation_navigator_set_indexing(&mut self, indexing: bool) {
        self.chrome.conversation_navigator.set_indexing(indexing);
    }
    pub fn conversation_navigator_set_error(&mut self, error: Option<String>) {
        self.chrome.conversation_navigator.set_error(error);
    }
    pub fn drain_conversation_navigator_action(&mut self) -> JsValue {
        let value = self
            .chrome
            .take_navigator_action()
            .map(|action| match action {
                Action::Activate(value) => {
                    serde_json::json!({"kind": "activate", "value": value})
                }
                Action::Dismiss => serde_json::json!({"kind": "dismiss"}),
            });
        serde_wasm_bindgen::to_value(&value).unwrap_or(JsValue::NULL)
    }
    pub fn conversation_navigator_attach(
        &mut self,
        session_id: &str,
        directory: Option<String>,
    ) {
        self.agent_switch_thread(session_id);
        if let Some(pane) = self.chrome.agent_pane_mut() {
            pane.set_directory(directory);
        }
    }
    pub fn enable_conversation_navigation_header(&mut self) {
        if let Some(pane) = self.chrome.agent_pane.as_mut() {
            pane.side_panel_mut().set_navigation_header_enabled(true);
        }
    }
}

fn navigator_rows(json: &str) -> Vec<Row> {
    let values: Vec<serde_json::Value> = serde_json::from_str(json).unwrap_or_default();
    values
        .into_iter()
        .map(|v| {
            let text = |key| {
                v.get(key)
                    .and_then(|x| x.as_str())
                    .unwrap_or_default()
                    .to_owned()
            };
            Row {
                value: text("value"),
                title: text("title"),
                description: text("description"),
                footer: text("footer"),
                is_current: v["is_current"].as_bool().unwrap_or(false),
                is_disabled: v["is_disabled"].as_bool().unwrap_or(false),
            }
        })
        .collect()
}
