//! Test-only construction seam for the production ingress handlers. No relay,
//! OpenTunnel identity, or production agent is constructed. The early-exit probe
//! owns only a `/bin/false` child through the real FastDaemon guard.
use super::*;

/// Exercise the production pre-readiness child-exit path. The supplied child
/// must be a test-only early-exit executable; this never reaches relay creation.
pub async fn early_exit(root: PathBuf) -> Result<(), String> {
    start_host(
        PathBuf::from("/bin/false"),
        root,
        "contract-host".into(),
        "contract-guest".into(),
        "contract-private-master-not-a-paired-device".into(),
        "contract".into(),
        false,
        "http://127.0.0.1:1".into(),
    )
    .await
    .map(|_| ())
}

pub struct LocalIngress {
    pub endpoint: String,
    ended: tokio::sync::watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

impl LocalIngress {
    pub async fn start(
        socket: String,
        root: PathBuf,
        private_token: &str,
        guest: &str,
    ) -> Self {
        let (ended, receiver) = tokio::sync::watch::channel(false);
        let state = Arc::new(Ingress {
            public_url: String::new(),
            socket,
            daemon_base: String::new(),
            workspace_id: "contract-only".into(),
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
            root,
            daemon_token: private_token.into(),
            host_hash: hash("contract-host"),
            guest_hash: hash(guest),
            failures: Mutex::new((Instant::now(), 0)),
            connections: Arc::new(Semaphore::new(8)),
            proxies: Arc::new(ProxyTracker::default()),
            ended: receiver,
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("ws://{}/session", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/session", get(upgrade))
            .with_state(state);
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            endpoint,
            ended,
            task,
        }
    }

    pub fn end(&self) {
        self.ended.send(true).unwrap();
    }

    pub fn ready_task(&self, token: &str) -> tokio::task::JoinHandle<Result<(), String>> {
        let endpoint = self.endpoint.clone();
        let token = token.to_owned();
        tokio::spawn(async move {
            tokio::time::timeout(
                Duration::from_secs(3),
                check_ingress_ready(&endpoint, &token),
            )
            .await
            .map_err(|_| "ingress readiness timeout".to_string())?
        })
    }

    pub async fn ready(&self, token: &str) -> Result<(), String> {
        tokio::time::timeout(
            Duration::from_secs(3),
            check_ingress_ready(&self.endpoint, token),
        )
        .await
        .map_err(|_| "ingress readiness timeout".to_string())?
    }
}

impl Drop for LocalIngress {
    fn drop(&mut self) {
        let _ = self.ended.send(true);
        self.task.abort();
    }
}
