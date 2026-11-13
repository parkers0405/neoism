//! Account usage is an existing local agent HTTP read, performed off the display thread.
use super::{Kind, Picker};
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
    pub label: String,
    #[serde(default)]
    pub is_default: bool,
    #[serde(default)]
    pub auth_type: String,
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
    fn plan_label(&self) -> String {
        let plan = match self.plan_type.as_deref().unwrap_or("").trim().to_ascii_lowercase().as_str() {
            "pro" => "Pro", "plus" => "Plus", "free" => "Free", "business" => "Business",
            "enterprise" => "Enterprise", "team" => "Team", "edu" => "Edu",
            _ => match self.auth_type.to_ascii_lowercase().as_str() {
                "oauth" => "ChatGPT",
                "api" | "api_key" | "api-key" | "apikey" => "API key",
                _ => "Plan unavailable",
            },
        };
        if self.is_default { format!("{plan} · default") } else { plan.into() }
    }
}

impl Window {
    fn remaining(&self) -> Option<f64> {
        self.used_percent.is_finite().then(|| 100.0 - self.used_percent.clamp(0.0, 100.0))
    }

    fn reset_label(&self, now: i64) -> String {
        let Some(reset) = self.reset_at else { return "Reset unavailable".into() };
        let seconds = reset.saturating_sub(now);
        if seconds <= 0 { return "Reset due now".into(); }
        let minutes = seconds.saturating_add(59) / 60;
        let hours = minutes / 60;
        let relative = if minutes < 60 { format!("Resets in {minutes}m") }
            else if hours < 24 { format!("Resets in {hours}h {}m", minutes % 60) }
            else { format!("Resets in {}d {}h", hours / 24, hours % 24) };
        // Gregorian month/day from Unix days, matching the original UTC reset label.
        let days = reset.div_euclid(86_400) + 719468;
        let era = days.div_euclid(146097);
        let doe = days - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = mp + if mp < 10 { 3 } else { -9 };
        let time = reset.rem_euclid(86_400);
        format!("{relative} · {month:02}/{day:02}, {:02}:{:02} UTC", time / 3600, time % 3600 / 60)
    }
}

/// Read-only account dashboard, using the GPUiX usage picker's exact geometry.
pub(super) fn view(app: &App, composer: [f32; 4]) -> rune::El {
    use crate::ui::theme::rgb;
    use rune::{el, text, tok::TextStyle, Tx, WheelEvent};
    let picker = app.composer.picker.as_ref().expect("usage picker");
    let p = app.pal;
    let s = app.ty.scale();
    let [cx, cy, cw, _] = composer;
    let top = app.rects.get(&format!("agent-area:{}", app.active_tab().id)).map_or(0.0, |r| r[1]) + 8.0 * s;
    let available = (cy - top - 6.0 * s).max(0.0);
    let header = (40.0 * s).min(available);
    let footer = if available >= header + 60.0 * s { 26.0 * s } else { 0.0 };
    let stacked = cw / s < 420.0;
    let accounts = &picker.usage.accounts;
    let windows = accounts.iter().map(|a| a.windows.len().max(1) + usize::from(a.error.is_some() && !a.windows.is_empty())).max().unwrap_or(1);
    let row_h = (if stacked { 62.0 } else { 24.0 } + 60.0 * windows as f32) * s;
    let preferred = if picker.loading { 46.0 * 3.0 * s } else { row_h * accounts.len().clamp(1, 3) as f32 };
    let body_h = preferred.min((available - header - footer).max(0.0));
    let height = header + body_h + footer;
    let max_scroll = (accounts.len() as f32 * row_h - body_h).max(0.0);
    picker.usage_max_scroll.set(max_scroll);
    let scroll = picker.usage_scroll.clamp(0.0, max_scroll);
    let label = |value: String, width: f32, bold: bool, muted: bool| {
        text(value).font(TextStyle::new(13.0 * s, if bold { 700.0 } else { 400.0 }).mono())
            .color(rgb(if muted { p.muted } else { p.fg })).w(width.max(0.0)).truncate()
    };
    let mut card = el().key("usage-picker").w(cw).h(height).fill(rgb(p.bg)).radius(14.0 * s)
        .border(s, rgb(p.border)).clip_rounded(14.0 * s)
        .on_press(|_: &mut App| {})
        .on_wheel(move |a: &mut App, e: WheelEvent, tx: &mut Tx| {
            if let Some(picker) = a.composer.picker.as_mut() {
                let delta = if e.precise { -e.dy } else if e.dy < 0.0 { row_h } else { -row_h };
                picker.usage_scroll = (picker.usage_scroll + delta).clamp(0.0, picker.usage_max_scroll.get());
                tx.animated();
            }
            true
        })
        .child(label(format!("Codex  ·  {} {}", accounts.len(), if accounts.len() == 1 { "account" } else { "accounts" }), cw - 64.0 * s, true, false).absolute(14.0 * s, 12.0 * s))
        .child(label("esc".into(), 28.0 * s, false, true).absolute(cw - 38.0 * s, 12.0 * s)
            .on_press(|a: &mut App| a.close_prompt_picker()));
    let mut body = el().w(cw - 2.0 * s).h(body_h).clip_rounded(0.0).absolute(s, header);
    if picker.loading {
        for i in 0..3 {
            body = body.child(el().w((cw - 44.0 * s) * [0.42, 0.6, 0.35][i]).h(10.0 * s)
                .radius(5.0 * s).fill(rgb(p.surface)).absolute(22.0 * s, (18.0 + i as f32 * 46.0) * s));
        }
    } else if let Some(message) = picker.usage_error.as_deref().or_else(|| accounts.is_empty().then_some("No Codex accounts connected")) {
        body = body.child(label(message.into(), cw - 28.0 * s, false, true).absolute(13.0 * s, 16.0 * s));
    } else {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs().min(i64::MAX as u64) as i64;
        let mut content = el().w(cw - 2.0 * s).h(accounts.len() as f32 * row_h)
            .absolute(0.0, 0.0).offset(0.0, -scroll).offset_motion(rune::Spring::new(0.188, 0.0));
        for (index, account) in accounts.iter().enumerate() {
            if (index as f32 + 1.0) * row_h < scroll - body_h || index as f32 * row_h > scroll + body_h * 2.0 { continue; }
            let mut row = el().key(format!("usage-account:{index}")).w(cw - 2.0 * s).h(row_h).absolute(0.0, index as f32 * row_h);
            if account.is_default {
                let blend = |shift: u32| ((((p.bg >> shift) & 255u32) as f32 * 0.65 + ((p.hover >> shift) & 255u32) as f32 * 0.35) as u32) << shift;
                row = row.fill(rgb(blend(16) | blend(8) | blend(0)));
            }
            let left_w = if stacked { cw - 28.0 * s } else { cw * 0.34 - 20.0 * s };
            row = row.child(label(if account.label.trim().is_empty() { "Account".into() } else { account.label.clone() }, left_w, true, false).absolute(13.0 * s, 12.0 * s))
                .child(label(account.plan_label(), left_w, false, true).absolute(13.0 * s, 33.0 * s));
            let wx = if stacked { 14.0 * s } else { cw * 0.34 };
            let ww = (cw - 14.0 * s - wx).max(0.0);
            let wy = if stacked { 58.0 } else { 12.0 } * s;
            for (wi, window) in account.windows.iter().enumerate() {
                let y = wy + wi as f32 * 60.0 * s;
                let remaining = window.remaining();
                let percent = remaining.map(|v| format!("{v:.0}% left")).unwrap_or_else(|| "Unavailable".into());
                let pw = (percent.chars().count() as f32 * 13.0 * 0.6 * s).min(ww);
                let title = match window.label.trim().to_ascii_lowercase().as_str() {
                    "weekly" => "Weekly limit", "5-hour" => "5-hour limit", "" => "Usage limit", _ => &window.label,
                };
                row = row.child(label(title.into(), ww - pw - 8.0 * s, false, false).absolute(wx - s, y))
                    .child(label(percent, pw, true, false).absolute(wx + ww - pw - s, y))
                    .child(el().w(ww).h(3.0 * s).fill(rgb(p.border)).absolute(wx - s, y + 23.0 * s));
                if let Some(value) = remaining {
                    row = row.child(el().w(ww * (value / 100.0) as f32).h(3.0 * s).fill(rgb(p.green)).absolute(wx - s, y + 23.0 * s));
                }
                row = row.child(label(window.reset_label(now), ww, false, true).absolute(wx - s, y + 33.0 * s));
            }
            if let Some(error) = &account.error {
                row = row.child(label(error.clone(), ww, false, false).color(rgb(if account.auth_type == "api" { p.muted } else { p.red }))
                    .absolute(wx - s, wy + account.windows.len() as f32 * 60.0 * s));
            } else if account.windows.is_empty() {
                row = row.child(label("Usage unavailable".into(), ww, false, true).absolute(wx - s, wy));
            }
            row = row.child(el().w((cw - 28.0 * s).max(0.0)).h(s).fill(rgb(p.border)).absolute(13.0 * s, row_h - s));
            content = content.child(row);
        }
        body = body.child(content);
        if max_scroll > 0.0 && body_h > 0.0 {
            let thumb_h = (body_h * body_h / (accounts.len() as f32 * row_h)).max(12.0 * s).min(body_h);
            body = body.child(el().w(2.0 * s).h(thumb_h).fill(rgb(p.dim)).absolute(cw - 6.0 * s, (body_h - thumb_h) * scroll / max_scroll));
        }
    }
    card = card.child(body);
    if footer > 0.0 {
        card = card.child(label("Enter refresh · Esc close".into(), cw - 28.0 * s, false, true)
            .absolute(14.0 * s, header + body_h + 6.0 * s)
            .on_press(|a: &mut App| a.open_usage()));
    }
    el().key("prompt-picker-layer").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .child(el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).on_press(|a: &mut App| a.close_prompt_picker()))
        .child(card.absolute(cx, cy - height - 6.0 * s))
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
    pub(crate) fn usage_fixture(&mut self, state: &str) {
        self.open_tab(crate::tabs::TabKind::Agent { session: None });
        let mut picker = Picker::new(Kind::Usage, "Codex", Vec::new());
        picker.loading = state == "loading";
        if state == "error" { picker.usage_error = Some("Could not load usage: connection unavailable".into()); }
        if state == "accounts" {
            picker.usage = serde_json::from_value(serde_json::json!({"accounts": [
                {"label": "Personal", "is_default": true, "auth_type": "oauth", "plan_type": "pro", "windows": [
                    {"label": "5-hour", "used_percent": 27, "reset_at": 4102452000i64},
                    {"label": "Weekly", "used_percent": 62, "reset_at": 4102884000i64}
                ]},
                {"label": "Work", "auth_type": "oauth", "plan_type": "plus", "windows": [
                    {"label": "5-hour", "used_percent": 100, "reset_at": null},
                    {"label": "Weekly", "used_percent": 12, "reset_at": 4102884000i64}
                ]},
                {"label": "API account", "auth_type": "api", "windows": [], "error": "Usage is unavailable for API key accounts"},
                {"label": "Disconnected account", "auth_type": "oauth", "windows": [], "error": "Please reconnect this account"}
            ]})).expect("usage fixture");
        }
        self.focus_picker(picker);
    }

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
        let mut picker = Picker::new(Kind::Usage, "Codex", Vec::new());
        match result {
            Ok(usage) => picker.usage = usage,
            Err(error) => picker.usage_error = Some(error),
        }
        self.focus_picker(picker);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_windows_show_remaining_and_utc_resets() {
        let mut window = Window { label: "Weekly".into(), used_percent: 27.5, reset_at: Some(1_735_689_600) };
        assert_eq!(window.remaining(), Some(72.5));
        assert_eq!(window.reset_label(1_735_600_000), "Resets in 1d 0h · 01/01, 00:00 UTC");
        assert_eq!(window.reset_label(1_735_689_600), "Reset due now");
        window.reset_at = None;
        assert_eq!(window.reset_label(0), "Reset unavailable");
        window.used_percent = f64::NAN;
        assert_eq!(window.remaining(), None);
        window.used_percent = 150.0;
        assert_eq!(window.remaining(), Some(0.0));
    }
}
