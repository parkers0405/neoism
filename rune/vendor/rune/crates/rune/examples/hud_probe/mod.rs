//! `overlay_hud_cost`'s and `window_scroll_cost`'s probes: thread and process CPU clocks, CPU
//! time by thread, and counts of the Metal calls a frame makes (command buffers committed,
//! encoders, presents, completion handlers), counted by replacing those methods on the GPU's
//! command buffer class.
#![allow(dead_code)]

pub fn thread_cpu_ms() -> f64 {
    #[cfg(unix)]
    {
        let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: a live, writable timespec; the clock needs no handle.
        unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) };
        t.tv_sec as f64 * 1e3 + t.tv_nsec as f64 / 1e6
    }
    #[cfg(not(unix))]
    0.0
}

pub fn process_cpu_ms() -> f64 {
    #[cfg(unix)]
    {
        // SAFETY: a zeroed rusage is valid, and getrusage only writes it.
        let mut u: libc::rusage = unsafe { std::mem::zeroed() };
        unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) };
        let ms = |t: libc::timeval| t.tv_sec as f64 * 1e3 + t.tv_usec as f64 / 1e3;
        ms(u.ru_utime) + ms(u.ru_stime)
    }
    #[cfg(not(unix))]
    0.0
}

/// Summed CPU ms of the named threads matching `name`.
pub fn thread_ms(threads: &[(String, f64)], name: &str) -> f64 {
    threads.iter().filter(|(n, _)| n == name).map(|(_, ms)| ms).sum()
}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::{CStr, c_char, c_void};
    use std::sync::atomic::{AtomicU64, Ordering};

    unsafe extern "C" {
        fn mach_port_deallocate(task: libc::mach_port_t, name: libc::mach_port_t) -> libc::kern_return_t;
    }

    /// CPU milliseconds of the threads alive now, by pthread name ("" for unnamed ones).
    #[allow(deprecated)] // mach_task_self: libc points at the mach2 crate, which Rune doesn't use.
    pub fn named_threads() -> Vec<(String, f64)> {
        let mut out = Vec::new();
        // SAFETY: task_threads hands back an array of send rights this function deallocates, and
        // each thread_info writes at most THREAD_BASIC_INFO_COUNT integers into the struct.
        unsafe {
            let mut list: libc::thread_act_array_t = std::ptr::null_mut();
            let mut count: libc::mach_msg_type_number_t = 0;
            if libc::task_threads(libc::mach_task_self(), &mut list, &mut count) != 0 {
                return out;
            }
            for i in 0..count as usize {
                let thread = *list.add(i);
                let mut info: libc::thread_basic_info = std::mem::zeroed();
                let mut n = libc::THREAD_BASIC_INFO_COUNT;
                if libc::thread_info(thread, libc::THREAD_BASIC_INFO as u32, (&raw mut info).cast(), &mut n) == 0 {
                    let ms = |t: libc::time_value_t| t.seconds as f64 * 1e3 + t.microseconds as f64 / 1e3;
                    let mut name = [0 as c_char; 64];
                    let pthread = libc::pthread_from_mach_thread_np(thread);
                    if pthread != 0 {
                        libc::pthread_getname_np(pthread, name.as_mut_ptr(), name.len());
                    }
                    let name = CStr::from_ptr(name.as_ptr()).to_string_lossy().into_owned();
                    out.push((name, ms(info.user_time) + ms(info.system_time)));
                }
                mach_port_deallocate(libc::mach_task_self(), thread);
            }
            libc::vm_deallocate(libc::mach_task_self(), list as usize, count as usize * size_of::<libc::thread_act_t>());
        }
        out
    }

    /// The process's retired instructions, cycles and interrupt wakeups so far. Instructions don't
    /// move with clock speed or core type, so they compare runs that CPU time can't.
    pub fn process_counts() -> [u64; 3] {
        // SAFETY: proc_pid_rusage fills the rusage_info_v4 it is handed for RUSAGE_INFO_V4.
        unsafe {
            let mut info: libc::rusage_info_v4 = std::mem::zeroed();
            if libc::proc_pid_rusage(libc::getpid(), libc::RUSAGE_INFO_V4, (&raw mut info).cast()) != 0 {
                return [0; 3];
            }
            [info.ri_instructions, info.ri_cycles, info.ri_interrupt_wkups]
        }
    }

    unsafe extern "C" {
        fn thread_selfcounts(kind: i32, buf: *mut u64, size: usize) -> i32;
    }

    /// The calling thread's retired instructions and cycles so far.
    pub fn thread_counts() -> [u64; 2] {
        let mut counts = [0u64; 2];
        // SAFETY: kind 1 (THSC_CPI) writes two u64s into the buffer it is handed.
        unsafe { thread_selfcounts(1, counts.as_mut_ptr(), size_of_val(&counts)) };
        counts
    }

    pub static COMMITS: AtomicU64 = AtomicU64::new(0);
    pub static RENDER_ENCODERS: AtomicU64 = AtomicU64::new(0);
    pub static BLIT_ENCODERS: AtomicU64 = AtomicU64::new(0);
    pub static PRESENTS: AtomicU64 = AtomicU64::new(0);
    pub static COMPLETED_HANDLERS: AtomicU64 = AtomicU64::new(0);

    pub fn metal_counts() -> [u64; 5] {
        [&COMMITS, &RENDER_ENCODERS, &BLIT_ENCODERS, &PRESENTS, &COMPLETED_HANDLERS].map(|c| c.load(Ordering::Relaxed))
    }

    type Id = *mut c_void;
    type Sel = *mut c_void;

    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> Id;
        fn sel_registerName(name: *const c_char) -> Sel;
        fn object_getClass(obj: Id) -> Id;
        fn class_getInstanceMethod(class: Id, sel: Sel) -> Id;
        fn method_setImplementation(method: Id, imp: *const c_void) -> *const c_void;
        fn objc_msgSend();
    }

    #[link(name = "Metal", kind = "framework")]
    unsafe extern "C" {
        fn MTLCreateSystemDefaultDevice() -> Id;
    }

    macro_rules! counted {
        ($name:ident, $orig:ident, $counter:ident, ($($arg:ident: $ty:ty),*) -> $ret:ty) => {
            static mut $orig: Option<unsafe extern "C" fn(Id, Sel $(, $ty)*) -> $ret> = None;
            unsafe extern "C" fn $name(this: Id, sel: Sel $(, $arg: $ty)*) -> $ret {
                $counter.fetch_add(1, Ordering::Relaxed);
                // SAFETY: the original implementation, saved before this one replaced it.
                unsafe { ($orig.unwrap())(this, sel $(, $arg)*) }
            }
        };
    }

    static mut COMMIT: Option<unsafe extern "C" fn(Id, Sel)> = None;
    /// Commits by command buffer label and committing thread, while `OVERLAY_COUNT_METAL=labels`.
    pub static LABELS: std::sync::Mutex<Vec<(String, u64)>> = std::sync::Mutex::new(Vec::new());
    pub static TALLY_LABELS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    unsafe extern "C" fn commit(this: Id, sel: Sel) {
        COMMITS.fetch_add(1, Ordering::Relaxed);
        if TALLY_LABELS.load(Ordering::Relaxed) {
            // SAFETY: -[MTLCommandBuffer label] returns an NSString or nil; UTF8String borrows it.
            let label = unsafe {
                let send: unsafe extern "C" fn(Id, Sel) -> Id = std::mem::transmute(objc_msgSend as *const c_void);
                let utf8: unsafe extern "C" fn(Id, Sel) -> *const c_char = std::mem::transmute(objc_msgSend as *const c_void);
                let label = send(this, sel_registerName(c"label".as_ptr()));
                if label.is_null() {
                    "(none)".to_string()
                } else {
                    CStr::from_ptr(utf8(label, sel_registerName(c"UTF8String".as_ptr()))).to_string_lossy().into_owned()
                }
            };
            let mut key = format!("{label} @ {}", std::thread::current().name().unwrap_or("?"));
            if std::env::var_os("OVERLAY_COMMIT_STACKS").is_some() {
                let trace = std::backtrace::Backtrace::force_capture().to_string();
                for line in trace
                    .lines()
                    .map(str::trim)
                    .filter(|l| l.contains("wgpu_core::") || l.contains("rune::") || l.contains("wgpu::"))
                    .take(8)
                {
                    key.push_str("\n      ");
                    key.push_str(line);
                }
            }
            let mut labels = LABELS.lock().unwrap();
            match labels.iter_mut().find(|(k, _)| *k == key) {
                Some(e) => e.1 += 1,
                None => labels.push((key, 1)),
            }
        }
        // SAFETY: the original implementation, saved before this one replaced it.
        unsafe { (COMMIT.unwrap())(this, sel) }
    }
    counted!(render_encoder, RENDER, RENDER_ENCODERS, (descriptor: Id) -> Id);
    counted!(blit_encoder, BLIT, BLIT_ENCODERS, () -> Id);
    counted!(present, PRESENT, PRESENTS, (drawable: Id) -> ());
    counted!(completed_handler, HANDLER, COMPLETED_HANDLERS, (block: Id) -> ());

    /// Counts the Metal calls above from now on, on whichever class the system GPU's command
    /// buffers have.
    pub fn count_metal() {
        // SAFETY: plain Objective-C runtime calls; each replacement has its method's signature, and
        // its original is stored before any command buffer can reach it.
        unsafe {
            let send0: unsafe extern "C" fn(Id, Sel) -> Id = std::mem::transmute(objc_msgSend as *const c_void);
            let sel = |s: &CStr| sel_registerName(s.as_ptr());
            let device = MTLCreateSystemDefaultDevice();
            assert!(!device.is_null() && !objc_getClass(c"NSObject".as_ptr()).is_null());
            let queue = send0(device, sel(c"newCommandQueue"));
            let class = object_getClass(send0(queue, sel(c"commandBufferWithUnretainedReferences")));
            let swap = |name: &CStr, imp: *const c_void| {
                let method = class_getInstanceMethod(class, sel(name));
                assert!(!method.is_null(), "no {name:?}");
                method_setImplementation(method, imp)
            };
            COMMIT = std::mem::transmute(swap(c"commit", commit as *const c_void));
            RENDER = std::mem::transmute(swap(c"renderCommandEncoderWithDescriptor:", render_encoder as *const c_void));
            BLIT = std::mem::transmute(swap(c"blitCommandEncoder", blit_encoder as *const c_void));
            PRESENT = std::mem::transmute(swap(c"presentDrawable:", present as *const c_void));
            HANDLER = std::mem::transmute(swap(c"addCompletedHandler:", completed_handler as *const c_void));
        }
    }

    /// Makes every window report itself on screen, so a Mac whose screen is locked still draws (its
    /// frames never reach the display): only for profiling.
    pub fn presume_visible() {
        extern "C" fn visible(_: Id, _: Sel) -> usize {
            1 << 1
        }
        // SAFETY: replaces -[NSWindow occlusionState] with one of the same signature.
        unsafe {
            let method = class_getInstanceMethod(objc_getClass(c"NSWindow".as_ptr()), sel_registerName(c"occlusionState".as_ptr()));
            assert!(!method.is_null());
            method_setImplementation(method, visible as *const c_void);
        }
    }
}

#[cfg(target_os = "macos")]
pub use mac::*;

#[cfg(not(target_os = "macos"))]
pub static LABELS: std::sync::Mutex<Vec<(String, u64)>> = std::sync::Mutex::new(Vec::new());
#[cfg(not(target_os = "macos"))]
pub static TALLY_LABELS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(not(target_os = "macos"))]
pub fn named_threads() -> Vec<(String, f64)> {
    Vec::new()
}
#[cfg(not(target_os = "macos"))]
pub fn process_counts() -> [u64; 3] {
    [0; 3]
}
#[cfg(not(target_os = "macos"))]
pub fn thread_counts() -> [u64; 2] {
    [0; 2]
}
#[cfg(not(target_os = "macos"))]
pub fn metal_counts() -> [u64; 5] {
    [0; 5]
}
#[cfg(not(target_os = "macos"))]
pub fn count_metal() {}
#[cfg(not(target_os = "macos"))]
pub fn presume_visible() {}
