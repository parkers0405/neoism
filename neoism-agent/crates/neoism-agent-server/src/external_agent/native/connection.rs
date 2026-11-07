use super::*;
use std::collections::VecDeque;
use std::sync::atomic::AtomicU64;
use std::sync::Mutex as StdMutex;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot, Notify};

pub(super) enum WireEvent {
    Event { operation: String, event: Value },
    Request { id: Value, operation: String, method: String, params: Value },
    Barrier(oneshot::Sender<()>),
    Closed(String),
}

type Pending = Arc<tokio::sync::Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

struct Lifetime(StdMutex<Option<oneshot::Sender<()>>>);
impl Lifetime {
    fn stop(&self) {
        if let Ok(mut slot) = self.0.lock() {
            if let Some(sender) = slot.take() { let _ = sender.send(()); }
        }
    }
}
impl Drop for Lifetime { fn drop(&mut self) { self.stop(); } }

#[derive(Clone)]
pub(super) struct NativeClient {
    writer: mpsc::Sender<Value>,
    pending: Pending,
    next_id: Arc<AtomicU64>,
    events: mpsc::Sender<WireEvent>,
    closed: Arc<AtomicBool>,
    done: Arc<Notify>,
    lifetime: Arc<Lifetime>,
}

pub(super) struct Call {
    pub(super) id: u64,
    receiver: Option<oneshot::Receiver<Result<Value, String>>>,
    pending: Pending,
}
impl Call {
    pub(super) async fn wait(mut self, timeout: Duration) -> Result<Value, String> {
        let result = tokio::time::timeout(timeout, self.receiver.take().unwrap()).await;
        self.pending.lock().await.remove(&self.id);
        result.map_err(|_| "native provider request timed out".to_string())?
            .map_err(|_| "native provider connection closed".to_string())?
    }
}
impl Drop for Call {
    fn drop(&mut self) {
        if let Ok(mut pending) = self.pending.try_lock() { pending.remove(&self.id); }
    }
}

impl NativeClient {
    pub(super) fn spawn(node: &str, host: &Path, cwd: &Path) -> Result<(Self, mpsc::Receiver<WireEvent>), String> {
        let mut command = tokio::process::Command::new(node);
        command.arg(host).current_dir(cwd)
            .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped()).kill_on_drop(true);
        crate::tool::process::set_new_process_group(&mut command);
        let mut child = command.spawn().map_err(|error| format!("Could not start native agent host: {error}"))?;
        let pid = child.id();
        let mut stdin = child.stdin.take().ok_or("native host has no stdin")?;
        let stdout = child.stdout.take().ok_or("native host has no stdout")?;
        let stderr = child.stderr.take().ok_or("native host has no stderr")?;
        let (writer, mut writes) = mpsc::channel::<Value>(256);
        let (events, receiver) = mpsc::channel(1024);
        let pending: Pending = Arc::default();
        let (shutdown, stop) = oneshot::channel();
        let lifetime = Arc::new(Lifetime(StdMutex::new(Some(shutdown))));
        let closed = Arc::new(AtomicBool::new(false));
        let done = Arc::new(Notify::new());
        let client = Self { writer, pending: pending.clone(), next_id: Arc::new(AtomicU64::new(1)),
            events: events.clone(), closed: closed.clone(), done: done.clone(), lifetime: lifetime.clone() };
        let weak = Arc::downgrade(&lifetime);
        tokio::spawn(async move {
            while let Some(value) = writes.recv().await {
                let mut line = match serde_json::to_vec(&value) { Ok(line) => line, Err(_) => continue };
                line.push(b'\n');
                if stdin.write_all(&line).await.is_err() { break; }
            }
            if let Some(lifetime) = weak.upgrade() { lifetime.stop(); }
        });
        let read_pending = pending.clone();
        let read_events = events.clone();
        let read_lifetime = Arc::downgrade(&lifetime);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.len() > 16 * 1024 * 1024 { break; }
                let value: Value = match serde_json::from_str(&line) {
                    Ok(value) => value,
                    Err(_) => break,
                };
                if let Some(method) = value.get("method").and_then(Value::as_str) {
                    let params = value.get("params").cloned().unwrap_or(Value::Null);
                    let operation = params.get("operationId").and_then(Value::as_str).unwrap_or_default().to_string();
                    let event = if let Some(id) = value.get("id") {
                        WireEvent::Request { id: id.clone(), operation, method: method.into(), params }
                    } else if method == "runtime.event" {
                        WireEvent::Event { operation, event: params.get("event").cloned().unwrap_or(Value::Null) }
                    } else { continue; };
                    if read_events.send(event).await.is_err() { break; }
                } else if let Some(id) = value.get("id").and_then(Value::as_u64) {
                    if let Some(sender) = read_pending.lock().await.remove(&id) {
                        let result = if let Some(error) = value.get("error") {
                            Err(error.get("message").and_then(Value::as_str).unwrap_or("native provider error").to_string())
                        } else { Ok(value.get("result").cloned().unwrap_or(Value::Null)) };
                        let _ = sender.send(result);
                    }
                }
            }
            if let Some(lifetime) = read_lifetime.upgrade() { lifetime.stop(); }
        });
        let tail = Arc::new(StdMutex::new(VecDeque::<String>::new()));
        let stderr_tail = tail.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(mut tail) = stderr_tail.lock() {
                    tail.push_back(line.chars().take(2048).collect());
                    while tail.len() > 16 { tail.pop_front(); }
                }
            }
        });
        tokio::spawn(async move {
            let status = tokio::select! {
                result = child.wait() => result.ok(),
                _ = stop => {
                    crate::tool::process::kill_process_group(pid);
                    let _ = child.start_kill();
                    child.wait().await.ok()
                }
            };
            crate::tool::process::kill_process_group(pid);
            closed.store(true, Ordering::SeqCst);
            let diagnostic = tail.lock().map(|tail| tail.iter().cloned().collect::<Vec<_>>().join("\n")).unwrap_or_default();
            let error = format!("native agent host exited ({status:?}){separator}{diagnostic}", separator = if diagnostic.is_empty() { "" } else { ":\n" });
            for (_, sender) in pending.lock().await.drain() { let _ = sender.send(Err(error.clone())); }
            done.notify_waiters();
            let _ = events.send(WireEvent::Closed(error)).await;
        });
        Ok((client, receiver))
    }

    pub(super) fn is_closed(&self) -> bool { self.closed.load(Ordering::SeqCst) }

    pub(super) async fn reserve(&self) -> Result<Call, String> {
        if self.is_closed() { return Err("native provider connection is closed".into()); }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            if pending.len() >= 4096 { return Err("too many pending native provider requests".into()); }
            pending.insert(id, sender);
        }
        Ok(Call { id, receiver: Some(receiver), pending: self.pending.clone() })
    }

    pub(super) async fn dispatch(&self, call: &Call, method: &str, params: Value) -> Result<(), String> {
        self.writer.send(json!({"jsonrpc":"2.0","id":call.id,"method":method,"params":params})).await
            .map_err(|_| "native provider writer closed".to_string())?;
        Ok(())
    }

    pub(super) async fn begin(&self, method: &str, params: Value) -> Result<Call, String> {
        let call = self.reserve().await?;
        self.dispatch(&call, method, params).await?;
        Ok(call)
    }

    pub(super) async fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        self.begin(method, params).await?.wait(timeout).await
    }

    pub(super) async fn reply(&self, id: Value, result: Result<Value, String>) {
        let reply = match result {
            Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
            Err(message) => json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":message}}),
        };
        let _ = self.writer.send(reply).await;
    }

    pub(super) async fn fence(&self) -> Result<(), String> {
        let (sender, receiver) = oneshot::channel();
        self.events.send(WireEvent::Barrier(sender)).await.map_err(|_| "native event stream closed".to_string())?;
        tokio::time::timeout(Duration::from_secs(10), receiver).await
            .map_err(|_| "native event projection timed out".to_string())?
            .map_err(|_| "native event projection closed".to_string())
    }

    pub(super) async fn close(&self) {
        if !self.is_closed() {
            let _ = self.call("runtime.close", json!({}), Duration::from_secs(5)).await;
        }
        self.lifetime.stop();
        let done = self.done.notified();
        tokio::pin!(done);
        done.as_mut().enable();
        if !self.is_closed() { let _ = tokio::time::timeout(Duration::from_secs(5), done).await; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (NativeClient, mpsc::Receiver<WireEvent>) {
        let services = crate::standard_services();
        let node = services.executables.resolve(&neoism_agent_service_api::ExecutableRequest::new(
            "node", neoism_agent_service_api::ExecutablePurpose::ExternalAgent,
        )).expect("native runtime tests require Node.js").path;
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/external_agent/native");
        NativeClient::spawn(node.to_str().unwrap(), &directory.join("connection_fixture.mjs"), &directory).unwrap()
    }

    #[tokio::test]
    async fn early_events_and_reverse_requests_are_owned_before_dispatch() {
        let (client, mut events) = fixture();
        let call = client.reserve().await.unwrap();
        let operation = call.id.to_string();
        client.dispatch(&call, "turn.start", json!({"text":"hello"})).await.unwrap();
        let WireEvent::Event { operation: owner, event } = events.recv().await.unwrap() else { panic!("missing early event") };
        assert_eq!(owner, operation);
        assert_eq!(event["text"], "early");
        let WireEvent::Request { id, operation: owner, method, .. } = events.recv().await.unwrap() else { panic!("missing reverse request") };
        assert_eq!(owner, operation);
        assert_eq!(method, "permission.request");
        client.reply(id, Ok(json!({"decision":"allow_once"}))).await;
        let result = call.wait(Duration::from_secs(5)).await.unwrap();
        assert_eq!(result["approval"]["decision"], "allow_once");
        let fence_client = client.clone();
        let fence = tokio::spawn(async move { fence_client.fence().await });
        let WireEvent::Event { operation: owner, event } = events.recv().await.unwrap() else { panic!("completion lost") };
        assert_eq!(owner, operation);
        assert_eq!(event["type"], "text.completed");
        let WireEvent::Barrier(sender) = events.recv().await.unwrap() else { panic!("barrier overtook event") };
        sender.send(()).unwrap();
        fence.await.unwrap().unwrap();
        assert_eq!(client.call("fail", json!({}), Duration::from_secs(5)).await.unwrap_err(), "native failure");
        client.close().await;
        assert!(client.is_closed());
    }

    #[tokio::test]
    async fn subprocess_death_rejects_pending_calls_and_retires_connection() {
        let (client, mut events) = fixture();
        let error = client.call("die", json!({}), Duration::from_secs(5)).await.unwrap_err();
        assert!(error.contains("native agent host exited"));
        assert!(client.is_closed());
        assert!(client.reserve().await.is_err());
        assert!(matches!(events.recv().await, Some(WireEvent::Closed(_))));
        client.close().await;
    }
}