use super::*;

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Barrier},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde_json::json;
use tokio::sync::broadcast::error::TryRecvError;

use super::super::{
    lsp_adapters::LanguageAdapter, path_to_file_uri, subscribe_diagnostics,
    DiagnosticsEvent, LspDiagnostic,
};

/// Exercises the same persistent process, JSON-RPC framing, reader thread,
/// notification bus, and per-server diagnostics cache used in production. The
/// fake server is compiled with the active Rust toolchain, so this test never
/// depends on rust-analyzer (or any other machine-global LSP) being installed.
#[test]
fn persistent_stdio_lsp_lifecycle_is_event_driven_and_consistent() {
    let workspace = TestWorkspace::new("transport");
    let file = workspace.path.join("src/main.rs");
    fs::create_dir_all(file.parent().expect("source parent")).expect("create src");
    fs::write(&file, "disk text must not replace the live buffer\n")
        .expect("write source file");

    let log = workspace.path.join("fake-lsp.log");
    let fake_server = compile_fake_server(&workspace);
    write_lsp_config(&workspace.path, &fake_server, &log, "serve");

    let configured_spec = configured_test_spec(&workspace.path);
    let spec = &configured_spec;
    let service = service();
    service.clear_diagnostics(&workspace.path, &file);
    let mut events = subscribe_diagnostics();

    service
        .sync(&workspace.path, &file, Some("broken_v1"), spec)
        .expect("initialize fake LSP and send didOpen");
    let opened = wait_for_event(&mut events, &file, |event| {
        event.diagnostics.len() == 1
            && event.diagnostics[0].message == "diagnostic broken_v1"
    });
    assert_eq!(opened.language, "protocol-test");
    assert_eq!(opened.server_id, "protocol-test");
    assert_eq!(opened.root, workspace.path);
    assert_eq!(
        opened.diagnostics[0].language.as_deref(),
        Some("protocol-test")
    );
    assert_eq!(
        service.cached_diagnostics(&workspace.path, &file),
        opened.diagnostics,
        "the push event and cache must expose the same snapshot"
    );
    assert!(service
        .live_languages(&workspace.path)
        .contains("protocol-test"));

    let actions = service
        .code_actions(&workspace.path, &file, 7, 2, spec)
        .expect("query code actions with cached diagnostics");
    assert_eq!(actions.as_array().map(Vec::len), Some(2));
    assert_eq!(actions[0]["title"], "Fix fake error");
    assert_eq!(actions[0]["isPreferred"], true);
    assert_eq!(actions[1]["disabled"]["reason"], "fixture is indexing");

    let resolved = crate::lsp::resolve_code_action(
        &workspace.path,
        &file,
        "protocol-test",
        actions[0].clone(),
    )
    .expect("resolve on the originating code-action server");
    assert!(resolved.pointer("/edit/changes").is_some());
    assert_eq!(resolved["command"]["command"], "fixture.afterFix");
    assert!(crate::lsp::resolve_code_action(
        &workspace.path,
        &file,
        "not-the-originating-server",
        actions[0].clone(),
    )
    .is_none());
    let executed = crate::lsp::execute_command(
        &workspace.path,
        &file,
        "protocol-test",
        json!({
            "command": "fixture.afterFix",
            "arguments": [{"uri": path_to_file_uri(&file)}]
        }),
    )
    .expect("execute command on the originating code-action server");
    assert_eq!(executed, json!({"executed": true}));

    let hover = service
        .hover(&workspace.path, &file, 0, 3, spec)
        .expect("query hover from persistent fake LSP");
    assert_eq!(hover.len(), 1);
    assert_eq!(hover[0].contents, "fake hover for broken_v1 at 0:3");

    service
        .sync(&workspace.path, &file, Some("broken_v2"), spec)
        .expect("send didChange");
    wait_for_event(&mut events, &file, |event| {
        event.diagnostics.len() == 1
            && event.diagnostics[0].message == "diagnostic broken_v2"
    });
    assert_eq!(
        service.cached_diagnostics(&workspace.path, &file)[0].message,
        "diagnostic broken_v2",
        "a newer publish must replace this server's old snapshot"
    );

    // A byte-identical buffer must not cause another didChange. The hover is a
    // protocol barrier: once it returns, the fake server has processed every
    // earlier notification on stdin, so the log assertion is race-free.
    service
        .sync(&workspace.path, &file, Some("broken_v2"), spec)
        .expect("deduplicate unchanged live text");
    let hover = service
        .hover(&workspace.path, &file, 4, 9, spec)
        .expect("hover after didChange");
    assert_eq!(hover[0].contents, "fake hover for broken_v2 at 4:9");
    assert_eq!(count_log_lines(&log, "didChange:"), 1);

    service
        .sync(&workspace.path, &file, Some("clean"), spec)
        .expect("send clearing didChange");
    let cleared =
        wait_for_event(&mut events, &file, |event| event.diagnostics.is_empty());
    assert!(cleared.diagnostics.is_empty());
    service
        .hover(&workspace.path, &file, 0, 0, spec)
        .expect("barrier after stale versioned publish");
    assert!(
        service.cached_diagnostics(&workspace.path, &file).is_empty(),
        "an empty current publish must clear errors and a later stale publish must stay ignored"
    );

    // Insert mode can produce several document revisions before the server
    // finishes analysing the first one. Send them without waiting between
    // calls: the transport must preserve didChange order, the current empty
    // publication must clear immediately, and the fake server's deliberately
    // late prior-version publication must not resurrect an error.
    service
        .sync(&workspace.path, &file, Some("broken_v1"), spec)
        .expect("send first rapid insert edit");
    service
        .sync(&workspace.path, &file, Some("broken_v2"), spec)
        .expect("send second rapid insert edit");
    service
        .sync(&workspace.path, &file, Some("clean"), spec)
        .expect("send rapid fixing edit");
    wait_for_event(&mut events, &file, |event| event.diagnostics.is_empty());
    service
        .hover(&workspace.path, &file, 0, 0, spec)
        .expect("rapid-edit protocol barrier");
    assert!(
        service
            .cached_diagnostics(&workspace.path, &file)
            .is_empty(),
        "fixing an error in the live buffer must clear it without a save or poll"
    );
    service
        .save(&workspace.path, &file, spec)
        .expect("send didSave after every queued insert edit");

    // A second server's snapshot for the same file must survive an empty
    // protocol-test publish. This ownership rule prevents alternating counts
    // when more than one server is attached.
    service
        .sync(&workspace.path, &file, Some("broken_v2"), spec)
        .expect("restore protocol-test diagnostic");
    wait_for_event(&mut events, &file, |event| {
        event.diagnostics.len() == 1
            && event.diagnostics[0].message == "diagnostic broken_v2"
    });
    service.store_diagnostics(
        &workspace.path,
        &file,
        "secondary-server",
        "secondary",
        vec![diagnostic("secondary")],
    );
    service
        .sync(&workspace.path, &file, Some("clean"), spec)
        .expect("clear only protocol-test-owned diagnostics");
    let ownership_event = wait_for_event(&mut events, &file, |event| {
        event.diagnostics.len() == 1 && event.diagnostics[0].message == "secondary"
    });
    assert_eq!(
        ownership_event.diagnostics[0].language.as_deref(),
        Some("secondary")
    );
    assert_eq!(
        service.cached_diagnostics(&workspace.path, &file),
        ownership_event.diagnostics
    );

    // Confirm the exact lifecycle and monotonically increasing full-document
    // versions observed by the server.
    service
        .hover(&workspace.path, &file, 0, 0, spec)
        .expect("final protocol barrier");
    let protocol_log = fs::read_to_string(&log).expect("read fake LSP log");
    assert!(protocol_log.contains("initialize:valid=true\n"));
    assert!(protocol_log.contains(
        "serverRequest:workspace/configuration:id=700:result=[{\"fixtureSetting\":\"settings-only\"},null]\n"
    ));
    assert!(protocol_log.contains(
        "serverRequest:window/workDoneProgress/create:id=progress-701:error=-32803\n"
    ));
    assert!(protocol_log
        .contains("serverRequest:client/registerCapability:id=702:error=-32803\n"));
    assert!(protocol_log
        .contains("serverRequest:client/unregisterCapability:id=703:error=-32803\n"));
    assert!(protocol_log.contains(
        "serverRequest:workspace/semanticTokens/refresh:id=704:error=-32803\n"
    ));
    assert!(protocol_log.contains(
        "serverRequest:client/registerCapability:workspaceFolders:id=705:result=null\n"
    ));
    assert!(protocol_log.contains(
        "serverRequest:client/unregisterCapability:workspaceFolders:id=706:result=null\n"
    ));
    assert!(protocol_log
        .contains("serverRequest:workspace/workspaceFolders:id=707:valid=true\n"));
    assert!(protocol_log.contains("initialized\n"));
    assert!(protocol_log.contains("didChangeConfiguration:settings-only\n"));
    assert!(protocol_log.contains("codeAction:valid=true\n"));
    assert!(protocol_log.contains("codeActionResolve:valid=true\n"));
    assert!(protocol_log.contains("executeCommand:valid=true\n"));
    assert!(protocol_log.contains("didSave:text=clean:includesText=true\n"));
    assert!(protocol_log
        .contains("didOpen:language=protocol-test:version=0:text=broken_v1\n"));
    assert_eq!(count_log_lines(&log, "didOpen:"), 1);
    assert_eq!(
        log_lines(&protocol_log, "didChange:"),
        vec![
            "didChange:version=1:text=broken_v2",
            "didChange:version=2:text=clean",
            "didChange:version=3:text=broken_v1",
            "didChange:version=4:text=broken_v2",
            "didChange:version=5:text=clean",
            "didChange:version=6:text=broken_v2",
            "didChange:version=7:text=clean",
        ]
    );

    service.store_diagnostics(
        &workspace.path,
        &file,
        "secondary-server",
        "secondary",
        Vec::new(),
    );
    service.clear_diagnostics(&workspace.path, &file);
}

#[test]
fn failed_initialize_is_recorded_as_unhealthy_and_never_connected() {
    let workspace = TestWorkspace::new("failed-initialize");
    let file = workspace.path.join("src/main.rs");
    fs::create_dir_all(file.parent().expect("source parent")).expect("create src");
    fs::write(&file, "fn main() {}\n").expect("write source file");

    let log = workspace.path.join("fake-lsp.log");
    let fake_server = compile_fake_server(&workspace);
    write_lsp_config(&workspace.path, &fake_server, &log, "fail-initialize");

    let configured_spec = configured_test_spec(&workspace.path);
    let spec = &configured_spec;
    let service = service();
    let error = service
        .sync(&workspace.path, &file, Some("fn main() {}"), spec)
        .expect_err("server that exits during initialize must fail attachment");
    let reason = service
        .broken_reason(&workspace.path, spec)
        .expect("failed client reason retained for status reporting");

    assert!(
        reason.contains("initialize") || reason.contains("response id"),
        "unexpected health reason: {reason}"
    );
    assert!(error.to_string().contains("initialize"));
    assert!(!service
        .live_languages(&workspace.path)
        .contains("protocol-test"));
    assert_eq!(
        fs::read_to_string(&log).expect("read failure log"),
        "initialize:forced-failure\n"
    );
}

#[test]
fn crashed_server_becomes_unhealthy_and_restarts_on_next_sync() {
    let workspace = TestWorkspace::new("restart-after-crash");
    let file = workspace.path.join("src/main.rs");
    fs::create_dir_all(file.parent().expect("source parent")).expect("create src");
    fs::write(&file, "disk text\n").expect("write source file");

    let log = workspace.path.join("fake-lsp.log");
    let fake_server = compile_fake_server(&workspace);
    write_lsp_config(&workspace.path, &fake_server, &log, "crash-once");

    let configured_spec = configured_test_spec(&workspace.path);
    let spec = &configured_spec;
    let service = service();
    service.clear_diagnostics(&workspace.path, &file);
    let mut events = subscribe_diagnostics();

    service
        .sync(&workspace.path, &file, Some("broken_v1"), spec)
        .expect("the first didOpen is delivered before the server crashes");
    wait_for_event(&mut events, &file, |event| {
        event.diagnostics.len() == 1
            && event.diagnostics[0].message == "diagnostic broken_v1"
    });

    let reason = wait_for_broken(service, &workspace.path, spec);
    assert!(
        reason.contains("exited"),
        "unexpected crash reason: {reason}"
    );
    assert!(!service
        .live_languages(&workspace.path)
        .contains("protocol-test"));

    thread::sleep(LSP_RECONNECT_BACKOFF + Duration::from_millis(25));
    service
        .sync(&workspace.path, &file, Some("broken_v2"), spec)
        .expect("the next sync must spawn and initialize a replacement server");
    wait_for_event(&mut events, &file, |event| {
        event.diagnostics.len() == 1
            && event.diagnostics[0].message == "diagnostic broken_v2"
    });
    assert!(service.broken_reason(&workspace.path, spec).is_none());
    assert!(service
        .live_languages(&workspace.path)
        .contains("protocol-test"));

    // A hover is the protocol barrier proving the replacement retained the
    // live didOpen text rather than silently falling back to stale disk text.
    let hover = service
        .hover(&workspace.path, &file, 2, 5, spec)
        .expect("hover through replacement server");
    assert_eq!(hover[0].contents, "fake hover for broken_v2 at 2:5");
    let protocol_log = fs::read_to_string(&log).expect("read restart log");
    assert_eq!(log_lines(&protocol_log, "initialize:valid=true").len(), 2);
    assert_eq!(count_log_lines(&log, "didOpen:"), 2);
    assert_eq!(count_log_lines(&log, "crash-after-open"), 1);

    service.clear_diagnostics(&workspace.path, &file);
}

#[test]
fn empty_hover_uses_one_exact_position_request() {
    let workspace = TestWorkspace::new("one-hover-request");
    let file = workspace.path.join("src/main.ptest");
    fs::create_dir_all(file.parent().expect("source parent")).expect("create src");
    fs::write(&file, "fn main() {}\n").expect("write source file");

    let log = workspace.path.join("fake-lsp.log");
    let fake_server = compile_fake_server(&workspace);
    write_lsp_config_with_extension(
        &workspace.path,
        &fake_server,
        &log,
        "empty-hover",
        "ptest",
    );
    let spec = super::super::lsp_adapters::adapters_for_root(&workspace.path)
        .into_iter()
        .find(|adapter| adapter.id == "protocol-test")
        .expect("configured exact-position adapter");
    service()
        .sync(&workspace.path, &file, Some("fn main() {}\n"), &spec)
        .expect("initialize exact-position fake LSP");

    let hover = super::super::hover(&workspace.path, &file, 0, 3);
    assert!(hover.is_empty());
    assert_eq!(
        count_log_lines(&log, "hover:"),
        1,
        "one editor query must produce one LSP request even when the response is empty"
    );
}

#[test]
fn concurrent_cold_opens_initialize_one_shared_client() {
    let workspace = TestWorkspace::new("singleflight-initialize");
    let first = workspace.path.join("src/first.ptest");
    let second = workspace.path.join("src/second.ptest");
    fs::create_dir_all(first.parent().expect("source parent")).expect("create src");
    fs::write(&first, "clean\n").expect("write first source");
    fs::write(&second, "clean\n").expect("write second source");

    let log = workspace.path.join("fake-lsp.log");
    let fake_server = compile_fake_server(&workspace);
    write_lsp_config_with_extension(
        &workspace.path,
        &fake_server,
        &log,
        "serve",
        "ptest",
    );
    let spec = configured_test_spec(&workspace.path);
    let barrier = Arc::new(Barrier::new(3));
    let mut workers = Vec::new();
    for file in [first.clone(), second.clone()] {
        let root = workspace.path.clone();
        let spec = spec.clone();
        let barrier = Arc::clone(&barrier);
        workers.push(thread::spawn(move || {
            barrier.wait();
            service()
                .sync(&root, &file, Some("clean\n"), &spec)
                .expect("concurrent cold-open sync");
        }));
    }
    barrier.wait();
    for worker in workers {
        worker.join().expect("cold-open worker");
    }
    service()
        .hover(&workspace.path, &first, 0, 0, &spec)
        .expect("protocol barrier after concurrent opens");

    let protocol_log = fs::read_to_string(&log).expect("read singleflight log");
    assert_eq!(log_lines(&protocol_log, "initialize:valid=true").len(), 1);
    assert_eq!(count_log_lines(&log, "didOpen:"), 2);
}

#[test]
fn request_failure_keeps_the_healthy_transport_attached() {
    let workspace = TestWorkspace::new("nonfatal-request-error");
    let first = workspace.path.join("src/first.ptest");
    let second = workspace.path.join("src/second.ptest");
    fs::create_dir_all(first.parent().expect("source parent")).expect("create src");
    fs::write(&first, "clean\n").expect("write first source");
    fs::write(&second, "clean\n").expect("write second source");

    let log = workspace.path.join("fake-lsp.log");
    let fake_server = compile_fake_server(&workspace);
    write_lsp_config_with_extension(
        &workspace.path,
        &fake_server,
        &log,
        "request-error",
        "ptest",
    );
    let spec = configured_test_spec(&workspace.path);
    let service = service();
    service
        .sync(&workspace.path, &first, Some("clean\n"), &spec)
        .expect("open first file");
    service
        .hover(&workspace.path, &first, 0, 0, &spec)
        .expect_err("fixture hover returns RequestFailed");
    assert!(service.broken_reason(&workspace.path, &spec).is_none());

    service
        .sync(&workspace.path, &second, Some("clean\n"), &spec)
        .expect("ordinary request failure must not force reinitialize");
    service
        .hover(&workspace.path, &second, 0, 0, &spec)
        .expect_err("second fixture hover returns RequestFailed");

    let protocol_log = fs::read_to_string(&log).expect("read request-error log");
    assert_eq!(log_lines(&protocol_log, "initialize:valid=true").len(), 1);
    assert_eq!(count_log_lines(&log, "didOpen:"), 2);
}

#[test]
fn changed_adapter_configuration_replaces_the_old_transport() {
    let workspace = TestWorkspace::new("config-replacement");
    let file = workspace.path.join("src/main.rs");
    fs::create_dir_all(file.parent().expect("source parent")).expect("create src");
    fs::write(&file, "fn main() {}\n").expect("write source file");
    let log = workspace.path.join("fake-lsp.log");
    let fake_server = compile_fake_server(&workspace);

    write_lsp_config(&workspace.path, &fake_server, &log, "serve");
    let first = configured_test_spec(&workspace.path);
    service()
        .sync(&workspace.path, &file, Some("broken_v1"), &first)
        .expect("start first configured transport");
    service()
        .hover(&workspace.path, &file, 0, 0, &first)
        .expect("first transport barrier");

    write_lsp_config(&workspace.path, &fake_server, &log, "replacement-transport");
    super::super::lsp_adapters::invalidate_adapter_cache(&workspace.path);
    let replacement = configured_test_spec(&workspace.path);
    service()
        .sync(&workspace.path, &file, Some("broken_v2"), &replacement)
        .expect("start replacement configured transport");
    service()
        .hover(&workspace.path, &file, 0, 0, &replacement)
        .expect("replacement transport barrier");

    let live_for_adapter = service()
        .clients
        .lock()
        .expect("client map")
        .keys()
        .filter(|key| key.root == workspace.path && key.adapter_id == "protocol-test")
        .count();
    assert_eq!(
        live_for_adapter, 1,
        "superseded transport leaked in client map"
    );
    let protocol_log = fs::read_to_string(&log).expect("read replacement log");
    assert_eq!(log_lines(&protocol_log, "initialize:valid=true").len(), 2);
    assert_eq!(count_log_lines(&log, "didOpen:"), 2);
}

#[test]
fn nested_project_client_keeps_outer_workspace_cache_and_event_ownership() {
    let workspace = TestWorkspace::new("nested-project-root");
    let project = workspace.path.join("fixtures/nested-project");
    let file = project.join("src/main.ptest");
    fs::create_dir_all(file.parent().expect("nested source parent"))
        .expect("create nested source");
    fs::write(&file, "broken_v1").expect("write nested source");
    fs::write(project.join("protocol.project"), "fixture")
        .expect("write nested project marker");

    let log = workspace.path.join("nested-fake-lsp.log");
    let fake_server = compile_fake_server(&workspace);
    let config = json!({
        "lsp": {
            "protocol-test": {
                "name": "Nested project protocol test LSP",
                "language": "protocol-test",
                "command": [
                    fake_server.to_string_lossy(),
                    "serve",
                    path_to_file_uri(&file),
                    log.to_string_lossy(),
                    path_to_file_uri(&project),
                ],
                "extensions": ["ptest"],
                "markers": ["protocol.project"],
                "initializationOptions": { "fixtureInit": "init-only" },
                "settings": {
                    "protocol-test": { "fixtureSetting": "settings-only" }
                }
            }
        }
    });
    fs::write(
        workspace.path.join("neoism.json"),
        serde_json::to_vec_pretty(&config).expect("serialize nested config"),
    )
    .expect("write nested adapter config");

    let spec = configured_test_spec(&workspace.path);
    let service = service();
    service.clear_diagnostics(&workspace.path, &file);
    let mut events = subscribe_diagnostics();
    service
        .sync(&workspace.path, &file, Some("broken_v1"), &spec)
        .expect("sync nested project document");

    let event = wait_for_event(&mut events, &file, |event| {
        event.diagnostics.len() == 1
            && event.diagnostics[0].message == "diagnostic broken_v1"
    });
    assert_eq!(event.root, workspace.path, "event keeps outer UI ownership");
    assert_eq!(
        service.cached_diagnostics(&workspace.path, &file),
        event.diagnostics,
        "outer workspace cache retrieves nested-project diagnostics"
    );
    let keys = service
        .clients
        .lock()
        .expect("client map")
        .keys()
        .filter(|key| key.root == workspace.path && key.adapter_id == "protocol-test")
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].project_root, project);
    assert!(fs::read_to_string(&log)
        .expect("nested protocol log")
        .contains("initialize:valid=true"));

    service
        .close_document(&workspace.path, &file)
        .expect("outer workspace closes nested document");
    assert!(service
        .cached_diagnostics(&workspace.path, &file)
        .is_empty());
}

fn configured_test_spec(root: &Path) -> LanguageAdapter {
    super::super::lsp_adapters::adapters_for_root(root)
        .into_iter()
        .find(|adapter| adapter.id == "protocol-test")
        .expect("configured protocol-test adapter")
}

fn diagnostic(message: &str) -> LspDiagnostic {
    LspDiagnostic {
        path: "src/main.rs".to_string(),
        range: None,
        severity: "error".to_string(),
        code: Some("secondary-error".to_string()),
        code_description: None,
        source: Some("secondary".to_string()),
        message: message.to_string(),
        tags: Vec::new(),
        related_information: Vec::new(),
        data: None,
        language: Some("secondary".to_string()),
    }
}

fn wait_for_event(
    receiver: &mut tokio::sync::broadcast::Receiver<DiagnosticsEvent>,
    file: &Path,
    predicate: impl Fn(&DiagnosticsEvent) -> bool,
) -> DiagnosticsEvent {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match receiver.try_recv() {
            Ok(event) if event.file == file.to_string_lossy() && predicate(&event) => {
                return event;
            }
            Ok(_) | Err(TryRecvError::Lagged(_)) | Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Closed) => panic!("diagnostics event bus closed"),
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for diagnostics event for {}",
            file.display()
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn wait_for_broken(service: &LspService, root: &Path, spec: &LanguageAdapter) -> String {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(reason) = service.broken_reason(root, spec) {
            return reason;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for crashed LSP health state"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn count_log_lines(log: &Path, prefix: &str) -> usize {
    fs::read_to_string(log)
        .expect("read fake LSP log")
        .lines()
        .filter(|line| line.starts_with(prefix))
        .count()
}

fn log_lines<'a>(log: &'a str, prefix: &str) -> Vec<&'a str> {
    log.lines()
        .filter(|line| line.starts_with(prefix))
        .collect()
}

fn write_lsp_config(root: &Path, server: &Path, log: &Path, mode: &str) {
    write_lsp_config_with_extension(root, server, log, mode, "rs");
}

fn write_lsp_config_with_extension(
    root: &Path,
    server: &Path,
    log: &Path,
    mode: &str,
    extension: &str,
) {
    let config = json!({
        "lsp": {
            "protocol-test": {
                "name": "Deterministic protocol test LSP",
                "language": "protocol-test",
                "command": [
                    server.to_string_lossy(),
                    mode,
                    path_to_file_uri(&root.join("src/main.rs")),
                    log.to_string_lossy(),
                    path_to_file_uri(root),
                ],
                "extensions": [extension],
                "initializationOptions": {
                    "fixtureInit": "init-only"
                },
                "settings": {
                    "protocol-test": {
                        "fixtureSetting": "settings-only"
                    }
                }
            }
        }
    });
    fs::write(
        root.join("neoism.json"),
        serde_json::to_vec_pretty(&config).expect("serialize LSP config"),
    )
    .expect("write LSP config");
}

fn compile_fake_server(workspace: &TestWorkspace) -> PathBuf {
    let source = workspace.path.join("fake_lsp.rs");
    let binary = workspace
        .path
        .join(format!("fake-lsp{}", env::consts::EXE_SUFFIX));
    fs::write(&source, FAKE_LSP_SOURCE).expect("write fake LSP source");
    let rustc = env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = Command::new(rustc)
        .arg("--edition=2021")
        .arg(&source)
        .arg("-o")
        .arg(&binary)
        .output()
        .expect("run rustc for fake LSP");
    assert!(
        output.status.success(),
        "compile fake LSP:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    binary
}

struct TestWorkspace {
    path: PathBuf,
}

impl TestWorkspace {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "neoism-lsp-e2e-{name}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create test workspace");
        Self { path }
    }
}

impl Drop for TestWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

const FAKE_LSP_SOURCE: &str = r####"
use std::{
    env,
    fs::OpenOptions,
    io::{self, BufRead, Write},
    path::Path,
};

fn main() {
    let args = env::args().collect::<Vec<_>>();
    let mode = &args[1];
    let uri = &args[2];
    let log = Path::new(&args[3]);
    let project_uri = &args[4];
    let crash_marker = log.with_extension("crashed-once");
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let stdout = io::stdout();
    let mut output = stdout.lock();
    let mut current_text = String::new();

    while let Some(body) = read_message(&mut input) {
        if has_method(&body, "initialize") {
            if mode == "fail-initialize" {
                append(log, "initialize:forced-failure");
                return;
            }
            let valid = body.contains("positionEncodings")
                && body.contains("utf-8")
                && body.contains("publishDiagnostics")
                && body.contains("relatedInformation")
                && body.contains("codeDescriptionSupport")
                && body.contains("dataSupport")
                && body.contains(r#""configuration":true"#)
                && body.contains(r#""executeCommand":{"dynamicRegistration":false}"#)
                // One occurrence is the capability and one is the initialize
                // parameter containing the actual workspace folder.
                && body.matches(r#""workspaceFolders""#).count() >= 2
                && body.contains(r#""isPreferredSupport":true"#)
                && body.contains(r#""disabledSupport":true"#)
                && body.contains(r#""properties":["edit","command"]"#)
                && body.contains("source.fixAll")
                && body.contains(r#""prepareSupport":false"#)
                && body.contains(r#""honorsChangeAnnotations":false"#)
                && body.contains(r#""didSave":true"#)
                && !body.contains(r#""workDoneProgress":true"#)
                && !body.contains(r#""didOpen":true"#)
                && !body.contains(r#""didChange":true"#)
                && !body.contains(r#""didClose":true"#)
                && body.contains(project_uri)
                && body.contains(r#""fixtureInit":"init-only""#)
                && !body.contains("settings-only");
            append(log, &format!("initialize:valid={valid}"));
            let id = request_id(&body);
            send(
                &mut output,
                &format!(
                    r#"{{"jsonrpc":"2.0","id":{id},"result":{{"capabilities":{{"hoverProvider":true,"codeActionProvider":{{"resolveProvider":true}},"executeCommandProvider":{{"commands":["fixture.afterFix"]}},"textDocumentSync":{{"openClose":true,"change":1,"save":{{"includeText":true}}}}}}}}}}"#,
                ),
            );
        } else if has_method(&body, "initialized") {
            append(log, "initialized");
            // Server-initiated requests are only legal after the initialize
            // response and the client's initialized notification. Keeping the
            // fixture in that order catches lifecycle regressions instead of
            // teaching the client to tolerate an invalid handshake.
            send_server_requests(&mut output);
        } else if has_response_id(&body, "700") {
            assert_response(
                &body,
                "700",
                r#"[{"fixtureSetting":"settings-only"},null]"#,
            );
            append(
                log,
                r#"serverRequest:workspace/configuration:id=700:result=[{"fixtureSetting":"settings-only"},null]"#,
            );
        } else if has_response_id(&body, r#""progress-701""#) {
            assert_error_response(&body, r#""progress-701""#, -32803);
            append(
                log,
                "serverRequest:window/workDoneProgress/create:id=progress-701:error=-32803",
            );
        } else if has_response_id(&body, "702") {
            assert_error_response(&body, "702", -32803);
            append(
                log,
                "serverRequest:client/registerCapability:id=702:error=-32803",
            );
        } else if has_response_id(&body, "703") {
            assert_error_response(&body, "703", -32803);
            append(
                log,
                "serverRequest:client/unregisterCapability:id=703:error=-32803",
            );
        } else if has_response_id(&body, "704") {
            assert_error_response(&body, "704", -32803);
            append(
                log,
                "serverRequest:workspace/semanticTokens/refresh:id=704:error=-32803",
            );
        } else if has_response_id(&body, "705") {
            assert_response(&body, "705", "null");
            append(
                log,
                "serverRequest:client/registerCapability:workspaceFolders:id=705:result=null",
            );
        } else if has_response_id(&body, "706") {
            assert_response(&body, "706", "null");
            append(
                log,
                "serverRequest:client/unregisterCapability:workspaceFolders:id=706:result=null",
            );
        } else if has_response_id(&body, "707") {
            let valid = body.contains(project_uri);
            assert!(valid, "workspaceFolders response omitted project: {body}");
            append(
                log,
                &format!("serverRequest:workspace/workspaceFolders:id=707:valid={valid}"),
            );
        } else if has_method(&body, "workspace/didChangeConfiguration") {
            let valid = body.contains(r#""fixtureSetting":"settings-only""#)
                && !body.contains("init-only");
            assert!(valid, "invalid didChangeConfiguration payload: {body}");
            append(log, "didChangeConfiguration:settings-only");
        } else if has_method(&body, "textDocument/didOpen") {
            current_text = classify_text(&body).to_string();
            let valid = body.contains(r#""languageId":"protocol-test""#)
                && body.contains(r#""version":0"#);
            assert!(valid, "invalid didOpen: {body}");
            append(
                log,
                &format!("didOpen:language=protocol-test:version=0:text={current_text}"),
            );
            publish_diagnostics(&mut output, uri, 0, &current_text);
            if mode == "crash-once" && !crash_marker.exists() {
                std::fs::write(&crash_marker, b"crashed").unwrap();
                append(log, "crash-after-open");
                return;
            }
        } else if has_method(&body, "textDocument/didChange") {
            current_text = classify_text(&body).to_string();
            let version = number_after(&body, r#""version":"#);
            assert!(body.contains("contentChanges"), "didChange is not full-text");
            assert!(
                !body.contains(r#""range""#),
                "Full (1) sync must use a range-less content change: {body}"
            );
            append(
                log,
                &format!("didChange:version={version}:text={current_text}"),
            );
            publish_diagnostics(&mut output, uri, version, &current_text);
            if current_text == "clean" {
                // Simulate slow analysis from the prior document revision
                // finishing after the current empty result. A correct client
                // ignores this instead of resurrecting the old diagnostic.
                publish_diagnostics(
                    &mut output,
                    uri,
                    version.saturating_sub(1),
                    "stale",
                );
            }
        } else if has_method(&body, "textDocument/didSave") {
            let includes_text = body.contains(&format!(
                r#""text":"{}""#,
                json_escape(&current_text)
            ));
            append(
                log,
                &format!("didSave:text={current_text}:includesText={includes_text}"),
            );
        } else if has_method(&body, "textDocument/hover") {
            let id = request_id(&body);
            let line = number_after(&body, r#""line":"#);
            let character = number_after(&body, r#""character":"#);
            append(log, &format!("hover:{line}:{character}:text={current_text}"));
            let value = json_escape(&format!(
                "fake hover for {current_text} at {line}:{character}"
            ));
            if mode == "request-error" {
                send(
                    &mut output,
                    &format!(
                        r#"{{"jsonrpc":"2.0","id":{id},"error":{{"code":-32803,"message":"fixture request failed"}}}}"#,
                    ),
                );
            } else if mode == "empty-hover" {
                send(
                    &mut output,
                    &format!(r#"{{"jsonrpc":"2.0","id":{id},"result":null}}"#),
                );
            } else {
                send(
                    &mut output,
                    &format!(
                        r#"{{"jsonrpc":"2.0","id":{id},"result":{{"contents":{{"kind":"markdown","value":"{value}"}}}}}}"#,
                    ),
                );
            }
        } else if has_method(&body, "textDocument/codeAction") {
            let id = request_id(&body);
            let valid = body.contains(r#""severity":1"#)
                && body.contains(r#""code":"fake-error""#)
                && body.contains(r#""source":"fake-lsp""#)
                && body.contains(r#""message":"diagnostic broken_v1""#)
                && body.contains(r#""start":{"character":2,"line":7}"#)
                && !body.contains(r#""severity":"error""#)
                && !body.contains(r#""path":"#)
                && !body.contains(r#""language":"protocol-test""#);
            assert!(valid, "codeAction context was not valid LSP wire data: {body}");
            append(log, &format!("codeAction:valid={valid}"));
            send(
                &mut output,
                &format!(
                    r#"{{"jsonrpc":"2.0","id":{id},"result":[{{"title":"Fix fake error","kind":"quickfix","isPreferred":true,"data":{{"fix":"fake"}}}},{{"title":"Unavailable fake fix","kind":"quickfix","disabled":{{"reason":"fixture is indexing"}}}}]}}"#
                ),
            );
        } else if has_method(&body, "codeAction/resolve") {
            let id = request_id(&body);
            let valid = body.contains(r#""title":"Fix fake error""#)
                && body.contains(r#""data":{"fix":"fake"}"#);
            assert!(valid, "codeAction/resolve payload was not preserved: {body}");
            append(log, &format!("codeActionResolve:valid={valid}"));
            send(
                &mut output,
                &format!(
                    r#"{{"jsonrpc":"2.0","id":{id},"result":{{"title":"Fix fake error","kind":"quickfix","isPreferred":true,"edit":{{"changes":{{"{}":[{{"range":{{"start":{{"line":7,"character":2}},"end":{{"line":7,"character":8}}}},"newText":"fixed"}}]}}}},"command":{{"title":"Finish fake fix","command":"fixture.afterFix","arguments":[{{"uri":"{}"}}]}}}}}}"#,
                    json_escape(uri),
                    json_escape(uri),
                ),
            );
        } else if has_method(&body, "workspace/executeCommand") {
            let id = request_id(&body);
            let valid = body.contains(r#""command":"fixture.afterFix""#)
                && body.contains(uri)
                && !body.contains(r#""title":"Finish fake fix""#);
            assert!(valid, "executeCommand params were not normalized: {body}");
            append(log, &format!("executeCommand:valid={valid}"));
            send(
                &mut output,
                &format!(
                    r#"{{"jsonrpc":"2.0","id":{id},"result":{{"executed":true}}}}"#
                ),
            );
        } else if has_method(&body, "shutdown") {
            let id = request_id(&body);
            send(
                &mut output,
                &format!(r#"{{"jsonrpc":"2.0","id":{id},"result":null}}"#),
            );
        } else if has_method(&body, "exit") {
            return;
        }
    }
}

fn read_message(reader: &mut impl BufRead) -> Option<String> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let mut body = vec![0; length?];
    reader.read_exact(&mut body).ok()?;
    String::from_utf8(body).ok()
}

fn send(writer: &mut impl Write, body: &str) {
    write!(writer, "Content-Length: {}\r\n\r\n{}", body.len(), body).unwrap();
    writer.flush().unwrap();
}

fn publish_diagnostics(
    writer: &mut impl Write,
    uri: &str,
    version: u64,
    text: &str,
) {
    let diagnostics = if text == "clean" {
        "[]".to_string()
    } else {
        format!(
            r#"[{{"range":{{"start":{{"line":7,"character":2}},"end":{{"line":7,"character":8}}}},"severity":1,"code":"fake-error","source":"fake-lsp","message":"diagnostic {}"}}]"#,
            json_escape(text),
        )
    };
    send(
        writer,
        &format!(
            r#"{{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{{"uri":"{}","version":{version},"diagnostics":{diagnostics}}}}}"#,
            json_escape(uri),
        ),
    );
}

fn send_server_requests(writer: &mut impl Write) {
    send(
        writer,
        r#"{"jsonrpc":"2.0","id":700,"method":"workspace/configuration","params":{"items":[{"section":"protocol-test"},{"section":"missing"}]}}"#,
    );
    send(
        writer,
        r#"{"jsonrpc":"2.0","id":"progress-701","method":"window/workDoneProgress/create","params":{"token":"indexing"}}"#,
    );
    send(
        writer,
        r#"{"jsonrpc":"2.0","id":702,"method":"client/registerCapability","params":{"registrations":[{"id":"fake-hover","method":"textDocument/hover","registerOptions":{}}]}}"#,
    );
    send(
        writer,
        r#"{"jsonrpc":"2.0","id":703,"method":"client/unregisterCapability","params":{"unregisterations":[{"id":"fake-hover","method":"textDocument/hover"}]}}"#,
    );
    send(
        writer,
        r#"{"jsonrpc":"2.0","id":704,"method":"workspace/semanticTokens/refresh"}"#,
    );
    send(
        writer,
        r#"{"jsonrpc":"2.0","id":705,"method":"client/registerCapability","params":{"registrations":[{"id":"workspace-folders","method":"workspace/didChangeWorkspaceFolders","registerOptions":{}}]}}"#,
    );
    send(
        writer,
        r#"{"jsonrpc":"2.0","id":706,"method":"client/unregisterCapability","params":{"unregisterations":[{"id":"workspace-folders","method":"workspace/didChangeWorkspaceFolders"}]}}"#,
    );
    send(
        writer,
        r#"{"jsonrpc":"2.0","id":707,"method":"workspace/workspaceFolders"}"#,
    );
}

fn assert_response(body: &str, id: &str, result: &str) {
    assert!(body.contains(r#""jsonrpc":"2.0""#), "missing jsonrpc: {body}");
    assert!(
        body.contains(&format!(r#""id":{id}"#)),
        "response id was not correlated: {body}"
    );
    assert!(
        body.contains(&format!(r#""result":{result}"#)),
        "unexpected response result: {body}"
    );
    assert!(!body.contains(r#""error""#), "unexpected response error: {body}");
}

fn assert_error_response(body: &str, id: &str, code: i64) {
    assert!(body.contains(r#""jsonrpc":"2.0""#), "missing jsonrpc: {body}");
    assert!(
        body.contains(&format!(r#""id":{id}"#)),
        "response id was not correlated: {body}"
    );
    assert!(
        body.contains(&format!(r#""code":{code}"#)),
        "unexpected response error: {body}"
    );
    assert!(body.contains(r#""error""#), "missing response error: {body}");
    assert!(!body.contains(r#""result""#), "error returned a result: {body}");
}

fn has_method(body: &str, method: &str) -> bool {
    body.contains(&format!(r#""method":"{method}""#))
}

fn has_response_id(body: &str, id: &str) -> bool {
    !body.contains(r#""method":"#) && body.contains(&format!(r#""id":{id}"#))
}

fn classify_text(body: &str) -> &'static str {
    if body.contains("broken_v1") {
        "broken_v1"
    } else if body.contains("broken_v2") {
        "broken_v2"
    } else if body.contains("clean") {
        "clean"
    } else {
        "unknown"
    }
}

fn request_id(body: &str) -> u64 {
    number_after(body, r#""id":"#)
}

fn number_after(body: &str, marker: &str) -> u64 {
    let tail = body.split_once(marker).unwrap().1;
    let digits = tail.chars().take_while(char::is_ascii_digit).collect::<String>();
    digits.parse().unwrap()
}

fn append(path: &Path, line: &str) {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    writeln!(file, "{line}").unwrap();
    file.flush().unwrap();
}

fn json_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}
"####;
