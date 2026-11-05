#![cfg(unix)]
use neoism_protocol::pty::{ClientMessage, ServerMessage, TerminalCompletion};
use neoism_workspace_daemon::sessions::{SessionOutputSubscription, SessionRegistry};
use std::time::{Duration, Instant};

struct Cleanup(SessionRegistry, String);
impl Drop for Cleanup { fn drop(&mut self) { self.0.handle(ClientMessage::ClosePty { session_id: self.1.clone() }); } }

async fn prompt(rx: &mut SessionOutputSubscription) -> String {
    let mut output = String::new();
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if let Ok(ServerMessage::PtyOutput { bytes, .. }) = rx.recv().await {
                output.push_str(&String::from_utf8_lossy(&bytes));
                // The private ready mark comes after the snapshot, unlike OSC B.
                if output.contains(";B\x07") && output.contains("neoism-control;") { break; }
            }
        }
    }).await.unwrap_or_else(|_| panic!("No prompt: {output:?}"));
    // PS1 may be delivered in a separate PTY read after our private ready mark.
    while let Ok(Ok(message)) = tokio::time::timeout(Duration::from_millis(40), rx.recv()).await {
        if let ServerMessage::PtyOutput { bytes, .. } = message { output.push_str(&String::from_utf8_lossy(&bytes)); }
    }
    output
}
fn request(id: &str, line: &str) -> ClientMessage {
    ClientMessage::CompleteTerminal { session_id: id.into(), line: line.into(), cursor: line.len() as u32 }
}
async fn entries(registry: &SessionRegistry, rx: &SessionOutputSubscription, id: &str, line: &str) -> Vec<TerminalCompletion> {
    match registry.handle_subscribed_async(request(id,line), Some(rx)).await.remove(0) {
        ServerMessage::TerminalCompletions { entries, line: echo, cursor, .. } => {
            assert_eq!(echo,line); assert_eq!(cursor, line.len() as u32); entries
        }
        other => panic!("completion {line:?}: {other:?}"),
    }
}
fn send(registry: &SessionRegistry, id: &str, command: &str) {
    registry.handle(ClientMessage::PtyInput { session_id:id.into(), bytes:format!("{command}\n").into_bytes() });
}

#[tokio::test(flavor="multi_thread", worker_threads=2)]
async fn real_bash_live_completion_is_rich_and_never_submits_queries() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("two words ' $ ; file"), "").unwrap();
    std::fs::write(root.path().join("literal trailing "), "").unwrap();
    std::fs::write(root.path().join("finished file"), "").unwrap();
    std::fs::create_dir(root.path().join("two directories")).unwrap();
    std::fs::write(root.path().join("package.json"), r#"{"scripts":{"dev:local":"vite --host","test:unit":"vitest"}}"#).unwrap();
    std::fs::write(root.path().join("tracked-change"), "changed").unwrap();
    let git = |args: &[&str]| { assert!(std::process::Command::new("git").args(args).current_dir(root.path()).status().unwrap().success()); };
    git(&["init","-q"]); git(&["symbolic-ref","HEAD","refs/heads/main"]);
    git(&["-c","user.name=Test","-c","user.email=test@example.test","commit","--allow-empty","-qm","init"]);
    git(&["branch","feature/completion"]); git(&["remote","add","completion-origin","https://example.test/repo"]);
    git(&["config","alias.co","checkout"]);
    git(&["config","alias.unsafe","!touch GIT_ALIAS_INJECTED"]);
    let (registry, mut rx) = SessionRegistry::new();
    let created = registry.handle_subscribed(ClientMessage::CreatePty { cwd:Some(root.path().to_str().unwrap().into()), cols:120,rows:24,shell:Some("/bin/bash".into()),shell_args:None,presentation:None },Some(&rx));
    let id = match &created[0] { ServerMessage::PtyCreated {session_id,..} => session_id.clone(), other => panic!("{other:?}") };
    let _cleanup = Cleanup(registry.clone(),id.clone());
    prompt(&mut rx).await;
    send(&registry,&id,"alias gc='git checkout'; alias gcc='gc'; alias neoevil='git checkout; touch ALIAS_INJECTED'; alias neosub='git checkout $(touch ALIAS_SUBSTITUTION)'; choices=('from ordinary array'); declare -A neo_global_map=([first]='from ordinary map'); neo_global_scalar='from scalar'; cur=('ordinary cur array'); cmd='ordinary cmd scalar'; _neo_globals_complete() { COMPREPLY=(\"${choices[@]}\" \"${neo_global_map[first]}\" \"$neo_global_scalar\" \"${cur[0]}\" \"$cmd\"); }; complete -F _neo_globals_complete neoglobals; complete -f neofiles; complete -W '--output= --verbose' neooptions");
    prompt(&mut rx).await;
    send(&registry,&id,"stty -echo; alias neoalias='echo alias'; neofunction() { :; }; export NEO_COMPLETION_TEST=live; _neo_test_complete() { COMPREPLY=( '--native-flag' 'native value' ); }; complete -F _neo_test_complete neotool; complete -W 'start stop --verbose' neowords; _neo_slow_complete() { printf '%s\\n' \"$$\" > worker-pid; sleep 30; }; complete -F _neo_slow_complete neoslow");
    prompt(&mut rx).await;
    send(&registry,&id,"builtin declare -F neofunction; /bin/bash --noprofile --norc -c 'shopt -s extglob; source \"$NEOISM_COMPLETION_DIR/state\"; builtin declare -F neofunction; printf \"SNAPSHOT_SOURCE_STATUS=%s\\n\" \"$?\"'");
    let snapshot_check = prompt(&mut rx).await;
    assert!(snapshot_check.contains("SNAPSHOT_SOURCE_STATUS=0"), "{snapshot_check:?}");
    for (line,label,kind) in [("neoal","neoalias","alias"),("neofun","neofunction","function"),("bas","bash","command"),("echo $NEO_COMPLETION_T","$NEO_COMPLETION_TEST","variable"),("neotool --n","--native-flag","option"),("neowords --v","--verbose","option"),("npm run dev","dev:local","script"),("git switch feature/","feature/completion","branch"),("git fetch completion-","completion-origin","remote"),("git add tracked","tracked-change","file")] {
        let found=entries(&registry,&rx,&id,line).await;
        assert!(found.iter().any(|e| e.label==label && e.kind==kind),"{line}: {found:?}");
    }
    let files=entries(&registry,&rx,&id,"cat two").await;
    let file=files.iter().find(|e|e.label=="two words ' $ ; file").unwrap();
    assert_eq!(file.value,"cat 'two words '\\'' $ ; file' ");
    assert!(files.iter().any(|e|e.kind=="directory" && e.label=="two directories/"));
    let middle="cat two --preserved";
    let reply=registry.handle_subscribed_async(ClientMessage::CompleteTerminal{session_id:id.clone(),line:middle.into(),cursor:7},Some(&rx)).await;
    assert!(matches!(&reply[0],ServerMessage::TerminalCompletions{entries,..} if entries.iter().any(|e|e.value.ends_with(" --preserved"))));
    let command = entries(&registry,&rx,&id,"gi").await.into_iter().find(|e|e.label=="git").unwrap();
    assert_eq!(format!("{}status", command.value), "git status");
    for (line, expected) in [("gc feature/","gc feature/completion "),("gcc feature/","gcc feature/completion "),("git co feature/","git co feature/completion ")] {
        let branch = entries(&registry,&rx,&id,line).await.into_iter().find(|e|e.kind=="branch" && e.label=="feature/completion").unwrap();
        assert_eq!(branch.value,expected);
    }
    let globals=entries(&registry,&rx,&id,"neoglobals ").await;
    for expected in ["from ordinary array","from ordinary map","from scalar","ordinary cur array","ordinary cmd scalar"] {
        assert!(globals.iter().any(|e|e.label==expected),"{globals:?}");
    }
    assert_eq!(globals[0].detail,"Installed Bash completion function","native arguments must rank before fallback files");
    let native_files=entries(&registry,&rx,&id,"neofiles two").await;
    assert!(native_files.iter().any(|e|e.kind=="file" && e.detail=="Registered Bash filename completion"));
    assert_eq!(entries(&registry,&rx,&id,"neooptions --ou").await[0].value,"neooptions --output=");
    send(&registry,&id,"_neo_delimited_complete() { COMPREPLY=( '--version ' 'feature/completion ' 'literal trailing ' 'finished file ' 'two directories/ ' ); }; complete -F _neo_delimited_complete -o nospace neodelimited");
    prompt(&mut rx).await;
    let delimited=entries(&registry,&rx,&id,"neodelimited ").await;
    for (label,value) in [("--version","neodelimited --version "),("feature/completion","neodelimited feature/completion "),("literal trailing ","neodelimited 'literal trailing ' "),("finished file","neodelimited 'finished file' "),("two directories/","neodelimited 'two directories/'")] {
        assert!(delimited.iter().any(|e|e.label==label&&e.value==value),"{label}: {delimited:?}");
    }
    for line in ["neoevil feature/","neosub feature/","git unsafe feature/"] { entries(&registry,&rx,&id,line).await; }
    for name in ["ALIAS_INJECTED","ALIAS_SUBSTITUTION","GIT_ALIAS_INJECTED"] { assert!(!root.path().join(name).exists()); }
    // Native callbacks can invoke runtimes such as V8; a virtual-address cap
    // would crash Node before it produces a completion despite low actual RSS.
    let node = std::env::var_os("PATH").and_then(|p|std::env::split_paths(&p).map(|d|d.join("node")).find(|p|p.is_file()));
    if node.is_some() {
        send(&registry,&id,"_neo_node_complete() { COMPREPLY=(\"$(command node -e 'process.stdout.write(\"node-native-runtime\")')\"); }; complete -F _neo_node_complete neonode");
        prompt(&mut rx).await;
        assert!(entries(&registry,&rx,&id,"neonode node-").await.iter().any(|e|e.label=="node-native-runtime" && e.detail=="Installed Bash completion function"));
    } else { eprintln!("Node not installed; Node-backed native completion test explicitly skipped"); }
    for _ in 0..3 {
        entries(&registry,&rx,&id,"touch WOULD_SUBMIT").await;
        assert!(matches!(registry.handle_subscribed_async(request(&id,"echo $(touch INJECTED)"),Some(&rx)).await[0],ServerMessage::Error{..}));
    }
    assert!(!root.path().join("WOULD_SUBMIT").exists()); assert!(!root.path().join("INJECTED").exists());
    // No terminal output, prompt or input-buffer mutation during completions.
    assert!(tokio::time::timeout(Duration::from_millis(150),rx.recv()).await.is_err());
    let peer=registry.subscribe();
    assert!(matches!(registry.handle_subscribed_async(request(&id,"git "),Some(&peer)).await[0],ServerMessage::Error{..}));
    let now=Instant::now();
    assert!(matches!(registry.handle_subscribed_async(request(&id,"neoslow "),Some(&rx)).await[0],ServerMessage::Error{..}));
    assert!(now.elapsed()<Duration::from_secs(3));
    let worker_pid: i32 = std::fs::read_to_string(root.path().join("worker-pid")).unwrap().trim().parse().unwrap();
    assert_eq!(unsafe { libc::kill(worker_pid, 0) }, -1, "timed-out worker must be killed and reaped");
    assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    // More than 256 KiB of matching command names must yield bounded entries,
    // not overflow the output pipe and force the entire query to fail.
    let bin=root.path().join("huge-bin"); std::fs::create_dir(&bin).unwrap();
    use std::os::unix::fs::PermissionsExt;
    for index in 0..4500 {
        let path=bin.join(format!("unrelated_{index:05}_{}", "x".repeat(64)));
        std::fs::write(&path,"").unwrap(); std::fs::set_permissions(path,std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let needle=bin.join("specific_late_completion"); std::fs::write(&needle,"").unwrap(); std::fs::set_permissions(needle,std::fs::Permissions::from_mode(0o700)).unwrap();
    send(&registry,&id,&format!("PATH='{}':\"$PATH\"",bin.display())); prompt(&mut rx).await;
    for query in ["", "unrelated_"] {
        let result=entries(&registry,&rx,&id,query).await;
        assert!(!result.is_empty() && result.len()<=200);
    }
    assert!(entries(&registry,&rx,&id,"specific_late_").await.iter().any(|e|e.label=="specific_late_completion"));
    let pending = "printf 'INPUT_PRESERVED=%s\\n' keep";
    registry.handle(ClientMessage::PtyInput { session_id:id.clone(), bytes:pending.as_bytes().to_vec() });
    entries(&registry,&rx,&id,pending).await;
    send(&registry,&id,"buffer");
    assert!(prompt(&mut rx).await.contains("INPUT_PRESERVED=keepbuffer"));
    send(&registry,&id,"printf 'UNCHANGED_PWD=%s\\n' \"$PWD\"; IFS= read -r untouched");
    tokio::time::sleep(Duration::from_millis(100)).await;
    let now=Instant::now();
    assert!(matches!(registry.handle_subscribed_async(request(&id,"git "),Some(&rx)).await[0],ServerMessage::Error{..}));
    assert!(now.elapsed()<Duration::from_millis(100));
    send(&registry,&id,"untouched");
    let output=prompt(&mut rx).await;
    assert!(output.contains(root.path().to_str().unwrap()),"{output:?}");
    registry.handle(ClientMessage::ClosePty{session_id:id.clone()});
    assert!(matches!(registry.handle_subscribed_async(request(&id,"git "),Some(&rx)).await[0],ServerMessage::Error{..}));
}

#[tokio::test(flavor="multi_thread", worker_threads=2)]
async fn real_zsh_explicit_host_fallback_when_installed() {
    let shell = std::env::var_os("PATH").and_then(|path| std::env::split_paths(&path).map(|p| p.join("zsh")).find(|p| p.is_file()));
    let Some(shell) = shell else { eprintln!("zsh not installed; native ZLE completion remains unsupported"); return; };
    let root=tempfile::tempdir().unwrap(); std::fs::write(root.path().join("zsh file"),"").unwrap();
    let (registry,mut rx)=SessionRegistry::new();
    let created=registry.handle_subscribed(ClientMessage::CreatePty{cwd:Some(root.path().to_str().unwrap().into()),cols:80,rows:24,shell:Some(shell.to_string_lossy().into_owned()),shell_args:None,presentation:None},Some(&rx));
    let id=match &created[0] {ServerMessage::PtyCreated{session_id,..}=>session_id.clone(), other=>panic!("{other:?}")};
    let _cleanup=Cleanup(registry.clone(),id.clone()); prompt(&mut rx).await;
    assert!(entries(&registry,&rx,&id,"cat zsh").await.iter().any(|e|e.label=="zsh file"));
    send(&registry,&id,"alias neo_zalias='echo alias'; neo_zfunction() { :; }; export NEO_ZVAR=secretvalue"); prompt(&mut rx).await;
    for (line,label,kind) in [("neo_za","neo_zalias","alias"),("neo_zf","neo_zfunction","function"),("echo $NEO_ZV","$NEO_ZVAR","variable")] {
        let found=entries(&registry,&rx,&id,line).await;
        assert!(found.iter().any(|e|e.label==label && e.kind==kind),"{found:?}");
        assert!(found.iter().all(|e|!e.detail.contains("secretvalue")));
    }
}