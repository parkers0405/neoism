use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{mpsc, Mutex, OnceLock},
    thread,
};

use neoism_agent_server::language_server;

/// One editor-originated document lifecycle event. Events for the same
/// document travel through one FIFO worker before they touch the shared LSP
/// service. Different files keep independent workers, so a cold server for one
/// project cannot stall live diagnostics in an unrelated language or file.
#[derive(Clone, Debug)]
enum LiveDocumentEvent {
    Sync { text: String },
    Save,
    Barrier { ready: mpsc::SyncSender<()> },
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct LiveDocumentKey {
    workspace_root: PathBuf,
    file: PathBuf,
}

fn live_document_senders(
) -> &'static Mutex<HashMap<LiveDocumentKey, mpsc::Sender<LiveDocumentEvent>>> {
    static SENDERS: OnceLock<
        Mutex<HashMap<LiveDocumentKey, mpsc::Sender<LiveDocumentEvent>>>,
    > = OnceLock::new();
    SENDERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn live_document_sender(
    workspace_root: &Path,
    file: &Path,
) -> mpsc::Sender<LiveDocumentEvent> {
    let key = LiveDocumentKey {
        workspace_root: workspace_root.to_path_buf(),
        file: file.to_path_buf(),
    };
    let mut senders = live_document_senders()
        .lock()
        .expect("live-document route map lock poisoned");
    senders
        .entry(key.clone())
        .or_insert_with(|| {
            let (sender, receiver) = mpsc::channel();
            let worker_name = format!(
                "neoism-lsp-live-{}",
                file.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("document")
            );
            thread::Builder::new()
                .name(worker_name)
                .spawn(move || {
                    run_live_document_events(receiver, |event| {
                        process_live_document_event(
                            &key.workspace_root,
                            &key.file,
                            event,
                        );
                    });
                })
                .expect("spawn ordered live-document LSP worker");
            sender
        })
        .clone()
}

/// Queue an authoritative live buffer snapshot immediately after an editor or
/// collaborative edit. This is the production `didOpen`/`didChange` path; it
/// is event-driven and never waits for the diagnostics/status recovery poll.
pub fn sync_document(workspace_root: &Path, file: &Path, text: String) {
    if language_server::language_id_for_path_in(workspace_root, file).is_none() {
        return;
    }
    if live_document_sender(workspace_root, file)
        .send(LiveDocumentEvent::Sync { text })
        .is_err()
    {
        tracing::warn!(
            file = %file.display(),
            "ordered live-document LSP worker stopped before didChange"
        );
    }
}

/// Queue `didSave` behind every preceding edit for this document. Keeping save
/// on the same FIFO prevents a fast `:w` from reaching a build-backed server
/// before the final insert-mode `didChange`.
pub fn save_document(workspace_root: &Path, file: &Path) {
    if language_server::language_id_for_path_in(workspace_root, file).is_none() {
        return;
    }
    if live_document_sender(workspace_root, file)
        .send(LiveDocumentEvent::Save)
        .is_err()
    {
        tracing::warn!(
            file = %file.display(),
            "ordered live-document LSP worker stopped before didSave"
        );
    }
}

/// Wait until every event already submitted for this document has reached the
/// LSP service. Interactive queries use this barrier instead of re-sending a
/// full snapshot on a separate thread, which could otherwise overtake queued
/// insert edits and regress the server to older text.
pub fn flush_document_sync(workspace_root: &Path, file: &Path) {
    if language_server::language_id_for_path_in(workspace_root, file).is_none() {
        return;
    }
    let (ready, completed) = mpsc::sync_channel(0);
    if live_document_sender(workspace_root, file)
        .send(LiveDocumentEvent::Barrier { ready })
        .is_ok()
    {
        let _ = completed.recv();
    }
}

fn process_live_document_event(
    workspace_root: &Path,
    file: &Path,
    event: LiveDocumentEvent,
) {
    match event {
        LiveDocumentEvent::Sync { text } => {
            language_server::sync_document(workspace_root, file, Some(&text));
        }
        LiveDocumentEvent::Save => {
            language_server::save_document(workspace_root, file);
        }
        LiveDocumentEvent::Barrier { ready } => {
            let _ = ready.send(());
        }
    }
}

fn run_live_document_events(
    receiver: mpsc::Receiver<LiveDocumentEvent>,
    mut process: impl FnMut(LiveDocumentEvent),
) {
    while let Ok(event) = receiver.recv() {
        process(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn live_edits_and_save_are_processed_in_submission_order() {
        let (sender, receiver) = mpsc::channel();
        let observed = Arc::new(Mutex::new(Vec::<String>::new()));
        let worker_observed = Arc::clone(&observed);
        let worker = thread::spawn(move || {
            run_live_document_events(receiver, |event| {
                let label = match event {
                    LiveDocumentEvent::Sync { text } => format!("sync:{text}"),
                    LiveDocumentEvent::Save => "save".to_string(),
                    LiveDocumentEvent::Barrier { ready } => {
                        let _ = ready.send(());
                        "barrier".to_string()
                    }
                };
                worker_observed.lock().unwrap().push(label);
            });
        });
        let events = [
            LiveDocumentEvent::Sync {
                text: "broken".to_string(),
            },
            LiveDocumentEvent::Sync {
                text: "fixed".to_string(),
            },
            LiveDocumentEvent::Save,
        ];
        for event in &events {
            sender.send(event.clone()).unwrap();
        }
        drop(sender);
        worker.join().unwrap();

        assert_eq!(
            *observed.lock().unwrap(),
            vec!["sync:broken", "sync:fixed", "save"]
        );
    }
}
