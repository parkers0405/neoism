//! The desktop's light or dark preference on Linux, from the XDG desktop portal
//! (`org.freedesktop.appearance` `color-scheme`). winit reports no theme on Wayland unless the
//! app set one, so hosts ask here before falling back to dark. A worker thread reads the setting
//! once and follows `SettingChanged`; a change asks every window to redraw, which rereads it.

use futures_util::StreamExt;
use std::{sync::{Arc, Mutex, OnceLock, Weak, atomic::{AtomicU8, Ordering}, mpsc}, time::Duration};
use winit::window::Window;
use zbus::{Connection, OwnedVariant, Proxy, Variant};

const DEST: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const INTERFACE: &str = "org.freedesktop.portal.Settings";
const NAMESPACE: &str = "org.freedesktop.appearance";
const KEY: &str = "color-scheme";
/// How long the first window waits for the portal's first answer, as sctk-adwaita does for its
/// frame; a slower answer redraws the windows when it arrives.
const FIRST_ANSWER: Duration = Duration::from_millis(100);
/// Connecting, D-Bus activation of the portal and the first read.
const RPC_TIMEOUT: Duration = Duration::from_secs(10);

/// The portal's numbering: 0 no preference (or not known yet), 1 dark, 2 light.
static SCHEME: AtomicU8 = AtomicU8::new(0);
static WATCH: OnceLock<()> = OnceLock::new();
static WINDOWS: Mutex<Vec<Weak<Window>>> = Mutex::new(Vec::new());

/// Some(true) when the desktop prefers dark, Some(false) light, None when it has no preference
/// or no portal answers.
pub(crate) fn dark() -> Option<bool> {
    WATCH.get_or_init(start);
    scheme_dark(SCHEME.load(Ordering::Relaxed).into())
}

/// Redraws `window` whenever the preference changes; its redraw rereads [`dark`].
pub(crate) fn follow(window: &Arc<Window>) {
    let mut windows = WINDOWS.lock().unwrap_or_else(|e| e.into_inner());
    windows.retain(|w| w.strong_count() > 0);
    windows.push(Arc::downgrade(window));
}

fn scheme_dark(scheme: u32) -> Option<bool> {
    match scheme {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

/// `color-scheme` as a `u32`. Portals answer `v`, and `Read` (before `ReadOne`) wraps it twice.
fn scheme(value: &Variant<'_>) -> Option<u32> {
    match value {
        Variant::U32(x) => Some(*x),
        Variant::Variant(inner) => scheme(inner),
        _ => None,
    }
}

fn store(value: &Variant<'_>) {
    let Some(scheme) = scheme(value) else { return };
    let scheme = if scheme <= 2 { scheme as u8 } else { 0 };
    if SCHEME.swap(scheme, Ordering::Relaxed) == scheme { return; }
    for window in WINDOWS.lock().unwrap_or_else(|e| e.into_inner()).iter().filter_map(Weak::upgrade) {
        window.request_redraw();
    }
}

fn start() {
    let (ready, first) = mpsc::sync_channel(1);
    let worker = std::thread::Builder::new().name("rune-appearance-portal".into()).spawn(move || {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
        runtime.block_on(watch(ready));
    });
    if worker.is_ok() { let _ = first.recv_timeout(FIRST_ANSWER); }
}

async fn watch(ready: mpsc::SyncSender<()>) {
    let setup = tokio::time::timeout(RPC_TIMEOUT, async {
        let conn = Connection::session().await.ok()?;
        let portal = Proxy::new(&conn, DEST, PATH, INTERFACE).await.ok()?;
        // Subscribe first, so a change between the read and the subscription isn't lost.
        let changes = portal.receive_signal("SettingChanged").await.ok()?;
        let value = match portal.call::<_, _, OwnedVariant>("ReadOne", &(NAMESPACE, KEY)).await {
            Ok(v) => Some(v),
            Err(_) => portal.call::<_, _, OwnedVariant>("Read", &(NAMESPACE, KEY)).await.ok(),
        };
        if let Some(v) = value { store(&v); }
        Some((conn, changes))
    }).await.ok().flatten();
    let _ = ready.send(());
    let Some((_conn, mut changes)) = setup else { return };
    while let Some(message) = changes.next().await {
        if let Ok((namespace, key, value)) = message.body().deserialize::<(String, String, OwnedVariant)>()
            && namespace == NAMESPACE && key == KEY
        {
            store(&value);
        }
    }
}

#[cfg(test)]
#[path = "portal_appearance_tests.rs"]
mod tests;
