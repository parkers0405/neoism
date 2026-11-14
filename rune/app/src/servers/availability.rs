//! Picker-scoped, credential-free daemon health checks. No socket handshakes,
//! registry writes, service launches, redirects or idle background polling.
use std::{collections::HashMap, sync::mpsc::{self, Receiver}, time::{Duration, Instant}};
use futures::{StreamExt, stream};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

const REFRESH: Duration = Duration::from_secs(5);
const FRESH: Duration = Duration::from_secs(15);
const TIMEOUT: Duration = Duration::from_secs(2);
type Results = Vec<(String, Option<bool>)>;

struct Job {
    rx: Receiver<Results>,
    cancelled: Arc<AtomicBool>,
}
impl Drop for Job {
    fn drop(&mut self) { self.cancelled.store(true, Ordering::Relaxed); }
}

#[derive(Default)]
pub(super) struct Availability {
    observations: HashMap<String, (Instant, Option<bool>)>,
    pending: Option<Job>,
    checked: Option<Instant>,
}

impl Availability {
    pub(super) fn online(&self, address: &str) -> Option<bool> {
        let key = health_url(address)?;
        self.observations.get(&key).filter(|(at, _)| at.elapsed() < FRESH).and_then(|(_, online)| *online)
    }

    /// Called only while Servers is open; one worker, at most four requests at a
    /// time, each with a total deadline. Closing cancels requests not yet started.
    pub(super) fn poll(&mut self, addresses: Vec<String>, wake: Option<rune::Waker>) {
        if let Some(job) = &self.pending {
            match job.rx.try_recv() {
                Ok(results) => {
                    let now = Instant::now();
                    self.observations = results.into_iter().map(|(url, online)| (url, (now, online))).collect();
                    self.pending = None;
                }
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => self.pending = None,
            }
        }
        if self.checked.is_some_and(|at| at.elapsed() < REFRESH) { return; }
        self.checked = Some(Instant::now());
        let mut urls: Vec<_> = addresses.iter().filter_map(|address| health_url(address)).collect();
        urls.sort();
        urls.dedup();
        if urls.is_empty() { return; }
        let (tx, rx) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        let spawned = std::thread::Builder::new().name("neoism-server-health".into()).spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return; };
            let results = runtime.block_on(async {
                let Ok(client) = reqwest::Client::builder().timeout(TIMEOUT)
                    .redirect(reqwest::redirect::Policy::none()).no_proxy().build() else { return Vec::new(); };
                stream::iter(urls.into_iter().map(|url| {
                    let client = client.clone();
                    let cancelled = worker_cancelled.clone();
                    async move {
                        let online = if cancelled.load(Ordering::Relaxed) { None } else { check(&client, &url).await };
                        (url, online)
                    }
                })).buffer_unordered(4).collect().await
            });
            if !worker_cancelled.load(Ordering::Relaxed) && tx.send(results).is_ok() {
                if let Some(wake) = wake { wake(); }
            }
        });
        if spawned.is_ok() { self.pending = Some(Job { rx, cancelled }); }
    }

    pub(super) fn close(&mut self) {
        // Keep the bounded in-flight batch so reopening cannot spawn a second
        // worker. No new checks start while closed; observations expire normally.
        if let Some(job) = &self.pending { job.cancelled.store(true, Ordering::Relaxed); }
        self.checked = None;
    }
}

fn health_url(address: &str) -> Option<String> {
    let mut url = url::Url::parse(address).ok()?;
    let scheme = match url.scheme() { "ws" | "http" => "http", "wss" | "https" => "https", _ => return None };
    url.set_scheme(scheme).ok()?;
    if url.host_str().is_none() { return None; }
    url.set_username("").ok()?;
    url.set_password(None).ok()?;
    url.set_query(None);
    url.set_fragment(None);
    url.set_path("/health");
    Some(url.into())
}

async fn check(client: &reqwest::Client, url: &str) -> Option<bool> {
    // Distinguish a known offline/unreachable endpoint from an unsupported health
    // route, auth gateway, captive portal or unrelated HTTP service (Unknown).
    let mut response = match client.get(url).send().await {
        Ok(response) => response,
        Err(_) => return Some(false),
    };
    if !response.status().is_success() { return None; }
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if body.len() + chunk.len() > 64 { return None; }
                body.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(_) => return None,
        }
    }
    (body == b"neoism-daemon").then_some(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_target_strips_all_credentials_and_session_paths() {
        assert_eq!(health_url("wss://user:secret@example.com:9877/session?token=secret#secret"),
            Some("https://example.com:9877/health".into()));
        assert_eq!(health_url("ws://127.0.0.1:9877/session"), Some("http://127.0.0.1:9877/health".into()));
        assert_eq!(health_url("This computer"), None);
        assert_eq!(health_url("file:///tmp/server"), None);
    }

    #[tokio::test]
    async fn health_requires_daemon_marker_and_does_not_follow_redirects() {
        use axum::{Router, routing::get};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, Router::new()
                .route("/health", get(|| async { "neoism-daemon" }))
                .route("/other", get(|| async { "OK" }))
                .route("/large", get(|| async { "x".repeat(100) }))
                .route("/redirect", get(|| async { axum::response::Redirect::temporary("/health") }))
                .route("/slow", get(|| async { tokio::time::sleep(Duration::from_secs(5)).await; "neoism-daemon" })))
                .await.unwrap();
        });
        let client = reqwest::Client::builder().timeout(Duration::from_millis(100))
            .redirect(reqwest::redirect::Policy::none()).no_proxy().build().unwrap();
        assert_eq!(check(&client, &format!("{base}/health")).await, Some(true));
        for path in ["other", "large", "redirect", "missing"] {
            assert_eq!(check(&client, &format!("{base}/{path}")).await, None);
        }
        assert_eq!(check(&client, &format!("{base}/slow")).await, Some(false));
        server.abort();
    }

    #[test]
    fn observations_expire_and_are_keyed_by_address_not_saved_id() {
        let mut availability = Availability::default();
        availability.observations.insert("http://localhost:9877/health".into(), (Instant::now(), Some(true)));
        assert_eq!(availability.online("ws://localhost:9877/session"), Some(true));
        assert_eq!(availability.online("ws://localhost:9878/session"), None);
        availability.observations.values_mut().next().unwrap().0 = Instant::now() - FRESH;
        assert_eq!(availability.online("ws://localhost:9877/session"), None);
    }

    #[test]
    fn closing_cancels_queued_checks_and_refresh_is_rate_limited() {
        let mut availability = Availability::default();
        let (tx, rx) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        availability.pending = Some(Job { rx, cancelled: cancelled.clone() });
        availability.close();
        assert!(cancelled.load(Ordering::Relaxed));
        drop(tx);
        availability.poll(Vec::new(), None);
        assert!(availability.pending.is_none());
        let checked = availability.checked;
        availability.poll(vec!["ws://127.0.0.1:1/session".into()], None);
        assert!(availability.pending.is_none(), "no worker starts before the refresh interval");
        assert_eq!(availability.checked, checked);
    }

    #[tokio::test]
    async fn picker_worker_populates_availability_without_blocking_caller() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("ws://{}/session", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, axum::Router::new().route("/health", axum::routing::get(|| async { "neoism-daemon" })))
                .await.unwrap();
        });
        let mut availability = Availability::default();
        availability.poll(vec![address.clone()], None);
        assert!(availability.pending.is_some());
        let deadline = Instant::now() + Duration::from_secs(3);
        while availability.online(&address).is_none() {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
            availability.poll(vec![address.clone()], None);
        }
        assert_eq!(availability.online(&address), Some(true));
        server.abort();
    }
}