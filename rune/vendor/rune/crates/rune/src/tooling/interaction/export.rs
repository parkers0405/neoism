//! Sends timed interactions to the local collector as OTLP/HTTP JSON traces, from one
//! background thread started with the first message. The UI thread only moves a finished
//! record onto a channel; JSON, sockets and retries happen here. A refused or slow connection
//! drops the batch.

use std::net::SocketAddr;
use std::sync::OnceLock;
use std::sync::mpsc::{Sender, SyncSender, channel};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use super::rollup::Roll;
use super::{PHASE_NAMES, Slow, SlowFrame, Stacks};
use crate::time::Instant;

pub(crate) enum Message {
    /// Unit tests keep slow interactions (`tests::SENT`) rather than send them.
    #[cfg_attr(test, allow(dead_code))]
    Slow(Box<Slow>),
    /// Unit tests keep slow frames (`tests::FRAMES`) rather than send them.
    #[cfg_attr(test, allow(dead_code))]
    Frame(Box<SlowFrame>),
    /// Every signature's counts over `[from, to]`.
    Rollup { rolls: Vec<Roll>, from: Instant, to: Instant },
    /// Answers once everything sent before it is posted.
    Flush(SyncSender<()>),
}

/// Queues `message` for the collector; false when there's none.
pub(crate) fn send(message: Message) -> bool {
    sender().is_some_and(|out| out.send(message).is_ok())
}

/// Waits up to `limit` for everything queued to be posted, for a process about to exit.
pub(crate) fn flush(limit: Duration) {
    let Some(Some(out)) = OUT.get() else { return };
    let (done, wait) = std::sync::mpsc::sync_channel(1);
    if out.send(Message::Flush(done)).is_ok() { let _ = wait.recv_timeout(limit); }
}

/// The posting thread's queue, made by the first send.
static OUT: OnceLock<Option<Sender<Message>>> = OnceLock::new();

fn sender() -> Option<&'static Sender<Message>> {
    OUT.get_or_init(|| {
        let to = super::gate::collector()?;
        let (out, messages) = channel::<Message>();
        let resource = resource();
        std::thread::Builder::new().name("rune-interactions".into()).spawn(move || {
            while let Ok(first) = messages.recv() {
                // A short wait gathers a burst into one request.
                if !matches!(first, Message::Flush(_)) { std::thread::sleep(Duration::from_millis(100)); }
                let mut spans = Vec::new();
                let mut flushed = Vec::new();
                for m in std::iter::once(first).chain(messages.try_iter()) {
                    match m {
                        Message::Slow(slow) => spans.extend(slow_spans(&slow)),
                        Message::Frame(frame) => spans.push(frame_span(&frame)),
                        Message::Rollup { rolls, from, to } => spans.extend(rolls.iter().map(|r| roll_span(r, from, to))),
                        Message::Flush(done) => flushed.push(done),
                    }
                }
                if !spans.is_empty() { post(to, &body(&resource, spans)); }
                for done in flushed { let _ = done.send(()); }
            }
        }).ok()?;
        Some(out)
    }).as_ref()
}

fn resource() -> Value {
    let service = std::env::var("OTEL_SERVICE_NAME").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| {
        std::env::current_exe().ok().and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned())).unwrap_or_else(|| "rune-app".into())
    });
    json!({"attributes": [
        kv("service.name", service.into()),
        kv("process.pid", u64::from(std::process::id()).into()),
        kv("rune.version", env!("CARGO_PKG_VERSION").into()),
    ]})
}

pub(crate) fn body(resource: &Value, spans: Vec<Value>) -> String {
    json!({"resourceSpans": [{
        "resource": resource,
        "scopeSpans": [{"scope": {"name": "rune.interaction"}, "spans": spans}],
    }]}).to_string()
}

/// A slow interaction as a trace: `ui.interaction` from input to present, with its stretches
/// (`queue_wait`, `update`, `encode`, `present_wait`) as children and the update's phases on
/// the `update` span.
pub(crate) fn slow_spans(s: &Slow) -> Vec<Value> {
    let trace = (u128::from(id()) << 64) | u128::from(id());
    let root = id();
    let mut attrs = vec![
        kv("rune.route", s.route.as_ref().into()),
        kv("rune.control", s.control.as_ref().into()),
        kv("rune.action", s.action.into()),
        kv("rune.signature", format!("{} {} {}", s.route, s.control, s.action).into()),
        kv("rune.input_to_present_ms", ms(s.arrived, s.present).into()),
        kv("rune.work_ms", ms(s.frame_start, s.submitted).into()),
        kv("rune.nodes", (s.nodes as u64).into()),
        kv("rune.reused_nodes", (s.reused as u64).into()),
        kv("rune.commits", u64::from(s.commits).into()),
        kv("rune.shaped", u64::from(s.shaped).into()),
    ];
    let why = crate::runtime::CommitWhy::names(s.why);
    if !why.is_empty() { attrs.push(kv("rune.commit_why", why.join(",").into())); }
    for (i, (key, n)) in s.rebuilt.iter().enumerate().take(3) {
        attrs.push(kv(["rune.rebuilt.0", "rune.rebuilt.1", "rune.rebuilt.2"][i], format!("{key} ({n})").into()));
    }
    attrs.push(kv("rune.cause", s.cause.name().into()));
    if let Some(cpu) = s.cpu_ms { attrs.push(kv("rune.cpu_ms", round(cpu).into())); }
    if let Some(stacks) = &s.stacks { stack_attrs(stacks, &mut attrs); }
    let mut out = vec![span(trace, root, None, "ui.interaction", s.arrived, s.present, attrs)];
    let mut update = vec![kv("rune.handler_ms", round(s.handler_ms).into())];
    for (name, ms) in PHASE_NAMES.iter().zip(s.phases) {
        if ms > 0.0 { update.push(kv(name, round(ms).into())); }
    }
    for (name, from, to, attrs) in [
        ("queue_wait", s.arrived, s.frame_start, vec![]),
        ("update", s.frame_start, s.built, update),
        ("encode", s.built, s.submitted, vec![]),
        ("present_wait", s.submitted, s.present, vec![]),
    ] {
        if to > from { out.push(span(trace, id(), Some(root), name, from, to, attrs)); }
    }
    out
}

/// A slow frame no input caused: its work, why it committed, and where its time went.
pub(crate) fn frame_span(f: &SlowFrame) -> Value {
    let trace = (u128::from(id()) << 64) | u128::from(id());
    let mut attrs = vec![
        kv("rune.work_ms", ms(f.start, f.built).into()),
        kv("rune.nodes", (f.nodes as u64).into()),
        kv("rune.cause", f.cause.name().into()),
    ];
    let why = crate::runtime::CommitWhy::names(f.why);
    if !why.is_empty() { attrs.push(kv("rune.commit_why", why.join(",").into())); }
    stack_attrs(&f.stacks, &mut attrs);
    span(trace, id(), None, "ui.frame", f.start, f.built, attrs)
}

/// The watchdog's samples: how many, how many found the thread waiting, the functions they
/// were in (`fn (n/N)`), and one sample's stack, innermost first, a line a frame.
fn stack_attrs(s: &Stacks, attrs: &mut Vec<Value>) {
    attrs.push(kv("rune.samples", u64::from(s.taken).into()));
    attrs.push(kv("rune.samples_waiting", u64::from(s.waiting).into()));
    for (i, (name, n)) in s.hot.iter().enumerate().take(3) {
        attrs.push(kv(["rune.hot.0", "rune.hot.1", "rune.hot.2"][i], format!("{name} ({n}/{})", s.taken).into()));
    }
    if !s.stack.is_empty() { attrs.push(kv("rune.stack", s.stack.join("\n").into())); }
}

pub(crate) fn roll_span(r: &Roll, from: Instant, to: Instant) -> Value {
    let trace = (u128::from(id()) << 64) | u128::from(id());
    span(trace, id(), None, "ui.rollup", from, to, vec![
        kv("rune.route", r.route.text.as_ref().into()),
        kv("rune.control", r.control.text.as_ref().into()),
        kv("rune.action", r.action.into()),
        kv("rune.count", u64::from(r.count).into()),
        kv("rune.slow", u64::from(r.slow).into()),
        kv("rune.scheduler_wait", u64::from(r.contended).into()),
        kv("rune.p50_ms", round(r.quantile(0.5)).into()),
        kv("rune.p98_ms", round(r.quantile(0.98)).into()),
        kv("rune.max_ms", round(r.max_ms).into()),
    ])
}

fn span(trace: u128, id: u64, parent: Option<u64>, name: &str, start: Instant, end: Instant, attrs: Vec<Value>) -> Value {
    let mut span = json!({
        "traceId": format!("{trace:032x}"), "spanId": format!("{id:016x}"),
        "name": name, "kind": 1,
        "startTimeUnixNano": unix_nanos(start).to_string(), "endTimeUnixNano": unix_nanos(end.max(start)).to_string(),
        "attributes": attrs,
    });
    if let Some(p) = parent { span["parentSpanId"] = format!("{p:016x}").into(); }
    span
}

fn kv(key: &str, value: Value) -> Value {
    let value = match value {
        Value::Bool(b) => json!({"boolValue": b}),
        Value::Number(n) if n.is_i64() || n.is_u64() => json!({"intValue": n.to_string()}),
        Value::Number(n) => json!({"doubleValue": n.as_f64()}),
        Value::String(s) => json!({"stringValue": s}),
        other => json!({"stringValue": other.to_string()}),
    };
    json!({"key": key, "value": value})
}

fn ms(from: Instant, to: Instant) -> f64 { round(to.saturating_duration_since(from).as_secs_f64() * 1000.0) }

fn round(ms: f64) -> f64 { (ms * 1000.0).round() / 1000.0 }

fn id() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    static SEED: OnceLock<std::collections::hash_map::RandomState> = OnceLock::new();
    let mut h = SEED.get_or_init(Default::default).build_hasher();
    h.write_u64(N.fetch_add(1, Ordering::Relaxed));
    h.finish().max(1)
}

fn unix_nanos(t: Instant) -> u128 {
    static ANCHOR: OnceLock<(Instant, u128)> = OnceLock::new();
    let (at, unix) = *ANCHOR.get_or_init(|| (Instant::now(), SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos())));
    match t.checked_duration_since(at) {
        Some(d) => unix + d.as_nanos(),
        None => unix.saturating_sub(at.duration_since(t).as_nanos()),
    }
}

/// One HTTP/1.1 POST on loopback.
fn post(to: SocketAddr, body: &str) {
    use std::io::{Read as _, Write as _};
    let Ok(mut s) = std::net::TcpStream::connect_timeout(&to, Duration::from_millis(250)) else { return };
    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let head = format!("POST /v1/traces HTTP/1.1\r\nHost: {to}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    if s.write_all(head.as_bytes()).and_then(|()| s.write_all(body.as_bytes())).is_ok() {
        let _ = s.read(&mut [0; 256]);
    }
}
