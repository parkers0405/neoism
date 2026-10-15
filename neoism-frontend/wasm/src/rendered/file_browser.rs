use super::*;
use neoism_ui::panels::file_browser::{
    FileBrowserEntry, FileBrowserLocation, FileBrowserMode,
};

#[wasm_bindgen]
impl ChromeBridge {
    pub fn open_file_browser(&mut self, mode: &str, start: &str, recents_json: &str) {
        let mode = match mode {
            "choose_directory" => FileBrowserMode::ChooseDirectory,
            "open_file" => FileBrowserMode::OpenFile,
            _ => FileBrowserMode::AttachImage,
        };
        let recents = serde_json::from_str(recents_json).unwrap_or_default();
        self.chrome.open_file_browser(mode, start, recents);
    }

    /// Hand the host a captured action rather than opening a browser without a
    /// target. JS binds it to the current daemon, workspace and pane before IO.
    pub fn drain_agent_file_browser_action(&mut self) -> JsValue {
        serde_wasm_bindgen::to_value(&self.pending_agent_file_browser.take())
            .unwrap_or(JsValue::NULL)
    }

    /// Existing sessions use `/cd`; blank panes keep a directory draft for
    /// their first CreateThread. Neither path touches/submits composer input.
    pub fn agent_change_directory(
        &mut self,
        session_id: Option<String>,
        path: &str,
    ) -> bool {
        if self.agent_session_id() != session_id
            || self.chrome.agent_pane().is_none()
            || path.trim().is_empty()
        {
            return false;
        }
        if let Some(session_id) = session_id {
            self.send_agent_envelope(
                &neoism_protocol::agent::AgentClientMessage::SlashCommand {
                    session_id,
                    text: format!("/cd {path}"),
                },
            )
        } else {
            // A submitted first prompt may be awaiting ThreadCreated while the
            // displayed session is still null. Its execution directory is fixed.
            crate::agent_directory::set_blank_directory(
                self.chrome.agent_pane_mut().expect("pane checked above"),
                path,
                self.agent_state.thread_create_inflight,
                self.agent_state.pending_prompt.is_some(),
            )
        }
    }

    pub fn agent_directory(&self) -> Option<String> {
        crate::agent_directory::execution_directory(
            self.chrome.agent_pane(),
            self.agent_state.default_directory.as_deref(),
        )
    }

    pub fn file_browser_path(&self) -> String {
        self.chrome.file_browser.current_path().to_owned()
    }

    pub fn file_browser_active(&self) -> bool {
        self.chrome.file_browser.is_active()
    }

    pub fn drain_file_browser_requests(&mut self) -> JsValue {
        serde_wasm_bindgen::to_value(&self.chrome.file_browser.drain_requests())
            .unwrap_or(JsValue::NULL)
    }

    pub fn set_file_browser_entries(&mut self, path: &str, entries_json: &str) -> bool {
        let entries: Vec<FileBrowserEntry> = match serde_json::from_str(entries_json) {
            Ok(v) => v,
            Err(_) => return false,
        };
        self.chrome.file_browser.set_listing(path, entries)
    }

    pub fn set_file_browser_locations(&mut self, locations_json: &str) -> bool {
        let locations: Vec<FileBrowserLocation> =
            match serde_json::from_str(locations_json) {
                Ok(value) => value,
                Err(_) => return false,
            };
        self.chrome.file_browser.set_locations(locations)
    }

    pub fn set_file_browser_error(&mut self, message: &str) {
        self.chrome.file_browser.set_error(message);
    }

    pub fn set_file_browser_recents(&mut self, recents_json: &str) {
        let recents = serde_json::from_str(recents_json).unwrap_or_default();
        self.chrome.file_browser.set_recents(recents);
    }

    pub fn drain_file_browser_selection(&mut self) -> JsValue {
        serde_wasm_bindgen::to_value(&self.chrome.file_browser.take_selection())
            .unwrap_or(JsValue::NULL)
    }

    pub fn file_browser_pointer_down(&mut self, x: f32, y: f32, click_count: u8) -> bool {
        if !self.chrome.file_browser.is_active() {
            return false;
        }
        self.chrome.file_browser.pointer_down(x, y, click_count);
        true
    }

    pub fn file_browser_scroll(&mut self, delta_pixels: f32) -> bool {
        if !self.chrome.file_browser.is_active() {
            return false;
        }
        self.chrome.file_browser.scroll_pixels(delta_pixels);
        true
    }
}
