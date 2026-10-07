//! Live subscription limits are separate from conversation token accounting.
use std::sync::atomic::{AtomicBool, Ordering};

use neoism_ui::panels::agent_pane::state::picker::NeoismAgentUsageAccount;

use super::*;
use crate::neoism::agent::api::api_request_json_while_active;

pub(super) struct PendingUsage {
    token: Arc<AtomicBool>,
    server: String,
}

impl Drop for PendingUsage {
    fn drop(&mut self) {
        self.token.store(false, Ordering::Release);
    }
}

impl NeoismAgentPane {
    pub fn open_usage_picker(&mut self) {
        if self.new_chat_source.provider().is_some() {
            return;
        }
        self.close_connect();
        self.picker = Some(NeoismAgentPicker::usage_loading());
        let token = Arc::new(AtomicBool::new(true));
        let server = self.server.clone();
        self.pending_usage = Some(PendingUsage {
            token: token.clone(),
            server: server.clone(),
        });
        let tx = self.background_sender();
        let worker_token = token.clone();
        if std::thread::Builder::new()
            .name("neoism-codex-usage".into())
            .spawn(move || {
                let result = fetch_usage_accounts(&server, &worker_token);
                if worker_token.load(Ordering::Acquire) {
                    let _ = tx.send(NeoismAgentBackgroundUpdate::UsageCompleted {
                        token: worker_token,
                        result,
                    });
                }
            })
            .is_err()
        {
            self.finish_usage_request(token, Err("Could not start usage refresh".into()));
        }
    }

    pub(super) fn finish_usage_request(
        &mut self,
        token: Arc<AtomicBool>,
        result: Result<Vec<NeoismAgentUsageAccount>, String>,
    ) {
        let Some(pending) = self.pending_usage.as_ref() else {
            return;
        };
        if !Arc::ptr_eq(&pending.token, &token) {
            return;
        }
        let owns_picker = self
            .picker
            .as_ref()
            .is_some_and(|picker| picker.kind == NeoismAgentPickerKind::Usage);
        if !token.load(Ordering::Acquire) || pending.server != self.server || !owns_picker
        {
            self.pending_usage = None;
            if owns_picker {
                self.picker = None;
            }
            return;
        }
        self.pending_usage = None;
        if let Some(picker) = self.picker.as_mut() {
            match result {
                Ok(accounts) => picker.set_usage_accounts(accounts),
                Err(error) => picker.set_usage_error(error),
            }
        }
    }
}

fn fetch_usage_accounts(
    server: &str,
    token: &AtomicBool,
) -> Result<Vec<NeoismAgentUsageAccount>, String> {
    let value = api_request_json_while_active(
        server,
        "GET",
        "/v2/providers/openai/usage",
        None,
        Duration::from_secs(45),
        token,
    )?
    .ok_or_else(|| "The agent returned an empty usage response".to_string())?;
    let accounts = value
        .get("accounts")
        .filter(|accounts| accounts.is_array())
        .ok_or_else(|| "The agent returned malformed usage data".to_string())?;
    serde_json::from_value(accounts.clone())
        .map_err(|_| "The agent returned malformed usage accounts".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loading_pane() -> (NeoismAgentPane, Arc<AtomicBool>) {
        let mut pane = NeoismAgentPane::default();
        let token = Arc::new(AtomicBool::new(true));
        pane.pending_usage = Some(PendingUsage {
            token: token.clone(),
            server: pane.server.clone(),
        });
        let mut picker =
            NeoismAgentPicker::new(NeoismAgentPickerKind::Usage, "Codex", Vec::new(), 0);
        picker.set_loading(true);
        pane.picker = Some(picker);
        (pane, token)
    }

    #[test]
    fn usage_escape_cancels_and_does_not_reopen() {
        let (mut pane, token) = loading_pane();
        pane.close_picker();
        assert!(!token.load(Ordering::Acquire));
        pane.finish_usage_request(token, Ok(Vec::new()));
        assert!(pane.picker.is_none());
    }

    #[test]
    fn usage_old_refresh_cannot_replace_new_request() {
        let (mut pane, token) = loading_pane();
        let replacement = Arc::new(AtomicBool::new(true));
        pane.pending_usage = Some(PendingUsage {
            token: replacement.clone(),
            server: pane.server.clone(),
        });
        pane.finish_usage_request(token, Err("stale error".into()));
        assert!(Arc::ptr_eq(
            &pane.pending_usage.as_ref().unwrap().token,
            &replacement
        ));
        assert!(pane.picker.as_ref().unwrap().loading);
    }

    #[test]
    fn usage_server_switch_dismisses_stale_loading_panel() {
        let (mut pane, token) = loading_pane();
        pane.server = "http://different-server:4096".into();
        pane.finish_usage_request(token, Ok(Vec::new()));
        assert!(pane.picker.is_none());
        assert!(pane.pending_usage.is_none());
    }

    #[test]
    fn usage_enter_refresh_is_not_a_prompt() {
        let (mut pane, _token) = loading_pane();
        pane.input = "unsent draft".into();
        assert!(pane.commit_picker());
        assert_eq!(pane.input, "unsent draft");
        assert!(pane.messages.is_empty());
        assert!(pane.session_id.is_none());
    }
}
