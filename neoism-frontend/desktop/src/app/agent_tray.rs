//! Linux process-wide execution observation, independent of panes and redraws.
//! A disconnected endpoint is unknown, never evidence of completion.
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use futures::StreamExt;
use serde::Deserialize;
use tokio::sync::mpsc;

pub(crate) struct Service {
    _observer: Observer,
    tray: neoism_notifier::agent_tray::AgentTray,
}
impl Service {
    pub(crate) fn start() -> std::io::Result<Self> {
        use neoism_notifier::agent_tray::{AgentTray, TrayState};
        // Discovery comes exclusively from actual credential/ready lifecycle
        // hooks. Never invent a default URL or replace its registered token.
        let tray = AgentTray::start()?;
        tray.set_state(TrayState::Unknown);
        let target = tray.clone();
        let observer = Observer::start(move |activity| {
            target.set_state(match activity {
                Activity::Idle => TrayState::Idle,
                Activity::Working { count } => TrayState::Working { count },
                Activity::Done => TrayState::Done,
                Activity::Unknown => TrayState::Unknown,
            })
        })?;
        Ok(Self {
            _observer: observer,
            tray,
        })
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.tray.stop();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Activity {
    Idle,
    Working { count: usize },
    Done,
    Unknown,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Execution {
    root_session_id: String,
    execution_id: String,
    revision: u64,
    finished: bool,
}

#[derive(Default)]
struct EndpointState {
    generation: u64,
    known: bool,
    rows: HashMap<String, Execution>,
}

#[derive(Default)]
struct Aggregate {
    endpoints: HashMap<String, EndpointState>,
    observed_busy: bool,
    done: bool,
}

impl Aggregate {
    fn unknown(&mut self, endpoint: &str, generation: u64) {
        let state = self.endpoints.entry(endpoint.to_owned()).or_default();
        if generation < state.generation {
            return;
        }
        state.generation = generation;
        state.known = false;
        // A reconnect cannot turn a previously observed run into a completion
        // notification: the interval while disconnected was not observed.
        self.observed_busy = false;
        self.done = false;
    }

    fn snapshot(&mut self, endpoint: &str, generation: u64, rows: Vec<Execution>) {
        let Some(state) = self.endpoints.get_mut(endpoint) else {
            return;
        };
        if state.generation != generation {
            return;
        }
        let present: HashSet<_> =
            rows.iter().map(|r| r.root_session_id.clone()).collect();
        // Missing an unfinished root (e.g. deletion) is not a finished edge.
        state.known = !state
            .rows
            .values()
            .any(|r| !r.finished && !present.contains(&r.root_session_id));
        for row in rows {
            if let Some(old) = state.rows.get(&row.root_session_id) {
                // Root revisions are monotone even across execution IDs. Equal
                // revisions with a different ID are inconsistent, not new work.
                if row.revision < old.revision
                    || (row.revision == old.revision
                        && row.execution_id != old.execution_id)
                    || (row.revision == old.revision && row.finished != old.finished)
                {
                    state.known = false;
                    continue;
                }
            }
            state.rows.insert(row.root_session_id.clone(), row);
        }
    }

    fn activity(&mut self) -> Activity {
        let count = self
            .endpoints
            .values()
            .filter(|e| e.known)
            .flat_map(|e| e.rows.values())
            .filter(|r| !r.finished)
            .count();
        let all_known = self.endpoints.values().all(|e| e.known);
        if count > 0 {
            // Do not arm Done while another endpoint is unknown.
            self.observed_busy = all_known;
            self.done = false;
            Activity::Working { count }
        } else if !all_known {
            self.observed_busy = false;
            self.done = false;
            Activity::Unknown
        } else {
            self.done |= self.observed_busy;
            self.observed_busy = false;
            if self.done {
                Activity::Done
            } else {
                Activity::Idle
            }
        }
    }
}

#[derive(Clone)]
struct Endpoint {
    url: String,
    token: Option<String>,
}

enum Command {
    Register(Endpoint),
    Unknown(String, u64),
    Snapshot(String, u64, Vec<Execution>),
    Stop,
}

#[derive(Default)]
struct Registry {
    endpoints: HashMap<String, Endpoint>,
    sender: Option<mpsc::UnboundedSender<Command>>,
}
fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}

/// Register a trusted LOCAL direct agent endpoint. Windows may register the
/// same endpoint repeatedly; closing/switching a window deliberately does not
/// unregister it: its work can outlive the window. Credential changes reconnect.
/// There is currently no endpoint/agent-owner shutdown identity in the desktop
/// lifecycle. The workspace service may reuse an independently owned agent, so
/// its child exit is NOT authority to retire this endpoint. A permanently lost
/// observed endpoint remains Unknown until process restart; never expire it on
/// a timeout, port failure, or window close and manufacture a Done transition.
/// Remote hosts, reverse proxies and hosted/workspace-scoped endpoints are not
/// supported by this local-only feature and are never probed globally.
fn local_endpoint(server: &str, token: Option<String>) -> Option<Endpoint> {
    let mut url = url::Url::parse(server).ok()?;
    let local = match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(host)) => host == "localhost",
        _ => false,
    };
    if !local
        || !matches!(url.scheme(), "http" | "https")
        || !matches!(url.path(), "" | "/")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    url.set_path("");
    Some(Endpoint {
        url: url.to_string().trim_end_matches('/').to_owned(),
        token,
    })
}

pub(crate) fn register_local_endpoint(server: &str, token: Option<String>) {
    let Some(endpoint) = local_endpoint(server, token) else {
        return;
    };
    let Ok(mut registry) = registry().lock() else {
        return;
    };
    registry.register(endpoint, true);
}

/// Successful anonymous readiness must not replace an already authorized token.
pub(crate) fn register_ready_local_endpoint(server: &str) {
    let Some(endpoint) = local_endpoint(server, None) else {
        return;
    };
    let Ok(mut registry) = registry().lock() else {
        return;
    };
    registry.register(endpoint, false);
}

impl Registry {
    fn register(&mut self, endpoint: Endpoint, replace_credential: bool) {
        let registry = self;
        if registry
            .endpoints
            .get(&endpoint.url)
            .is_some_and(|old| !replace_credential || old.token == endpoint.token)
        {
            return;
        }
        registry
            .endpoints
            .insert(endpoint.url.clone(), endpoint.clone());
        if let Some(sender) = &registry.sender {
            let _ = sender.send(Command::Register(endpoint));
        }
    }

    fn subscribe(&mut self, sender: mpsc::UnboundedSender<Command>) {
        for endpoint in self.endpoints.values() {
            let _ = sender.send(Command::Register(endpoint.clone()));
        }
        self.sender = Some(sender);
    }
}

/// Drop stops all subscriptions. Callback runs on the observer thread and must
/// be nonblocking (e.g. notifier.set_state or a channel send), never a UI drain.
pub(crate) struct Observer {
    sender: mpsc::UnboundedSender<Command>,
}
impl Observer {
    pub(crate) fn start(
        callback: impl Fn(Activity) + Send + Sync + 'static,
    ) -> std::io::Result<Self> {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let initial = {
            let mut registry = registry().lock().unwrap();
            registry.subscribe(sender.clone());
            if registry.endpoints.is_empty() {
                Activity::Idle
            } else {
                Activity::Unknown
            }
        };
        let task_sender = sender.clone();
        let callback = Arc::new(callback);
        std::thread::Builder::new()
            .name("agent-tray-observer".into())
            .spawn(move || {
                let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    callback(Activity::Unknown);
                    return;
                };
                runtime.block_on(async move {
                    let mut aggregate = Aggregate::default();
                    let mut jobs: HashMap<String, tokio::task::JoinHandle<()>> =
                        HashMap::new();
                    let mut generation = 0;
                    let mut last = Some(initial);
                    callback(initial);
                    while let Some(command) = receiver.recv().await {
                        match command {
                            Command::Register(endpoint) => {
                                generation += 1;
                                if let Some(job) = jobs.remove(&endpoint.url) {
                                    job.abort();
                                }
                                aggregate.unknown(&endpoint.url, generation);
                                jobs.insert(
                                    endpoint.url.clone(),
                                    tokio::spawn(observe(
                                        endpoint,
                                        generation,
                                        task_sender.clone(),
                                    )),
                                );
                            }
                            Command::Unknown(endpoint, generation) => {
                                aggregate.unknown(&endpoint, generation)
                            }
                            Command::Snapshot(endpoint, generation, rows) => {
                                aggregate.snapshot(&endpoint, generation, rows)
                            }
                            Command::Stop => break,
                        }
                        let next = aggregate.activity();
                        if last != Some(next) {
                            callback(next);
                            last = Some(next);
                        }
                    }
                    for job in jobs.into_values() {
                        job.abort();
                    }
                });
            })?;
        Ok(Self { sender })
    }
}
impl Drop for Observer {
    fn drop(&mut self) {
        if let Ok(mut registry) = registry().lock() {
            if registry
                .sender
                .as_ref()
                .is_some_and(|s| s.same_channel(&self.sender))
            {
                registry.sender = None;
            }
        }
        let _ = self.sender.send(Command::Stop);
    }
}

async fn observe(
    endpoint: Endpoint,
    generation: u64,
    sender: mpsc::UnboundedSender<Command>,
) {
    let Ok(client) = reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(3))
        .redirect(reqwest::redirect::Policy::none())
        .build()
    else {
        return;
    };
    loop {
        let _ = sender.send(Command::Unknown(endpoint.url.clone(), generation));
        // Each connection starts with the server's subscription-before-snapshot
        // handshake. Never resume from an idle status or an empty token stream.
        let mut request =
            client.get(format!("{}/v2/execution-activity/events", endpoint.url));
        if let Some(token) = &endpoint.token {
            request = request.bearer_auth(token);
        }
        if let Ok(Ok(response)) =
            tokio::time::timeout(Duration::from_secs(5), request.send()).await
        {
            if response.status().is_success() {
                let mut stream = response.bytes_stream();
                let mut buffer = Vec::new();
                'stream: loop {
                    let Ok(Some(Ok(bytes))) =
                        tokio::time::timeout(Duration::from_secs(45), stream.next())
                            .await
                    else {
                        break;
                    };
                    buffer.extend_from_slice(&bytes);
                    if buffer.len() > 8 * 1024 * 1024 {
                        break;
                    }
                    while let Some(end) = buffer.iter().position(|b| *b == b'\n') {
                        let line: Vec<_> = buffer.drain(..=end).collect();
                        if let Some(data) = line
                            .strip_prefix(b"data: ")
                            .or_else(|| line.strip_prefix(b"data:"))
                        {
                            let Ok(rows) = serde_json::from_slice::<Vec<Execution>>(data)
                            else {
                                break 'stream;
                            };
                            if sender
                                .send(Command::Snapshot(
                                    endpoint.url.clone(),
                                    generation,
                                    rows,
                                ))
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
                }
            } else if matches!(response.status().as_u16(), 401 | 403) {
                // No global hosted fallback, auth bypass or retry storm. A
                // credential re-registration starts a new generation.
                return;
            }
        }
        let _ = sender.send(Command::Unknown(endpoint.url.clone(), generation));
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn row(root: &str, id: &str, revision: u64, finished: bool) -> Execution {
        Execution {
            root_session_id: root.into(),
            execution_id: id.into(),
            revision,
            finished,
        }
    }
    #[test]
    fn empty_registry_does_not_subscribe_a_speculative_default() {
        let mut registry = Registry::default();
        let (sender, mut receiver) = mpsc::unbounded_channel();
        registry.subscribe(sender);
        assert!(receiver.try_recv().is_err());
        assert!(registry.endpoints.is_empty());
        assert_eq!(Aggregate::default().activity(), Activity::Idle);
    }

    #[test]
    fn ready_discovery_and_resubscribe_preserve_canonical_authorized_endpoint() {
        let mut registry = Registry::default();
        let server = "http://127.0.0.1:4197";
        registry.register(
            local_endpoint(&format!("{server}/"), Some("authorized".into())).unwrap(),
            true,
        );
        registry.register(local_endpoint(server, None).unwrap(), false);
        registry.register(local_endpoint(&format!("{server}/"), None).unwrap(), false);
        assert_eq!(registry.endpoints.len(), 1);
        for _ in 0..2 {
            let (sender, mut receiver) = mpsc::unbounded_channel();
            registry.subscribe(sender);
            let Command::Register(endpoint) = receiver.try_recv().unwrap() else {
                panic!("expected known endpoint")
            };
            assert_eq!(endpoint.url, server);
            assert_eq!(endpoint.token.as_deref(), Some("authorized"));
            assert!(receiver.try_recv().is_err());
        }
        assert!(local_endpoint("https://remote.example", None).is_none());
        assert!(local_endpoint("http://127.0.0.1:4197/agent", None).is_none());
    }

    #[test]
    fn concurrent_roots_and_background_gaps_require_authoritative_finish() {
        let mut a = Aggregate::default();
        a.unknown("local", 1);
        a.snapshot(
            "local",
            1,
            vec![row("a", "1", 1, false), row("b", "2", 1, false)],
        );
        assert_eq!(a.activity(), Activity::Working { count: 2 });
        a.snapshot(
            "local",
            1,
            vec![row("a", "1", 2, true), row("b", "2", 2, false)],
        );
        assert_eq!(a.activity(), Activity::Working { count: 1 });
        a.snapshot(
            "local",
            1,
            vec![row("a", "1", 2, true), row("b", "2", 3, true)],
        );
        assert_eq!(a.activity(), Activity::Done);
        a.snapshot(
            "local",
            1,
            vec![row("a", "3", 3, false), row("b", "2", 3, true)],
        );
        assert_eq!(a.activity(), Activity::Working { count: 1 });
    }
    #[test]
    fn reconnect_and_stale_generation_never_complete() {
        let mut a = Aggregate::default();
        a.unknown("local", 1);
        a.snapshot("local", 1, vec![row("a", "1", 1, false)]);
        assert_eq!(a.activity(), Activity::Working { count: 1 });
        a.unknown("local", 2);
        assert_eq!(a.activity(), Activity::Unknown);
        a.snapshot("local", 1, vec![row("a", "1", 2, true)]);
        assert_eq!(a.activity(), Activity::Unknown);
        a.snapshot("local", 2, vec![row("a", "1", 2, true)]);
        assert_eq!(a.activity(), Activity::Idle);
    }
    #[test]
    fn stale_root_revision_and_missing_root_cannot_finish() {
        let mut a = Aggregate::default();
        a.unknown("local", 1);
        a.snapshot("local", 1, vec![row("a", "new", 5, false)]);
        a.activity();
        a.snapshot("local", 1, vec![row("a", "old", 4, true)]);
        assert_eq!(a.activity(), Activity::Unknown);
        a.snapshot("local", 1, vec![]);
        assert_eq!(a.activity(), Activity::Unknown);
    }
    #[test]
    fn initial_finished_snapshot_is_idle_and_equal_revision_new_id_is_unknown() {
        let mut a = Aggregate::default();
        a.unknown("local", 1);
        a.snapshot("local", 1, vec![row("a", "old", 4, true)]);
        assert_eq!(a.activity(), Activity::Idle);
        a.snapshot("local", 1, vec![row("a", "new", 4, false)]);
        assert_eq!(a.activity(), Activity::Unknown);
        a.snapshot("local", 1, vec![row("a", "new", 5, false)]);
        assert_eq!(a.activity(), Activity::Working { count: 1 });
    }

    #[test]
    fn independent_endpoints_do_not_share_root_ids() {
        let mut a = Aggregate::default();
        a.unknown("one", 1);
        a.unknown("two", 2);
        a.snapshot("one", 1, vec![row("a", "1", 1, false)]);
        a.snapshot("two", 2, vec![row("a", "1", 1, false)]);
        assert_eq!(a.activity(), Activity::Working { count: 2 });
        a.unknown("two", 2);
        a.snapshot("one", 1, vec![row("a", "1", 2, true)]);
        assert_eq!(a.activity(), Activity::Unknown);
    }
}
