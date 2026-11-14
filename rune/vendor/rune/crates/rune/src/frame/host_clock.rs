//! The host's monotonic clock in nanoseconds and the calling thread's CPU time, both precise:
//! Windows reads `QueryPerformanceCounter` and `QueryThreadCycleTime`, never `GetThreadTimes`,
//! which moves in 15.6 ms ticks.

/// Nanoseconds on the host's monotonic clock: Windows `QueryPerformanceCounter` scaled with
/// 128-bit math, `mach_absolute_time` through its timebase, else `CLOCK_MONOTONIC`.
#[cfg(not(target_arch = "wasm32"))]
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn now_ns() -> u64 {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
        static FREQ: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
        let freq = *FREQ.get_or_init(|| {
            let mut f = 0i64;
            // SAFETY: `f` is a live, writable i64.
            unsafe { QueryPerformanceFrequency(&mut f) };
            f.max(1) as u64
        });
        let mut count = 0i64;
        // SAFETY: `count` is a live, writable i64.
        unsafe { QueryPerformanceCounter(&mut count) };
        (count as u128 * 1_000_000_000 / freq as u128) as u64
    }
    #[cfg(target_vendor = "apple")]
    {
        #[repr(C)]
        struct Timebase { numer: u32, denom: u32 }
        unsafe extern "C" {
            fn mach_timebase_info(info: *mut Timebase) -> i32;
            fn mach_absolute_time() -> u64;
        }
        static BASE: std::sync::OnceLock<(u32, u32)> = std::sync::OnceLock::new();
        let (numer, denom) = *BASE.get_or_init(|| {
            let mut info = Timebase { numer: 0, denom: 0 };
            // SAFETY: `info` is a live, writable timebase struct.
            unsafe { mach_timebase_info(&mut info) };
            (info.numer.max(1), info.denom.max(1))
        });
        // SAFETY: no preconditions.
        let ticks = unsafe { mach_absolute_time() };
        (ticks as u128 * numer as u128 / denom as u128) as u64
    }
    #[cfg(all(unix, not(target_vendor = "apple")))]
    {
        let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: `t` is a live, writable timespec.
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut t) };
        t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64
    }
}

/// The calling thread's CPU time in raw units: cycles on Windows, nanoseconds elsewhere.
/// [`cpu_ns`] converts.
#[cfg(any(unix, windows))]
pub(crate) fn thread_cpu() -> u64 {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::{Threading::GetCurrentThread, WindowsProgramming::QueryThreadCycleTime};
        let mut cycles = 0u64;
        // SAFETY: the pseudo-handle is valid for the calling thread; `cycles` is live and writable.
        unsafe { QueryThreadCycleTime(GetCurrentThread(), &mut cycles) };
        cycles
    }
    #[cfg(unix)]
    {
        let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: `t` is a live, writable timespec and the clock needs no handle.
        let ok = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) };
        assert_eq!(ok, 0, "thread CPU clock unavailable");
        t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64
    }
}

/// Raw [`thread_cpu`] units in nanoseconds.
#[cfg(any(unix, windows))]
pub(crate) fn cpu_ns(raw: u64) -> f64 {
    #[cfg(windows)]
    return raw as f64 / cycles_per_ns();
    #[cfg(unix)]
    return raw as f64;
}

/// Thread cycles per nanosecond, measured once. On x86-64 the cycles are the TSC's, which runs
/// at a constant rate, so two readings against QPC 2 ms apart give it without spinning.
/// Elsewhere a thread spins 20 ms and counts its own cycles, the best of three rounds, since
/// preemption only ever lowers a round.
#[cfg(windows)]
pub(crate) fn cycles_per_ns() -> f64 {
    static RATE: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *RATE.get_or_init(|| {
        #[cfg(target_arch = "x86_64")]
        let rate = {
            // SAFETY: RDTSC has no preconditions on x86-64.
            #[allow(unused_unsafe)]
            let tsc = || unsafe { core::arch::x86_64::_rdtsc() };
            let (t0, c0) = (now_ns(), tsc());
            std::thread::sleep(std::time::Duration::from_millis(2));
            let (t1, c1) = (now_ns(), tsc());
            (c1 - c0) as f64 / (t1 - t0).max(1) as f64
        };
        #[cfg(not(target_arch = "x86_64"))]
        let rate = {
            let round = || {
                let (t0, c0) = (now_ns(), thread_cpu());
                while now_ns() - t0 < 20_000_000 { std::hint::spin_loop(); }
                (thread_cpu() - c0) as f64 / (now_ns() - t0) as f64
            };
            (0..3).map(|_| round()).fold(0.0, f64::max)
        };
        if rate > 0.0 { rate } else { 1.0 }
    })
}
