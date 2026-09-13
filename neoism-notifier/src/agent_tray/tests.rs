use super::*;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

#[test]
fn animation_is_bounded_and_shape_coded() {
    let mut done = View::new(TrayState::Done);
    let mut changes = 0;
    let mut previous = pixmaps(done);
    for _ in 0..12 {
        assert!(done.animated());
        done.advance();
        let next = pixmaps(done);
        changes += usize::from(previous != next);
        previous = next;
    }
    assert_eq!(changes, 5); // three on phases, finishing on steady off phase
    assert!(!done.animated());
    assert_eq!(done.status(), "Active");
    assert!(!View::new(TrayState::Idle).animated());
    assert!(!View::new(TrayState::Unknown).animated());
    assert_ne!(pixmaps(done), pixmaps(View::new(TrayState::Unknown)));
    let working = View::new(TrayState::Working { count: 7 });
    assert!(working.tooltip().3.contains('7'));
    let mut next = working;
    next.advance();
    assert_ne!(pixmaps(working), pixmaps(next));
    for (w, h, pixels) in pixmaps(working) {
        assert_eq!(pixels.len(), (w * h * 4) as usize);
        assert!(pixels.chunks_exact(4).all(|p| p[0] == 0 || p[0] == 255));
    }
}

#[test]
fn updates_coalesce_and_clicks_do_not_cancel_work() {
    let shared = Arc::new(Shared::default());
    for count in 1..1000 {
        shared.set_state(TrayState::Working { count });
    }
    assert_eq!(shared.updates.len(), 1);
    assert_eq!(
        shared.inner.lock().unwrap().desired,
        TrayState::Working { count: 999 }
    );
    shared.acknowledge(true);
    assert_eq!(
        shared.inner.lock().unwrap().desired,
        TrayState::Working { count: 999 }
    );
    shared.set_state(TrayState::Done);
    shared.updates.try_recv().unwrap();
    shared.set_state(TrayState::Done);
    assert!(shared.updates.is_empty());
    let (tx, rx) = mpsc::sync_channel(1);
    shared.inner.lock().unwrap().activations.push(tx);
    shared.acknowledge(true);
    rx.try_recv().unwrap();
    assert_eq!(shared.inner.lock().unwrap().desired, TrayState::Idle);
    shared.set_state(TrayState::Unknown);
    shared.acknowledge(false);
    assert_eq!(shared.inner.lock().unwrap().desired, TrayState::Unknown);
}

#[test]
fn explicit_stop_cancels_even_pending_setup_without_joining() {
    let shared = Arc::new(Shared::default());
    let (stop, stopped) = async_channel::bounded(1);
    let tray = AgentTray(Arc::new(Handle { shared, stop }));
    let other = tray.clone();
    tray.stop();
    assert!(other.0.stop.is_closed());
    future::block_on(until_stopped(stopped, future::pending()));
}

struct Bus(Child, String);
impl Bus {
    fn new() -> Self {
        let mut child = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .expect("dbus-daemon required for private-bus protocol test");
        let mut address = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        Self(child, address.trim().into())
    }
}
impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct Watcher(async_channel::Sender<String>);
#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    async fn register_status_notifier_item(&self, service: String) {
        self.0.send(service).await.unwrap();
    }
}

#[test]
fn real_bus_properties_signals_registration_restart_and_disconnect() {
    let bus = Bus::new();
    future::block_on(future::race(
        async {
            let shared = Arc::new(Shared::default());
            let service = Builder::address(bus.1.as_str())
                .unwrap()
                .serve_at(PATH, Item(shared.clone()))
                .unwrap()
                .build()
                .await
                .unwrap();
            let name = service.unique_name().unwrap().to_string();
            let client = Builder::address(bus.1.as_str())
                .unwrap()
                .build()
                .await
                .unwrap();
            let proxy = Proxy::new(&client, name.as_str(), PATH, IFACE)
                .await
                .unwrap();
            assert_eq!(proxy.get_property::<String>("Id").await.unwrap(), "neoism");
            assert_eq!(
                proxy.get_property::<String>("Category").await.unwrap(),
                "ApplicationStatus"
            );
            assert_eq!(
                proxy.get_property::<String>("Title").await.unwrap(),
                "Neoism"
            );
            assert!(!proxy.get_property::<bool>("ItemIsMenu").await.unwrap());
            assert_eq!(proxy.get_property::<u32>("WindowId").await.unwrap(), 0);
            let _: OwnedObjectPath = proxy.get_property("Menu").await.unwrap();
            let icons: Pixmaps = proxy.get_property("IconPixmap").await.unwrap();
            assert_eq!(icons, pixmaps(View::new(TrayState::Idle)));
            let _: Tooltip = proxy.get_property("ToolTip").await.unwrap();
            let introspect = Proxy::new(
                &client,
                name.as_str(),
                PATH,
                "org.freedesktop.DBus.Introspectable",
            )
            .await
            .unwrap();
            let xml: String = introspect.call("Introspect", &()).await.unwrap();
            assert!(xml.contains("type=\"a(iiay)\""), "{xml}");
            assert!(xml.contains("type=\"(sa(iiay)ss)\""), "{xml}");
            assert!(xml.contains("name=\"NewToolTip\""), "{xml}");
            let (tx, rx) = async_channel::bounded(4);
            let watcher = Builder::address(bus.1.as_str())
                .unwrap()
                .serve_at("/StatusNotifierWatcher", Watcher(tx))
                .unwrap()
                .build()
                .await
                .unwrap();
            // Start without an owner, then simulate host arrival and restart.
            future::race(
                async {
                    async_io::Timer::after(Duration::from_millis(100)).await;
                    watcher.request_name(WATCHER).await.unwrap();
                    assert_eq!(rx.recv().await.unwrap(), PATH);
                    watcher.release_name(WATCHER).await.unwrap();
                    watcher.request_name(WATCHER).await.unwrap();
                    assert_eq!(rx.recv().await.unwrap(), PATH);
                },
                async {
                    watch(&service).await.unwrap();
                    panic!("watch ended");
                },
            )
            .await;
            let mut icons = proxy.receive_signal("NewIcon").await.unwrap();
            let mut statuses = proxy.receive_signal("NewStatus").await.unwrap();
            future::race(
                async {
                    shared.set_state(TrayState::Done);
                    icons.next().await.unwrap();
                    let status: (String,) =
                        statuses.next().await.unwrap().body().deserialize().unwrap();
                    assert_eq!(status.0, "NeedsAttention");
                    // Read directly, avoiding proxy property cache (SNI uses New* signals).
                    assert_eq!(Item(shared.clone()).status(), "NeedsAttention");
                    let flash: Pixmaps = proxy.get_property("IconPixmap").await.unwrap();
                    icons.next().await.unwrap();
                    icons.next().await.unwrap();
                    assert_ne!(
                        flash,
                        proxy.get_property::<Pixmaps>("IconPixmap").await.unwrap()
                    );
                    shared.set_state(TrayState::Working { count: 3 });
                    assert_eq!(
                        statuses
                            .next()
                            .await
                            .unwrap()
                            .body()
                            .deserialize::<(String,)>()
                            .unwrap()
                            .0,
                        "Active"
                    );
                    assert_eq!(
                        shared.inner.lock().unwrap().view.state,
                        TrayState::Working { count: 3 }
                    );
                    proxy
                        .call::<_, _, ()>("Activate", &(0i32, 0i32))
                        .await
                        .unwrap();
                    assert_eq!(
                        shared.inner.lock().unwrap().desired,
                        TrayState::Working { count: 3 }
                    );
                    shared.set_state(TrayState::Done);
                    proxy
                        .call::<_, _, ()>("Activate", &(0i32, 0i32))
                        .await
                        .unwrap();
                    assert_eq!(shared.inner.lock().unwrap().desired, TrayState::Idle);
                },
                async {
                    animate(&service, &shared).await.unwrap();
                    panic!("animation ended");
                },
            )
            .await;
            drop(icons);
            drop(statuses);
            let (stop, stopped) = async_channel::bounded(1);
            let handle = AgentTray(Arc::new(Handle {
                shared: shared.clone(),
                stop,
            }));
            let clone = handle.clone();
            drop(handle);
            assert!(
                !clone.0.stop.is_closed(),
                "non-last drop must preserve service"
            );
            future::zip(
                until_stopped(stopped, async move {
                    // The production cancellation wrapper must release the connection
                    // even when no animation or watcher event will wake it.
                    animate(&service, &shared).await.unwrap();
                }),
                async move {
                    async_io::Timer::after(Duration::from_millis(20)).await;
                    drop(clone);
                },
            )
            .await;
            let dbus = Proxy::new(
                &client,
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
            )
            .await
            .unwrap();
            // Verify service teardown really releases the bus connection.
            for _ in 0..100 {
                let owned: bool =
                    dbus.call("NameHasOwner", &(name.as_str(),)).await.unwrap();
                if !owned {
                    return;
                }
                async_io::Timer::after(Duration::from_millis(10)).await;
            }
            panic!("service connection retained after drop");
        },
        async {
            async_io::Timer::after(Duration::from_secs(10)).await;
            panic!("private-bus test timed out");
        },
    ));
}

/// Opt-in only: changes the actual session tray, never requests focus or pins it.
/// NEOISM_TRAY_LIVE=1 cargo test -p neoism-notifier live_smoke -- --ignored --nocapture
#[test]
#[ignore = "requires reviewed, explicitly authorized real desktop tray"]
fn live_smoke() {
    assert_eq!(std::env::var("NEOISM_TRAY_LIVE").as_deref(), Ok("1"));
    let tray = AgentTray::start().unwrap();
    let hold = std::env::var("NEOISM_TRAY_LIVE_HOLD_SECS")
        .ok().and_then(|s| s.parse::<u64>().ok()).unwrap_or(5).clamp(3, 30);
    tray.set_state(TrayState::Working { count: 3 });
    println!("LIVE WORKING pid={}", std::process::id());
    std::thread::sleep(Duration::from_secs(hold));
    // Discover our unique connection through the host's standard item list.
    let bus = zbus::blocking::Connection::session().unwrap();
    let watcher =
        zbus::blocking::Proxy::new(&bus, WATCHER, "/StatusNotifierWatcher", WATCHER)
            .unwrap();
    let items: Vec<String> = watcher
        .get_property("RegisteredStatusNotifierItems")
        .unwrap();
    let mut found = false;
    for entry in items {
        let Some((name, path)) = entry.split_once('/') else {
            continue;
        };
        let path = format!("/{path}");
        let dbus = zbus::blocking::Proxy::new(&bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus").unwrap();
        let owner: u32 = dbus.call("GetConnectionUnixProcessID", &(name,)).unwrap_or(0);
        if owner != std::process::id() { continue; }
        let Ok(item) = zbus::blocking::Proxy::new(&bus, name, path.as_str(), IFACE)
        else {
            continue;
        };
        if item.get_property::<String>("Id").ok().as_deref() != Some("neoism") {
            continue;
        }
        let _: Pixmaps = item.get_property("IconPixmap").unwrap();
        let tip: Tooltip = item.get_property("ToolTip").unwrap();
        assert!(tip.3.contains("3 agent"));
        let intro = zbus::blocking::Proxy::new(
            &bus,
            name,
            path.as_str(),
            "org.freedesktop.DBus.Introspectable",
        )
        .unwrap();
        let xml: String = intro.call("Introspect", &()).unwrap();
        println!("{entry}\n{xml}");
        found = true;
    }
    assert!(found, "Neoism did not register with the real host");
    tray.set_state(TrayState::Done);
    println!("LIVE DONE");
    std::thread::sleep(Duration::from_secs(hold.max(6)));
    tray.acknowledge();
    std::thread::sleep(Duration::from_secs(3));
    tray.stop();
}
