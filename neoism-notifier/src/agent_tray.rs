//! Linux StatusNotifierItem. `start` only spawns a private executor thread.
//! Handles share one process-wide item; last-handle drop or `stop` cancels it.
//! No activation ever requests window focus.
use futures_lite::{future, StreamExt};
use std::sync::{mpsc, Arc, Mutex, Weak};
use std::time::{Duration, Instant};
use zbus::zvariant::OwnedObjectPath;
use zbus::{connection::Builder, Connection, Proxy};

const PATH: &str = "/StatusNotifierItem";
const IFACE: &str = "org.kde.StatusNotifierItem";
const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const TICK: Duration = Duration::from_millis(250);
type Pixmaps = Vec<(i32, i32, Vec<u8>)>;
type Tooltip = (String, Pixmaps, String, String);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayState {
    Idle,
    Working { count: usize },
    Done,
    Unknown,
}

#[derive(Clone)]
pub struct AgentTray(Arc<Handle>);
struct Handle {
    shared: Arc<Shared>,
    stop: async_channel::Sender<()>,
}
impl Drop for Handle {
    fn drop(&mut self) {
        self.stop.close();
    }
}
static WORKER: Mutex<()> = Mutex::new(());
static INSTANCE: Mutex<Option<Weak<Handle>>> = Mutex::new(None);

impl AgentTray {
    /// Returns immediately after spawning the worker. Only thread creation errors
    /// are returned; missing session bus/watcher is a background, nonfatal failure.
    /// A watcher appearing or restarting on the connected bus is registered again.
    pub fn start() -> std::io::Result<Self> {
        let mut instance = INSTANCE.lock().unwrap();
        if let Some(handle) = instance.as_ref().and_then(Weak::upgrade) {
            if !handle.stop.is_closed() && handle.stop.is_empty() {
                return Ok(Self(handle));
            }
        }
        let shared = Arc::new(Shared::default());
        let worker = shared.clone();
        let (stop, stopped) = async_channel::bounded(1);
        std::thread::Builder::new()
            .name("neoism-tray".into())
            .spawn(move || {
                // Serialize teardown/restart without ever joining on the caller.
                let _worker_guard = WORKER.lock().unwrap();
                if stopped.is_closed() {
                    return;
                }
                future::block_on(until_stopped(stopped, async {
                    if let Err(error) = run(worker).await {
                        eprintln!("neoism agent tray unavailable: {error}");
                    }
                }));
            })?;
        let handle = Arc::new(Handle { shared, stop });
        *instance = Some(Arc::downgrade(&handle));
        Ok(Self(handle))
    }

    /// Cancels the process-wide backend, including pending bus operations.
    /// Nonblocking; existing clones become inert. A subsequent start creates a new item.
    pub fn stop(&self) {
        self.0.stop.close();
    }

    /// Latest value wins. Repeating a state never restarts its animation.
    pub fn set_state(&self, state: TrayState) {
        self.0.shared.set_state(state);
    }

    /// Clears Done only; never cancels current work or turns Unknown into Done.
    pub fn acknowledge(&self) {
        self.0.shared.acknowledge(false);
    }

    /// Optional bounded activation channel. Clicks acknowledge before notification;
    /// queued clicks coalesce, and disconnected subscriptions are removed.
    pub fn subscribe_activation(&self) -> mpsc::Receiver<()> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.0.shared.inner.lock().unwrap().activations.push(tx);
        rx
    }
}

async fn until_stopped(
    stopped: async_channel::Receiver<()>,
    task: impl std::future::Future<Output = ()>,
) {
    future::race(task, async {
        let _ = stopped.recv().await;
    })
    .await;
}

struct Inner {
    desired: TrayState,
    view: View,
    activations: Vec<mpsc::SyncSender<()>>,
}
struct Shared {
    inner: Mutex<Inner>,
    wake: async_channel::Sender<()>,
    updates: async_channel::Receiver<()>,
}
impl Default for Shared {
    fn default() -> Self {
        let (wake, updates) = async_channel::bounded(1);
        Self {
            inner: Mutex::new(Inner {
                desired: TrayState::Idle,
                view: View::new(TrayState::Idle),
                activations: Vec::new(),
            }),
            wake,
            updates,
        }
    }
}
impl Shared {
    fn set_state(&self, state: TrayState) {
        let mut inner = self.inner.lock().unwrap();
        if inner.desired != state {
            inner.desired = state;
            let _ = self.wake.try_send(());
        }
    }
    fn acknowledge(&self, clicked: bool) {
        let mut inner = self.inner.lock().unwrap();
        if inner.desired == TrayState::Done {
            inner.desired = TrayState::Idle;
            let _ = self.wake.try_send(());
        }
        if clicked {
            inner.activations.retain(|tx| {
                !matches!(tx.try_send(()), Err(mpsc::TrySendError::Disconnected(_)))
            });
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct View {
    state: TrayState,
    frame: u8,
}
impl View {
    fn new(state: TrayState) -> Self {
        Self { state, frame: 0 }
    }
    fn animated(self) -> bool {
        matches!(self.state, TrayState::Working { .. })
            || (self.state == TrayState::Done && self.frame < 12)
    }
    fn advance(&mut self) {
        match self.state {
            TrayState::Working { .. } => self.frame = (self.frame + 1) % 8,
            TrayState::Done => self.frame = (self.frame + 1).min(12),
            _ => {}
        }
    }
    fn status(self) -> &'static str {
        if self.state == TrayState::Done && self.frame < 12 {
            "NeedsAttention"
        } else {
            "Active"
        }
    }
    fn tooltip(self) -> Tooltip {
        let text = match self.state {
            TrayState::Idle => "No agent activity".into(),
            TrayState::Working { count: 1 } => "1 agent run active".into(),
            TrayState::Working { count } => format!("{count} agent runs active"),
            TrayState::Done => "Agent run finished - click to acknowledge".into(),
            TrayState::Unknown => "Agent activity unknown".into(),
        };
        (String::new(), vec![], "Neoism".into(), text)
    }
}

struct Item(Arc<Shared>);
impl Item {
    fn view(&self) -> View {
        self.0.inner.lock().unwrap().view
    }
}
#[zbus::interface(name = "org.kde.StatusNotifierItem")]
impl Item {
    fn activate(&self, _x: i32, _y: i32) {
        self.0.acknowledge(true);
    }
    fn secondary_activate(&self, _x: i32, _y: i32) {
        self.0.acknowledge(true);
    }
    fn context_menu(&self, _x: i32, _y: i32) {}
    fn scroll(&self, _delta: i32, _orientation: &str) {}
    #[zbus(property)]
    fn category(&self) -> &str {
        "ApplicationStatus"
    }
    #[zbus(property)]
    fn id(&self) -> &str {
        "neoism"
    }
    #[zbus(property)]
    fn title(&self) -> &str {
        "Neoism"
    }
    #[zbus(property)]
    fn status(&self) -> &str {
        self.view().status()
    }
    #[zbus(property)]
    fn window_id(&self) -> u32 {
        0
    }
    #[zbus(property)]
    fn icon_name(&self) -> &str {
        ""
    }
    #[zbus(property)]
    fn icon_pixmap(&self) -> Pixmaps {
        pixmaps(self.view())
    }
    #[zbus(property)]
    fn overlay_icon_name(&self) -> &str {
        ""
    }
    #[zbus(property)]
    fn overlay_icon_pixmap(&self) -> Pixmaps {
        vec![]
    }
    #[zbus(property)]
    fn attention_icon_name(&self) -> &str {
        ""
    }
    #[zbus(property)]
    fn attention_icon_pixmap(&self) -> Pixmaps {
        pixmaps(self.view())
    }
    #[zbus(property)]
    fn attention_movie_name(&self) -> &str {
        ""
    }
    #[zbus(property)]
    fn tool_tip(&self) -> Tooltip {
        self.view().tooltip()
    }
    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn menu(&self) -> OwnedObjectPath {
        OwnedObjectPath::try_from("/").unwrap()
    }
    #[zbus(signal)]
    async fn new_icon(ctxt: &zbus::SignalContext<'_>) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn new_attention_icon(ctxt: &zbus::SignalContext<'_>) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn new_tool_tip(ctxt: &zbus::SignalContext<'_>) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn new_status(ctxt: &zbus::SignalContext<'_>, status: &str)
        -> zbus::Result<()>;
}

async fn register(connection: &Connection) {
    let result = async {
        let proxy =
            Proxy::new(connection, WATCHER, "/StatusNotifierWatcher", WATCHER).await?;
        proxy
            .call::<_, _, ()>("RegisterStatusNotifierItem", &(PATH,))
            .await
    }
    .await;
    // Missing watchers are normal. Subscribe before registering to avoid races.
    let _ = result;
}
async fn watch(connection: &Connection) -> zbus::Result<()> {
    let dbus = Proxy::new(
        connection,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    )
    .await?;
    let mut changes = dbus
        .receive_signal_with_args("NameOwnerChanged", &[(0, WATCHER)])
        .await?;
    register(connection).await;
    while let Some(message) = changes.next().await {
        if let Ok((_, _, new)) = message.body().deserialize::<(String, String, String)>()
        {
            if !new.is_empty() {
                register(connection).await;
            }
        }
    }
    Ok(())
}
async fn run(shared: Arc<Shared>) -> zbus::Result<()> {
    // No UI runtime required. All bus setup and I/O is cancellable by dropping
    // this future. No detached watcher retains a connection after cancellation.
    let connection = Builder::session()?
        .serve_at(PATH, Item(shared.clone()))?
        .build()
        .await?;
    future::race(watch(&connection), animate(&connection, &shared)).await
}
async fn animate(connection: &Connection, shared: &Shared) -> zbus::Result<()> {
    let mut deadline = Instant::now();
    loop {
        let (old, view) = {
            let mut inner = shared.inner.lock().unwrap();
            let old = inner.view;
            if inner.desired != inner.view.state {
                inner.view = View::new(inner.desired);
                deadline = Instant::now() + TICK;
            } else if inner.view.animated() && Instant::now() >= deadline {
                inner.view.advance();
                deadline = Instant::now() + TICK;
            }
            (old, inner.view)
        };
        if old != view {
            // Also honor PropertiesChanged for clients using standard D-Bus
            // property caches; SNI-specific hosts listen to New* below.
            let interface = connection
                .object_server()
                .interface::<_, Item>(PATH)
                .await?;
            let item = interface.get().await;
            item.icon_pixmap_changed(interface.signal_context()).await?;
            item.attention_icon_pixmap_changed(interface.signal_context())
                .await?;
            if old.state != view.state {
                item.tool_tip_changed(interface.signal_context()).await?;
            }
            if old.status() != view.status() {
                item.status_changed(interface.signal_context()).await?;
            }
            drop(item);
            connection
                .emit_signal(None::<&str>, PATH, IFACE, "NewIcon", &())
                .await?;
            connection
                .emit_signal(None::<&str>, PATH, IFACE, "NewAttentionIcon", &())
                .await?;
            if old.state != view.state {
                connection
                    .emit_signal(None::<&str>, PATH, IFACE, "NewToolTip", &())
                    .await?;
            }
            if old.status() != view.status() {
                connection
                    .emit_signal(
                        None::<&str>,
                        PATH,
                        IFACE,
                        "NewStatus",
                        &(view.status(),),
                    )
                    .await?;
            }
        }
        if view.animated() {
            future::race(
                async {
                    let _ = shared.updates.recv().await;
                },
                async {
                    async_io::Timer::at(deadline).await;
                },
            )
            .await;
        } else {
            let _ = shared.updates.recv().await;
        }
    }
}

// Pixel-aligned N wordmark plus a shape-coded badge. SNI byte order is A,R,G,B
// (network order), NOT native-endian u32. Both host scale choices are embedded.
fn pixmaps(view: View) -> Pixmaps {
    [32, 64]
        .into_iter()
        .map(|size| {
            let mut bytes = vec![0; size * size * 4];
            let flash =
                view.state == TrayState::Done && view.frame < 12 && view.frame % 4 < 2;
            for y in 0..size {
                for x in 0..size {
                    let (px, py) = (x * 32 / size, y * 32 / size);
                    let n = (5..21).contains(&py)
                        && ((3..6).contains(&px)
                            || (15..18).contains(&px)
                            || (px >= 3 + (py - 5) * 12 / 16
                                && px < 6 + (py - 5) * 12 / 16));
                    let (bx, by) = (px as i32 - 19, py as i32 - 19);
                    let badge = if (0..12).contains(&bx) && (0..12).contains(&by) {
                        match view.state {
                            TrayState::Idle => {
                                (3..9).contains(&bx) && (5..7).contains(&by)
                            }
                            TrayState::Unknown => glyph(
                                &[
                                    ".####.", "##..##", "....##", "...##.", "..##..",
                                    "......", "..##..",
                                ],
                                bx,
                                by,
                            ),
                            TrayState::Done => glyph(
                                &[
                                    ".....#", "....##", "#..##.", "####..", ".##...",
                                    "......", "......",
                                ],
                                bx,
                                by,
                            ),
                            TrayState::Working { .. } => {
                                let points = [
                                    (6, 1),
                                    (9, 2),
                                    (10, 6),
                                    (9, 9),
                                    (6, 10),
                                    (2, 9),
                                    (1, 6),
                                    (2, 2),
                                ];
                                points.iter().enumerate().any(|(i, &(cx, cy))| {
                                    i != view.frame as usize
                                        && (bx - cx).abs() <= 1
                                        && (by - cy).abs() <= 1
                                })
                            }
                        }
                    } else {
                        false
                    };
                    let color = if badge {
                        if flash {
                            [255, 255, 255, 255]
                        } else {
                            [255, 115, 210, 245]
                        }
                    } else if n {
                        if flash {
                            [255, 115, 210, 245]
                        } else {
                            [255, 225, 230, 240]
                        }
                    } else {
                        [0, 0, 0, 0]
                    };
                    bytes[(y * size + x) * 4..(y * size + x + 1) * 4]
                        .copy_from_slice(&color);
                }
            }
            (size as i32, size as i32, bytes)
        })
        .collect()
}
#[cfg(test)]
mod tests;

fn glyph(rows: &[&str], x: i32, y: i32) -> bool {
    rows.get(y as usize)
        .and_then(|row| row.as_bytes().get((x / 2) as usize))
        .is_some_and(|b| *b == b'#')
}
