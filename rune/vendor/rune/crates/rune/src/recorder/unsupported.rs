//! Platforms without a recorder yet: starting fails with a clear message. Fake microphones
//! still work everywhere.
use super::{MicPermission, Options, Shared};
use std::path::Path;
use std::sync::Arc;

pub(crate) enum Session {}

pub(super) fn permission() -> MicPermission {
    MicPermission::Restricted
}

pub(super) fn request_permission(callback: Box<dyn FnOnce(MicPermission) + Send>) {
    callback(MicPermission::Restricted)
}

pub(super) fn preflight() -> Result<(), String> {
    Err("recording isn't supported on this platform".into())
}

pub(super) fn start(_: &Arc<Shared>, _: u64, _: &Path, _: &Options) -> Result<Session, String> {
    Err("recording isn't supported on this platform".into())
}

pub(super) fn pause(session: &Session) {
    match *session {}
}

pub(super) fn resume(session: &Session) {
    match *session {}
}

pub(super) fn stop(session: Session) -> Result<f64, String> {
    match session {}
}

pub(super) fn finish(session: Session) {
    match session {}
}

pub(super) fn cancel(session: Session) {
    match session {}
}
