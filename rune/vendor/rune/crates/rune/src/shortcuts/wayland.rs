//! xdg-desktop-portal GlobalShortcuts. Each binding owns a session so unregister works on
//! portal v1 (which cannot remove a binding from an existing session).
use super::{Backend, Shortcut, ShortcutError, ShortcutId, Sink};
use futures_util::StreamExt;
use std::{collections::HashMap, sync::mpsc, thread::JoinHandle, time::Duration};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use zbus::{Connection, OwnedObjectPath, OwnedVariant, Proxy, Variant, fdo};

const DEST: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const INTERFACE: &str = "org.freedesktop.portal.GlobalShortcuts";
const RPC_TIMEOUT: Duration = Duration::from_secs(5);
const CONSENT_TIMEOUT: Duration = Duration::from_secs(120);
type Reply = mpsc::SyncSender<Result<(), ShortcutError>>;
enum Command { Register(ShortcutId, Shortcut, Reply), Unregister(ShortcutId, Reply) }
pub(super) struct Native { commands: Option<UnboundedSender<Command>>, worker: Option<JoinHandle<()>> }

fn dbus_error(e: zbus::Error) -> ShortcutError {
    // zbus reports the standard org.freedesktop.DBus.Error.* replies as typed FDO errors.
    if let zbus::Error::FDO(fdo) = &e
        && matches!(**fdo, fdo::Error::ServiceUnknown(_) | fdo::Error::NameHasNoOwner(_) | fdo::Error::UnknownInterface(_)
            | fdo::Error::UnknownMethod(_) | fdo::Error::UnknownObject(_)) {
        return ShortcutError::Unsupported;
    }
    if let zbus::Error::MethodError(name, _, _) = &e {
        return match name.as_str() {
            "org.freedesktop.portal.Error.Exist" => ShortcutError::Conflict,
            "org.freedesktop.portal.Error.NotAllowed" | "org.freedesktop.portal.Error.Cancelled" => ShortcutError::PermissionDenied,
            "org.freedesktop.portal.Error.InvalidArgument" => ShortcutError::InvalidAccelerator(e.to_string()),
            _ => ShortcutError::Backend(e.to_string()),
        };
    }
    ShortcutError::Backend(e.to_string())
}
fn response_status(status: u32) -> Result<(), ShortcutError> {
    match status { 0 => Ok(()), 1 => Err(ShortcutError::PermissionDenied), n => Err(ShortcutError::Backend(format!("portal response {n}"))) }
}
fn timed_out() -> ShortcutError { ShortcutError::Backend("portal request timed out".into()) }

async fn request<B: serde::Serialize + zbus::Type>(conn: &Connection, portal: &Proxy<'_>, token: &str,
    method: &str, body: &B) -> Result<HashMap<String, OwnedVariant>, ShortcutError> {
    let sender = conn.unique_name().ok_or(ShortcutError::Unsupported)?.trim_start_matches(':').replace('.', "_");
    let path = format!("{PATH}/request/{sender}/{token}");
    let proxy = Proxy::new(conn, DEST, path.as_str(), "org.freedesktop.portal.Request").await.map_err(dbus_error)?;
    // Subscribe before making the request: a portal can respond before the method returns.
    let mut responses = proxy.receive_signal("Response").await.map_err(dbus_error)?;
    let result = tokio::time::timeout(CONSENT_TIMEOUT, async {
        let returned: OwnedObjectPath = portal.call(method, body).await.map_err(dbus_error)?;
        if returned.as_str() != path { return Err(ShortcutError::Backend("portal returned an unexpected request path".into())); }
        let response = responses.next().await.ok_or_else(|| ShortcutError::Backend("portal disconnected".into()))?;
        let (status, values): (u32, HashMap<String, OwnedVariant>) = response.body().deserialize().map_err(dbus_error)?;
        response_status(status)?;
        Ok(values)
    }).await.unwrap_or_else(|_| Err(timed_out()));
    if result.is_err() {
        let _ = tokio::time::timeout(RPC_TIMEOUT, proxy.call::<_, _, ()>("Close", &())).await;
    }
    result
}

async fn close(conn: &Connection, session: &OwnedObjectPath) -> Result<(), ShortcutError> {
    tokio::time::timeout(RPC_TIMEOUT, async {
        let proxy = Proxy::new(conn, DEST, session.as_str(), "org.freedesktop.portal.Session").await.map_err(dbus_error)?;
        proxy.call::<_, _, ()>("Close", &()).await.map_err(dbus_error)
    }).await.unwrap_or_else(|_| Err(timed_out()))
}

async fn bind(conn: &Connection, portal: &Proxy<'_>, id: ShortcutId, shortcut: &Shortcut) -> Result<OwnedObjectPath, ShortcutError> {
    let create = format!("rune_{}_create", id.0);
    let session_token = format!("rune_{}_session", id.0);
    let options = HashMap::from([("handle_token", Variant::from(create.as_str())), ("session_handle_token", Variant::from(session_token.as_str()))]);
    let response = request(conn, portal, &create, "CreateSession", &(options,)).await?;
    let value = response.get("session_handle").ok_or_else(|| ShortcutError::Backend("portal omitted session_handle".into()))?;
    let session = if let Ok(path) = <&str>::try_from(value) {
        OwnedObjectPath::try_from(path.to_owned()).map_err(|e| ShortcutError::Backend(e.to_string()))?
    } else { OwnedObjectPath::try_from(value.try_clone().map_err(|e| ShortcutError::Backend(e.to_string()))?)
        .map_err(|e| ShortcutError::Backend(e.to_string()))? };
    let bind_token = format!("rune_{}_bind", id.0);
    let options = HashMap::from([("handle_token", Variant::from(bind_token.as_str()))]);
    let trigger = shortcut.portal_trigger();
    let description = format!("Shortcut {shortcut}");
    let info = HashMap::from([("description", Variant::from(description.as_str())), ("preferred_trigger", Variant::from(trigger.as_str()))]);
    let shortcuts = vec![(id.0.to_string(), info)];
    match request(conn, portal, &bind_token, "BindShortcuts", &(&session, shortcuts, "", options)).await {
        Ok(response) => {
            // Success with no matching binding is not a registration. The compositor chooses
            // the actual trigger; a preferred-trigger conflict is not standardized by the portal.
            let bindings = response.get("shortcuts").and_then(|v| v.try_clone().ok())
                .and_then(|v| Vec::<(String, HashMap<String, OwnedVariant>)>::try_from(v).ok());
            if bindings.as_ref().is_some_and(|v| v.iter().any(|(key, _)| key == &id.0.to_string())) { Ok(session) }
            else { let _ = close(conn, &session).await; Err(ShortcutError::PermissionDenied) }
        }
        Err(e) => { let _ = close(conn, &session).await; Err(e) }
    }
}

impl Native {
    pub(super) fn new(sink: Sink) -> Result<Self, ShortcutError> {
        let (commands, mut receiver) = unbounded_channel();
        let (ready, result) = mpsc::sync_channel(1);
        let worker = std::thread::Builder::new().name("rune-shortcuts-portal".into()).spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(r) => r, Err(e) => { let _ = ready.send(Err(ShortcutError::Backend(e.to_string()))); return; }
            };
            runtime.block_on(async move {
                let setup = tokio::time::timeout(RPC_TIMEOUT, async {
                    // No session bus means this desktop cannot supply the portal.
                    let conn = Connection::session().await.map_err(|_| ShortcutError::Unsupported)?;
                    let portal = Proxy::new(&conn, DEST, PATH, INTERFACE).await.map_err(dbus_error)?;
                    // Some portal frontends report InvalidArgs rather than UnknownInterface
                    // when a backend for this interface is absent. The property is fixed here.
                    let _: u32 = portal.get_property("version").await.map_err(|_| ShortcutError::Unsupported)?;
                    let events = portal.receive_signal("Activated").await.map_err(dbus_error)?;
                    Ok::<_, ShortcutError>((conn, portal, events))
                }).await.unwrap_or(Err(ShortcutError::Unsupported));
                let (conn, portal, mut events) = match setup {
                    Ok(v) => { let _ = ready.send(Ok(())); v },
                    Err(e) => { let _ = ready.send(Err(e)); return; }
                };
                let mut sessions: HashMap<ShortcutId, OwnedObjectPath> = HashMap::new();
                loop {
                    tokio::select! {
                        command = receiver.recv() => match command {
                            Some(Command::Register(id, shortcut, reply)) => {
                                let result = bind(&conn, &portal, id, &shortcut).await.map(|session| { sessions.insert(id, session); });
                                let _ = reply.send(result);
                            }
                            Some(Command::Unregister(id, reply)) => {
                                let result = match sessions.get(&id) {
                                    Some(session) => close(&conn, session).await,
                                    None => Err(ShortcutError::UnknownId),
                                };
                                if result.is_ok() { sessions.remove(&id); }
                                let _ = reply.send(result);
                            }
                            None => break,
                        },
                        event = events.next() => {
                            let Some(event) = event else { break };
                            if let Ok((session, key, _, options)) = event.body().deserialize::<(OwnedObjectPath, String, u64, HashMap<String, OwnedVariant>)>()
                                && let Ok(id) = key.parse::<u32>().map(ShortcutId)
                                && sessions.get(&id) == Some(&session) {
                                let token = options.get("activation_token").and_then(|v| <&str>::try_from(v).ok()).map(str::to_owned);
                                sink(id, token);
                            }
                        }
                    }
                }
                for session in sessions.values() { let _ = close(&conn, session).await; }
                let _ = conn.close().await;
            });
        }).map_err(|e| ShortcutError::Backend(e.to_string()))?;
        let mut native = Self { commands: Some(commands), worker: Some(worker) };
        if let Err(e) = result.recv().unwrap_or(Err(ShortcutError::Unsupported)) {
            native.commands.take();
            return Err(e);
        }
        Ok(native)
    }
    fn send(&self, command: impl FnOnce(Reply) -> Command) -> Result<(), ShortcutError> {
        let (reply, result) = mpsc::sync_channel(1);
        self.commands.as_ref().ok_or(ShortcutError::NotRunning)?.send(command(reply)).map_err(|_| ShortcutError::Backend("portal disconnected".into()))?;
        result.recv().unwrap_or_else(|_| Err(ShortcutError::Backend("portal disconnected".into())))
    }
}
impl Backend for Native {
    fn register(&mut self, id: ShortcutId, shortcut: &Shortcut) -> Result<(), ShortcutError> { self.send(|reply| Command::Register(id, shortcut.clone(), reply)) }
    fn unregister(&mut self, id: ShortcutId) -> Result<(), ShortcutError> { self.send(|reply| Command::Unregister(id, reply)) }
}
impl Drop for Native {
    fn drop(&mut self) {
        self.commands.take();
        if let Some(worker) = self.worker.take() { let _ = worker.join(); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn portal_cancellation_is_not_a_conflict() {
        assert_eq!(response_status(1), Err(ShortcutError::PermissionDenied));
        assert!(matches!(response_status(2), Err(ShortcutError::Backend(_))));
    }
    #[test]
    fn missing_portal_is_unsupported() {
        let fdo = |e| zbus::Error::FDO(Box::new(e));
        assert_eq!(dbus_error(fdo(fdo::Error::ServiceUnknown("portal".into()))), ShortcutError::Unsupported);
        assert_eq!(dbus_error(fdo(fdo::Error::UnknownMethod("BindShortcuts".into()))), ShortcutError::Unsupported);
        assert!(matches!(dbus_error(fdo(fdo::Error::Failed("x".into()))), ShortcutError::Backend(_)));
    }
}
