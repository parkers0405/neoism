use super::*;
use neoism_ui::panels::file_browser::{FileBrowserEntry, FileBrowserLocation, FileBrowserMode};

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
        let locations: Vec<FileBrowserLocation> = match serde_json::from_str(locations_json) {
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
        if !self.chrome.file_browser.is_active() { return false; }
        self.chrome.file_browser.pointer_down(x, y, click_count);
        true
    }

    pub fn file_browser_scroll(&mut self, delta_pixels: f32) -> bool {
        if !self.chrome.file_browser.is_active() { return false; }
        self.chrome.file_browser.scroll_pixels(delta_pixels);
        true
    }
}