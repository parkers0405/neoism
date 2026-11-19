//! Real stdio peers and real HTTP daemon; no mocked MCP or board transport.
use neochat::{client::Client, protocol::{Action, Discovery, InviteRequest}};
use serde_json::{json, Value};
use std::{io::{BufRead, BufReader, Write}, process::{Child, ChildStdin, Command, Stdio}, sync::mpsc, time::{Duration, Instant}};

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); }
}

struct Peer {
    _process: Process,
    input: ChildStdin,
    output: mpsc::Receiver<String>,
    id: u64,
}
impl Peer {
    fn command() -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_neochat-mcp"));
        c.env_remove("NEOCHAT_URL").env_remove("NEOCHAT_TOKEN").env_remove("NEOCHAT_DATA_DIR");
        c
    }
    fn new(url: &str, token: &str) -> Self {
        let mut command = Self::command();
        command.env("NEOCHAT_URL", url).env("NEOCHAT_TOKEN", token);
        Self::spawn(command)
    }
    fn spawn(mut command: Command) -> Self {
        let mut child = command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        let input = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, output) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if tx.send(line.unwrap()).is_err() { break; }
            }
        });
        let mut peer = Self { _process: Process(child), input, output, id: 0 };
        let init = peer.rpc("initialize", json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"integration-test","version":"1"}}));
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(init["result"]["capabilities"]["tools"]["listChanged"], false);
        peer.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        peer
    }
    fn send(&mut self, value: Value) {
        writeln!(self.input, "{value}").unwrap();
        self.input.flush().unwrap();
    }
    fn receive(&self) -> Value {
        let line = self.output.recv_timeout(Duration::from_secs(8)).expect("MCP response timed out");
        serde_json::from_str(&line).expect("stdout must contain only newline JSON-RPC")
    }
    fn rpc(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        self.send(json!({"jsonrpc":"2.0","id":self.id,"method":method,"params":params}));
        let r = self.receive();
        assert_eq!(r["jsonrpc"], "2.0");
        assert_eq!(r["id"], self.id);
        r
    }
    fn tool(&mut self, name: &str, arguments: Value) -> Value {
        self.rpc("tools/call", json!({"name":name,"arguments":arguments}))["result"].clone()
    }
    fn ok(&mut self, name: &str, arguments: Value) -> Value {
        let r = self.tool(name, arguments);
        assert_eq!(r["isError"], false, "{r}");
        let text: Value = serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(text, r["structuredContent"]);
        text
    }
}

fn daemon(dir: &std::path::Path) -> (Process, Discovery) {
    let process = Process(Command::new(env!("CARGO_BIN_EXE_neochatd"))
        .args(["--bind", "127.0.0.1:0", "--data-dir"]).arg(dir)
        .stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(bytes) = std::fs::read(dir.join("endpoint.json")) {
            if let Ok(discovery) = serde_json::from_slice(&bytes) { return (process, discovery); }
        }
        assert!(Instant::now() < deadline, "daemon startup timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn two_actors_coordinate_over_stdio_with_private_isolation() {
    let dir = tempfile::tempdir().unwrap();
    let (_daemon, discovery) = daemon(dir.path());
    let board = Client::new(&discovery.url, &discovery.token, None).unwrap();
    for channel in ["work", "private"] {
        board.action(&Action::CreateChannel { channel: channel.into() }).unwrap();
    }
    let admin = Client::discover_admin(Some(dir.path())).unwrap();
    let invite = |actor: &str, channels: Vec<String>| admin.invite(&InviteRequest { actor:actor.into(), channels, expires_at:None }).unwrap();
    let alice = invite("alice", vec!["work".into(), "private".into()]);
    let bob = invite("bob", vec!["work".into()]);
    assert_ne!(alice.token, bob.token);
    let mut a = Peer::new(&discovery.url, &alice.token);
    let mut b = Peer::new(&discovery.url, &bob.token);
    assert_eq!(a.rpc("ping", json!({}))["result"], json!({}));
    let catalog = a.rpc("tools/list", json!({}));
    let names: Vec<_> = catalog["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["channels","read","post","reply","wait","tasks_list","tasks_create","tasks_claim","tasks_renew","tasks_complete","tasks_fail","members","tasks_assign","tasks_block","tasks_unblock","tasks_cancel"]);
    for tool in catalog["result"]["tools"].as_array().unwrap() {
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
    }

    let task = a.ok("tasks_create", json!({"channel":"work","text":"coordinate two independent actors"}));
    let id = task["id"].as_u64().unwrap();
    let task_cursor = task["cursor"].as_u64().unwrap();
    let listed = b.ok("tasks_list", json!({"channel":"work"}));
    assert_eq!(listed["messages"][0]["author"], "alice");
    assert_eq!(listed["messages"][0]["id"], id);

    // Both attempt to claim; exactly one owns the lease, and ownership is actor-bound.
    b.ok("tasks_claim", json!({"id":id,"lease_secs":60}));
    let rejected_claim = a.tool("tasks_claim", json!({"id":id,"lease_secs":60}));
    assert_eq!(rejected_claim["isError"], true);
    assert_eq!(rejected_claim["structuredContent"]["error"]["code"], "conflict");
    assert_eq!(a.tool("tasks_renew", json!({"id":id,"lease_secs":60}))["isError"], true);
    b.ok("tasks_renew", json!({"id":id,"lease_secs":60}));
    let events = a.ok("wait", json!({"after":task_cursor,"wait_ms":0,"limit":1}));
    assert_eq!(events["events"][0]["actor"], "bob");
    assert_eq!(events["events"][0]["message"]["task"]["owner"], "bob");
    assert_eq!(events["has_more"], true);
    let resumed = a.ok("wait", json!({"after":events["next_cursor"],"wait_ms":0}));
    assert!(resumed["next_cursor"].as_u64().unwrap() > events["next_cursor"].as_u64().unwrap());

    let done = b.ok("tasks_complete", json!({"id":id,"text":"result transferred"}));
    let done_events = a.ok("wait", json!({"after":resumed["next_cursor"],"wait_ms":0}));
    assert!(done_events["events"].as_array().unwrap().iter().any(|e| e["message"]["task"]["state"] == "completed"));
    assert_eq!(done_events["next_cursor"], done["cursor"]);
    a.ok("reply", json!({"thread":id,"text":"received"}));

    let secret = a.ok("post", json!({"channel":"private","text":"PRIVATE_SENTINEL"}));
    let channels = b.ok("channels", json!({}));
    assert_eq!(channels["channels"], json!(["work"]));
    assert!(!b.ok("read", json!({})).to_string().contains("PRIVATE_SENTINEL"));
    assert!(!b.ok("wait", json!({"after":0})).to_string().contains("PRIVATE_SENTINEL"));
    assert!(b.ok("read", json!({"channel":"private"}))["messages"].as_array().unwrap().is_empty());
    assert_eq!(b.tool("reply", json!({"thread":secret["id"],"text":"not allowed"}))["isError"], true);
    assert_eq!(b.tool("post", json!({"channel":"private","text":"not allowed"}))["isError"], true);
    assert_eq!(a.ok("read", json!({"channel":"private"}))["messages"][0]["text"], "PRIVATE_SENTINEL");

    // Start a bounded long poll in one process and transfer an event from the other.
    let cursor = board.snapshot().unwrap().cursor;
    b.id += 1;
    b.send(json!({"jsonrpc":"2.0","id":b.id,"method":"tools/call","params":{"name":"wait","arguments":{"after":cursor,"wait_ms":2000}}}));
    a.ok("post", json!({"channel":"work","text":"wake up peer"}));
    let transfer = b.receive();
    assert!(transfer["result"]["structuredContent"]["events"].as_array().unwrap().iter().any(|e| e["message"]["text"] == "wake up peer"));
    let cursor = transfer["result"]["structuredContent"]["next_cursor"].clone();
    let start = Instant::now();
    assert!(b.ok("wait", json!({"after":cursor,"wait_ms":30}))["events"].as_array().unwrap().is_empty());
    assert!(start.elapsed() < Duration::from_secs(2));

    // Fail is deliberately separate from complete.
    let task = a.ok("tasks_create", json!({"channel":"work","text":"failure path","needs":[],"assignee":"bob"}));
    b.ok("tasks_claim", json!({"id":task["id"],"lease_secs":60}));
    b.ok("tasks_fail", json!({"id":task["id"],"text":"cannot proceed"}));
    assert!(a.ok("tasks_list", json!({}))["messages"].as_array().unwrap().iter().any(|m| m["task"]["state"] == "failed"));
    let task=a.ok("tasks_create",json!({"channel":"work","text":"test release"}));let id=&task["id"];
    a.ok("tasks_assign",json!({"id":id,"assignee":"bob"}));
    a.ok("tasks_block",json!({"id":id,"text":"waiting for review"}));
    assert_eq!(b.ok("tasks_list",json!({"thread":id,"assignee":"bob","state":"blocked"}))["messages"].as_array().unwrap().len(),1);
    a.ok("tasks_unblock",json!({"id":id}));a.ok("tasks_cancel",json!({"id":id}));
    assert_eq!(b.ok("tasks_list",json!({"thread":id,"state":"canceled"}))["messages"].as_array().unwrap().len(),1);

    // Transport errors are JSON-RPC errors; execution failures are MCP tool errors.
    for arguments in [json!({"after":0,"wait_ms":25001}), json!({"after":-1}), json!({"after":0,"limit":101}), json!({"after":0,"command":"sh"})] {
        assert_eq!(a.rpc("tools/call", json!({"name":"wait","arguments":arguments}))["error"]["code"], -32602);
    }
    assert_eq!(a.rpc("tools/call", json!({"name":"admin","arguments":{}}))["error"]["code"], -32602);
    assert_eq!(a.rpc("unknown", json!({}))["error"]["code"], -32601);
    a.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":999}}));
    assert_eq!(a.rpc("ping", json!({}))["result"], json!({}));
    a.input.write_all(b"{invalid\n").unwrap(); a.input.flush().unwrap();
    assert_eq!(a.receive()["error"]["code"], -32700);
    a.input.write_all(&vec![b'x'; 128 * 1024 + 1]).unwrap();
    a.input.write_all(b"\n").unwrap(); a.input.flush().unwrap();
    assert_eq!(a.receive()["error"]["code"], -32700);
    assert_eq!(a.rpc("ping", json!({}))["result"], json!({}));

    // Explicit local discovery works with no admin-token file available.
    std::fs::remove_file(dir.path().join("admin-token")).unwrap();
    let mut command = Peer::command(); command.arg("--data-dir").arg(dir.path());
    let mut local = Peer::spawn(command);
    assert!(local.ok("channels", json!({}))["channels"].as_array().unwrap().iter().any(|c| c == "private"));
}

#[test]
fn explicit_credentials_and_safe_transport_required_without_secret_logging() {
    for (url, token) in [(None, None), (Some("https://example.invalid"), None), (None, Some("SECRET_SENTINEL")), (Some("http://example.invalid"), Some("SECRET_SENTINEL")), (Some("https://user:SECRET_SENTINEL@example.invalid"), Some("SECRET_SENTINEL"))] {
        let mut command = Peer::command();
        if let Some(url) = url { command.env("NEOCHAT_URL",url); }
        if let Some(token) = token { command.env("NEOCHAT_TOKEN",token); }
        let output = command.output().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("SECRET_SENTINEL"));
    }
}