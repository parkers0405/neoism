use neochat::{board::Board, client::Client, protocol::*};
use std::{path::Path, process::{Child, Command, Stdio}, time::{Duration, Instant}};

const TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const ADMIN: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn post(board: &Board, channel: &str) -> u64 {
    board.action(TOKEN,None,None,Action::Post { channel:channel.into(),text:"attachment target".into(),labels:Default::default(),task:false,needs:vec![],assignee:None }).unwrap().id.unwrap()
}

#[test]
fn sqlite_authorization_quotas_and_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("board.sqlite");
    let board = Board::open(&path,TOKEN,ADMIN).unwrap();
    let id = post(&board,"general");
    let secret = post(&board,"private");
    let invite = board.create_invite(ADMIN,InviteRequest { actor:"reader".into(),channels:vec!["general".into()],expires_at:None }).unwrap();
    let bytes = vec![0,255,1,128];
    let a = board.attach(TOKEN,id,"data.bin",bytes.clone()).unwrap();
    assert_eq!(a.size,4);
    assert_eq!(board.download_attachment(&invite.token,a.id).unwrap(),bytes);
    let hidden = board.attach(TOKEN,secret,"secret",vec![7]).unwrap();
    assert_eq!(board.attachments(&invite.token,secret).unwrap_err().0,404);
    assert_eq!(board.attach(&invite.token,secret,"x",vec![]).unwrap_err().0,404);
    assert_eq!(board.download_attachment(&invite.token,hidden.id).unwrap_err().0,404);
    assert_eq!(board.download_attachment(&invite.token,999999).unwrap_err().0,404);
    assert_eq!(board.attach(TOKEN,999999,"x",vec![]).unwrap_err().0,404);
    assert_eq!(board.attach("invalid",id,"x",vec![]).unwrap_err().0,401);
    assert_eq!(board.attachments(ADMIN,id).unwrap_err().0,401);
    assert_eq!(board.download_attachment("invalid",a.id).unwrap_err().0,401);
    for name in ["", "..", "../escape", "a/b", "a\\b", "C:evil", "a\0b", "a\nb", " padded "] {
        assert_eq!(board.attach(TOKEN,id,name,vec![]).unwrap_err().0,400,"{name:?}");
    }
    assert!(board.attach(TOKEN,id,&"é".repeat(128),vec![]).is_err());
    assert_eq!(board.attach(TOKEN,id,"large",vec![0;MAX_ATTACHMENT_BYTES+1]).unwrap_err().0,413);
    for _ in 1..MAX_MESSAGE_ATTACHMENTS { board.attach(TOKEN,id,"empty",vec![]).unwrap(); }
    assert_eq!(board.attach(TOKEN,id,"overflow",vec![]).unwrap_err().0,413);
    let quota = post(&board,"general");
    for _ in 0..4 { board.attach(TOKEN,quota,"max",vec![42;MAX_ATTACHMENT_BYTES]).unwrap(); }
    assert_eq!(board.attach(TOKEN,quota,"overflow",vec![1]).unwrap_err().0,413);
    assert_eq!(board.attachments(TOKEN,quota).unwrap().len(),4);
    let expired = board.create_invite(ADMIN,InviteRequest { actor:"expired".into(),channels:vec![],expires_at:Some(neochat::board::now()+3600) }).unwrap();
    // Deterministic expiration without a wall-clock sleep.
    rusqlite::Connection::open(&path).unwrap().execute("UPDATE credentials SET expires=0 WHERE actor='expired'",[]).unwrap();
    assert_eq!(board.attach(&expired.token,id,"x",vec![]).unwrap_err().0,401);
    assert_eq!(board.attachments(&expired.token,id).unwrap_err().0,401);
    assert_eq!(board.download_attachment(&expired.token,a.id).unwrap_err().0,401);
    board.revoke_invite(ADMIN,invite.id).unwrap();
    assert_eq!(board.download_attachment(&invite.token,a.id).unwrap_err().0,401);
    drop(board);
    let reopened = Board::open(&path,TOKEN,ADMIN).unwrap();
    assert_eq!(reopened.attachments(TOKEN,id).unwrap()[0],a);
    assert_eq!(reopened.download_attachment(TOKEN,a.id).unwrap(),bytes);
    assert_eq!(reopened.attachments(TOKEN,quota).unwrap().len(),4);
}

struct Daemon(Child);
impl Drop for Daemon { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }
fn start(dir: &Path) -> (Daemon,Discovery) {
    let daemon = Daemon(Command::new(env!("CARGO_BIN_EXE_neochatd")).arg("--data-dir").arg(dir).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
    let deadline = Instant::now()+Duration::from_secs(10);
    loop {
        if let Ok(bytes) = std::fs::read(dir.join("endpoint.json")) {
            if let Ok(record) = serde_json::from_slice(&bytes) { return (daemon,record); }
        }
        assert!(Instant::now()<deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn http_client_cli_and_process_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (daemon, endpoint) = start(dir.path());
    let client = Client::new(&endpoint.url,&endpoint.token,None).unwrap();
    let id = client.action(&Action::Post { channel:"general".into(),text:"files".into(),labels:Default::default(),task:false,needs:vec![],assignee:None }).unwrap().id.unwrap();
    let cursor = client.snapshot().unwrap().cursor;
    let payload = b"<script>alert(1)</script>\0\xff".to_vec();
    let a = client.attach(id,"unsafe.html",payload.clone()).unwrap();
    assert_eq!(client.attachments(id).unwrap(),vec![a.clone()]);
    assert_eq!(client.download_attachment(a.id).unwrap(),payload);
    assert!(client.events(cursor,0).unwrap().iter().any(|e| matches!(&e.kind,EventKind::Message { message } if message.id==id)));
    let http = reqwest::blocking::Client::new();
    let url = format!("{}/v1/attachments/{}",endpoint.url,a.id);
    assert_eq!(http.get(&url).send().unwrap().status(),401);
    let response = http.get(&url).bearer_auth(&endpoint.token).send().unwrap();
    assert_eq!(response.headers()["content-type"],"application/octet-stream");
    assert_eq!(response.headers()["content-disposition"],"attachment");
    assert_eq!(response.headers()["x-content-type-options"],"nosniff");
    assert_eq!(response.headers()["cache-control"],"no-store");
    let upload = format!("{}/v1/messages/{id}/attachments",endpoint.url);
    assert_eq!(http.get(&upload).send().unwrap().status(),401);
    assert_eq!(http.post(&upload).query(&[("name","x")]).body(vec![0]).send().unwrap().status(),401);
    assert_eq!(http.post(&upload).bearer_auth(&endpoint.token).query(&[("name","huge")]).body(vec![0;MAX_ATTACHMENT_BYTES+1]).send().unwrap().status(),413);
    // A full-size raw upload is permitted (the normal action limit is only 128 KiB).
    let max = client.attach(id,"max",vec![5;MAX_ATTACHMENT_BYTES]).unwrap();
    assert_eq!(client.download_attachment(max.id).unwrap().len(),MAX_ATTACHMENT_BYTES);
    let admin = Client::new(&endpoint.url,&std::fs::read_to_string(dir.path().join("admin-token")).unwrap(),None).unwrap();
    let restricted = admin.invite(&InviteRequest { actor:"other".into(),channels:vec!["elsewhere".into()],expires_at:None }).unwrap();
    let other = Client::new(&endpoint.url,&restricted.token,None).unwrap();
    assert!(other.attach(id,"x",vec![]).unwrap_err().to_string().starts_with("404"));
    assert!(other.attachments(id).unwrap_err().to_string().starts_with("404"));
    assert!(other.download_attachment(a.id).unwrap_err().to_string().starts_with("404"));
    let run = |args: &[&str]| Command::new(env!("CARGO_BIN_EXE_neochat")).args(["--url",&endpoint.url,"--token",&endpoint.token]).args(args).output().unwrap();
    let source = dir.path().join("source.bin");
    std::fs::write(&source,&payload).unwrap();
    let output = run(&["--json","attach",&id.to_string(),source.to_str().unwrap(),"--name","cli.bin"]);
    assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
    let cli_attachment: Attachment = serde_json::from_slice(&output.stdout).unwrap();
    let list = run(&["--json","attachments",&id.to_string()]);
    assert!(serde_json::from_slice::<Vec<Attachment>>(&list.stdout).unwrap().contains(&cli_attachment));
    let destination = dir.path().join("download.bin");
    assert!(run(&["download-attachment",&cli_attachment.id.to_string(),destination.to_str().unwrap()]).status.success());
    assert_eq!(std::fs::read(&destination).unwrap(),payload);
    assert!(!run(&["download-attachment",&a.id.to_string(),destination.to_str().unwrap()]).status.success());
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&destination).unwrap().permissions().mode() & 0o777,0o600);
    }
    drop(daemon);
    // Abrupt shutdown leaves a stale discovery record; remove it before waiting for the new one.
    std::fs::remove_file(dir.path().join("endpoint.json")).unwrap();
    let (_daemon,next) = start(dir.path());
    let client = Client::new(&next.url,&next.token,None).unwrap();
    assert!(client.attachments(id).unwrap().contains(&a));
    assert_eq!(client.download_attachment(a.id).unwrap(),payload);
}