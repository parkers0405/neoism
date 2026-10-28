//! Real grid shells keep their own prompt and the authenticated directory API.
#![cfg(target_os = "linux")]
use neoism_protocol::pty::{ClientMessage, Presentation, ServerMessage};
use neoism_workspace_daemon::sessions::{SessionOutputSubscription, SessionRegistry};
use std::time::Duration;
struct Cleanup(SessionRegistry,String);
impl Drop for Cleanup {fn drop(&mut self){self.0.handle(ClientMessage::ClosePty{session_id:self.1.clone()});}}

#[test]
fn configured_grid_profiles_keep_visible_prompt_and_safe_directory_control() {
    for name in ["bash", "zsh"] {
        let shell=std::env::var_os("PATH").and_then(|path|std::env::split_paths(&path).map(|directory|directory.join(name)).find(|path|path.is_file()));
        let Some(shell)=shell else {continue};
        let home=tempfile::tempdir().unwrap();
        let profiles=home.path().join("custom-zdotdir");
        std::fs::create_dir(&profiles).unwrap();
        std::fs::write(home.path().join(".bashrc"),"PS1='GRID_PROFILE>>> '\nprintf 'PROFILE_%s\\n' LOADED\n").unwrap();
        std::fs::write(profiles.join(".zshenv"),"export GRID_ZSHENV=loaded\n").unwrap();
        std::fs::write(profiles.join(".zshrc"),"PROMPT='GRID_PROFILE>>> '\nRPROMPT='right-profile'\nprintf 'PROFILE_%s:%s\\n' LOADED $GRID_ZSHENV\n").unwrap();
        let output=std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact","grid_shell_child","--ignored","--nocapture"])
            .env("HOME",home.path()).env("ZDOTDIR",&profiles).env("GRID_TEST_SHELL",&shell)
            .output().unwrap();
        assert!(output.status.success(),"{}: {}\n{}",shell.display(),String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
    }
}

async fn until(rx:&mut SessionOutputSubscription,needle:&str)->String {
    let mut output=String::new();
    tokio::time::timeout(Duration::from_secs(8),async {
        while let Ok(event)=rx.recv().await {
            if let ServerMessage::PtyOutput{bytes,..}=event {
                output.push_str(&String::from_utf8_lossy(&bytes));
                if output.contains(needle) {return;}
            }
        }
    }).await.unwrap_or_else(|_|panic!("missing {needle:?}: {output:?}"));
    output
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
#[ignore="only run in a subprocess with isolated shell profiles"]
async fn grid_shell_child() {
    let root=std::env::var("HOME").unwrap();
    let shell=std::env::var("GRID_TEST_SHELL").unwrap();
    let target=std::path::Path::new(&root).join("dir 'quoted' ; $(touch INJECTED)");
    std::fs::create_dir(&target).unwrap();
    let (registry,mut rx)=SessionRegistry::new();
    let created=registry.handle_subscribed(ClientMessage::CreatePty{cwd:Some(root.clone()),cols:120,rows:24,shell:Some(shell.clone()),presentation:Some(Presentation::Grid)},Some(&rx));
    let id=match &created[..] {
        [ServerMessage::PtyCreated{session_id,presentation:Some(Presentation::Grid),..}]=>session_id.clone(),
        other=>panic!("{other:?}"),
    };
    let _cleanup=Cleanup(registry.clone(),id.clone());
    let startup=until(&mut rx,"GRID_PROFILE>>> ").await;
    assert!(startup.contains("PROFILE_LOADED"),"{startup:?}");
    assert!(startup.contains("\x1b]7;file://"),"{startup:?}");
    assert!(startup.contains("\x1b]777;neoism-control;"),"{startup:?}");
    if shell.ends_with("zsh") {assert!(startup.contains("PROFILE_LOADED:loaded"),"{startup:?}");}
    // Do not inject cd while a foreground built-in read owns the terminal.
    registry.handle(ClientMessage::PtyInput{session_id:id.clone(),bytes:b"stty -echo; printf 'BUSY_%s\\n' READY; if read -t 0.5 answer; then printf 'BAD_INPUT=%s\\n' \"$answer\"; else printf 'NO_%s\\n' INPUT; fi\r".to_vec()});
    until(&mut rx,"BUSY_READY").await;
    let replies=registry.handle_subscribed(ClientMessage::ChangeTerminalDirectory{session_id:id.clone(),directory:target.to_str().unwrap().into(),request_id:"grid-cd".into()},Some(&rx));
    assert!(replies.is_empty(),"{replies:?}");
    let mut bytes=String::new();
    tokio::time::timeout(Duration::from_secs(8),async {
        loop { match rx.recv().await.unwrap() {
            ServerMessage::PtyOutput{bytes:chunk,..}=>bytes.push_str(&String::from_utf8_lossy(&chunk)),
            ServerMessage::TerminalDirectoryChanged{request_id,directory,..} if request_id=="grid-cd"=>{assert_eq!(directory,target.to_str().unwrap());break;},
            ServerMessage::TerminalDirectoryChangeFailed{message,..}=>panic!("{message}"),_=>{}
        }}
    }).await.expect("safe directory confirmation");
    assert!(bytes.contains("NO_INPUT"),"{bytes:?}");
    assert!(!bytes.contains("BAD_INPUT="),"{bytes:?}");
    assert!(!std::path::Path::new(&root).join("INJECTED").exists());
    registry.handle(ClientMessage::PtyInput{session_id:id.clone(),bytes:b"printf 'PWD_%s:' CHECK; pwd; printf 'DONE_%s\\n' CHECK\r".to_vec()});
    let output=until(&mut rx,"DONE_CHECK").await;
    assert!(output.contains(target.to_str().unwrap()),"{output:?}");
    let replay=registry.handle_subscribed(ClientMessage::AttachPty{session_id:id.clone(),cursor:None},Some(&rx));
    assert!(matches!(replay.first(),Some(ServerMessage::PtyCreated{session_id,presentation:Some(Presentation::Grid),..}) if session_id==&id));
    registry.handle(ClientMessage::ClosePty{session_id:id});
}
