use corcovado::channel::{channel, Receiver, Sender};
use std::ffi::c_void;
use std::io::Error;
use std::num::NonZeroU32;

use windows_sys::Win32::Foundation::{
    CloseHandle, BOOLEAN, HANDLE, INVALID_HANDLE_VALUE, STILL_ACTIVE,
};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessId, RegisterWaitForSingleObject, UnregisterWaitEx,
    INFINITE, WT_EXECUTEINWAITTHREAD, WT_EXECUTEONLYONCE,
};

use crate::ChildEvent;

// The watcher owns this allocation. The callback only borrows it; even a
// one-shot registration must be unregistered before its context can be freed.
struct CallbackContext {
    event_tx: Sender<ChildEvent>,
    #[cfg(test)]
    drops: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(test)]
impl Drop for CallbackContext {
    fn drop(&mut self) {
        self.drops.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// WinAPI callback to run when child process exits.
extern "system" fn child_exit_callback(ctx: *mut c_void, timed_out: BOOLEAN) {
    if timed_out != 0 {
        return;
    }

    let context = unsafe { &*ctx.cast::<CallbackContext>() };
    // Unbounded send: no user code, watcher access, or application locks here.
    // In particular, never drop/unregister this watcher from its own callback.
    let _ = context.event_tx.send(ChildEvent::Exited);
}

pub struct ChildExitWatcher {
    wait_handle: HANDLE,
    context: Option<Box<CallbackContext>>,
    event_rx: Receiver<ChildEvent>,
    child_handle: HANDLE,
    pid: Option<NonZeroU32>,
}

// HANDLE is not Send, so Send is not derived automatically for ChildExitWatcher, but raw pointers
// are generally safe to send between threads as long as the type they deference to is Send, which
// c_void is. (see https://doc.rust-lang.org/nomicon/send-and-sync.html).
unsafe impl Send for ChildExitWatcher {}

impl ChildExitWatcher {
    /// Takes ownership of `child_handle` only on success. The caller must own
    /// that handle (duplicate it first if another owner, e.g. Child, retains it).
    pub fn new(child_handle: HANDLE) -> Result<ChildExitWatcher, Error> {
        Self::with_registration(child_handle, |process, context| {
            let mut wait_handle = std::ptr::null_mut();
            let success = unsafe {
                RegisterWaitForSingleObject(
                    &mut wait_handle,
                    process,
                    Some(child_exit_callback),
                    context,
                    INFINITE,
                    WT_EXECUTEINWAITTHREAD | WT_EXECUTEONLYONCE,
                )
            };
            if success == 0 {
                Err(Error::last_os_error())
            } else {
                Ok(wait_handle)
            }
        })
    }

    fn with_registration(
        child_handle: HANDLE,
        register: impl FnOnce(HANDLE, *mut c_void) -> Result<HANDLE, Error>,
    ) -> Result<Self, Error> {
        let (event_tx, event_rx) = channel::<ChildEvent>();
        let context = Box::new(CallbackContext {
            event_tx,
            #[cfg(test)]
            drops: Default::default(),
        });
        // Stable allocation, including if the callback runs before registration
        // returns. Failure does not register a callback, so ordinary Box cleanup
        // is correct and the caller still owns the process handle.
        let wait_handle = register(
            child_handle,
            (&*context as *const CallbackContext).cast_mut().cast(),
        )?;
        let pid = unsafe { NonZeroU32::new(GetProcessId(child_handle)) };
        Ok(Self {
            wait_handle,
            context: Some(context),
            event_rx,
            child_handle,
            pid,
        })
    }

    pub fn event_rx(&self) -> &Receiver<ChildEvent> {
        &self.event_rx
    }

    pub fn raw_handle(&self) -> HANDLE {
        self.child_handle
    }

    pub fn exit_code(&self) -> Result<Option<u32>, Error> {
        let mut code = 0;
        let success = unsafe { GetExitCodeProcess(self.child_handle, &mut code) };
        if success == 0 {
            return Err(Error::last_os_error());
        }

        if code == STILL_ACTIVE as u32 {
            Ok(None)
        } else {
            Ok(Some(code))
        }
    }

    pub fn pid(&self) -> Option<NonZeroU32> {
        self.pid
    }
}

impl Drop for ChildExitWatcher {
    fn drop(&mut self) {
        // INVALID_HANDLE_VALUE waits for in-flight callbacks, unlike UnregisterWait.
        // Safe here because our callback never drops the watcher or waits for
        // its owner; do not hold locks needed by the callback around this call.
        let success = unsafe { UnregisterWaitEx(self.wait_handle, INVALID_HANDLE_VALUE) };
        if success == 0 {
            // Unexpected OS failure: completion is not proven. Prefer retaining
            // the context AND process handle over callback UAF / closing a handle
            // still being waited on. This is not the normal cancellation path.
            let error = Error::last_os_error();
            if let Some(context) = self.context.take() {
                std::mem::forget(context);
            }
            tracing::error!("UnregisterWaitEx failed: {error}");
            return;
        }
        drop(self.context.take());
        unsafe { CloseHandle(self.child_handle) };
    }
}

#[cfg(test)]
mod tests {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, IntoRawHandle, OwnedHandle};
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use corcovado::{event::Events, Poll, PollOpt, Ready, Token};
    use windows_sys::Win32::Foundation::{
        DuplicateHandle, GetHandleInformation, DUPLICATE_SAME_ACCESS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    use super::*;

    fn child() -> Child {
        Command::new("cmd.exe")
            .args(["/D", "/Q", "/K"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    }

    fn duplicate(child: &Child) -> OwnedHandle {
        let mut handle = std::ptr::null_mut();
        unsafe {
            let process = GetCurrentProcess();
            assert_ne!(
                DuplicateHandle(
                    process,
                    child.as_raw_handle(),
                    process,
                    &mut handle,
                    0,
                    0,
                    DUPLICATE_SAME_ACCESS
                ),
                0
            );
            OwnedHandle::from_raw_handle(handle)
        }
    }

    fn watcher(child: &Child) -> ChildExitWatcher {
        let handle = duplicate(child);
        let watcher = ChildExitWatcher::new(handle.as_raw_handle()).unwrap();
        let _ = handle.into_raw_handle();
        watcher
    }

    fn valid(handle: HANDLE) -> bool {
        let mut flags = 0;
        unsafe { GetHandleInformation(handle, &mut flags) != 0 }
    }

    #[test]
    fn event_is_emitted_when_child_exits() {
        let mut child = child();
        let watcher = watcher(&child);
        let drops = watcher.context.as_ref().unwrap().drops.clone();
        let handle = watcher.raw_handle();
        let mut events = Events::with_capacity(1);
        let poll = Poll::new().unwrap();
        let token = Token::from(0usize);
        poll.register(
            watcher.event_rx(),
            token,
            Ready::readable(),
            PollOpt::oneshot(),
        )
        .unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        poll.poll(&mut events, Some(Duration::from_secs(5)))
            .unwrap();
        assert_eq!(events.iter().next().unwrap().token(), token);
        assert_eq!(watcher.event_rx().try_recv(), Ok(ChildEvent::Exited));
        assert_eq!(
            drops.load(Ordering::SeqCst),
            0,
            "callback borrows, never frees"
        );
        drop(watcher);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert!(!valid(handle));
        assert!(valid(child.as_raw_handle()), "Child retains its own handle");
    }

    #[test]
    fn watcher_dropped_before_child_exit_reclaims_context() {
        let mut child = child();
        let watcher = watcher(&child);
        let drops = watcher.context.as_ref().unwrap().drops.clone();
        let handle = watcher.raw_handle();
        assert!(child.try_wait().unwrap().is_none());
        drop(watcher);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert!(!valid(handle));
        assert!(child.try_wait().unwrap().is_none());
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn registration_failure_reclaims_context_but_not_callers_handle() {
        let mut child = child();
        let handle = duplicate(&child);
        let mut drops = None;
        let result =
            ChildExitWatcher::with_registration(handle.as_raw_handle(), |_, context| {
                drops =
                    Some(unsafe { &*context.cast::<CallbackContext>() }.drops.clone());
                Err(Error::other("injected registration failure"))
            });
        assert!(
            matches!(result, Err(ref e) if e.to_string() == "injected registration failure")
        );
        assert_eq!(drops.unwrap().load(Ordering::SeqCst), 1);
        assert!(valid(handle.as_raw_handle()));
        drop(handle);
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn child_exit_racing_drop_reclaims_context_once() {
        for _ in 0..32 {
            let mut child = child();
            let watcher = watcher(&child);
            let drops = watcher.context.as_ref().unwrap().drops.clone();
            child.kill().unwrap();
            // Don't wait for the callback: cancellation may race its execution.
            drop(watcher);
            assert_eq!(drops.load(Ordering::SeqCst), 1);
            child.wait().unwrap();
        }
    }
}
