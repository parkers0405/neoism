//! Standalone worker. CEF subprocess dispatch precedes IPC initialization.
mod engine;
mod theme;
use cef::{rc::Rc, *};
use neoism_interactive_artifacts::ipc::{
    self as wire, Control, FrameMetadata, Output, PROTOCOL_VERSION,
};
use std::{
    io::{self, BufReader, BufWriter, Write},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

type Deadline = Arc<Mutex<Option<Instant>>>;
wrap_browser_process_handler! {
    struct Process { deadline: Deadline }
    impl BrowserProcessHandler {
        fn on_schedule_message_pump_work(&self,delay_ms:i64){
            let when=Instant::now()+Duration::from_millis(delay_ms.clamp(0,60_000) as u64);
            let mut d=self.deadline.lock().unwrap();if d.is_none_or(|old|when<old){*d=Some(when);}
        }
    }
}
wrap_app! {
    struct WorkerApp { experimental: bool, process:BrowserProcessHandler }
    impl App {
        fn browser_process_handler(&self)->Option<BrowserProcessHandler>{Some(self.process.clone())}
        fn on_before_command_line_processing(&self,_process_type:Option<&CefString>,line:Option<&mut CommandLine>){
            if let Some(c)=line {
                for flag in ["disable-gpu","disable-gpu-compositing","noerrdialogs","disable-background-networking","disable-component-update","disable-sync"] {c.append_switch(Some(&flag.into()));}
                // No silent sandbox fallback. Explicit opt-in only, propagated by CEF.
                if self.experimental {c.append_switch(Some(&"no-sandbox".into()));}
            }
        }
    }
}
struct PrivateCache(std::path::PathBuf);
impl PrivateCache {
    fn new() -> io::Result<Self> {
        use std::os::unix::fs::DirBuilderExt;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir()
            .join(format!("neoism-cef-{}-{nonce}", std::process::id()));
        std::fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self(path))
    }
}
impl Drop for PrivateCache {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn main() {
    if let Err(e) = run() {
        eprintln!("neoism-cef-runtime: {e}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    let args = cef::args::Args::new();
    let cmd = args.as_cmd_line().ok_or("CEF command line unavailable")?;
    let child = cmd.has_switch(Some(&"type".into())) == 1;
    let experimental = std::env::var("NEOISM_CEF_NO_SANDBOX").as_deref() == Ok("1");
    // Child must never use host pipes, including during execute_process diagnostics.
    if child {
        let _ = filedescriptor::FileDescriptor::redirect_stdio(
            &io::stderr(),
            filedescriptor::StdioDescriptor::Stdout,
        )?;
        let null = std::fs::File::open("/dev/null")?;
        let _ = filedescriptor::FileDescriptor::redirect_stdio(
            &null,
            filedescriptor::StdioDescriptor::Stdin,
        )?;
    }
    let deadline = Arc::new(Mutex::new(None));
    let mut app = WorkerApp::new(experimental, Process::new(deadline.clone()));
    let code = execute_process(
        Some(args.as_main_args()),
        Some(&mut app),
        std::ptr::null_mut(),
    );
    if child {
        std::process::exit(code.max(0));
    }
    if code != -1 {
        return Err(format!("CEF execute_process returned {code}").into());
    }
    let cli: Vec<_> = std::env::args().skip(1).collect();
    if cli != ["--stdio"] && cli != ["--stdio", "--experimental"] {
        return Err("Requires --stdio [--experimental]; NEOISM_CEF_NO_SANDBOX=1 is the only sandbox bypass".into());
    }
    let stdout = filedescriptor::FileDescriptor::redirect_stdio(
        &io::stderr(),
        filedescriptor::StdioDescriptor::Stdout,
    )?;
    // Saved descriptors are CLOEXEC (filedescriptor); subprocesses inherit null stdin
    // and stderr stdout, not protocol descriptors. No child can consume controls.
    let null = std::fs::File::open("/dev/null")?;
    let stdin = filedescriptor::FileDescriptor::redirect_stdio(
        &null,
        filedescriptor::StdioDescriptor::Stdin,
    )?;
    let mut writer = BufWriter::new(stdout);
    wire::write_json(
        &mut writer,
        &Output::Hello {
            version: PROTOCOL_VERSION,
        },
    )?;
    writer.flush()?;
    if experimental {
        eprintln!(
            "WARNING: experimental unsandboxed CEF. Trusted synthetic content only."
        );
    }
    let resources = std::env::var("NEOISM_CEF_DIR")
        .or_else(|_| std::env::var("CEF_PATH"))
        .unwrap_or_else(|_| {
            let beside = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|p| p.to_owned()));
            if let Some(p) = beside.filter(|p| p.join("resources.pak").is_file()) {
                return p.to_string_lossy().into_owned();
            }
            format!(
                "{}/.local/share/cef",
                std::env::var("HOME").unwrap_or_default()
            )
        });
    let cache = PrivateCache::new()?;
    let settings = Settings {
        root_cache_path: cache.0.to_string_lossy().as_ref().into(),
        windowless_rendering_enabled: 1,
        external_message_pump: 1,
        no_sandbox: experimental as i32,
        resources_dir_path: resources.as_str().into(),
        locales_dir_path: format!("{resources}/locales").as_str().into(),
        ..Default::default()
    };
    if initialize(
        Some(args.as_main_args()),
        Some(&settings),
        Some(&mut app),
        std::ptr::null_mut(),
    ) != 1
    {
        wire::write_json(
            &mut writer,
            &Output::Fatal(
                "CEF initialization failed; sandbox is not silently disabled".into(),
            ),
        )?;
        writer.flush()?;
        return Err("CEF initialization failed".into());
    }
    let (tx, rx) = mpsc::sync_channel(32);
    std::thread::Builder::new()
        .name("cef-control".into())
        .spawn(move || {
            let mut reader = BufReader::new(stdin);
            loop {
                let message =
                    wire::read_json::<Control>(&mut reader).map_err(|e| e.to_string());
                let end = !matches!(message, Ok(Some(_)));
                if tx.send(message).is_err() || end {
                    break;
                }
            }
        })?;
    let mut host = engine::Host::new();
    let mut sequence = 0u64;
    let mut previous_visible = None;
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        loop {
            let now = Instant::now();
            let due = deadline
                .lock()
                .unwrap()
                .map(|d| d.saturating_duration_since(now))
                .unwrap_or(Duration::from_millis(100));
            // 2ms ceiling handles schedule callbacks arriving from CEF threads while
            // recv_timeout is asleep; hidden/idle backs off to 20ms, no busy-spin.
            let delay =
                due.min(Duration::from_millis(if host.visible() { 2 } else { 20 }));
            let first = match rx.recv_timeout(delay) {
                Ok(v) => Some(v),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(_) => break,
            };
            let batch = first
                .into_iter()
                .chain((0..31).map_while(|_| rx.try_recv().ok()));
            let mut stop = false;
            for event in batch {
                let Some(control) = event.map_err(|e| format!("Control pipe: {e}"))?
                else {
                    stop = true;
                    break;
                };
                control.validate()?;
                match control {
                    Control::Shutdown => {
                        stop = true;
                        break;
                    }
                    Control::Reconcile {
                        document,
                        generation,
                    } => host.reconcile(document, generation)?,
                    Control::Resize {
                        key,
                        viewport,
                        generation,
                    } => host.resize(&key, viewport, generation)?,
                    Control::Input { key, input } => host.input(&key, input)?,
                    Control::Destroy { key } => host.destroy(&key),
                    Control::Pump => (),
                }
            }
            if stop {
                break;
            }
            *deadline.lock().unwrap() = None;
            do_message_loop_work();
            // Metrics and pixels share readiness and transaction identity; publish
            // metrics first so the host can fit the row before consuming pixels.
            for (key, generation, revision, height) in host.layout_metrics() {
                wire::write_json(
                    &mut writer,
                    &Output::ContentHeight {
                        key,
                        generation,
                        revision,
                        height,
                    },
                )?;
            }
            for (key, generation, revision, viewport, pixels) in host.frames() {
                sequence = sequence.checked_add(1).ok_or("Frame sequence exhausted")?;
                let meta = FrameMetadata {
                    key,
                    generation,
                    revision,
                    sequence,
                    width: viewport.width,
                    height: viewport.height,
                    stride: viewport.width as usize * 4,
                    bytes: pixels.len(),
                };
                wire::write_frame(&mut writer, &meta, &pixels)?;
            }
            // Status means visible rendering demand, not JS-animation detection.
            if previous_visible != Some(host.visible()) {
                wire::write_json(
                    &mut writer,
                    &Output::Status {
                        animating: host.visible(),
                        diagnostics: Vec::new(),
                    },
                )?;
                previous_visible = Some(host.visible());
            }
            writer.flush()?;
        }
        Ok(())
    })();
    if let Err(e) = &result {
        let _ = wire::write_json(&mut writer, &Output::Fatal(e.to_string()));
        let _ = writer.flush();
    }
    host.shutdown()?;
    drop(host);
    shutdown();
    result
}
