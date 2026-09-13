//! Literal Unicode dispatch only. No layout mapping, modifiers, clipboard, or
//! implicit Return/Tab events. Progress counts dispatched units, not insertion.
use anyhow::{Context, Result, ensure};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnitKind {
    Utf16CodeUnits,
    UnicodeScalars,
}
pub(crate) struct NativePlan {
    packets: Vec<Vec<u16>>,
    #[cfg(target_os = "macos")]
    events: Vec<(core_graphics::event::CGEvent, core_graphics::event::CGEvent)>,
}
impl NativePlan {
    pub(crate) fn total_units(&self) -> usize {
        if cfg!(target_os = "windows") {
            self.packets.iter().map(Vec::len).sum()
        } else {
            self.packets.len()
        }
    }
    pub(crate) fn unit_kind(&self) -> UnitKind {
        if cfg!(target_os = "windows") {
            UnitKind::Utf16CodeUnits
        } else {
            UnitKind::UnicodeScalars
        }
    }
}
#[derive(Debug)]
pub(crate) struct NativeError {
    /// Some events were dispatched or a surrogate pair was only partly sent.
    pub uncertain: bool,
    pub cleanup_failed: bool,
    source: anyhow::Error,
}
impl std::fmt::Display for NativeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Native Unicode dispatch failed (uncertain={}, cleanup_failed={}): {:#}",
            self.uncertain, self.cleanup_failed, self.source
        )
    }
}
impl std::error::Error for NativeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}
fn validate(text: &str) -> Result<Vec<Vec<u16>>> {
    ensure!(
        text.chars().count() <= 512 && text.len() <= 2048,
        "Native text exceeds 512 scalars / 2048 bytes"
    );
    // Validate the ENTIRE input before allocating/posting any native event.
    ensure!(
        !text.chars().any(char::is_control),
        "Literal native text rejects controls; use explicit key actions"
    );
    Ok(text
        .chars()
        .map(|ch| ch.encode_utf16(&mut [0; 2]).to_vec())
        .collect())
}

pub(crate) fn prepare_native(text: &str) -> Result<NativePlan> {
    let packets = validate(text)?;
    readiness()?;
    #[cfg(target_os = "macos")]
    {
        use core_graphics::{
            event::{CGEvent, CGEventFlags},
            event_source::{CGEventSource, CGEventSourceStateID},
        };
        let source = CGEventSource::new(CGEventSourceStateID::Private)
            .map_err(|_| anyhow::anyhow!("Cannot create Unicode event source"))?;
        let mut events = Vec::with_capacity(packets.len());
        for ch in text.chars() {
            let down = CGEvent::new_keyboard_event(source.clone(), 0, true)
                .map_err(|_| anyhow::anyhow!("Cannot prepare Unicode key-down"))?;
            let up = CGEvent::new_keyboard_event(source.clone(), 0, false)
                .map_err(|_| anyhow::anyhow!("Cannot prepare Unicode key-up"))?;
            down.set_string(&ch.to_string());
            up.set_string(&ch.to_string());
            down.set_flags(CGEventFlags::CGEventFlagNull);
            up.set_flags(CGEventFlags::CGEventFlagNull);
            events.push((down, up));
        }
        return Ok(NativePlan { packets, events });
    }
    #[cfg(not(target_os = "macos"))]
    Ok(NativePlan { packets })
}

#[cfg(target_os = "windows")]
fn readiness() -> Result<()> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    // Read-only checks. UIPI cannot be conclusively predicted: SendInput's result
    // remains authoritative for dispatch, never application acceptance.
    #[link(name = "user32")]
    unsafe extern "system" {
        fn OpenInputDesktop(
            flags: u32,
            inherit: i32,
            access: u32,
        ) -> *mut std::ffi::c_void;
        fn CloseDesktop(desktop: *mut std::ffi::c_void) -> i32;
    }
    let desktop = unsafe { OpenInputDesktop(0, 0, 1) };
    ensure!(!desktop.is_null(), "Interactive input desktop unavailable");
    unsafe {
        CloseDesktop(desktop);
    }
    for key in [0x10, 0x11, 0x12, 0x5b, 0x5c] {
        ensure!(
            unsafe { GetAsyncKeyState(key) } >= 0,
            "Release held modifiers before native typing"
        );
    }
    Ok(())
}
#[cfg(target_os = "macos")]
fn readiness() -> Result<()> {
    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }
    ensure!(
        unsafe { AXIsProcessTrusted() },
        "Accessibility permission is required for native typing"
    );
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventSourceFlagsState(state: i32) -> u64;
    }
    // Combined session state; do not synthesize or release the user's modifiers.
    ensure!(
        unsafe { CGEventSourceFlagsState(0) }
            & ((1 << 17) | (1 << 18) | (1 << 19) | (1 << 20))
            == 0,
        "Release held modifiers before native typing"
    );
    Ok(())
}
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn readiness() -> Result<()> {
    anyhow::bail!("Native Unicode typing is supported only on Windows/macOS")
}

// Never format or propagate an arbitrary panic payload. Even its destructor
// can panic, so dispose of it behind a second boundary. Do not replace the
// process-wide panic hook here (other threads may be using it).
struct CallFailure {
    source: anyhow::Error,
    panicked: bool,
}
fn invoke(
    stage: &'static str,
    call: impl FnOnce() -> Result<()>,
) -> std::result::Result<(), CallFailure> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(call)) {
        Ok(result) => result.map_err(|source| CallFailure {
            source,
            panicked: false,
        }),
        Err(payload) => {
            if let Err(nested) =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(payload)))
            {
                // An adversarial destructor must not unwind through key cleanup.
                std::mem::forget(nested);
            }
            Err(CallFailure {
                source: anyhow::anyhow!("Native {} callback panicked", stage),
                panicked: true,
            })
        }
    }
}
fn failure(error: CallFailure, uncertain: bool, cleanup_failed: bool) -> anyhow::Error {
    NativeError {
        uncertain,
        cleanup_failed,
        source: error.source,
    }
    .into()
}

// One press/release at a time. A normally rejected down MUST NOT cause an
// unmatched up. A panicking down has UNKNOWN acceptance and requires cleanup.
// Every callback (including cleanup and progress) has its own unwind boundary;
// no callback can unwind past an outstanding accepted/possibly accepted down.
// Cleanup gets exactly one attempt, without consulting guards/readiness: it
// releases only our Unicode packet, never the user's physical modifiers.
fn dispatch(
    count: usize,
    mut send: impl FnMut(usize, bool) -> Result<()>,
    guard: &mut dyn FnMut() -> Result<()>,
    progress: &mut dyn FnMut(usize),
) -> Result<()> {
    let mut any = false;
    for index in 0..count {
        if let Err(error) = invoke("guard", &mut *guard) {
            return Err(failure(error, any, false));
        }
        if let Err(error) = invoke("key-down", || send(index, true)) {
            let uncertain = any || error.panicked;
            let cleanup_failed = error.panicked
                && invoke("cleanup key-up", || send(index, false)).is_err();
            return Err(failure(error, uncertain, cleanup_failed));
        }
        any = true;
        if let Err(error) = invoke("guard", &mut *guard)
            .and_then(|()| invoke("key-up", || send(index, false)))
        {
            let cleanup_failed = invoke("cleanup key-up", || send(index, false)).is_err();
            return Err(failure(error, true, cleanup_failed));
        }
        // The unit is already released: a progress panic must not send another up
        // or retry either the callback or the keypress.
        if let Err(error) = invoke("progress", || {
            progress(1);
            Ok(())
        }) {
            return Err(failure(error, true, false));
        }
    }
    Ok(())
}

fn check_boundary(
    guard: &mut dyn FnMut() -> Result<()>,
    probe: &mut dyn FnMut() -> Result<()>,
) -> Result<()> {
    guard()?;
    probe().context("Native typing readiness changed")
}
pub(crate) fn execute_native(
    plan: &NativePlan,
    guard: &mut dyn FnMut() -> Result<()>,
    progress: &mut dyn FnMut(usize),
) -> Result<()> {
    // Check after the caller guard, immediately before each native event. A
    // physical modifier can still race any OS check, but never knowingly proceed
    // once one is observed. Cleanup intentionally bypasses this gate.
    let mut checked_guard = || check_boundary(guard, &mut readiness);
    invoke("guard", &mut checked_guard).map_err(|error| failure(error, false, false))?;
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
        let units: Vec<u16> = plan.packets.iter().flatten().copied().collect();
        return dispatch(
            units.len(),
            |index, down| {
                let event = INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: 0,
                            wScan: units[index],
                            dwFlags: KEYEVENTF_UNICODE
                                | if down { 0 } else { KEYEVENTF_KEYUP },
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                };
                ensure!(
                    unsafe { SendInput(1, &event, std::mem::size_of::<INPUT>() as i32) }
                        == 1,
                    "Unicode SendInput rejected; UIPI/desktop restrictions may apply"
                );
                Ok(())
            },
            &mut checked_guard,
            progress,
        );
    }
    #[cfg(target_os = "macos")]
    {
        return dispatch(
            plan.events.len(),
            |index, down| {
                let pair = &plan.events[index];
                (if down { &pair.0 } else { &pair.1 })
                    .post(core_graphics::event::CGEventTapLocation::HID);
                Ok(()) // CGEventPost has no delivery/acceptance acknowledgement.
            },
            &mut checked_guard,
            progress,
        );
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = (plan, progress);
        anyhow::bail!("Native Unicode typing unsupported on this OS")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn whole_text_validation() {
        for text in [
            "prefix\n",
            "prefix\r",
            "prefix\t",
            "prefix\0",
            "prefix\u{85}",
        ] {
            assert!(validate(text).is_err());
        }
        assert_eq!(
            validate("A😀é").unwrap(),
            vec![vec![65], vec![0xd83d, 0xde00], vec![233]]
        );
    }
    #[test]
    fn rejected_down_has_no_up() {
        let mut calls = vec![];
        let e = dispatch(
            2,
            |i, d| {
                calls.push((i, d));
                anyhow::bail!("reject")
            },
            &mut || Ok(()),
            &mut |_| panic!(),
        )
        .unwrap_err();
        assert_eq!(calls, vec![(0, true)]);
        assert!(!e.downcast_ref::<NativeError>().unwrap().uncertain);
    }
    #[test]
    fn partial_surrogate_is_uncertain() {
        let mut n = 0;
        let e = dispatch(
            2,
            |i, _| {
                ensure!(i == 0, "reject second surrogate");
                Ok(())
            },
            &mut || Ok(()),
            &mut |delta| n += delta,
        )
        .unwrap_err();
        assert_eq!(n, 1);
        assert!(e.downcast_ref::<NativeError>().unwrap().uncertain);
    }
    #[test]
    fn failed_release_cleanup() {
        let mut calls = vec![];
        let e = dispatch(
            1,
            |i, d| {
                calls.push((i, d));
                ensure!(d, "release failed");
                Ok(())
            },
            &mut || Ok(()),
            &mut |_| panic!(),
        )
        .unwrap_err();
        assert_eq!(calls, vec![(0, true), (0, false), (0, false)]);
        assert!(e.downcast_ref::<NativeError>().unwrap().cleanup_failed);
    }
    #[test]
    fn cancellation_releases() {
        let mut checks = 0;
        let mut calls = vec![];
        let e = dispatch(
            1,
            |i, d| {
                calls.push((i, d));
                Ok(())
            },
            &mut || {
                checks += 1;
                ensure!(checks == 1, "cancel");
                Ok(())
            },
            &mut |_| panic!(),
        )
        .unwrap_err();
        assert_eq!(calls, vec![(0, true), (0, false)]);
        assert!(e.downcast_ref::<NativeError>().unwrap().uncertain);
    }
    fn assert_panic_error(error: &anyhow::Error, cleanup_failed: bool) {
        let native = error.downcast_ref::<NativeError>().unwrap();
        assert!(native.uncertain);
        assert_eq!(native.cleanup_failed, cleanup_failed);
        assert!(format!("{error:#}").contains("callback panicked"));
        assert!(!format!("{error:#}").contains("private payload"));
    }
    #[test]
    fn guard_panic_after_down_cleans_up_once() {
        let mut checks = 0;
        let mut calls = vec![];
        let error = dispatch(
            2,
            |i, down| {
                calls.push((i, down));
                Ok(())
            },
            &mut || {
                checks += 1;
                if checks == 2 {
                    panic!("private payload");
                }
                Ok(())
            },
            &mut |_| panic!("progress must not run"),
        )
        .unwrap_err();
        assert_eq!(calls, vec![(0, true), (0, false)]);
        assert_panic_error(&error, false);
    }
    #[test]
    fn down_panic_has_unknown_acceptance_and_one_cleanup() {
        let mut calls = vec![];
        let error = dispatch(
            2,
            |i, down| {
                calls.push((i, down));
                if down {
                    panic!("private payload");
                }
                Ok(())
            },
            &mut || Ok(()),
            &mut |_| panic!("progress must not run"),
        )
        .unwrap_err();
        assert_eq!(calls, vec![(0, true), (0, false)]);
        assert_panic_error(&error, false);
    }
    #[test]
    fn release_panic_has_only_one_additional_cleanup_attempt() {
        for cleanup_panics in [false, true] {
            let mut calls = vec![];
            let error = dispatch(
                2,
                |i, down| {
                    calls.push((i, down));
                    if !down && (calls.len() == 2 || cleanup_panics) {
                        panic!("private payload");
                    }
                    Ok(())
                },
                &mut || Ok(()),
                &mut |_| panic!("progress must not run"),
            )
            .unwrap_err();
            assert_eq!(calls, vec![(0, true), (0, false), (0, false)]);
            assert_panic_error(&error, cleanup_panics);
        }
    }
    #[test]
    fn progress_panic_does_not_repeat_released_unit_or_send_extra_up() {
        let mut calls = vec![];
        let mut deltas = vec![];
        let error = dispatch(
            2,
            |i, down| {
                calls.push((i, down));
                Ok(())
            },
            &mut || Ok(()),
            &mut |delta| {
                deltas.push(delta);
                panic!("private payload");
            },
        )
        .unwrap_err();
        assert_eq!(calls, vec![(0, true), (0, false)]);
        assert_eq!(deltas, vec![1]);
        assert_panic_error(&error, false);
    }
    #[test]
    fn guard_panic_before_down_sends_nothing() {
        let error = dispatch(
            2,
            |_, _| panic!("must not send"),
            &mut || panic!("private payload"),
            &mut |_| panic!("must not report"),
        )
        .unwrap_err();
        let native = error.downcast_ref::<NativeError>().unwrap();
        assert!(!native.uncertain);
        assert!(!native.cleanup_failed);
    }
    #[test]
    fn readiness_change_stops_at_each_boundary_and_only_releases_our_packet() {
        // Boundary 2: after first down. Boundary 3: before second down.
        for fail_at in [2, 3] {
            let mut checks = 0;
            let mut calls = vec![];
            let mut deltas = vec![];
            let mut probe = || {
                checks += 1;
                ensure!(checks < fail_at, "physical modifier appeared");
                Ok(())
            };
            let error = dispatch(
                3,
                |i, down| {
                    calls.push((i, down));
                    Ok(())
                },
                &mut || check_boundary(&mut || Ok(()), &mut probe),
                &mut |delta| deltas.push(delta),
            )
            .unwrap_err();
            assert_eq!(checks, fail_at); // Cleanup must NOT run readiness again.
            assert_eq!(calls, vec![(0, true), (0, false)]);
            assert_eq!(deltas, if fail_at == 2 { vec![] } else { vec![1] });
            assert!(error.downcast_ref::<NativeError>().unwrap().uncertain);
        }
    }
    #[test]
    fn successful_progress_remains_delta_units() {
        let mut calls = vec![];
        let mut deltas = vec![];
        dispatch(
            2,
            |i, down| {
                calls.push((i, down));
                Ok(())
            },
            &mut || Ok(()),
            &mut |delta| deltas.push(delta),
        )
        .unwrap();
        assert_eq!(calls, vec![(0, true), (0, false), (1, true), (1, false)]);
        assert_eq!(deltas, vec![1, 1]);
    }
}
