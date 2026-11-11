//! Account usage is an existing local agent HTTP read, performed off the display thread.
use super::{Kind, Picker, picker::Row};
use crate::app::App;
use serde::Deserialize;
use std::{net::SocketAddr, sync::mpsc};

#[derive(Deserialize, Default)]
pub struct Usage {
    #[serde(default)]
    pub accounts: Vec<Account>,
}
#[derive(Deserialize)]
pub struct Account {
    pub connection_id: String,
    pub label: String,
    pub plan_type: Option<String>,
    pub error: Option<String>,
    #[serde(default)]
    pub windows: Vec<Window>,
}
#[derive(Deserialize)]
pub struct Window {
    pub label: String,
    pub used_percent: f64,
    pub reset_at: Option<i64>,
}

impl Account {
    fn description(&self) -> String {
        if let Some(error) = &self.error {
            return error.clone();
        }
        if self.windows.is_empty() {
            return "No usage windows reported".into();
        }
        self.windows
            .iter()
            .map(|w| {
                let reset = w
                    .reset_at
                    .map(|at| {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs() as i64;
                        let minutes = ((at - now).max(0) + 59) / 60;
                        if minutes >= 60 {
                            format!(" · resets in {}h {}m", minutes / 60, minutes % 60)
                        } else {
                            format!(" · resets in {minutes}m")
                        }
                    })
                    .unwrap_or_default();
                format!("{}: {}% used{reset}", w.label, w.used_percent)
            })
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

fn fetch() -> Result<Usage, String> {
    let configured = std::env::var("NEOISM_AGENT_SERVER")
        .or_else(|_| std::env::var("NEOISM_SERVER"))
        .ok();
    // Beside the daemon's own endpoint, wherever this system keeps it.
    let published = std::fs::read(neoism_agent_service_api::daemon_credential::canonical_daemon_token_path().with_file_name("agent-endpoint.json")).ok()
        .and_then(|data| serde_json::from_slice::<serde_json::Value>(&data).ok())
        .and_then(|record| record["address"].as_str().map(|a| format!("http://{a}")));
    let endpoint = configured
        .or(published)
        .unwrap_or_else(|| "http://127.0.0.1:4096".into());
    let authority = endpoint
        .strip_prefix("http://")
        .ok_or("Usage requires a local HTTP agent endpoint")?
        .trim_end_matches('/');
    let address: SocketAddr = authority
        .parse()
        .map_err(|_| "Invalid local agent address")?;
    if !address.ip().is_loopback() || address.port() == 0 {
        return Err("Agent endpoint must be local".into());
    }
    let output = crate::cli::quiet("curl")
        .args([
            "--silent",
            "--show-error",
            "--fail",
            "--max-time",
            "20",
            "--max-filesize",
            "1048576",
            &format!("http://{address}/v2/providers/openai/usage"),
        ])
        .output()
        .map_err(|e| format!("Could not read usage: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Invalid usage response: {e}"))
}

impl App {
    pub fn open_usage(&mut self) {
        self.focus_picker(Picker::loading(Kind::Usage, "Codex usage"));
        let (send, receive) = mpsc::channel();
        self.composer.usage_request = Some(receive);
        let wake = self.waker.clone();
        std::thread::spawn(move || {
            let _ = send.send(fetch());
            if let Some(wake) = wake {
                wake();
            }
        });
    }

    pub fn poll_usage(&mut self) -> bool {
        let result = self
            .composer
            .usage_request
            .as_ref()
            .and_then(|r| r.try_recv().ok());
        let Some(result) = result else { return false };
        self.composer.usage_request = None;
        if !self
            .composer
            .picker
            .as_ref()
            .is_some_and(|p| p.kind == Kind::Usage)
        {
            return false;
        }
        let mut rows = match result {
            Ok(usage) => usage
                .accounts
                .iter()
                .map(|a| {
                    Row::new(
                        &a.label,
                        a.description(),
                        a.plan_type.clone().unwrap_or_default(),
                        &a.connection_id,
                    )
                })
                .collect::<Vec<_>>(),
            Err(error) => vec![Row::new("Could not load usage", error, "", "")],
        };
        if rows.is_empty() {
            rows.push(Row::new(
                "No Codex accounts connected",
                "Use /connect to add an account",
                "",
                "",
            ));
        }
        rows.push(Row::new(
            "Refresh",
            "Fetch current usage and reset times",
            "",
            "refresh",
        ));
        self.focus_picker(Picker::new(Kind::Usage, "Codex usage", rows));
        true
    }
}
