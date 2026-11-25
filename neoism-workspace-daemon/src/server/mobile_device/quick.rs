//! The local desktop asks its already-running daemon to enable the phone plane.
use super::*;

#[derive(Clone)]
struct Control {
    local_key: Digest32,
    service: Arc<tokio::sync::Mutex<Service>>,
}

enum Service {
    Configured(MobileState),
    #[cfg(any(target_os = "macos", target_os = "linux", windows))]
    Automatic(Option<(MobileState, Listener)>),
}

impl Control {
    fn new(service: Service) -> anyhow::Result<Self> {
        let key = std::env::var(crate::daemon_token::DAEMON_TOKEN_ENV)?;
        anyhow::ensure!(!key.trim().is_empty(), "missing local daemon credential");
        Ok(Self { local_key: digest(&key), service: Arc::new(tokio::sync::Mutex::new(service)) })
    }

    fn allowed(&self, peer: Option<ConnectInfo<SocketAddr>>, headers: &HeaderMap) -> bool {
        peer.is_none_or(|ConnectInfo(addr)| addr.ip().is_loopback()) && !headers.contains_key(header::ORIGIN)
            && extract_bearer(headers).is_some_and(|key| bool::from(self.local_key.ct_eq(&digest(&key))))
    }

    async fn ready(&self) -> anyhow::Result<MobileState> {
        match &mut *self.service.lock().await {
            Service::Configured(state) => Ok(state.clone()),
            #[cfg(any(target_os = "macos", target_os = "linux", windows))]
            Service::Automatic(slot) => {
                if let Some((state, _)) = slot { return Ok(state.clone()); }
                let config = provision::provision().await?;
                let (state, listener) = listen(config.addr, config.origin, self.local_key, &config.cert, &config.key, config.ca.as_deref(), true).await?;
                let marker = enabled_file();
                std::fs::create_dir_all(marker.parent().unwrap())?;
                std::fs::write(marker, b"enabled\n")?;
                *slot = Some((state.clone(), listener));
                Ok(state)
            }
        }
    }
}

async fn quick_invite(State(control): State<Control>, peer: Option<ConnectInfo<SocketAddr>>, headers: HeaderMap) -> Response {
    if !control.allowed(peer, &headers) { return StatusCode::FORBIDDEN.into_response(); }
    match control.ready().await {
        Ok(state) => mint_invite(&state, "").into_response(),
        Err(error) => {
            tracing::warn!(%error, "phone pairing unavailable");
            let hint = if error.downcast_ref::<std::io::Error>().is_some_and(|e| e.kind() == std::io::ErrorKind::AddrInUse) {
                "The private phone port is already in use. Close the other phone host, then try again."
            } else {
                "Could not start private phone access. Make sure Tailscale is installed and connected on this computer, then try again."
            };
            (StatusCode::SERVICE_UNAVAILABLE, private_json(serde_json::json!({"error": hint}))).into_response()
        }
    }
}

fn routes() -> Router<Control> {
    Router::new().route("/mobile-device/quick-invite", post(quick_invite)).layer(DefaultBodyLimit::max(4096))
}

pub(super) fn configured(state: MobileState) -> anyhow::Result<Router> {
    Ok(routes().with_state(Control::new(Service::Configured(state))?))
}

#[cfg(any(target_os = "macos", target_os = "linux", windows))]
fn enabled_file() -> std::path::PathBuf { crate::auth::data_dir().join("phone-access.enabled") }

#[cfg(any(target_os = "macos", target_os = "linux", windows))]
async fn active(control: Control, peer: Option<ConnectInfo<SocketAddr>>, headers: &HeaderMap) -> Result<MobileState, StatusCode> {
    if !control.allowed(peer, headers) { return Err(StatusCode::FORBIDDEN); }
    match &*control.service.lock().await {
        Service::Automatic(Some((state, _))) => Ok(state.clone()),
        _ => Err(StatusCode::SERVICE_UNAVAILABLE),
    }
}

#[cfg(any(target_os = "macos", target_os = "linux", windows))]
pub(super) fn automatic() -> anyhow::Result<(Router, Listener)> {
    let control = Control::new(Service::Automatic(None))?;
    let restore = control.clone();
    let task = tokio::spawn(async move {
        if !enabled_file().is_file() { return; }
        // The VPN may come up after the daemon during an update or login. A
        // disconnected tailnet must never prevent the main workspace starting.
        loop {
            match restore.ready().await {
                Ok(_) => return,
                Err(error) => tracing::debug!(%error, "waiting to restore private phone access"),
            }
            tokio::time::sleep(Duration::from_secs(15)).await;
        }
    });
    let router = routes().route("/mobile-device/devices", get(|State(c), p: Option<ConnectInfo<SocketAddr>>, h: HeaderMap| async move {
        devices(State(active(c, p, &h).await?), p, h).await
    })).route("/mobile-device/devices/:id", delete(|State(c), p: Option<ConnectInfo<SocketAddr>>, h: HeaderMap, id: Path<Uuid>| async move {
        match active(c, p, &h).await { Ok(state) => revoke(State(state), p, h, id).await, Err(code) => code }
    })).with_state(control);
    Ok((router, Listener(task)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    #[tokio::test]
    async fn local_qr_authorizes_one_passwordless_claim_not_a_legacy_invite() {
        use super::super::tests::{serve, state};
        let state = state();
        let control = Control { local_key: digest("desktop-secret"), service: Arc::new(tokio::sync::Mutex::new(Service::Configured(state.clone()))) };
        let (local, _local_task) = serve(routes().with_state(control).merge(operator_router(state.clone()))).await;
        let (phone, _phone_task) = serve(phone_router(state)).await;
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let mint = format!("http://{local}/mobile-device/quick-invite");
        let claim = format!("http://{phone}/mobile-device/claim");
        assert_eq!(http.post(&mint).send().await.unwrap().status(), 403);
        assert_eq!(http.post(&mint).bearer_auth("desktop-secret").header("Origin", "https://example.invalid").send().await.unwrap().status(), 403);
        assert_eq!(http.post(format!("http://{phone}/mobile-device/quick-invite")).send().await.unwrap().status(), 404);
        let qr: Value = http.post(mint).bearer_auth("desktop-secret").send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
        let fields = |qr: Value| Url::parse(qr["invite"].as_str().unwrap()).unwrap().query_pairs().into_owned().collect::<HashMap<_, _>>();
        let quick = fields(qr);
        assert_eq!(quick["v"], "2");
        let body = json!({"code":quick["code"], "device_label":"phone"});
        assert_eq!(http.post(&claim).json(&body).send().await.unwrap().status(), 200);
        assert_eq!(http.post(&claim).json(&body).send().await.unwrap().status(), 401);
        let legacy = http.post(format!("http://{local}/mobile-device/invite")).bearer_auth("operator-test-key").json(&json!({"password":"separate secret"})).send().await.unwrap().json().await.unwrap();
        let body = json!({"code":fields(legacy)["code"], "device_label":"phone"});
        assert_eq!(http.post(&claim).json(&body).send().await.unwrap().status(), 401);
    }
}