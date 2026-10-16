//! Desktop inline MCP authorization. The pane owns cancellation, not a modal.
use std::thread;
use std::time::{Duration, Instant};

use neoism_ui::widgets::modal::ModalActivity;
use neoism_ui::panels::agent_pane::state::picker::McpAuthorizationDetail;
use serde_json::{json, Value};

use super::{AgentBackgroundSender, NeoismAgentBackgroundUpdate, NeoismAgentPane, NeoismAgentPickerKind};
use crate::neoism::agent::api::{api_request_json_with_read_timeout, mcp_oauth_redirect_uri, percent_encode};

pub(super) struct PendingMcpOAuth {
    activity: ModalActivity,
    server: String,
    directory: Option<String>,
    detail: McpAuthorizationDetail,
}

impl Drop for PendingMcpOAuth {
    fn drop(&mut self) { self.activity.cancel(); }
}

impl NeoismAgentPane {
    pub(crate) fn open_mcp_oauth_activity(&mut self, name: String, connect_only: bool) -> ModalActivity {
        let activity = ModalActivity::default();
        let (body, status) = if connect_only {
            ("Connecting to the MCP server…", "Connecting")
        } else {
            ("Preparing MCP authorization…", "Not connected")
        };
        self.pending_mcp_oauth = Some(PendingMcpOAuth {
            activity: activity.clone(), server: self.server.clone(), directory: self.directory.clone(),
            detail: McpAuthorizationDetail {
                name: name.clone(), body: body.into(),
                status: status.into(), busy: true, url: None,
            },
        });
        self.reattach_mcp_oauth(&name);
        activity
    }

    pub(crate) fn start_mcp_oauth_monitor(&self, name: String, activity: ModalActivity, connect_only: bool) {
        let server = self.server.clone();
        let directory = self.directory.clone();
        let tx = self.background_tx.clone();
        thread::spawn(move || monitor(server, directory, name, activity, tx, connect_only));
    }

    pub(super) fn cancel_out_of_scope_mcp_oauth(&mut self) {
        if self.pending_mcp_oauth.as_ref().is_some_and(|pending| {
            pending.server != self.server || pending.directory != self.directory
        }) {
            if self.picker.as_ref().is_some_and(|picker| picker.mcp_authorization.is_some()) {
                self.picker = None;
            }
            self.pending_mcp_oauth = None;
        }
    }

    pub(crate) fn reattach_mcp_oauth(&mut self, name: &str) {
        self.cancel_out_of_scope_mcp_oauth();
        let Some(pending) = &self.pending_mcp_oauth else { return; };
        if pending.detail.name != name || !pending.detail.busy || pending.activity.is_cancelled() { return; }
        if let Some(picker) = self.picker.as_mut().filter(|p| {
            p.kind == NeoismAgentPickerKind::McpActions && p.title == format!("{name} actions")
        }) { picker.set_mcp_authorization(pending.detail.clone()); }
    }

    pub(super) fn apply_mcp_oauth_progress(&mut self, activity: ModalActivity, detail: McpAuthorizationDetail) -> bool {
        self.cancel_out_of_scope_mcp_oauth();
        let Some(pending) = self.pending_mcp_oauth.as_mut() else { return false; };
        if activity.is_cancelled() || pending.activity != activity || pending.detail.name != detail.name { return false; }
        pending.detail = detail.clone();
        // Back and close are sticky: progress may update the attempt, never reopen UI.
        if let Some(picker) = self.picker.as_mut().filter(|p| {
            p.kind == NeoismAgentPickerKind::McpActions
                && p.mcp_authorization.as_ref().is_some_and(|d| d.name == detail.name)
        }) { picker.set_mcp_authorization(detail.clone()); }
        if !detail.busy && detail.status == "Connected" && self.picker.as_ref().is_some_and(|p| {
            matches!(p.kind, NeoismAgentPickerKind::Mcp | NeoismAgentPickerKind::McpActions)
        }) { self.show_mcp(); }
        true
    }
}

// The authenticate route synthesizes this one failure after credentials exist,
// before the runtime is connected. No other failed status is authorization.
fn authorized(value: &Value) -> Result<bool, String> {
    match value.get("status").and_then(Value::as_str) {
        Some("connected") => Ok(true),
        Some("needs_auth") => Ok(false),
        Some("failed") if value.get("error").and_then(Value::as_str) == Some("MCP client runtime is not connected yet") => Ok(true),
        Some("failed" | "needs_client_registration") => Err(value.get("error").and_then(Value::as_str)
            .unwrap_or("MCP authorization is unavailable").to_string()),
        Some("disabled") => Err("MCP server is disabled".into()),
        _ => Err("MCP server returned an unexpected authorization status".into()),
    }
}

fn valid_authorization_url(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
}

pub(crate) fn open_authorization_url(url: &str) -> Result<(), String> {
    if !valid_authorization_url(url) { return Err("Invalid MCP authorization URL (expected HTTP or HTTPS)".into()); }
    crate::background_process::open_url(url).map(|_| ()).map_err(|error| error.to_string())
}

fn monitor(server: String, directory: Option<String>, name: String, activity: ModalActivity,
    tx: AgentBackgroundSender, connect_only: bool) {
    monitor_with_launcher(server, directory, name, activity, tx, connect_only, open_authorization_url);
}

fn monitor_with_launcher(server: String, directory: Option<String>, name: String, activity: ModalActivity,
    tx: AgentBackgroundSender, connect_only: bool, launch: impl FnOnce(&str) -> Result<(), String>) {
    let deadline = Instant::now() + Duration::from_secs(if connect_only { 35 } else { 600 });
    let query = directory.as_deref().map(|dir| format!("?directory={}", percent_encode(dir))).unwrap_or_default();
    let base = format!("/v2/plugins/dev.neoism.mcp/{}", percent_encode(&name));
    let mut link = None;
    let progress = |body: String, status: &str, busy: bool, url: Option<String>| {
        if !activity.is_cancelled() && tx.send(NeoismAgentBackgroundUpdate::McpOAuthProgress {
            activity: activity.clone(), detail: McpAuthorizationDetail {
                name: name.clone(), body, status: status.into(), busy, url,
            },
        }).is_err() { activity.cancel(); }
    };
    let request = |suffix: &str, query: &str, timeout: Duration| -> Result<Value, String> {
        if activity.is_cancelled() { return Err("Authorization cancelled".into()); }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() { return Err("Authorization timed out".into()); }
        let result = api_request_json_with_read_timeout(&server, "POST", &format!("{base}{suffix}{query}"),
            Some(&json!({})), timeout.min(remaining));
        if activity.is_cancelled() { return Err("Authorization cancelled".into()); }
        result.map(|value| value.unwrap_or(Value::Null))
    };
    let result = (|| -> Result<(), String> {
        if !connect_only {
            let redirect = mcp_oauth_redirect_uri(&server, &name)?;
            let auth_query = format!("{query}{}redirectUri={}", if query.is_empty() { "?" } else { "&" }, percent_encode(&redirect));
            let value = request("/auth", &auth_query, Duration::from_secs(15))?;
            let url = value.get("authorizationUrl").and_then(Value::as_str)
                .filter(|url| valid_authorization_url(url)).ok_or("MCP server returned no valid authorization URL")?;
            link = Some(url.to_string());
            if activity.is_cancelled() { return Ok(()); }
            let launch = launch(url);
            let body = match launch {
                Ok(_) => "Waiting for authorization in your browser. This picker will update automatically.".into(),
                Err(error) => format!("Could not open your browser ({error}). Use the authorization link to continue."),
            };
            progress(body, "Not connected", true, link.clone());
            loop {
                if activity.is_cancelled() { return Ok(()); }
                let value = request("/auth/authenticate", &query, Duration::from_secs(5))?;
                if authorized(&value)? { break; }
                for _ in 0..10 {
                    if activity.is_cancelled() { return Ok(()); }
                    thread::sleep(Duration::from_millis(100));
                }
            }
        }
        if activity.is_cancelled() { return Ok(()); }
        // The callback state has been consumed. Never offer the spent link again.
        link = None;
        progress("Authorization received. Connecting to the MCP server…".into(), "Connecting", true, None);
        let connected = request("/connect", &query, Duration::from_secs(35));
        if activity.is_cancelled() { return Ok(()); }
        match connected {
            Ok(Value::Bool(true)) => progress("MCP server connected.".into(), "Connected", false, None),
            Ok(_) => progress("Authorization succeeded, but the MCP runtime did not connect.".into(), "Authorized but not connected", false, link.clone()),
            Err(error) => progress(format!("Authorization succeeded, but connecting failed: {error}"), "Authorized but not connected", false, link.clone()),
        }
        Ok(())
    })();
    if let Err(error) = result { progress(error, if connect_only { "Authorized but not connected" } else { "Not connected" }, false, link); }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authenticate_accepts_only_exact_preconnection_sentinel() {
        assert_eq!(authorized(&json!({"status":"needs_auth"})), Ok(false));
        assert_eq!(authorized(&json!({"status":"failed","error":"MCP client runtime is not connected yet"})), Ok(true));
        assert!(authorized(&json!({"status":"failed","error":"unavailable"})).is_err());
        assert!(authorized(&json!({"status":"needs_client_registration","error":"MCP client runtime is not connected yet"})).is_err());
        assert!(authorized(&json!(true)).is_err());
    }
    // Exercise the real HTTP worker without launching a browser. A short-lived
    // server checks the auth-only redirect query and the poll/connect sequence.
    fn run_worker_case(statuses: Vec<Value>, cancel_after_ready: bool, connect_only: bool, connected: bool) -> Vec<McpAuthorizationDetail> {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let server = format!("http://{}/agent", listener.local_addr().unwrap());
        let expected_redirect = percent_encode(&mcp_oauth_redirect_uri(&server, "example").unwrap());
        let activity = ModalActivity::default();
        let cancel = activity.clone();
        let mock = thread::spawn(move || {
            let mut replies = Vec::new();
            if !connect_only {
                replies.push(("/auth", json!({"authorizationUrl":"https://Example.com/oauth)?state=a)b#Frag)"})));
                for status in statuses { replies.push(("/auth/authenticate", status)); }
            }
            let failure = replies.last().is_some_and(|(_, value)| authorized(value).is_err());
            if !failure && !cancel_after_ready { replies.push(("/connect", json!(connected))); }
            for (suffix, reply) in replies {
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut socket = loop {
                    match listener.accept() {
                        Ok((socket, _)) => break socket,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < deadline, "worker did not request {suffix}");
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) => panic!("{error}"),
                    }
                };
                socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut bytes = Vec::new();
                let mut chunk = [0; 1024];
                loop {
                    let count = socket.read(&mut chunk).unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&chunk[..count]);
                    if let Some(header_end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..header_end]);
                        let content_length = headers.lines().find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            if key.eq_ignore_ascii_case("content-length") { value.trim().parse::<usize>().ok() } else { None }
                        }).unwrap_or(0);
                        if bytes.len() >= header_end + 4 + content_length { break; }
                    }
                }
                let request = String::from_utf8_lossy(&bytes);
                let mut path = format!("/agent/v2/plugins/dev.neoism.mcp/example{suffix}?directory=%2Fworkspace");
                if suffix == "/auth" { path.push_str(&format!("&redirectUri={expected_redirect}")); }
                assert!(request.starts_with(&format!("POST {path} HTTP/1.1\r\n")), "{request}");
                if cancel_after_ready && suffix == "/auth/authenticate" { cancel.cancel(); }
                let body = reply.to_string();
                write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
            }
        });
        let pane = NeoismAgentPane::default();
        monitor_with_launcher(server, Some("/workspace".into()), "example".into(), activity,
            pane.background_tx.clone(), connect_only, |url| {
                assert_eq!(url, "https://Example.com/oauth)?state=a)b#Frag)");
                Ok(())
            });
        mock.join().unwrap();
        pane.background_rx.try_iter().filter_map(|event| match event {
            NeoismAgentBackgroundUpdate::McpOAuthProgress { detail, .. } => Some(detail), _ => None,
        }).collect()
    }

    #[test]
    fn needs_auth_then_exact_sentinel_connects_and_consumes_link() {
        let progress = run_worker_case(vec![json!({"status":"needs_auth"}),
            json!({"status":"failed","error":"MCP client runtime is not connected yet"})], false, false, true);
        assert!(progress.iter().any(|d| d.url.is_some()));
        assert_eq!(progress.last().unwrap().status, "Connected");
        assert!(progress.iter().filter(|d| d.status == "Connecting" || d.status == "Connected").all(|d| d.url.is_none()));
    }

    #[test]
    fn genuine_auth_failure_does_not_connect() {
        let progress = run_worker_case(vec![json!({"status":"failed","error":"denied"})], false, false, true);
        assert_eq!(progress.last().unwrap().body, "denied");
        assert_eq!(progress.last().unwrap().status, "Not connected");
    }

    #[test]
    fn cancelled_ready_response_never_connects_or_reports_success() {
        let progress = run_worker_case(vec![json!({"status":"failed","error":"MCP client runtime is not connected yet"})], true, false, true);
        assert!(progress.iter().all(|d| d.status != "Connecting" && d.status != "Connected"));
    }

    #[test]
    fn runtime_connect_false_is_not_connected_and_has_no_spent_link() {
        let progress = run_worker_case(vec![json!({"status":"failed","error":"MCP client runtime is not connected yet"})], false, false, false);
        let detail = progress.last().unwrap();
        assert_eq!(detail.status, "Authorized but not connected");
        assert!(detail.url.is_none());
        assert!(!detail.busy);
    }

    #[test]
    fn retry_connection_skips_auth_routes() {
        let progress = run_worker_case(Vec::new(), false, true, true);
        assert_eq!(progress.last().unwrap().status, "Connected");
        assert!(progress.iter().all(|d| d.url.is_none()));
    }

    #[test]
    fn authorization_link_requires_http_but_preserves_exact_url() {
        assert!(valid_authorization_url("https://Example.com/oauth)?state=a)b#Frag)"));
        assert!(!valid_authorization_url("file:///tmp/oauth"));
        assert!(!valid_authorization_url("javascript:alert(1)"));
    }
}
