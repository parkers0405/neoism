//! xdg-desktop-portal FileChooser over zbus. Each dialog runs on its own thread with a
//! current-thread tokio runtime, so any executor can poll it and the event loop never blocks.
//! Dropping the future closes the portal request.
use super::{FileDialogError, FileDialogKind, FileDialogOptions};
use super::portal_format::{filters, outcome, parent_window, path_bytes};
use futures_util::StreamExt;
use std::{collections::HashMap, path::PathBuf, sync::atomic::{AtomicU32, AtomicU64, Ordering}, time::Duration};
use tokio::sync::oneshot;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use zbus::{Connection, OwnedObjectPath, OwnedVariant, Proxy, Variant, fdo};

const DEST: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const INTERFACE: &str = "org.freedesktop.portal.FileChooser";
/// Connecting, D-Bus activation of the portal and the method call. The user's choice itself
/// has no timeout.
const RPC_TIMEOUT: Duration = Duration::from_secs(30);

/// The focused X11 window, the dialog's parent. 0 is none, and Wayland windows stay 0.
static PARENT: AtomicU32 = AtomicU32::new(0);
static TOKEN: AtomicU64 = AtomicU64::new(0);

pub(crate) fn window_focus(window: &winit::window::Window, focused: bool) {
    let Ok(handle) = window.window_handle() else { return };
    let id = match handle.as_raw() {
        RawWindowHandle::Xlib(w) => u32::try_from(w.window).unwrap_or(0),
        RawWindowHandle::Xcb(w) => w.window.get(),
        _ => 0,
    };
    if id == 0 { return; }
    if focused { PARENT.store(id, Ordering::Relaxed); }
    else { let _ = PARENT.compare_exchange(id, 0, Ordering::Relaxed, Ordering::Relaxed); }
}

fn unavailable() -> FileDialogError { FileDialogError("no XDG desktop portal FileChooser is available".into()) }
fn dbus_error(e: zbus::Error) -> FileDialogError {
    if let zbus::Error::FDO(fdo) = &e
        && matches!(**fdo, fdo::Error::ServiceUnknown(_) | fdo::Error::NameHasNoOwner(_) | fdo::Error::UnknownInterface(_)
            | fdo::Error::UnknownMethod(_) | fdo::Error::UnknownObject(_)
            | fdo::Error::SpawnExecFailed(_) | fdo::Error::SpawnForkFailed(_) | fdo::Error::SpawnChildExited(_)
            | fdo::Error::SpawnChildSignaled(_) | fdo::Error::SpawnFailed(_) | fdo::Error::SpawnServiceNotFound(_)) {
        return unavailable();
    }
    FileDialogError(e.to_string())
}

fn request_options<'a>(kind: FileDialogKind, options: &'a FileDialogOptions, token: &'a str) -> HashMap<&'static str, Variant<'a>> {
    let mut map = HashMap::from([("handle_token", Variant::from(token)), ("modal", Variant::from(true))]);
    let filters = filters(options);
    if !filters.is_empty() && kind != FileDialogKind::Folder { map.insert("filters", Variant::from(filters)); }
    if let Some(bytes) = options.directory.as_deref().and_then(path_bytes) {
        map.insert("current_folder", Variant::from(bytes));
    }
    if kind == FileDialogKind::Folder { map.insert("directory", Variant::from(true)); }
    if kind == FileDialogKind::Save && let Some(name) = options.file_name.as_deref().filter(|s| !s.is_empty()) {
        map.insert("current_name", Variant::from(name));
    }
    map
}
/// A reply's `uris`, empty when absent or mistyped.
fn uris(results: &HashMap<String, OwnedVariant>) -> Vec<String> {
    results.get("uris").and_then(|v| v.try_clone().ok()).and_then(|v| Vec::<String>::try_from(v).ok()).unwrap_or_default()
}

pub(super) async fn show(kind: FileDialogKind, options: FileDialogOptions) -> Result<Option<PathBuf>, FileDialogError> {
    let (reply, result) = oneshot::channel();
    let parent = parent_window(PARENT.load(Ordering::Relaxed));
    std::thread::Builder::new().name("rune-file-dialog".into()).spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(r) => r,
            Err(e) => { let _ = reply.send(Err(FileDialogError(e.to_string()))); return; }
        };
        runtime.block_on(run(kind, options, parent, reply));
    }).map_err(|e| FileDialogError(e.to_string()))?;
    result.await.unwrap_or_else(|_| Err(FileDialogError("file dialog worker stopped".into())))
}

async fn run(kind: FileDialogKind, options: FileDialogOptions, parent: String, mut reply: oneshot::Sender<Result<Option<PathBuf>, FileDialogError>>) {
    let token = format!("rune_file_dialog_{}", TOKEN.fetch_add(1, Ordering::Relaxed));
    let started = tokio::time::timeout(RPC_TIMEOUT, async {
        // No session bus means this desktop cannot supply the portal.
        let conn = tokio::select! {
            conn = Connection::session() => conn.map_err(|_| unavailable())?,
            _ = reply.closed() => return Ok(None),
        };
        let portal = Proxy::new(&conn, DEST, PATH, INTERFACE).await.map_err(dbus_error)?;
        let sender = conn.unique_name().ok_or_else(unavailable)?.trim_start_matches(':').replace('.', "_");
        let path = format!("{PATH}/request/{sender}/{token}");
        let mut request = Proxy::new(&conn, DEST, path.clone(), "org.freedesktop.portal.Request").await.map_err(dbus_error)?;
        // Subscribe before calling: the portal can respond before the method returns.
        let mut responses = tokio::select! {
            res = request.receive_signal("Response") => res.map_err(dbus_error)?,
            _ = reply.closed() => { let _ = conn.close().await; return Ok(None); }
        };
        // A folder is OpenFile's `directory` choice (portal version 3).
        let method = match kind { FileDialogKind::Open | FileDialogKind::Folder => "OpenFile", FileDialogKind::Save => "SaveFile" };
        let body = (parent.as_str(), options.title.as_str(), request_options(kind, &options, &token));
        let handle: OwnedObjectPath = portal.call(method, &body).await.map_err(dbus_error)?;
        if handle.as_str() != path {
            // A portal older than handle_token support picks its own path.
            request = Proxy::new(&conn, DEST, handle.as_str().to_owned(), "org.freedesktop.portal.Request").await.map_err(dbus_error)?;
            responses = request.receive_signal("Response").await.map_err(dbus_error)?;
        }
        Ok::<_, FileDialogError>(Some((conn, request, responses)))
    }).await.unwrap_or_else(|_| Err(FileDialogError("file dialog portal timed out".into())));
    let (conn, request, mut responses) = match started {
        Ok(Some(v)) => v,
        Ok(None) => return,
        Err(e) => { let _ = reply.send(Err(e)); return; }
    };
    tokio::select! {
        response = responses.next() => {
            let result = match response {
                Some(message) => message.body().deserialize::<(u32, HashMap<String, OwnedVariant>)>()
                    .map_err(|e| FileDialogError(e.to_string()))
                    .and_then(|(status, results)| outcome(status, &uris(&results))),
                None => Err(FileDialogError("file dialog portal disconnected".into())),
            };
            let _ = reply.send(result);
        }
        _ = reply.closed() => {
            let _ = tokio::time::timeout(RPC_TIMEOUT, request.call::<_, _, ()>("Close", &())).await;
        }
    }
    let _ = conn.close().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reply_uris_are_read_from_the_results_map() {
        let list = vec!["file:///tmp/a%20b".to_string(), "file:///tmp/c".to_string()];
        let results = HashMap::from([("uris".to_string(), OwnedVariant::try_from(Variant::from(list.clone())).unwrap())]);
        assert_eq!(uris(&results), list);
        assert_eq!(outcome(0, &uris(&results)).unwrap(), Some(PathBuf::from("/tmp/a b")));
        assert!(uris(&HashMap::new()).is_empty());
        let mistyped = HashMap::from([("uris".to_string(), OwnedVariant::try_from(Variant::from(7u32)).unwrap())]);
        assert!(uris(&mistyped).is_empty());
    }
    #[test]
    fn options_carry_token_filters_folder_and_save_name() {
        let options = FileDialogOptions { title: "Import".into(), directory: Some("/home/a/Downloads".into()),
            file_name: Some("export.csv".into()), filters: vec![] }.filter("Exports", &["csv"]);
        let open = request_options(FileDialogKind::Open, &options, "t1");
        assert_eq!(open.get("handle_token"), Some(&Variant::from("t1")));
        assert_eq!(open.get("filters"), Some(&Variant::from(vec![("Exports".to_string(), vec![(0u32, "*.csv".to_string())])])));
        assert_eq!(open.get("current_folder"), Some(&Variant::from(b"/home/a/Downloads\0".to_vec())));
        assert!(!open.contains_key("current_name"));
        assert!(!open.contains_key("directory"));
        let folder = request_options(FileDialogKind::Folder, &options, "t4");
        assert_eq!(folder.get("directory"), Some(&Variant::from(true)));
        assert!(!folder.contains_key("filters"));
        assert_eq!(folder.get("current_folder"), Some(&Variant::from(b"/home/a/Downloads\0".to_vec())));
        let save = request_options(FileDialogKind::Save, &options, "t2");
        assert_eq!(save.get("current_name"), Some(&Variant::from("export.csv")));
        let empty = FileDialogOptions { directory: Some(PathBuf::new()), file_name: Some(String::new()), ..Default::default() }
            .filter("Empty", &[]);
        let empty_save = request_options(FileDialogKind::Save, &empty, "t3");
        assert!(!empty_save.contains_key("filters"));
        assert!(!empty_save.contains_key("current_folder"));
        assert!(!empty_save.contains_key("current_name"));
    }
    #[test]
    fn missing_portal_is_a_clear_error() {
        let fdo = |e| zbus::Error::FDO(Box::new(e));
        assert_eq!(dbus_error(fdo(fdo::Error::ServiceUnknown("portal".into()))), unavailable());
        assert_eq!(dbus_error(fdo(fdo::Error::UnknownMethod("OpenFile".into()))), unavailable());
        assert_eq!(dbus_error(fdo(fdo::Error::SpawnChildExited("portal".into()))), unavailable());
        assert_ne!(dbus_error(fdo(fdo::Error::Failed("x".into()))), unavailable());
    }
}
