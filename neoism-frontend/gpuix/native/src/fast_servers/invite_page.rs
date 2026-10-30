//! Public, secretless handoff page; never derive an origin from request headers.
use axum::{
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
use base64::Engine;
use sha2::{Digest, Sha256};
use neoism_agent_service_api::browser_pages::{self, escape_html as escape};

const MAX_INVITE_BYTES: usize = 4096;
const DOWNLOAD: &str = "https://github.com/parkers0405/neoism/releases/latest";
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
    invite.hidden = false;
    invite.focus(); invite.select(); invite.setSelectionRange(0, invite.value.length);
    status.textContent = 'Link selected. Copy it, then paste into Add Server → Join with Fast tunnel enabled.';
  }
});
try { window.location.assign(openLink.href); } catch (_) {}
"#;

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
        let content = format!(r#"<section class="content" aria-labelledby="page-title"><h2 id="page-title">Join {name}</h2><p>A shared workspace. Ask your host for the password.</p><div class="actions"><a class="button" id="open" href="{link}">Open in Neoism <span aria-hidden="true">&#8599;</span></a><button class="text-link" id="copy" type="button">Copy invite</button></div><p class="note" id="status" role="status">Your browser may ask to open Neoism. You can also paste the invite into Add Server &rarr; Join with Fast tunnel enabled.</p><input class="invite" id="invite" aria-label="Server invite" readonly hidden value="{invite}" spellcheck="false"><p class="note download"><a href="{DOWNLOAD}" rel="noreferrer">Download Neoism</a> &middot; Invite expires when the host stops sharing.</p></section>"#,
            name = escape(&name), link = escape(&link), invite = escape(invite));
        Html(browser_pages::render("Join a workspace | Neoism", &content, SCRIPT)).into_response()
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
        "default-src 'none'; script-src 'sha256-{}' 'sha256-{}'; style-src 'sha256-{}'; font-src data:; img-src data:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'; connect-src 'none'",
        csp_hash(browser_pages::WORDMARK_SCRIPT), csp_hash(SCRIPT), csp_hash(browser_pages::style())).parse().unwrap());
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
