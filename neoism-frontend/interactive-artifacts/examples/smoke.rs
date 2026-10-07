//! Standalone native smoke driver; not launched automatically by Neoism.
//! NEOISM_SERVO_EXPERIMENTAL=1 cargo run -p neoism-interactive-artifacts --features servo --example smoke
#[cfg(all(feature = "servo", not(target_arch = "wasm32")))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use neoism_interactive_artifacts::{
        servo_host::Host, ArtifactDocument, Theme, Viewport,
    };
    use std::{
        sync::{mpsc, Arc},
        time::{Duration, Instant},
    };
    if std::env::var("NEOISM_SERVO_EXPERIMENTAL").as_deref() != Ok("1") {
        return Err("Set NEOISM_SERVO_EXPERIMENTAL=1 to opt into the unsandboxed experimental engine".into());
    }
    let (tx, rx) = mpsc::channel();
    let mut host = Host::new(Arc::new(move || {
        let _ = tx.send(());
    }))?;
    eprintln!("{:?}", host.sandbox_status());
    host.reconcile(ArtifactDocument {
        key: "smoke".into(), revision: 1, visible: true, theme: Theme::Light,
        styles: neoism_interactive_artifacts::ArtifactStyles::default(),
        viewport: Viewport { width: 320, height: 320, scale: 1.0 },
        html: "<!doctype html><style>html,body{margin:0;background:rgb(255,0,0)}button{margin:40px}</style><button onclick=\"this.textContent='Clicked'\">Servo</button>".into(),
    })?;
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        let update = host.pump()?;
        for diagnostic in update.diagnostics {
            eprintln!("{diagnostic}");
        }
        if let Some(frame) = update.frames.into_iter().next() {
            let pixel = frame.rgba.get(..4).ok_or("Empty RGBA frame")?;
            // Verify actual page painting rather than just a blank context readback.
            if pixel[0] > 240 && pixel[1] < 15 && pixel[2] < 15 {
                println!(
                    "Servo painted {}x{} RGBA frame, {} bytes",
                    frame.width,
                    frame.height,
                    frame.rgba.len()
                );
                host.destroy("smoke");
                host.shutdown();
                return Ok(());
            }
        }
        let _ = rx.recv_timeout(Duration::from_millis(16));
        while rx.try_recv().is_ok() {}
    }
    host.shutdown();
    Err("No red Servo frame within 20 seconds".into())
}
#[cfg(any(not(feature = "servo"), target_arch = "wasm32"))]
fn main() {
    eprintln!("Native smoke driver requires --features servo");
}
