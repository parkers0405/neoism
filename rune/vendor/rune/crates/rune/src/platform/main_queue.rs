//! Work for the main thread from anywhere: the main dispatch queue, which the event loop drains.

#[link(name = "System", kind = "dylib")]
unsafe extern "C" {
    static _dispatch_main_q: u8;
    fn dispatch_async_f(queue: *const u8, context: *mut std::ffi::c_void, work: extern "C" fn(*mut std::ffi::c_void));
}

/// Run `f` on the main queue, which the window event loop drains.
pub(crate) fn on_main(f: impl FnOnce() + Send + 'static) {
    type Job = Box<dyn FnOnce() + Send>;
    extern "C" fn run(context: *mut std::ffi::c_void) {
        // SAFETY: `context` is the box `on_main` leaked, delivered exactly once.
        let job = unsafe { Box::from_raw(context.cast::<Job>()) };
        job();
    }
    let job: Job = Box::new(f);
    let context = Box::into_raw(Box::new(job)).cast();
    // SAFETY: the main queue outlives the process; `run` frees `context`.
    unsafe { dispatch_async_f(&raw const _dispatch_main_q, context, run) };
}
