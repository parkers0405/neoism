//! Public, secretless handoff page; never derive an origin from request headers.
use axum::{
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
use base64::Engine;
use sha2::{Digest, Sha256};

const MAX_INVITE_BYTES: usize = 4096;
const DOWNLOAD: &str = "https://github.com/parkers0405/neoism/releases/latest";
const STYLE: &str = r#"*{box-sizing:border-box}body{margin:0;background:#101116;color:#ececf3;font:16px/1.6 system-ui,sans-serif;min-height:100vh;display:grid;place-items:center;padding:24px}main{width:100%;max-width:540px;padding:36px;border:1px solid #30303d;border-radius:20px;background:#191a22}header{color:#b9a1ff;font-weight:750;letter-spacing:.18em}h1{font-size:28px;line-height:1.3;overflow-wrap:anywhere}p{color:#b9bac9}.open{display:block;background:#b9a1ff;color:#171122;text-align:center;padding:15px;border-radius:10px;font-weight:750;text-decoration:none;margin:24px 0}button{background:#292a36;color:#ececf3;border:1px solid #555568;border-radius:8px;padding:10px 18px;font:inherit;cursor:pointer}a{color:#c8b5ff}input{width:100%;background:#101116;color:#d8d7e5;border:1px solid #424252;border-radius:8px;padding:10px;font:inherit;margin:8px 0 16px}small{color:#9696aa}a:focus-visible,button:focus-visible,input:focus-visible{outline:3px solid #c8b5ff;outline-offset:4px}"#;
// No location/query/fragment reads: capabilities are not supported by this page.
// A real anchor preserves the user-gesture fallback even with JS disabled.
const SCRIPT: &str = r#"'use strict';
const openLink = document.getElementById('open');
const invite = document.getElementById('invite');
const status = document.getElementById('status');
document.getElementById('copy').addEventListener('click', async () => {
  try {
    if (!navigator.clipboard) throw new Error('unavailable');
    await navigator.clipboard.writeText(invite.value);
    status.textContent = 'Invite copied. Paste it into Add Server → Join with Fast tunnel enabled.';
  } catch (_) {
    invite.focus(); invite.select(); invite.setSelectionRange(0, invite.value.length);
    status.textContent = 'Link selected. Copy it, then paste into Add Server → Join with Fast tunnel enabled.';
  }
});
try { window.location.assign(openLink.href); } catch (_) {}
"#;

fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        out.push_str(match ch {
            '&' => "&amp;",
            '<' => "&lt;",
            '>' => "&gt;",
            '"' => "&quot;",
            '\'' => "&#39;",
            _ => {
                out.push(ch);
                continue;
            }
        });
    }
    out
}

/// Percent-encode the complete canonical HTTPS invite as ONE native query value.
fn deep_link(invite: &str) -> Result<String, StatusCode> {
    if invite.len() > MAX_INVITE_BYTES {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    let url = url::Url::parse(invite).map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.path() != "/"
        || url.fragment().is_some()
        || url.query_pairs().any(|(key, _)| key != "name")
    {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    let mut encoded = String::with_capacity(invite.len() * 3);
    for byte in invite.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write;
            write!(encoded, "%{byte:02X}").unwrap();
        }
    }
    let link = format!("neoism://join?invite={encoded}");
    // Native envelope parser bounds the entire scheme URI to 8 KiB.
    if link.len() > 8 * 1024 {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    Ok(link)
}

fn csp_hash(source: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(Sha256::digest(source.as_bytes()))
}

pub(crate) fn render(invite: &str, query: Option<&str>) -> Response {
    let response = (|| {
        // Known display metadata only. Even a supplied name is never reflected:
        // the host-owned canonical invite supplies both the label and payload.
        let query = query.unwrap_or("");
        if query.len() > MAX_INVITE_BYTES
            || url::form_urlencoded::parse(query.as_bytes()).any(|(key, _)| key != "name")
        {
            return StatusCode::BAD_REQUEST.into_response();
        }
        let link = match deep_link(invite) {
            Ok(link) => link,
            Err(status) => return status.into_response(),
        };
        let url = url::Url::parse(invite).unwrap();
        let name = url
            .query_pairs()
            .find(|(key, _)| key == "name")
            .map(|(_, value)| value.into_owned())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| url.host_str().unwrap().to_owned());
        Html(format!(r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Join a Fast Server · Neoism</title><style>{STYLE}</style></head><body><main><header>NEOISM</header><h1>Join {name}</h1><p>A temporary workspace shared through OpenTunnel. Open it in the Neoism desktop app; ask your host for the password.</p><a class="open" id="open" href="{link}">Open in Neoism</a><p id="status" role="status">Your browser may ask permission to open Neoism. If nothing happens, use the button above.</p><label for="invite">Fast Server invite</label><input id="invite" readonly value="{invite}" spellcheck="false"><button id="copy" type="button">Copy invite</button><p>Or copy this link and paste it into <strong>Add Server → Join with Fast tunnel enabled</strong> in Neoism.</p><p><a href="{DOWNLOAD}" rel="noreferrer">Download Neoism</a></p><small>This invite works only while the host is running. No workspace is opened in your browser.</small></main><script>{SCRIPT}</script></body></html>"#,
            name = escape(&name), link = escape(&link), invite = escape(invite))).into_response()
    })();
    secure(response)
}

pub(super) fn secure(mut response: Response) -> Response {
    let headers = response.headers_mut();
    for (key, value) in [
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        ("x-frame-options", "DENY"),
        ("cache-control", "no-store"),
        (
            "permissions-policy",
            "camera=(), microphone=(), geolocation=()",
        ),
    ] {
        headers.insert(key, value.parse().unwrap());
    }
    headers.insert("content-security-policy", format!(
        "default-src 'none'; script-src 'sha256-{}'; style-src 'sha256-{}'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'; connect-src 'none'",
        csp_hash(SCRIPT), csp_hash(STYLE)).parse().unwrap());
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn escaping_and_native_payload_round_trip() {
        assert_eq!(escape("<&\"'>"), "&lt;&amp;&quot;&#39;&gt;");
        let mut invite = url::Url::parse("https://sdk.opentunnel.xyz/").unwrap();
        invite
            .query_pairs_mut()
            .append_pair("name", "雪 & /? \"<script>");
        let link = deep_link(invite.as_str()).unwrap();
        assert!(link.starts_with("neoism://join?invite=https%3A%2F%2F"));
        let parsed = url::Url::parse(&link).unwrap();
        assert_eq!(parsed.host_str(), Some("join"));
        assert_eq!(
            parsed.query_pairs().collect::<Vec<_>>(),
            vec![("invite".into(), invite.as_str().into())]
        );
        let join = super::super::normalize_fast_join(
            parsed.query_pairs().next().unwrap().1.into_owned(),
            None,
        )
        .unwrap();
        assert_eq!(join.name, "雪 & /? \"<script>");
        assert!(join.token.is_none());
    }
    #[test]
    fn rejects_credentials_and_bounds_payload() {
        for invite in [
            "http://sdk/",
            "https://user:secret@sdk/",
            "https://sdk/?token=secret",
            "https://sdk/#neoism-fast=secret",
            "https://sdk/admin",
        ] {
            assert!(deep_link(invite).is_err());
        }
        assert!(deep_link(&format!(
            "https://sdk/?name={}",
            "a".repeat(MAX_INVITE_BYTES)
        ))
        .is_err());
    }
}
