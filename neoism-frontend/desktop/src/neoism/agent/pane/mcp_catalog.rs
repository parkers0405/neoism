//! Passive, visible-only MCP catalog refresh, independent of chat event streams.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::Duration;

use serde_json::Value;

use super::{NeoismAgentBackgroundUpdate, NeoismAgentPane, NeoismAgentPickerKind};
use crate::neoism::agent::api::{api_request_json_with_read_timeout, percent_encode};

#[derive(Default)]
pub(super) struct McpCatalogMonitor {
    token: u64,
    active: Option<Arc<AtomicBool>>,
    request_busy: Arc<AtomicBool>,
    refresh_requested: Arc<AtomicBool>,
    reported_error: bool,
}

impl Drop for McpCatalogMonitor {
    fn drop(&mut self) {
        if let Some(active) = self.active.take() {
            active.store(false, Ordering::Release);
        }
    }
}

impl NeoismAgentPane {
    /// Explicit actions/invalidation interrupt the passive poll interval. Repeated
    /// requests coalesce, including one trailing read if HTTP is already running.
    pub(crate) fn request_mcp_catalog_refresh(&mut self) {
        self.mcp_catalog_monitor
            .refresh_requested
            .store(true, Ordering::Release);
        if let Some(picker) = self.picker.as_mut().filter(|picker| {
            matches!(
                picker.kind,
                NeoismAgentPickerKind::Mcp | NeoismAgentPickerKind::McpActions
            )
        }) {
            // Auth detail owns its live state; passive catalog work must not
            // disable its browser link or Back action.
            if picker.mcp_authorization.is_none() {
                picker.set_loading(true);
            }
        }
        self.reconcile_mcp_catalog_monitor();
    }

    pub(crate) fn reconcile_mcp_catalog_monitor(&mut self) {
        let visible = self.picker.as_ref().is_some_and(|picker| {
            matches!(
                picker.kind,
                NeoismAgentPickerKind::Mcp | NeoismAgentPickerKind::McpActions
            )
        });
        if !visible {
            if let Some(active) = self.mcp_catalog_monitor.active.take() {
                active.store(false, Ordering::Release);
            }
            return;
        }
        if self.mcp_catalog_monitor.active.is_some() {
            return;
        }
        self.mcp_catalog_monitor.token = self.mcp_catalog_monitor.token.wrapping_add(1);
        self.mcp_catalog_monitor.reported_error = false;
        let token = self.mcp_catalog_monitor.token;
        let active = Arc::new(AtomicBool::new(true));
        self.mcp_catalog_monitor.active = Some(active.clone());
        let server = self.server.clone();
        let directory = self.directory.clone();
        let tx = self.background_tx.clone();
        let request_busy = self.mcp_catalog_monitor.request_busy.clone();
        let refresh_requested = self.mcp_catalog_monitor.refresh_requested.clone();
        thread::spawn(move || {
            let path = directory
                .as_deref()
                .map(|dir| {
                    format!(
                        "/v2/plugins/dev.neoism.mcp/catalog?directory={}",
                        percent_encode(dir)
                    )
                })
                .unwrap_or_else(|| "/v2/plugins/dev.neoism.mcp/catalog".into());
            while active.load(Ordering::Acquire) {
                if request_busy
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                    .is_err()
                {
                    thread::sleep(Duration::from_millis(100));
                    continue;
                }
                // Clear before HTTP, not after: an invalidation during the read
                // must survive and request one immediate trailing read.
                refresh_requested.store(false, Ordering::Release);
                // One worker owns all reads. Neither opening a picker nor drawing
                // a frame blocks on HTTP, and GET never connects an MCP server.
                let result = api_request_json_with_read_timeout(
                    &server,
                    "GET",
                    &path,
                    None,
                    Duration::from_secs(5),
                )
                .map(|value| value.unwrap_or(Value::Null));
                request_busy.store(false, Ordering::Release);
                if !active.load(Ordering::Acquire) {
                    break;
                }
                if tx
                    .send(NeoismAgentBackgroundUpdate::McpCatalogRefreshed {
                        token,
                        server: server.clone(),
                        directory: directory.clone(),
                        result,
                    })
                    .is_err()
                {
                    break;
                }
                for _ in 0..20 {
                    if !active.load(Ordering::Acquire) {
                        return;
                    }
                    if refresh_requested.load(Ordering::Acquire) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
        });
    }

    pub(crate) fn finish_mcp_catalog_refresh(
        &mut self,
        token: u64,
        server: String,
        directory: Option<String>,
        result: Result<Value, String>,
    ) {
        if token != self.mcp_catalog_monitor.token
            || self.mcp_catalog_monitor.active.is_none()
        {
            return;
        }
        if self.server != server || self.directory != directory {
            if let Some(active) = self.mcp_catalog_monitor.active.take() {
                active.store(false, Ordering::Release);
            }
            self.reconcile_mcp_catalog_monitor();
            return;
        }
        let Some(picker) = self.picker.as_mut().filter(|picker| {
            matches!(
                picker.kind,
                NeoismAgentPickerKind::Mcp | NeoismAgentPickerKind::McpActions
            )
        }) else {
            return;
        };
        // A response queued before an explicit invalidation must not unlock
        // stale actions while its coalesced trailing read is still pending.
        if picker.loading
            && (self
                .mcp_catalog_monitor
                .refresh_requested
                .load(Ordering::Acquire)
                || self
                    .mcp_catalog_monitor
                    .request_busy
                    .load(Ordering::Acquire))
        {
            return;
        }
        match result {
            Ok(status) => {
                picker.apply_mcp_status(&status);
                self.mcp_catalog_monitor.reported_error = false;
            }
            Err(error) => {
                picker.set_loading(false);
                // Do not spam the transcript on every retry during an outage.
                if !self.mcp_catalog_monitor.reported_error {
                    self.mcp_catalog_monitor.reported_error = true;
                    if picker.mcp_authorization.is_none() {
                        self.system_message("MCP", error);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_refresh_keeps_inline_authorization_actions_interactive() {
        use neoism_ui::panels::agent_pane::state::picker::{McpAuthorizationDetail, NeoismAgentPicker};

        let mut pane = NeoismAgentPane::default();
        pane.mcp_catalog_monitor.active = Some(Arc::new(AtomicBool::new(true)));
        let mut picker = NeoismAgentPicker::new(NeoismAgentPickerKind::McpActions, "webflow actions", Vec::new(), 0);
        picker.set_mcp_authorization(McpAuthorizationDetail {
            name: "webflow".into(), body: "Waiting in your browser".into(),
            status: "Not connected".into(), busy: true,
            url: Some("https://example.com/auth".into()),
        });
        pane.picker = Some(picker);
        pane.request_mcp_catalog_refresh();
        assert!(!pane.picker.as_ref().unwrap().loading);
        pane.finish_mcp_catalog_refresh(
            0, pane.server.clone(), pane.directory.clone(),
            Err("A catalog refresh failed\nwithout failing authorization".into()),
        );
        assert!(pane.picker.as_ref().unwrap().mcp_authorization.is_some());
        assert!(pane.drain_ui_events().is_empty());
        assert!(!pane.picker.as_ref().unwrap().loading);
    }

    #[test]
    fn explicit_refresh_coalesces_and_keeps_actions_locked_until_trailing_read() {
        let mut pane = NeoismAgentPane::default();
        // Stand in for an existing worker so this test never starts HTTP.
        pane.mcp_catalog_monitor.active = Some(Arc::new(AtomicBool::new(true)));
        pane.show_mcp();
        assert!(pane
            .mcp_catalog_monitor
            .refresh_requested
            .load(Ordering::Acquire));
        assert!(pane.picker.as_ref().unwrap().loading);

        // Consume the initial request before HTTP, then invalidate mid-read.
        pane.mcp_catalog_monitor
            .refresh_requested
            .store(false, Ordering::Release);
        pane.mcp_catalog_monitor
            .request_busy
            .store(true, Ordering::Release);
        pane.show_mcp();
        pane.show_mcp();
        assert!(pane
            .mcp_catalog_monitor
            .refresh_requested
            .load(Ordering::Acquire));
        let status = serde_json::json!({"webflow": {"status": {"status": "connected"}}});
        pane.finish_mcp_catalog_refresh(
            0,
            pane.server.clone(),
            pane.directory.clone(),
            Ok(status.clone()),
        );
        assert!(pane.picker.as_ref().unwrap().loading);
        assert!(pane.picker.as_ref().unwrap().options().is_empty());

        // The bit is consumed only before the trailing read. An older queued
        // result must not unlock actions while that read is still in flight.
        pane.mcp_catalog_monitor
            .refresh_requested
            .store(false, Ordering::Release);
        pane.finish_mcp_catalog_refresh(
            0,
            pane.server.clone(),
            pane.directory.clone(),
            Ok(status.clone()),
        );
        assert!(pane.picker.as_ref().unwrap().loading);
        pane.mcp_catalog_monitor
            .request_busy
            .store(false, Ordering::Release);
        pane.finish_mcp_catalog_refresh(
            0,
            pane.server.clone(),
            pane.directory.clone(),
            Ok(status),
        );
        let picker = pane.picker.as_ref().unwrap();
        assert!(!picker.loading);
        assert_eq!(picker.selected_option().unwrap().footer, "connected");
    }
}
