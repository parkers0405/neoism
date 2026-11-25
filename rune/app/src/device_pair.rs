//! Device-wide chat pairing, separate from a Fast Server workspace invite.
use std::{sync::{Arc, Mutex}, time::Duration};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

pub(crate) fn decode_ca(encoded: &str) -> Result<Vec<u8>, String> {
    if encoded.len() > 1400 { return Err("Pairing certificate is too large".into()); }
    let der = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| "Invalid pairing certificate")?;
    let mut roots = rustls::RootCertStore::empty();
    roots.add(der.clone().into()).map_err(|_| "Invalid pairing certificate")?;
    Ok(der)
}

pub(crate) const SESSION: &str = "/mobile-device/session";
pub(crate) fn is_session(endpoint: &str) -> bool {
    url::Url::parse(endpoint).is_ok_and(|url| url.scheme() == "wss" && url.path() == SESSION)
}

impl crate::app::App {
    pub(crate) fn paired_chat_device(&self) -> bool {
        self.mobile && self.server.link.current.as_ref().is_some_and(|server| is_session(&server.endpoint))
    }
}
pub(crate) type Reply<T> = Arc<Mutex<Option<Result<T, String>>>>;

pub(crate) struct Invite {
    pub origin: url::Url,
    pub code: String,
    pub name: String,
    pub ca: Option<String>,
    pub scan_only: bool,
}

pub(crate) fn is_device_link(input: &str) -> bool { input.trim().starts_with("neoism://device") }

pub(crate) fn parse(input: &str) -> Result<Invite, String> {
    let invalid = || "Invalid device pairing link".to_owned();
    let input = input.trim();
    if input.len() > 8192 { return Err(invalid()); }
    let url = url::Url::parse(input).map_err(|_| invalid())?;
    if url.scheme() != "neoism" || url.host_str() != Some("device") || !matches!(url.path(), "" | "/")
        || !url.username().is_empty() || url.password().is_some() || url.port().is_some() || url.fragment().is_some()
    { return Err(invalid()); }
    let (mut origin, mut code, mut name, mut ca) = (None, None, None, None);
    let mut scan_only = false;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "v" if !scan_only && value == "2" => scan_only = true,
            "origin" if origin.is_none() => origin = Some(value.into_owned()),
            "code" if code.is_none() => code = Some(value.into_owned()),
            "name" if name.is_none() => name = Some(value.into_owned()),
            "ca" if ca.is_none() => { decode_ca(&value)?; ca = Some(value.into_owned()); }
            _ => return Err(invalid()),
        }
    }
    let origin = url::Url::parse(&origin.ok_or_else(invalid)?).map_err(|_| invalid())?;
    if origin.scheme() != "https" || origin.host().is_none() || !origin.username().is_empty()
        || origin.password().is_some() || origin.path() != "/" || origin.query().is_some() || origin.fragment().is_some()
    { return Err("Device pairing requires a trusted HTTPS host".into()); }
    let code = code.filter(|s| !s.is_empty() && s.len() <= 256 && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))).ok_or_else(invalid)?;
    let name = name.filter(|s| !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control))
        .unwrap_or_else(|| origin.host_str().unwrap_or("Computer").to_owned());
    Ok(Invite { origin, code, name, ca, scan_only })
}

/// Only display the local daemon's sanitized diagnostic, not arbitrary upstream HTML or bodies.
async fn unavailable(mut response: reqwest::Response) -> String {
    let fallback = "Connect this computer to Tailscale, then try again.";
    let mut bytes = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) if chunk.len() <= 4096 - bytes.len() => bytes.extend_from_slice(&chunk),
            Ok(None) => break,
            _ => return fallback.into(),
        }
    }
    #[derive(serde::Deserialize)]
    struct Diagnostic { error: String }
    serde_json::from_slice::<Diagnostic>(&bytes).ok().map(|d| d.error)
        .filter(|s| !s.trim().is_empty() && s.len() <= 1024 && !s.chars().any(char::is_control))
        .unwrap_or_else(|| fallback.into())
}

pub(crate) fn request<T: serde::de::DeserializeOwned + Send + 'static>(
    url: url::Url, body: serde_json::Value, bearer: Option<String>, ca: Option<String>, wake: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
) -> Reply<T> {
    let reply: Reply<T> = Default::default();
    let result = reply.clone();
    std::thread::spawn(move || {
        let work = || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|_| "Could not start pairing".to_owned())?;
            runtime.block_on(async {
                // The first quick invite can include daemon-side Tailscale discovery and TLS setup.
                let timeout = Duration::from_secs(if bearer.is_some() { 30 } else { 10 });
                let mut client = reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).timeout(timeout);
                if let Some(ca) = ca {
                    let cert = reqwest::Certificate::from_der(&decode_ca(&ca)?).map_err(|_| "Invalid pairing certificate")?;
                    client = client.tls_built_in_root_certs(false).add_root_certificate(cert);
                }
                let client = client.build().map_err(|_| "Could not initialize pairing".to_owned())?;
                let claiming = bearer.is_none();
                let password = body.get("password").is_some();
                let request = client.post(url).json(&body);
                let request = if let Some(token) = bearer { request.bearer_auth(token) } else { request };
                let response = request.send().await.map_err(|_| if claiming { "Could not reach the computer. Connect both devices to the same tailnet and scan a fresh code." } else { "Could not reach the local daemon. Restart it and try again." }.to_owned())?;
                match response.status().as_u16() {
                    200..=299 => response.json().await.map_err(|_| "Invalid response from pairing service".to_owned()),
                    401 if claiming && password => Err("The pairing password did not match, or the code expired. Generate a fresh code and use its password.".into()),
                    401 | 403 if claiming => Err("The code expired, was already used, or was denied. Scan a fresh code from the computer.".into()),
                    401 | 403 => Err("The local daemon rejected its token. Restart the local Neoism daemon and try again.".into()),
                    429 => Err("The host reached its pairing limit. Wait for unused QR codes to expire or remove an unused paired device.".into()),
                    404 | 405 if !claiming => Err("This daemon does not support quick pairing. Update and restart the local Neoism daemon, then try again.".into()),
                    404 => Err("The host does not have device pairing enabled, or this code has expired".into()),
                    410 => Err("This code expired. Generate a new code on the computer.".into()),
                    503 if !claiming => Err(unavailable(response).await),
                    503 => Err("Check that the computer is connected to Tailscale and its HTTPS pairing service is running.".into()),
                    _ => Err("The host rejected the pairing request".into()),
                }
            })
        };
        *result.lock().unwrap() = Some(work());
        if let Some(wake) = wake { wake(); }
    });
    reply
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn quick_invite_refuses_redirects_even_on_same_origin() {
        use axum::{Router, routing::post, response::Redirect};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mobile-device/quick-invite", listener.local_addr().unwrap()).parse().unwrap();
        let service = Router::new().route("/mobile-device/quick-invite", post(|| async { Redirect::temporary("/untrusted") }))
            .route("/untrusted", post(|| async { axum::Json(serde_json::json!({"unexpected": true})) }));
        let server = tokio::spawn(async move { axum::serve(listener, service).await.unwrap() });
        let (done, mut ready) = tokio::sync::mpsc::unbounded_channel();
        let reply = request::<serde_json::Value>(url, serde_json::json!({}), Some("fixture".into()), None, Some(Arc::new(move || { let _ = done.send(()); })));
        tokio::time::timeout(Duration::from_secs(5), ready.recv()).await.unwrap().unwrap();
        assert!(reply.lock().unwrap().take().unwrap().is_err());
        server.abort();
    }

    #[tokio::test]
    async fn unavailable_shows_daemon_remediation_but_not_malformed_or_oversize_bodies() {
        async fn diagnostic(body: &str) -> String {
            let response = axum::http::Response::builder().status(503).body(body.to_owned()).unwrap();
            unavailable(reqwest::Response::from(response)).await
        }
        assert_eq!(diagnostic(r#"{"error":"Close the other phone host, then retry."}"#).await, "Close the other phone host, then retry.");
        let fallback = diagnostic("<html>untrusted upstream</html>").await;
        assert!(fallback.contains("Tailscale"));
        assert_eq!(diagnostic(r#"{"error":"bad\ncontrol"}"#).await, fallback);
        assert_eq!(diagnostic(&"x".repeat(4097)).await, fallback);
    }

    #[test]
    fn device_invite_requires_https_and_unambiguous_fields() {
        let good = "neoism://device?origin=https%3A%2F%2Fmac.example&code=123456&name=Studio";
        let invite = parse(good).unwrap();
        assert_eq!(invite.origin.host_str(), Some("mac.example"));
        assert_eq!(invite.name, "Studio");
        assert!(!invite.scan_only);
        assert!(parse(&format!("{good}&v=2")).unwrap().scan_only);
        for version in ["2&v=2", "", "3"] { assert!(parse(&format!("{good}&v={version}")).is_err()); }
        assert!(parse(&good.replace("https", "http")).is_err());
        assert!(parse(&format!("{good}&code=987654")).is_err());
        assert!(parse(&format!("{good}&ca=invalid")).is_err());
        assert!(parse(&good.replace("mac.example", "user%40mac.example")).is_err());
    }
}