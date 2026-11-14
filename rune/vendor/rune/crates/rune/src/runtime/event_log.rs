//! `Runtime::events`: input markers for `--trace` and headless inspection. A live window
//! records nothing unless tracing, and every runtime keeps at most `CAP` unread markers.

/// Unread markers kept before the oldest half is dropped.
pub(crate) const CAP: usize = 512;

/// Records a formatted marker in `rt.events` when anything reads them (a trace writer, or a
/// headless run: tests, scripts); formats nothing otherwise. Reads fields, not `&self`, so it
/// works while another field is borrowed.
macro_rules! log_event {
    ($rt:expr, $($fmt:tt)+) => {
        if $rt.record_events || $rt.trace_channels.is_some() {
            let event = format!($($fmt)+);
            $crate::runtime::event_log::push(&mut $rt.events, event);
        }
    };
}
pub(crate) use log_event;

pub(crate) fn push(events: &mut Vec<String>, event: String) {
    if events.len() >= CAP { events.drain(..CAP / 2); }
    events.push(event);
}
