//! Platforms without a player yet: every clip fails with a clear message, and video draws the
//! neutral placeholder.
use super::*;

fn unsupported(kind: Kind) -> String {
    match kind {
        Kind::Video => "video playback isn't supported on this platform yet".into(),
        Kind::Audio => "audio playback isn't supported on this platform yet".into(),
    }
}

#[derive(Default)]
pub(crate) struct Native;

pub(super) fn created(shared: &Arc<Shared>) {
    shared.ctl.lock().unwrap().fail(unsupported(shared.kind));
}

pub(super) fn enable() {}

pub(super) fn probe(_: &mut Native, _: &MediaSource, _: &[(String, String)], kind: Kind) -> Result<Meta, String> {
    Err(unsupported(kind))
}

pub(super) fn decode(_: &mut Native, _: &MediaSource, _: &[(String, String)], _: f64) -> Result<ByteFrame, String> {
    Err(unsupported(Kind::Video))
}

pub(super) fn apply(_: &Arc<Shared>) {}

pub(super) fn drive(_: &Arc<Shared>) {}

pub(super) fn preload(_: &Arc<Shared>) {}

pub(super) fn live_time(_: &Shared) -> Option<f64> {
    None
}

pub(super) fn pull(_: &Arc<Shared>, _: &NativeSurface, _: Instant) -> bool {
    false
}
