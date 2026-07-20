use super::*;

#[wasm_bindgen]
impl ChromeBridge {
    // -------- service callback installers ------------------------

    pub fn set_list_dir(&self, cb: js_sys::Function) {
        self.services_state.0.borrow_mut().list_dir = Some(cb);
    }
    pub fn set_read_file(&self, cb: js_sys::Function) {
        self.services_state.0.borrow_mut().read_file = Some(cb);
    }
    pub fn set_write_file(&self, cb: js_sys::Function) {
        self.services_state.0.borrow_mut().write_file = Some(cb);
    }
    pub fn set_stat(&self, cb: js_sys::Function) {
        self.services_state.0.borrow_mut().stat = Some(cb);
    }
    pub fn set_clipboard_read(&self, cb: js_sys::Function) {
        self.services_state.0.borrow_mut().clipboard_read = Some(cb);
    }
    pub fn set_clipboard_write(&self, cb: js_sys::Function) {
        self.services_state.0.borrow_mut().clipboard_write = Some(cb);
    }
    /// JS pushes the latest clipboard contents here so the sync
    /// `ClipboardService::read()` shim has something to return.
    pub fn set_clipboard_value(&self, text: Option<String>) {
        self.services_state.0.borrow_mut().clipboard_cached = text;
    }
    pub fn set_command_run(&self, cb: js_sys::Function) {
        self.services_state.0.borrow_mut().command_run = Some(cb);
    }
    pub fn set_git_status(&self, cb: js_sys::Function) {
        self.services_state.0.borrow_mut().git_status = Some(cb);
    }
    pub fn set_git_diff(&self, cb: js_sys::Function) {
        self.services_state.0.borrow_mut().git_diff = Some(cb);
    }

    // -------- search-service callback installers -----------------
    //
    // Each callback receives `(req_id, envelope_json)` where the
    // envelope is a serialized `SearchClientMessage`. The TS host
    // ships the envelope to the workspace daemon over the
    // existing websocket and routes the reply back through
    // `service_reply(req_id, payload_json)`.

    pub fn set_search_collect_files(&self, cb: js_sys::Function) {
        self.services_state.0.borrow_mut().search_collect_files = Some(cb);
    }
    pub fn set_search_files(&self, cb: js_sys::Function) {
        self.services_state.0.borrow_mut().search_files = Some(cb);
    }
    pub fn set_search_grep(&self, cb: js_sys::Function) {
        self.services_state.0.borrow_mut().search_grep = Some(cb);
    }
    pub fn set_search_git_changes(&self, cb: js_sys::Function) {
        self.services_state.0.borrow_mut().search_git_changes = Some(cb);
    }
    pub fn set_search_git_repo_root(&self, cb: js_sys::Function) {
        self.services_state.0.borrow_mut().search_git_repo_root = Some(cb);
    }

    /// Install the JS callback that delivers OS-notification
    /// requests. Signature: `(title: string, body: string,
    /// level: "info" | "warn" | "error") => void`. The TS host is
    /// responsible for funneling the call through the browser's
    /// `Notification` API (lazily requesting permission) and
    /// falling back to the in-app toast stack when the API is
    /// unavailable or permission was denied.
    ///
    /// The bridge fires this from any
    /// `NotificationService::notify` call shared chrome makes; if
    /// no callback is installed the request is dropped silently
    /// (matches the other JS-backed service shims).
    pub fn set_notification_outbox(&self, cb: js_sys::Function) {
        self.services_state.0.borrow_mut().notification_outbox = Some(cb);
    }
}
