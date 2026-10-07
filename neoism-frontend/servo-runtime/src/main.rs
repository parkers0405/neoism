//! Pipe worker. Never add this executable to the Neoism root dependency graph.
use neoism_interactive_artifacts::ipc::{
    self as wire, Control, FrameMetadata, Output, PROTOCOL_VERSION,
};
use neoism_servo_runtime::engine::Host;
use std::io::{self, BufReader, BufWriter, Write};
use std::sync::{mpsc, Arc};
use std::time::Duration;

enum Event {
    Control(Control),
    Wake,
    End(Result<(), String>),
}
fn receive_timeout(animating: bool, presentation: Option<Duration>) -> Option<Duration> {
    if animating {
        Some(presentation.map_or(Duration::from_millis(16), |delay| {
            delay.min(Duration::from_millis(16))
        }))
    } else {
        presentation
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("neoism-servo-runtime: {error}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args != ["--stdio", "--experimental"] {
        return Err("Requires --stdio --experimental; this worker has NO OS sandbox and is for trusted experimental content only".into());
    }
    // Save the original pipe descriptor, then redirect ALL process stdout to stderr
    // before constructing Servo. Upstream Rust/C native diagnostics cannot corrupt IPC.
    // This safe descriptor API is isolated to the standalone worker process.
    let protocol_stdout = filedescriptor::FileDescriptor::redirect_stdio(
        &io::stderr(),
        filedescriptor::StdioDescriptor::Stdout,
    )?;
    let mut writer = BufWriter::new(protocol_stdout);
    wire::write_json(
        &mut writer,
        &Output::Hello {
            version: PROTOCOL_VERSION,
        },
    )?;
    writer.flush()?;
    let (sender, receiver) = mpsc::sync_channel::<Event>(32);
    let wake_sender = sender.clone();
    let mut host = match Host::new(Arc::new(move || {
        let _ = wake_sender.try_send(Event::Wake);
    })) {
        Ok(host) => host,
        Err(error) => {
            wire::write_json(&mut writer, &Output::Fatal(error.to_string()))?;
            writer.flush()?;
            return Err(error.into());
        }
    };
    std::thread::Builder::new()
        .name("servo-runtime-control".into())
        .spawn(move || {
            let mut reader = BufReader::new(io::stdin());
            loop {
                match wire::read_json::<Control>(&mut reader) {
                    Ok(Some(control)) => {
                        if sender.send(Event::Control(control)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => {
                        let _ = sender.send(Event::End(Ok(())));
                        break;
                    }
                    Err(error) => {
                        let _ = sender.send(Event::End(Err(error.to_string())));
                        break;
                    }
                }
            }
        })?;
    let mut generations = std::collections::HashMap::<String, u64>::new();
    let mut sequence = 0u64;
    let mut previous_animating = false;
    let mut stopping = false;
    while !stopping {
        // A final dirty frame can arrive before the rate-limit deadline and
        // then stop animating. Never enter an unbounded recv with it retained.
        let timeout =
            receive_timeout(previous_animating, host.pending_presentation_delay());
        let first = match if let Some(timeout) = timeout {
            receiver.recv_timeout(timeout)
        } else {
            receiver
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
        } {
            Ok(event) => Some(event),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let mut diagnostics = Vec::new();
        // Bounded batch so continuous input cannot starve engine turns and frame output.
        let batch = first
            .into_iter()
            .chain((0..31).map_while(|_| receiver.try_recv().ok()));
        for event in batch {
            match event {
                Event::Wake => (),
                Event::End(result) => {
                    if let Err(error) = result {
                        wire::write_json(
                            &mut writer,
                            &Output::Fatal(format!("Control pipe: {error}")),
                        )?;
                        writer.flush()?;
                    }
                    stopping = true;
                    break;
                }
                Event::Control(Control::Shutdown) => {
                    stopping = true;
                    break;
                }
                Event::Control(control) => {
                    let result = control.validate().and_then(|_| match control {
                        Control::Reconcile {
                            document,
                            generation,
                        } => {
                            let key = document.key.clone();
                            host.reconcile(document).map(|_| {
                                generations.insert(key, generation);
                            })
                        }
                        Control::Resize {
                            key,
                            viewport,
                            generation,
                        } => host.resize(&key, viewport).map(|_| {
                            generations.insert(key, generation);
                        }),
                        Control::Input { key, input } => host.input(&key, input),
                        Control::Destroy { key } => {
                            host.destroy(&key);
                            generations.remove(&key);
                            Ok(())
                        }
                        Control::Pump | Control::Shutdown => Ok(()),
                    });
                    if let Err(error) = result {
                        wire::write_json(
                            &mut writer,
                            &Output::Fatal(format!("Artifact request: {error}")),
                        )?;
                        writer.flush()?;
                        stopping = true;
                        break;
                    }
                }
            }
        }
        if stopping {
            break;
        }
        match host.pump() {
            Ok(update) => {
                let changed = !update.frames.is_empty();
                for frame in update.frames {
                    let Some(generation) = generations.get(&frame.key).copied() else {
                        continue;
                    };
                    sequence =
                        sequence.checked_add(1).ok_or("Frame sequence exhausted")?;
                    let metadata = FrameMetadata {
                        key: frame.key,
                        generation,
                        revision: frame.revision,
                        sequence,
                        width: frame.width,
                        height: frame.height,
                        stride: frame.stride,
                        bytes: frame.rgba.len(),
                    };
                    wire::write_frame(&mut writer, &metadata, &frame.rgba)?;
                }
                diagnostics.extend(update.diagnostics);
                if changed
                    || update.animating != previous_animating
                    || !diagnostics.is_empty()
                {
                    wire::write_json(
                        &mut writer,
                        &Output::Status {
                            animating: update.animating,
                            diagnostics,
                        },
                    )?;
                    writer.flush()?;
                }
                previous_animating = update.animating;
            }
            Err(error) => {
                wire::write_json(&mut writer, &Output::Fatal(error.to_string()))?;
                writer.flush()?;
                stopping = true;
            }
        }
    }
    host.shutdown();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_receive_keeps_final_dirty_deadline() {
        assert_eq!(receive_timeout(false, None), None);
        let deadline = Duration::from_millis(25);
        assert_eq!(receive_timeout(false, Some(deadline)), Some(deadline));
        // No Servo wake, parent Pump, or further input: the idle channel must
        // timeout and deliver the retained last frame's presentation turn.
        let (_sender, receiver) = mpsc::channel::<Event>();
        assert!(matches!(
            receiver.recv_timeout(
                receive_timeout(false, Some(Duration::from_millis(1))).unwrap()
            ),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
    }

    #[test]
    fn animation_and_presentation_use_earliest_deadline() {
        assert_eq!(receive_timeout(true, None), Some(Duration::from_millis(16)));
        assert_eq!(
            receive_timeout(true, Some(Duration::from_millis(30))),
            Some(Duration::from_millis(16))
        );
        assert_eq!(
            receive_timeout(true, Some(Duration::from_millis(3))),
            Some(Duration::from_millis(3))
        );
        assert_eq!(
            receive_timeout(false, Some(Duration::ZERO)),
            Some(Duration::ZERO)
        );
    }
}
