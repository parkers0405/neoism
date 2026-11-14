//! Reading another thread's call stack from outside it, for the watchdog (`watchdog`): suspend
//! the thread for a few microseconds, copy its return addresses by following frame pointers
//! inside its stack, resume it, and only then name them. Nothing between suspend and resume
//! allocates or takes a lock, so a thread stopped inside `malloc` can't deadlock the sampler.
//! Names are function symbols only: never stack memory, arguments, paths or environment.
//!
//! Apple Silicon Macs only (their ABI keeps frame pointers in every frame); elsewhere a
//! [`Target`] is never made and the watchdog doesn't start.
#![cfg_attr(not(all(target_os = "macos", target_arch = "aarch64")), allow(dead_code))]

use std::sync::Arc;

/// Return addresses kept per sample, innermost first.
pub(crate) const DEPTH: usize = 32;

/// One look at a thread's stack.
pub(crate) struct Raw {
    pub pcs: [usize; DEPTH],
    pub len: usize,
    /// The thread was waiting (on IO, a lock, a sleep), not running or ready to run.
    pub waiting: bool,
    /// The thread's CPU time so far, in µs.
    pub cpu_us: u64,
}

/// A named frame: the function, and whether it's the app's own code (in the main executable,
/// outside `std`, `core` and `alloc`).
#[derive(Clone)]
pub(crate) struct Frame {
    pub name: Arc<str>,
    pub app: bool,
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(crate) use mac::{Symbols, Target};

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod mac {
    use std::ffi::{CStr, c_void};

    use rustc_hash::FxHashMap;

    use super::{DEPTH, Frame, Raw};

    type Port = u32;

    #[repr(C)]
    #[derive(Default)]
    struct ThreadState {
        x: [u64; 29],
        fp: u64,
        lr: u64,
        sp: u64,
        pc: u64,
        cpsr: u32,
        pad: u32,
    }
    const ARM_THREAD_STATE64: i32 = 6;
    const ARM_THREAD_STATE64_COUNT: u32 = (size_of::<ThreadState>() / 4) as u32;

    #[repr(C)]
    #[derive(Default)]
    struct BasicInfo {
        user: [i32; 2],
        system: [i32; 2],
        cpu_usage: i32,
        policy: i32,
        run_state: i32,
        flags: i32,
        suspend_count: i32,
        sleep_time: i32,
    }
    const THREAD_BASIC_INFO: u32 = 3;
    const THREAD_BASIC_INFO_COUNT: u32 = (size_of::<BasicInfo>() / 4) as u32;
    const TH_STATE_RUNNING: i32 = 1;

    // Pointer authentication signs return addresses in system frameworks; addresses fit in 47 bits.
    const ADDRESS_MASK: usize = (1 << 47) - 1;

    unsafe extern "C" {
        fn mach_thread_self() -> Port;
        fn thread_suspend(thread: Port) -> i32;
        fn thread_resume(thread: Port) -> i32;
        fn thread_get_state(thread: Port, flavor: i32, state: *mut u32, count: *mut u32) -> i32;
        fn thread_info(thread: Port, flavor: u32, info: *mut i32, count: *mut u32) -> i32;
        fn pthread_get_stackaddr_np(thread: libc::pthread_t) -> *mut c_void;
        fn pthread_get_stacksize_np(thread: libc::pthread_t) -> usize;
        fn _dyld_get_image_header(index: u32) -> *const c_void;
    }

    /// A thread that can be sampled from another: its Mach port and stack range.
    #[derive(Clone, Copy)]
    pub(crate) struct Target {
        port: Port,
        low: usize,
        high: usize,
    }

    impl Target {
        /// The calling thread.
        pub(crate) fn current() -> Option<Self> {
            // SAFETY: plain queries of the calling thread; the port reference is kept for the
            // process's life (the watchdog samples it until exit).
            unsafe {
                let me = libc::pthread_self();
                let high = pthread_get_stackaddr_np(me) as usize;
                let size = pthread_get_stacksize_np(me);
                (high > size && size > 0).then(|| Self { port: mach_thread_self(), low: high - size, high })
            }
        }

        /// Samples the thread if `still()` holds once it's stopped: `None` when it doesn't, or
        /// the thread can't be stopped or read.
        pub(crate) fn sample(&self, still: impl Fn() -> bool) -> Option<Raw> {
            let mut info = BasicInfo::default();
            let mut count = THREAD_BASIC_INFO_COUNT;
            // SAFETY: `info` is a live buffer of `count` 32-bit words, as the flavor needs.
            if unsafe { thread_info(self.port, THREAD_BASIC_INFO, (&raw mut info).cast(), &mut count) } != 0 { return None; }
            let cpu_us = [info.user, info.system].iter().map(|t| t[0] as u64 * 1_000_000 + t[1] as u64).sum();
            let mut raw = Raw { pcs: [0; DEPTH], len: 0, waiting: info.run_state != TH_STATE_RUNNING, cpu_us };
            // SAFETY: suspending a thread of this process that isn't the caller (`watchdog`
            // samples only the UI thread from its own thread). It's resumed below on every path.
            if unsafe { thread_suspend(self.port) } != 0 { return None; }
            let walked = still() && self.walk(&mut raw);
            // SAFETY: resumes the suspension above.
            unsafe { thread_resume(self.port) };
            walked.then_some(raw)
        }

        /// Copies the stopped thread's return addresses. No allocation, no locks.
        fn walk(&self, raw: &mut Raw) -> bool {
            let mut state = ThreadState::default();
            let mut count = ARM_THREAD_STATE64_COUNT;
            // SAFETY: `state` is a live buffer of `count` 32-bit words, as the flavor needs.
            if unsafe { thread_get_state(self.port, ARM_THREAD_STATE64, (&raw mut state).cast(), &mut count) } != 0 { return false; }
            fn push(raw: &mut Raw, pc: usize) {
                let pc = pc & ADDRESS_MASK;
                if pc != 0 && raw.len < DEPTH && (raw.len == 0 || raw.pcs[raw.len - 1] != pc) {
                    raw.pcs[raw.len] = pc;
                    raw.len += 1;
                }
            }
            push(raw, state.pc as usize);
            // A leaf that saved no frame record (a syscall stub) returns through `lr`; one that
            // did has also saved it at `fp + 8`, where the walk below finds it.
            let mut fp = state.fp as usize;
            let saved = |fp: usize| (fp >= self.low && fp.is_multiple_of(8) && fp + 16 <= self.high).then(|| {
                // SAFETY: `fp` and `fp + 8` lie inside the stopped thread's own stack.
                unsafe { (*(fp as *const usize), *((fp + 8) as *const usize)) }
            });
            if saved(fp).is_some_and(|(_, ret)| ret & ADDRESS_MASK != state.lr as usize & ADDRESS_MASK) { push(raw, state.lr as usize); }
            while let Some((next, ret)) = saved(fp) {
                if ret & ADDRESS_MASK == 0 || raw.len == DEPTH { break; }
                push(raw, ret);
                if next <= fp { break; }
                fp = next;
            }
            true
        }
    }

    /// Function names for return addresses, remembered: `dladdr` over the loaded images'
    /// symbol tables, Rust names demangled without their hashes.
    #[derive(Default)]
    pub(crate) struct Symbols {
        known: FxHashMap<usize, Frame>,
    }

    impl Symbols {
        /// The function holding `pc`; `inner` for the sample's innermost frame, which is the
        /// instruction itself rather than a return address after a call.
        pub(crate) fn frame(&mut self, pc: usize, inner: bool) -> Frame {
            let at = if inner { pc } else { pc.saturating_sub(1) };
            self.known.entry(at).or_insert_with(|| name(at)).clone()
        }
    }

    fn name(at: usize) -> Frame {
        // SAFETY: `Dl_info` is plain pointers, for which all zeroes is valid (null).
        let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
        // SAFETY: `info` is a live, writable `Dl_info`; dladdr only reads loaded images.
        if unsafe { libc::dladdr(at as *const c_void, &mut info) } == 0 || info.dli_sname.is_null() {
            return Frame { name: format!("{at:#x}").into(), app: false };
        }
        // SAFETY: dladdr returned a NUL-terminated symbol name that lives as long as its image.
        let symbol = unsafe { CStr::from_ptr(info.dli_sname) }.to_string_lossy();
        let mut name = super::demangled(&symbol);
        // SAFETY: image 0 is the main executable, loaded for the process's life.
        let main = unsafe { _dyld_get_image_header(0) };
        let app = std::ptr::eq(info.dli_fbase.cast_const(), main) && !super::library(&name);
        if !std::ptr::eq(info.dli_fbase.cast_const(), main) && !info.dli_fname.is_null() {
            // SAFETY: as above, the image's NUL-terminated path.
            let path = unsafe { CStr::from_ptr(info.dli_fname) }.to_string_lossy();
            let image = path.rsplit('/').next().unwrap_or("").trim_end_matches(".dylib");
            if !image.is_empty() { name = format!("{name} ({image})"); }
        }
        Frame { name: name.into(), app }
    }
}

/// `symbol` demangled, without its hash, cut to 160 characters.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(crate) fn demangled(symbol: &str) -> String {
    let mut name = format!("{:#}", rustc_demangle::demangle(symbol.strip_prefix('_').filter(|s| s.starts_with("_ZN") || s.starts_with("_R")).unwrap_or(symbol)));
    if name.len() > 160 {
        let mut cut = 157;
        while !name.is_char_boundary(cut) { cut -= 1; }
        name.truncate(cut);
        name.push_str("...");
    }
    name
}

/// Whether `name` is the standard library's, so never the place an app spent its time.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(crate) fn library(name: &str) -> bool {
    let name = name.trim_start_matches('<');
    ["std::", "core::", "alloc::", "__rust", "rust_"].iter().any(|p| name.starts_with(p))
}
