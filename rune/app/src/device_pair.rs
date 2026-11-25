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
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
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
        .unwrap_or_else(|| origin.host_str().unwrap_or("Mac").to_owned());
    Ok(Invite { origin, code, name, ca })
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
                let mut client = reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(10));
                if let Some(ca) = ca {
                    let cert = reqwest::Certificate::from_der(&decode_ca(&ca)?).map_err(|_| "Invalid pairing certificate")?;
                    client = client.tls_built_in_root_certs(false).add_root_certificate(cert);
                }
                let client = client.build().map_err(|_| "Could not initialize pairing".to_owned())?;
                let request = client.post(url).json(&body);
                let claiming = bearer.is_none();
                let request = if let Some(token) = bearer { request.bearer_auth(token) } else { request };
                let response = request.send().await.map_err(|_| "Could not reach the pairing service".to_owned())?;
                match response.status().as_u16() {
                    200..=299 => response.json().await.map_err(|_| "Invalid response from pairing service".to_owned()),
                    401 if claiming => Err("The pairing password did not match, or this QR code expired or was already used. Generate a fresh code on the Mac, scan it, and enter the same password within 60 seconds.".into()),
                    401 | 403 if !claiming => Err("The Mac denied access to the local pairing controls. Reopen Neoism using the configured pairing daemon.".into()),
                    429 => Err("The host reached its pairing limit. Wait for unused QR codes to expire or remove an unused paired device.".into()),
                    404 => Err("The host does not have device pairing enabled, or this code has expired".into()),
                    410 => Err("This code expired. Generate a new code on the Mac".into()),
                    503 => Err("The host needs a private, reachable HTTPS address for device pairing".into()),
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

    #[test]
    fn device_invite_requires_https_and_unambiguous_fields() {
        let good = "neoism://device?origin=https%3A%2F%2Fmac.example&code=123456&name=Studio";
        let invite = parse(good).unwrap();
        assert_eq!(invite.origin.host_str(), Some("mac.example"));
        assert_eq!(invite.name, "Studio");
        assert!(parse(&good.replace("https", "http")).is_err());
        assert!(parse(&format!("{good}&code=987654")).is_err());
        assert!(parse(&format!("{good}&ca=invalid")).is_err());
        assert!(parse(&good.replace("mac.example", "user%40mac.example")).is_err());
    }
}