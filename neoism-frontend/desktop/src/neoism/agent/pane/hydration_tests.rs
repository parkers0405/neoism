use super::*;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct OpeningServer {
    endpoint: String,
    paths: Arc<Mutex<Vec<String>>>,
    release_metadata: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl OpeningServer {
    fn new() -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let paths = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let release_metadata = Arc::new(AtomicBool::new(false));
        let requested = paths.clone();
        let stopped = stop.clone();
        let released = release_metadata.clone();
        let worker = std::thread::spawn(move || {
            let mut workers = Vec::new();
            while !stopped.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut socket, _)) => {
                        let requested = requested.clone();
                        let stopped = stopped.clone();
                        let released = released.clone();
                        workers.push(std::thread::spawn(move || {
                            socket.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
                            let mut request = Vec::new();
                            let mut chunk = [0; 1024];
                            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                                let Ok(size) = socket.read(&mut chunk) else { return; };
                                if size == 0 || request.len() > 8192 { return; }
                                request.extend_from_slice(&chunk[..size]);
                            }
                            let path = String::from_utf8_lossy(&request).split_whitespace()
                                .nth(1).unwrap_or("").to_string();
                            requested.lock().unwrap().push(path.clone());
                            if path.contains("/events") {
                                let _ = socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n: connected\n\n");
                                while !stopped.load(Ordering::Acquire) {
                                    std::thread::sleep(Duration::from_millis(2));
                                }
                                return;
                            }
                            if path == "/v2/sessions/root" {
                                while !released.load(Ordering::Acquire) && !stopped.load(Ordering::Acquire) {
                                    std::thread::sleep(Duration::from_millis(2));
                                }
                            }
                            let (status, body) = if path.contains("/messages?") {
                                ("200 OK", r#"{"items":[{"info":{"id":"message","role":"assistant"},"parts":[{"id":"answer","type":"text","text":"ready text"}]}]}"#)
                            } else if path == "/v2/sessions/root" {
                                ("503 Service Unavailable", r#"{"error":"metadata unavailable"}"#)
                            } else if path.contains("health") {
                                ("200 OK", r#"{"healthy":true,"version":"test","providerCredentialStore":"test"}"#)
                            } else {
                                ("200 OK", "{}")
                            };
                            let _ = write!(socket, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                        }));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("test listener: {error}"),
                }
            }
            for worker in workers {
                worker.join().unwrap();
            }
        });
        Self {
            endpoint,
            paths,
            release_metadata,
            stop,
            worker: Some(worker),
        }
    }
}

impl Drop for OpeningServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.release_metadata.store(true, Ordering::Release);
        self.worker.take().unwrap().join().unwrap();
    }
}

#[test]
#[ignore = "requires NEOISM_AGENT_BENCH_SERVER and NEOISM_AGENT_BENCH_SESSION"]
fn conversation_open_live_backend_timings() {
    let server = std::env::var("NEOISM_AGENT_BENCH_SERVER").expect("benchmark endpoint");
    let session = std::env::var("NEOISM_AGENT_BENCH_SESSION").expect("saved session id");
    // Diagnostic only: these reads used to run inside every click callback.
    let io_started = Instant::now();
    let history = crate::neoism::agent::prompt_history::load();
    let history_us = io_started.elapsed().as_micros();
    let config_started = Instant::now();
    let source = neoism_backend::config::Config::load().agent.default_chat_source;
    eprintln!("former constructor IO history_us={history_us} config_us={} entries={}", config_started.elapsed().as_micros(), history.len());
    for run in 0..3 {
        let started = Instant::now();
        let mut pane = NeoismAgentPane::with_directory(
            std::env::var("NEOISM_AGENT_BENCH_DIRECTORY").ok(),
        );
        pane.set_default_chat_source(source);
        pane.discard_queued_draft_defaults();
        pane.server = server.clone();
        let constructed = started.elapsed();
        pane.switch_session(session.clone());
        let activated = started.elapsed();
        let mut longest_drain = Duration::ZERO;
        while pane.messages.is_empty() && started.elapsed() < Duration::from_secs(15) {
            let draining = Instant::now();
            pane.drain_server_updates();
            pane.drain_background_updates();
            longest_drain = longest_drain.max(draining.elapsed());
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!pane.messages.is_empty(), "saved transcript did not arrive");
        eprintln!(
            "live conversation run={run} construct_us={} activate_us={} transcript_us={} longest_drain_us={} blocks={}",
            constructed.as_micros(), activated.as_micros(), started.elapsed().as_micros(),
            longest_drain.as_micros(), pane.messages.len(),
        );
    }
}

#[test]
fn conversation_open_history_hydration_preserves_new_prompts_and_recall() {
    let mut pane = NeoismAgentPane::default();
    pane.sent_history = vec!["just sent".into(), "second".into()];
    pane.history_index = Some(1);
    pane.input = "edited recall".into();
    pane.history_draft = "draft".into();
    pane.background_sender().send(NeoismAgentBackgroundUpdate::PromptHistoryLoaded(
        vec!["old".into(), "just sent".into()],
    )).unwrap();
    pane.drain_background_updates();
    assert_eq!(pane.sent_history, ["old", "just sent", "second"]);
    assert_eq!(pane.history_index, Some(2));
    assert_eq!(pane.input, "edited recall");
    assert_eq!(pane.history_draft, "draft");
}

#[test]
fn conversation_open_history_hydration_caps_loaded_history() {
    let mut pane = NeoismAgentPane::default();
    pane.sent_history = vec!["local".into()];
    pane.history_index = Some(0);
    pane.background_sender().send(NeoismAgentBackgroundUpdate::PromptHistoryLoaded(
        (0..1100).map(|n| n.to_string()).collect(),
    )).unwrap();
    pane.drain_background_updates();
    assert_eq!(pane.sent_history.len(), 1000);
    assert_eq!(pane.sent_history.last().unwrap(), "local");
    assert_eq!(pane.history_index, Some(999));
}

#[test]
fn conversation_open_sidebar_clock_includes_pane_construction() {
    let mut pane = NeoismAgentPane::default();
    pane.conversation_open_started = Some(("root".into(), Instant::now()));
    let clicked = Instant::now() - Duration::from_secs(1);
    pane.record_conversation_open_started(Some(clicked));
    assert_eq!(pane.conversation_open_started, Some(("root".into(), clicked)));
    pane.conversation_open_started = None;
    pane.record_conversation_open_started(Some(clicked));
    assert!(pane.conversation_open_started.is_none());
}

#[test]
fn conversation_open_identical_preload_keeps_warm_layout_epoch() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".into());
    pane.messages = vec![NeoismAgentMessage::assistant("ready text").with_id("answer")];
    let epoch = pane.timeline_layout_epoch;
    pane.background_sender().send(NeoismAgentBackgroundUpdate::SessionPreloaded {
        session_id: "root".into(),
        messages: pane.messages.clone(),
        raw_count: 1,
        requested_limit: 32,
        oldest_cursor: Some("message".into()),
    }).unwrap();
    pane.drain_background_updates();
    assert_eq!(pane.timeline_layout_epoch, epoch);
    assert_eq!(pane.messages[0].text, "ready text");
}

#[test]
fn conversation_open_subscribed_text_does_not_wait_for_metadata_or_duplicate_history() {
    let server = OpeningServer::new();
    let mut pane = NeoismAgentPane::default();
    pane.server = server.endpoint.clone();
    pane.switch_session("root".into());
    let deadline = Instant::now() + Duration::from_secs(3);
    while !pane
        .messages
        .iter()
        .any(|message| message.text == "ready text")
        && Instant::now() < deadline
    {
        pane.drain_server_updates();
        pane.drain_background_updates();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        pane.messages
            .iter()
            .any(|message| message.text == "ready text"),
        "text must appear while the independent metadata request is held"
    );
    assert!(!server.release_metadata.load(Ordering::Acquire));
    let paths = server.paths.lock().unwrap();
    let history: Vec<_> = paths
        .iter()
        .filter(|path| path.contains("/messages?"))
        .collect();
    assert_eq!(
        history.len(),
        1,
        "opening must use one subscription-safe baseline: {paths:?}"
    );
    assert!(history[0].contains("limit=32"));
    drop(paths);
    server.release_metadata.store(true, Ordering::Release);
    let deadline = Instant::now() + Duration::from_secs(3);
    while pane.session_preloads_in_flight.contains("root") && Instant::now() < deadline {
        pane.drain_background_updates();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(!pane.session_preloads_in_flight.contains("root"));
    assert!(
        pane.messages
            .iter()
            .any(|message| message.text == "ready text"),
        "a metadata failure must not discard an authorized transcript"
    );
}

#[test]
fn conversation_open_history_uses_stored_count_and_keeps_worker_slot_until_finished() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".into());
    pane.session_preloads_in_flight.insert("root".into());
    pane.background_sender()
        .send(NeoismAgentBackgroundUpdate::SessionPreloaded {
            session_id: "root".into(),
            messages: (0..40)
                .map(|id| NeoismAgentMessage::assistant("part").with_id(id.to_string()))
                .collect(),
            raw_count: 1,
            requested_limit: 32,
            oldest_cursor: Some("stored-message".into()),
        })
        .unwrap();
    pane.drain_background_updates();
    assert_eq!(pane.messages.len(), 40);
    assert!(
        !pane.timeline_history.has_older,
        "expanded render blocks are not stored messages"
    );
    assert!(pane.session_preloads_in_flight.contains("root"));
    pane.background_sender()
        .send(NeoismAgentBackgroundUpdate::SessionPreloadFinished {
            session_id: "root".into(),
        })
        .unwrap();
    pane.drain_background_updates();
    assert!(!pane.session_preloads_in_flight.contains("root"));
}

#[test]
fn conversation_open_subscription_revalidation_removes_deleted_preview() {
    let mut pane = NeoismAgentPane::default();
    pane.session_cache
        .insert("root".into(), CachedAgentSession::live_only());
    pane.apply_initial_history_preview(
        "root",
        super::super::api::SessionMessagePage {
            blocks: vec![NeoismAgentMessage::assistant("deleted").with_id("old")],
            raw_count: 1,
            oldest_cursor: Some("old".into()),
        },
        32,
    );
    pane.activate_cached_session("root");
    pane.messages
        .push(NeoismAgentMessage::assistant("live text").with_id("live"));
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root",
        [AgentSessionUpdate::Messages {
            messages: vec![NeoismAgentMessage::assistant("new text").with_id("new")],
            oldest_cursor: Some("new".into()),
        }],
    ));
    pane.drain_server_updates();
    assert!(!pane.messages.iter().any(|message| message.id == "old"));
    assert!(pane.messages.iter().any(|message| message.id == "new"));
    assert!(pane.messages.iter().any(|message| message.id == "live"));
    assert!(pane.initial_history_preview.is_none());
}

#[test]
fn conversation_open_failed_subscription_revalidation_discards_only_preview() {
    let mut pane = NeoismAgentPane::default();
    pane.session_cache
        .insert("root".into(), CachedAgentSession::live_only());
    pane.apply_initial_history_preview(
        "root",
        super::super::api::SessionMessagePage {
            blocks: vec![NeoismAgentMessage::assistant("stale").with_id("old")],
            raw_count: 1,
            oldest_cursor: Some("old".into()),
        },
        32,
    );
    pane.activate_cached_session("root");
    pane.messages
        .push(NeoismAgentMessage::assistant("live").with_id("live"));
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root",
        [AgentSessionUpdate::HistoryRefreshFailed {
            session_id: "root".into(),
            error: "unauthorized".into(),
        }],
    ));
    // Budgeted ingestion may yield to the next frame under concurrent load.
    let deadline = Instant::now() + Duration::from_secs(2);
    while pane.initial_history_preview.is_some() && Instant::now() < deadline {
        pane.drain_server_updates();
        std::thread::yield_now();
    }
    assert!(!pane.messages.iter().any(|message| message.id == "old"));
    assert!(pane.messages.iter().any(|message| message.id == "live"));
    assert!(pane.initial_history_preview.is_none());
}

#[test]
fn conversation_open_newest_page_does_not_reset_older_pagination() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".into());
    pane.timeline_history.oldest_loaded_cursor = Some("older-loaded-page".into());
    pane.timeline_history.has_older = true;
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root",
        [AgentSessionUpdate::HistoryPageLoaded {
            session_id: "root".into(),
            raw_count: 1,
            requested_limit: 32,
            oldest_cursor: Some("newest-page".into()),
        }],
    ));
    pane.drain_server_updates();
    assert_eq!(
        pane.timeline_history.oldest_loaded_cursor.as_deref(),
        Some("older-loaded-page")
    );
    assert!(pane.timeline_history.has_older);

    pane.timeline_history.oldest_loaded_cursor = None;
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root",
        [AgentSessionUpdate::HistoryPageLoaded {
            session_id: "root".into(),
            raw_count: 1,
            requested_limit: 32,
            oldest_cursor: Some("newest-page".into()),
        }],
    ));
    pane.drain_server_updates();
    assert!(!pane.timeline_history.has_older);
}
