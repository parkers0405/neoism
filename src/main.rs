//! `neoism-rm-agent` — the on-device companion that makes reMarkable
//! handwriting appear *live* in Neoism.
//!
//! xochitl (the reMarkable's app) won't push to us, so this little binary
//! runs on the tablet, watches its per-page `.rm` stroke files, and
//! whenever one changes it parses the strokes and streams them to Neoism
//! over a [`BridgeMsg`] frame. Neoism merges them into the note's CRDT ink
//! layer (by stable stroke id, so unchanged pages are no-ops).
//!
//! It depends only on `neoism-sync` + std so it cross-compiles small to
//! the tablet's `armv7-unknown-linux-gnueabihf`. Install + autostart is
//! handled by the bridge (scp the binary + a systemd unit, or via toltec).
//!
//! Usage: `neoism-rm-agent --connect <neoism-host:port> [--xochitl DIR]`

use std::collections::HashMap;
use std::io::Write;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use neoism_sync::{bridge::BridgeMsg, remarkable};

/// Where xochitl keeps documents on a stock reMarkable.
const DEFAULT_XOCHITL: &str = "/home/root/.local/share/remarkable/xochitl";
/// How often to scan for changed pages. ~500ms reads as "live" while
/// staying gentle on the e-ink device's CPU/battery.
const POLL: Duration = Duration::from_millis(500);
/// Backoff before redialing Neoism after a dropped/refused connection.
const RECONNECT: Duration = Duration::from_secs(2);

struct Args {
    xochitl: PathBuf,
    connect: String,
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("neoism-rm-agent: {e}\n");
            print_help();
            std::process::exit(2);
        }
    };
    let firmware = read_firmware();
    eprintln!(
        "neoism-rm-agent: watching {} (firmware {firmware}); target {}",
        args.xochitl.display(),
        args.connect
    );

    // Reconnect forever — the tablet sleeps, the network blips, Neoism
    // restarts; the agent should just quietly re-establish.
    loop {
        match TcpStream::connect(&args.connect) {
            Ok(mut stream) => {
                eprintln!("connected to {}", args.connect);
                if let Err(e) = run_session(&mut stream, &args.xochitl, &firmware) {
                    eprintln!("session ended: {e}");
                }
            }
            Err(e) => eprintln!("connect to {} failed: {e}", args.connect),
        }
        std::thread::sleep(RECONNECT);
    }
}

/// Greet Neoism, then stream changed pages until the socket dies.
fn run_session(
    stream: &mut TcpStream,
    xochitl: &Path,
    firmware: &str,
) -> std::io::Result<()> {
    let hello = BridgeMsg::Hello {
        device: "reMarkable".into(),
        firmware: firmware.to_string(),
    };
    stream.write_all(&hello.encode_frame())?;

    let mut seen: HashMap<PathBuf, SystemTime> = HashMap::new();
    loop {
        for (path, page_id) in rm_pages(xochitl) {
            let mtime = std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            if seen.get(&path) == Some(&mtime) {
                continue; // unchanged since last scan
            }
            seen.insert(path.clone(), mtime);

            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(_) => continue, // mid-write; catch it next pass
            };
            match remarkable::parse_rm(&bytes) {
                Ok(strokes) => {
                    let msg = BridgeMsg::PageInk { page_id, strokes };
                    stream.write_all(&msg.encode_frame())?;
                }
                // v6 isn't wired yet — log and keep going rather than die.
                Err(e) => eprintln!("skip {}: {e}", path.display()),
            }
        }
        std::thread::sleep(POLL);
    }
}

/// Every `<doc-uuid>/<page-uuid>.rm` under the xochitl root, paired with a
/// `"<doc>/<page>"` id so Neoism can map it to the right note.
fn rm_pages(root: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let Ok(docs) = std::fs::read_dir(root) else {
        return out;
    };
    for doc in docs.flatten() {
        let doc_path = doc.path();
        if !doc_path.is_dir() {
            continue;
        }
        let doc_id = doc.file_name().to_string_lossy().into_owned();
        let Ok(pages) = std::fs::read_dir(&doc_path) else {
            continue;
        };
        for page in pages.flatten() {
            let p = page.path();
            if p.extension().and_then(|e| e.to_str()) == Some("rm") {
                let page_id = p
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                out.push((p, format!("{doc_id}/{page_id}")));
            }
        }
    }
    out
}

/// Best-effort firmware string so Neoism picks the right `.rm`/`.content`
/// handling. Falls back to "unknown" off-device (e.g. when host-testing).
fn read_firmware() -> String {
    for path in ["/usr/share/remarkable/update.conf", "/etc/version"] {
        if let Ok(contents) = std::fs::read_to_string(path) {
            for line in contents.lines() {
                if let Some(v) = line.strip_prefix("REMARKABLE_RELEASE_VERSION=") {
                    return v.trim().to_string();
                }
            }
            let trimmed = contents.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    "unknown".to_string()
}

fn parse_args() -> Result<Args, String> {
    let mut xochitl = PathBuf::from(DEFAULT_XOCHITL);
    let mut connect = None;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--xochitl" => {
                xochitl = PathBuf::from(it.next().ok_or("--xochitl needs a directory")?);
            }
            "--connect" => {
                connect = Some(it.next().ok_or("--connect needs host:port")?);
            }
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Args {
        xochitl,
        connect: connect.ok_or("missing --connect <neoism-host:port>")?,
    })
}

fn print_help() {
    eprintln!(
        "neoism-rm-agent — stream reMarkable handwriting to Neoism\n\n\
         USAGE:\n  neoism-rm-agent --connect <host:port> [--xochitl DIR]\n\n\
         OPTIONS:\n  \
         --connect <host:port>   Neoism's listening address (required)\n  \
         --xochitl <DIR>         xochitl document dir (default: {DEFAULT_XOCHITL})\n  \
         -h, --help              show this help"
    );
}
