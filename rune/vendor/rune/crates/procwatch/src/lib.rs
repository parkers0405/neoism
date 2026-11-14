//! A native app's own memory and CPU, watched from inside.
//!
//! - [`snapshot`]: physical footprint (what Activity Monitor and jetsam count), its lifetime
//!   peak, resident size, total CPU including threads that have exited, open file descriptors,
//!   and each live thread's name and CPU. [`snapshot_of`] reads the same of another process of
//!   this user, with no task port, so it works on a hardened release build.
//! - [`usage`]: just the footprint, its peak, resident size and open files, without walking
//!   threads or touching the heap counters: cheap enough for a recorder to sample every second.
//! - [`alloc::Counting`]: an optional counting global allocator for dev builds and tests.
//! - [`Gauge`]: named byte counts for memory the allocator never sees (pixel buffers,
//!   IOSurfaces, GPU textures). With both, [`Snapshot::unaccounted`] is the footprint nothing
//!   claims, which is where leaked native handles show up.
//! - [`watch`]: a thread that samples every few seconds, keeps the latest for [`latest`], and
//!   writes a line to stderr when the footprint passes a threshold, keeps growing, or the
//!   process burns CPU, naming the busy threads (or "exited threads"). [`watch_with`] also
//!   hands each warning to a callback.
//! - [`serve`]: a fresh snapshot as JSON on a loopback port, for a dev panel or `curl`.
//!
//! macOS only for now; elsewhere [`snapshot`] returns `None`, [`usage`] has every field `None`,
//! and [`watch`] does nothing.

pub mod alloc;
pub mod gauge;

pub use gauge::Gauge;
pub use gauge::Lease;
pub use gauge::gauges;

use std::fmt::Write as _;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Thread {
    /// The kernel's id for the thread, stable for its life.
    pub id: u64,
    pub name: String,
    /// CPU seconds since the thread started.
    pub cpu_s: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub pid: i32,
    /// Physical footprint: dirty memory, compressed and swapped included.
    pub footprint: u64,
    pub footprint_peak: u64,
    pub resident: u64,
    /// User plus system CPU seconds over the process's life, exited threads included.
    pub cpu_s: f64,
    /// Open file descriptors, sockets and pipes included.
    pub fds: u32,
    pub threads: Vec<Thread>,
    pub heap: Option<alloc::Heap>,
    /// Every [`Gauge`] used so far, biggest first.
    pub gauges: Vec<(&'static str, i64)>,
}

impl Snapshot {
    /// CPU seconds spent by threads that have since exited.
    pub fn exited_cpu_s(&self) -> f64 {
        (self.cpu_s - self.threads.iter().map(|t| t.cpu_s).sum::<f64>()).max(0.0)
    }

    /// Footprint minus the counted Rust heap and every gauge; `None` without
    /// [`alloc::Counting`]. It has a floor (code, stacks, allocator overhead, system
    /// libraries), so watch it grow rather than reading it alone.
    pub fn unaccounted(&self) -> Option<i64> {
        let heap = self.heap.as_ref()?;
        Some(self.footprint as i64 - heap.live.max(0) - self.gauges.iter().map(|(_, b)| b).sum::<i64>())
    }

    pub fn to_json(&self) -> String {
        let mut s = String::new();
        let _ = write!(
            s,
            "{{\"pid\":{},\"footprint\":{},\"footprint_peak\":{},\"resident\":{},\"cpu_s\":{:.3},\"exited_cpu_s\":{:.3},\"fds\":{},\"threads\":[",
            self.pid,
            self.footprint,
            self.footprint_peak,
            self.resident,
            self.cpu_s,
            self.exited_cpu_s(),
            self.fds,
        );
        for (i, t) in self.threads.iter().enumerate() {
            let _ = write!(s, "{}{{\"name\":\"{}\",\"cpu_s\":{:.3}}}", if i > 0 { "," } else { "" }, clean(&t.name), t.cpu_s);
        }
        s.push(']');
        if let Some(h) = &self.heap {
            let _ = write!(
                s,
                ",\"heap\":{{\"live\":{},\"peak\":{},\"allocations\":{},\"largest\":{},\"big_live\":{},\"big_blocks\":{},\"tags\":",
                h.live, h.peak, h.allocations, h.largest, h.big_live, h.big_blocks
            );
            json_map(&mut s, &h.tags);
            s.push('}');
        }
        s.push_str(",\"gauges\":");
        json_map(&mut s, &self.gauges);
        if let Some(bytes) = self.unaccounted() {
            let _ = write!(s, ",\"unaccounted\":{bytes}");
        }
        s.push('}');
        s
    }
}

fn clean(name: &str) -> String {
    name.chars().filter(|c| !c.is_control() && *c != '"' && *c != '\\').collect()
}

fn json_map(s: &mut String, entries: &[(&'static str, i64)]) {
    s.push('{');
    for (i, (name, bytes)) in entries.iter().enumerate() {
        let _ = write!(s, "{}\"{}\":{bytes}", if i > 0 { "," } else { "" }, clean(name));
    }
    s.push('}');
}

/// This process now.
pub fn snapshot() -> Option<Snapshot> {
    let mut s = snapshot_of(std::process::id() as i32)?;
    s.heap = alloc::heap();
    s.gauges = gauges();
    Some(s)
}

/// Another process of this user (or this one), read through `proc_pid_rusage` and
/// `proc_pidinfo`: no task port, so hardened apps can be read too.
#[cfg(target_os = "macos")]
pub fn snapshot_of(pid: i32) -> Option<Snapshot> {
    use std::mem::{MaybeUninit, size_of};
    let usage = rusage(pid)?;
    // SAFETY: plain libproc and mach calls into buffers sized for them.
    unsafe {
        let ticks = ticks_to_seconds();
        const PROC_PIDLISTTHREADS: i32 = 6;
        let mut ids = vec![0u64; 512];
        let bytes = libc::proc_pidinfo(pid, PROC_PIDLISTTHREADS, 0, ids.as_mut_ptr().cast(), (ids.len() * 8) as i32);
        ids.truncate(bytes.max(0) as usize / 8);
        let threads = ids
            .iter()
            .filter_map(|&id| {
                let mut info = MaybeUninit::<libc::proc_threadinfo>::zeroed();
                let size = size_of::<libc::proc_threadinfo>() as i32;
                if libc::proc_pidinfo(pid, libc::PROC_PIDTHREADINFO, id, info.as_mut_ptr().cast(), size) != size {
                    return None;
                }
                let info = info.assume_init();
                let name = std::ffi::CStr::from_ptr(info.pth_name.as_ptr()).to_string_lossy().into_owned();
                Some(Thread { id, name, cpu_s: (info.pth_user_time + info.pth_system_time) as f64 / 1e9 })
            })
            .collect();
        Some(Snapshot {
            pid,
            footprint: usage.ri_phys_footprint,
            footprint_peak: usage.ri_lifetime_max_phys_footprint,
            resident: usage.ri_resident_size,
            cpu_s: (usage.ri_user_time + usage.ri_system_time) as f64 * ticks,
            fds: open_fds(pid).unwrap_or(0),
            threads,
            heap: None,
            gauges: Vec::new(),
        })
    }
}

#[cfg(not(target_os = "macos"))]
pub fn snapshot_of(_pid: i32) -> Option<Snapshot> {
    None
}

/// This process's memory and open files, each `None` where it can't be read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Usage {
    /// Physical footprint, as in [`Snapshot::footprint`].
    pub footprint: Option<u64>,
    pub footprint_peak: Option<u64>,
    pub resident: Option<u64>,
    pub fds: Option<u32>,
}

/// [`snapshot`]'s memory and file counts without its thread walk or heap counters: two
/// libproc calls for memory and two for files. A failed read leaves its fields `None`.
#[cfg(target_os = "macos")]
pub fn usage() -> Usage {
    let pid = std::process::id() as i32;
    let mut out = Usage { fds: open_fds(pid), ..Usage::default() };
    if let Some(u) = rusage(pid) {
        out.footprint = Some(u.ri_phys_footprint);
        out.footprint_peak = Some(u.ri_lifetime_max_phys_footprint);
        out.resident = Some(u.ri_resident_size);
    }
    out
}

#[cfg(not(target_os = "macos"))]
pub fn usage() -> Usage {
    Usage::default()
}

#[cfg(target_os = "macos")]
fn rusage(pid: i32) -> Option<libc::rusage_info_v4> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage_info_v4>::zeroed();
    // SAFETY: a zeroed buffer of the size RUSAGE_INFO_V4 fills; read only on success.
    unsafe {
        if libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V4, usage.as_mut_ptr().cast()) != 0 {
            return None;
        }
        Some(usage.assume_init())
    }
}

/// `PROC_PIDLISTFDS` fills one 8-byte `proc_fdinfo` per descriptor. Asking with no buffer
/// returns a size with headroom; the second call returns what it filled. Either call
/// returns 0 on failure, so 0 is `None`.
#[cfg(target_os = "macos")]
fn open_fds(pid: i32) -> Option<u32> {
    const PROC_PIDLISTFDS: i32 = 1;
    const FDINFO: usize = 8;
    // SAFETY: the first call writes nothing; the second writes at most `buf`'s length.
    unsafe {
        let want = libc::proc_pidinfo(pid, PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0);
        if want <= 0 {
            return None;
        }
        let mut buf = vec![0u64; want as usize / FDINFO + 1];
        let got = libc::proc_pidinfo(pid, PROC_PIDLISTFDS, 0, buf.as_mut_ptr().cast(), (buf.len() * FDINFO) as i32);
        (got > 0).then(|| (got as usize / FDINFO) as u32)
    }
}

/// `rusage_info` times are mach ticks (nanoseconds on Intel, 125/3 ns on Apple silicon).
#[cfg(target_os = "macos")]
fn ticks_to_seconds() -> f64 {
    #[repr(C)]
    struct Timebase {
        numer: u32,
        denom: u32,
    }
    unsafe extern "C" {
        fn mach_timebase_info(info: *mut Timebase) -> i32;
    }
    let mut base = Timebase { numer: 0, denom: 0 };
    // SAFETY: fills `base`.
    unsafe { mach_timebase_info(&mut base) };
    if base.denom == 0 { 1e-9 } else { base.numer as f64 / base.denom as f64 / 1e9 }
}

#[derive(Clone, Debug)]
pub struct Config {
    /// How often to sample.
    pub every: Duration,
    /// A status line this often even when all is well; `None`: only warnings.
    pub report: Option<Duration>,
    /// Warn when the footprint passes this, then again at each doubling.
    pub warn_footprint: u64,
    /// Warn when the footprint grew by this much over `growth_window`, every sample of it up.
    pub warn_growth: u64,
    pub growth_window: Duration,
    /// Warn when the process averaged more than this many cores over `cpu_window`.
    pub warn_cpu_cores: f64,
    pub cpu_window: Duration,
    /// Prefix for log lines.
    pub name: &'static str,
}

impl Config {
    /// Quiet in a release build: one line an hour, and warnings.
    pub fn release(name: &'static str) -> Config {
        Config {
            every: Duration::from_secs(10),
            report: Some(Duration::from_secs(3600)),
            warn_footprint: 1 << 30,
            warn_growth: 256 << 20,
            growth_window: Duration::from_secs(60),
            warn_cpu_cores: 0.8,
            cpu_window: Duration::from_secs(60),
            name,
        }
    }

    /// Chatty in a dev build: a line a minute, lower thresholds.
    pub fn dev(name: &'static str) -> Config {
        Config {
            every: Duration::from_secs(2),
            report: Some(Duration::from_secs(60)),
            warn_footprint: 512 << 20,
            warn_growth: 64 << 20,
            growth_window: Duration::from_secs(20),
            warn_cpu_cores: 0.5,
            cpu_window: Duration::from_secs(20),
            ..Config::release(name)
        }
    }
}

/// What a [`Watcher`] noticed in a sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The footprint passed `warn_footprint`, or doubled since the last such warning.
    Footprint,
    /// The footprint rose every sample for `growth_window`, by at least `warn_growth`.
    Growth,
    /// The process averaged at least `warn_cpu_cores` over `cpu_window`.
    Cpu,
    /// The periodic status line.
    Report,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub kind: Kind,
    pub line: String,
}

impl Event {
    pub fn is_warning(&self) -> bool {
        self.kind != Kind::Report
    }
}

impl std::fmt::Display for Event {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.line)
    }
}

static LATEST: Mutex<Option<Snapshot>> = Mutex::new(None);

/// The watcher's last sample, if it runs.
pub fn latest() -> Option<Snapshot> {
    LATEST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Samples on a `procwatch` thread for the life of the process, logging to stderr.
pub fn watch(config: Config) {
    watch_with(config, |_, _| {});
}

/// [`watch`], and calls `on_warning` on the watcher thread for every warning (not the
/// periodic report): to drop a cache, write a capture, or post the numbers somewhere.
pub fn watch_with(config: Config, on_warning: impl Fn(&Event, &Snapshot) + Send + 'static) {
    if snapshot().is_none() {
        return;
    }
    let _ = std::thread::Builder::new().name("procwatch".into()).spawn(move || {
        let mut watcher = Watcher::new(config);
        loop {
            if let Some(s) = snapshot() {
                for event in watcher.observe(Instant::now(), &s) {
                    eprintln!("{event}");
                    if event.is_warning() {
                        on_warning(&event, &s);
                    }
                }
                *LATEST.lock().unwrap_or_else(|e| e.into_inner()) = Some(s);
            }
            std::thread::sleep(watcher.config.every);
        }
    });
}

/// The judging half of [`watch`], fed samples so tests can drive it.
pub struct Watcher {
    pub config: Config,
    history: Vec<(Instant, Snapshot)>,
    warned_footprint: u64,
    warned_growth: Option<Instant>,
    warned_cpu: Option<Instant>,
    reported: Option<Instant>,
}

impl Watcher {
    pub fn new(config: Config) -> Watcher {
        Watcher { config, history: Vec::new(), warned_footprint: 0, warned_growth: None, warned_cpu: None, reported: None }
    }

    /// Takes a sample; returns what to log.
    pub fn observe(&mut self, now: Instant, s: &Snapshot) -> Vec<Event> {
        let c = &self.config;
        let mut out = Vec::new();
        // Keep one sample at or past the longest window, so a window always has a start even
        // when sleeps run a little long.
        let keep = c.growth_window.max(c.cpu_window);
        if let Some(edge) = self.history.iter().rposition(|(at, _)| now.duration_since(*at) >= keep) {
            self.history.drain(..edge);
        }
        self.history.push((now, s.clone()));
        let history = &self.history;
        // The samples from the last one at or past `window` ago up to now.
        let since = |window: Duration| {
            let start = history.iter().rposition(|(at, _)| now.duration_since(*at) >= window).unwrap_or(0);
            &history[start..]
        };
        let quiet = |warned: Option<Instant>, window: Duration| warned.is_none_or(|at| now.duration_since(at) >= window);
        let event = |kind, line| Event { kind, line };

        if s.footprint >= c.warn_footprint && s.footprint >= self.warned_footprint.saturating_mul(2) {
            self.warned_footprint = s.footprint;
            out.push(event(
                Kind::Footprint,
                format!("{}: warning: footprint {} (peak {}){}", c.name, mb(s.footprint), mb(s.footprint_peak), memory_note(s)),
            ));
        }
        let window = since(c.growth_window);
        if let Some((then, first)) = window.first()
            && now.duration_since(*then) >= c.growth_window.mul_f64(0.9)
        {
            let rising = window.windows(2).all(|w| w[1].1.footprint >= w[0].1.footprint);
            let grew = s.footprint.saturating_sub(first.footprint);
            if rising && grew >= c.warn_growth && quiet(self.warned_growth, c.growth_window) {
                self.warned_growth = Some(now);
                out.push(event(
                    Kind::Growth,
                    format!(
                        "{}: warning: footprint grew {} in {:.0}s to {}{}",
                        c.name,
                        mb(grew),
                        c.growth_window.as_secs_f64(),
                        mb(s.footprint),
                        memory_note(s)
                    ),
                ));
            }
        }
        if let Some((then, old)) = since(c.cpu_window).first() {
            let span = now.duration_since(*then).as_secs_f64();
            let cores = (s.cpu_s - old.cpu_s) / span.max(1e-9);
            if span >= c.cpu_window.as_secs_f64() * 0.9 && cores >= c.warn_cpu_cores && quiet(self.warned_cpu, c.cpu_window) {
                self.warned_cpu = Some(now);
                out.push(event(Kind::Cpu, format!("{}: warning: {:.0}% CPU over {:.0}s: {}", c.name, cores * 100.0, span, busiest(old, s, span))));
            }
        }
        if let Some(every) = c.report
            && self.reported.is_none_or(|at| now.duration_since(at) >= every)
        {
            self.reported = Some(now);
            out.push(event(
                Kind::Report,
                format!(
                    "{}: footprint {} (peak {}), resident {}, CPU {:.1}s ({:.1}s in exited threads), {} threads, {} fds{}",
                    c.name,
                    mb(s.footprint),
                    mb(s.footprint_peak),
                    mb(s.resident),
                    s.cpu_s,
                    s.exited_cpu_s(),
                    s.threads.len(),
                    s.fds,
                    memory_note(s)
                ),
            ));
        }
        out
    }
}

/// The threads that used the most CPU between two samples, as percentages of one core.
fn busiest(old: &Snapshot, new: &Snapshot, span: f64) -> String {
    let mut used: Vec<(String, f64)> = new
        .threads
        .iter()
        .map(|t| {
            let before = old.threads.iter().find(|o| o.id == t.id).map_or(0.0, |o| o.cpu_s);
            let name = if t.name.is_empty() { "unnamed".to_owned() } else { t.name.clone() };
            (name, (t.cpu_s - before).max(0.0))
        })
        .collect();
    // Whatever the live threads don't account for went to threads that exited in between,
    // counting only their CPU since `old`, not their whole lives.
    let exited = (new.cpu_s - old.cpu_s) - used.iter().map(|(_, s)| s).sum::<f64>();
    if exited > 0.01 * span {
        used.push(("exited threads".into(), exited));
    }
    used.sort_by(|a, b| b.1.total_cmp(&a.1));
    used.iter()
        .take(4)
        .filter(|(_, s)| *s > 0.0)
        .map(|(n, s)| format!("{n} {:.0}%", 100.0 * s / span.max(1e-9)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Where the footprint sits: the counted heap, the biggest gauges, and what nothing claims.
fn memory_note(s: &Snapshot) -> String {
    let mut note = match &s.heap {
        Some(h) if h.big_blocks > 0 => format!(
            "; heap {} live, {} in {} big blocks, largest request {}",
            mb(h.live.max(0) as u64),
            mb(h.big_live.max(0) as u64),
            h.big_blocks,
            mb(h.largest as u64)
        ),
        Some(h) => format!("; heap {} live (peak {})", mb(h.live.max(0) as u64), mb(h.peak.max(0) as u64)),
        None => String::new(),
    };
    let top: Vec<String> = s.gauges.iter().filter(|(_, b)| *b > 0).take(3).map(|(n, b)| format!("{n} {}", mb(*b as u64))).collect();
    if !top.is_empty() {
        let _ = write!(note, "; {}", top.join(", "));
    }
    if let Some(bytes) = s.unaccounted() {
        let _ = write!(note, "; unaccounted {}", mb(bytes.max(0) as u64));
    }
    note
}

pub fn mb(bytes: u64) -> String {
    if bytes >= 10 << 30 {
        format!("{:.0} GB", bytes as f64 / (1u64 << 30) as f64)
    } else if bytes >= 1 << 30 {
        format!("{:.1} GB", bytes as f64 / (1u64 << 30) as f64)
    } else {
        format!("{:.0} MB", bytes as f64 / (1u64 << 20) as f64)
    }
}

/// Answers every request on `addr` (keep it loopback) with a fresh [`snapshot`] as JSON.
pub fn serve(addr: std::net::SocketAddr) -> std::io::Result<std::net::SocketAddr> {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind(addr)?;
    let bound = listener.local_addr()?;
    std::thread::Builder::new().name("procwatch-http".into()).spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            if !stream.peer_addr().is_ok_and(|p| p.ip().is_loopback()) {
                continue;
            }
            let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
            let _ = stream.read(&mut [0; 1024]);
            let body = snapshot().map_or_else(|| "null".to_owned(), |s| s.to_json());
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    })?;
    Ok(bound)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[global_allocator]
    static ALLOC: alloc::Counting = alloc::Counting::system();

    #[test]
    fn counts_this_threads_allocations_and_live_heap() {
        let live = alloc::heap().unwrap().live;
        let before = alloc::thread_allocations();
        let held = std::hint::black_box(vec![0u8; 1 << 20]);
        assert_eq!(alloc::thread_allocations() - before, 1);
        // Other tests allocate in parallel, so allow slack.
        assert!(alloc::heap().unwrap().live - live >= (1 << 20) - (256 << 10));
        drop(held);
        let quiet = alloc::thread_allocations();
        let _ = std::hint::black_box(1 + 1);
        assert_eq!(alloc::thread_allocations(), quiet);
    }

    #[test]
    fn thread_bytes_count_requests_not_frees_on_this_thread_only() {
        use std::alloc::{GlobalAlloc, Layout};
        let allocator = alloc::Counting::system();
        let before = alloc::thread_allocated_bytes().unwrap();
        // SAFETY: valid layouts, checked allocations, each freed once with its layout.
        unsafe {
            let layout = Layout::from_size_align(128, 8).unwrap();
            let p = allocator.alloc_zeroed(layout);
            assert!(!p.is_null());
            let p = allocator.realloc(p, layout, 256);
            assert!(!p.is_null());
            allocator.dealloc(p, Layout::from_size_align(256, 8).unwrap());
        }
        assert_eq!(alloc::thread_allocated_bytes().unwrap() - before, 384);
        let threads: Vec<_> = (0..8)
            .map(|_| {
                std::thread::spawn(|| {
                    let start = alloc::thread_allocated_bytes().unwrap();
                    for _ in 0..10_000 {
                        drop(std::hint::black_box(vec![42u8; 128]));
                    }
                    alloc::thread_allocated_bytes().unwrap() - start
                })
            })
            .collect();
        for thread in threads {
            assert_eq!(thread.join().unwrap(), 1_280_000);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn usage_reads_memory_and_files_without_threads() {
        let u = usage();
        let footprint = u.footprint.unwrap();
        assert!(footprint > 0 && u.footprint_peak.unwrap() >= footprint, "{u:?}");
        assert!(u.resident.unwrap() > 0 && u.fds.unwrap() >= 3, "{u:?}");
        let opened: Vec<_> = (0..8).map(|_| std::fs::File::open("Cargo.toml").unwrap()).collect();
        assert!(usage().fds.unwrap() >= u.fds.unwrap() + 4);
        drop(opened);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn usage_is_unknown_off_macos() {
        assert_eq!(usage(), Usage::default());
    }

    fn at(footprint: u64, cpu_s: f64) -> Snapshot {
        Snapshot {
            footprint,
            footprint_peak: footprint,
            cpu_s,
            threads: vec![Thread { id: 1, name: "main".into(), cpu_s: 1.0 }],
            ..Default::default()
        }
    }

    fn lines(events: &[Event]) -> String {
        events.iter().map(|e| e.line.as_str()).collect::<Vec<_>>().join("\n")
    }

    #[cfg(target_os = "macos")]
    fn thread_cpu_s() -> f64 {
        let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: fills `t`.
        unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) };
        t.tv_sec as f64 + t.tv_nsec as f64 / 1e9
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn reads_this_process() {
        let s = snapshot().unwrap();
        assert!(s.footprint > 0 && s.footprint_peak >= s.footprint, "{s:?}");
        assert!(!s.threads.is_empty());
        assert_eq!(s.pid, std::process::id() as i32);
        // stdin, stdout and stderr at least.
        assert!(s.fds >= 3, "{s:?}");
        let opened: Vec<_> = (0..8).map(|_| std::fs::File::open("Cargo.toml").unwrap()).collect();
        assert!(snapshot().unwrap().fds >= s.fds + 4, "{s:?}");
        drop(opened);
        // A thread that burns CPU and exits still counts.
        let before = snapshot().unwrap();
        std::thread::spawn(|| {
            // Spin on the thread's own CPU clock: on a loaded machine 300 ms of wall time can
            // be well under 200 ms of CPU.
            let t = Instant::now();
            let mut x = 0u64;
            while thread_cpu_s() < 0.3 && t.elapsed() < Duration::from_secs(10) {
                x = std::hint::black_box(x.wrapping_add(1));
            }
        })
        .join()
        .unwrap();
        let after = snapshot().unwrap();
        assert!(after.exited_cpu_s() - before.exited_cpu_s() > 0.2, "{before:?} {after:?}");
    }

    #[test]
    fn json_parses_and_subtracts_heap_and_gauges() {
        let s = Snapshot {
            pid: 7,
            footprint: 1000,
            threads: vec![Thread { id: 1, name: "a\"b\\c\n".into(), cpu_s: 0.5 }],
            heap: Some(alloc::Heap { live: 300, tags: vec![("untagged", 300)], ..Default::default() }),
            gauges: vec![("video.frames", 200), ("gpu.textures", 100)],
            ..Default::default()
        };
        assert_eq!(s.unaccounted(), Some(400));
        let v: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
        assert_eq!(v["pid"], 7);
        assert_eq!(v["threads"][0]["name"], "abc");
        assert_eq!(v["heap"]["tags"]["untagged"], 300);
        assert_eq!(v["gauges"]["video.frames"], 200);
        assert_eq!(v["unaccounted"], 400);
        // Without the counting allocator there's no honest unaccounted figure.
        let bare = Snapshot { heap: None, ..s };
        assert_eq!(bare.unaccounted(), None);
        let v: serde_json::Value = serde_json::from_str(&bare.to_json()).unwrap();
        assert!(v.get("unaccounted").is_none() && v.get("heap").is_none());
    }

    #[test]
    fn warnings_name_where_the_memory_is() {
        let mut w = Watcher::new(Config { report: None, ..Config::release("t") });
        let s = Snapshot {
            footprint: 2 << 30,
            heap: Some(alloc::Heap { live: 100 << 20, ..Default::default() }),
            gauges: vec![("video.frames", 300 << 20)],
            ..at(2 << 30, 0.0)
        };
        let events = w.observe(Instant::now(), &s);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, Kind::Footprint);
        assert!(events[0].line.contains("heap 100 MB live") && events[0].line.ends_with("; video.frames 300 MB; unaccounted 1.6 GB"), "{}", events[0].line);
    }

    #[test]
    fn warns_on_size_growth_and_cpu_once_per_window() {
        let mut w = Watcher::new(Config { report: None, ..Config::release("t") });
        let t0 = Instant::now();
        let mut events = Vec::new();
        for i in 0..=12u64 {
            // 100 MB every 10 s, and a full core.
            events.extend(w.observe(t0 + Duration::from_secs(i * 10), &at((50 + 100 * i) << 20, i as f64 * 10.0)));
        }
        let joined = lines(&events);
        assert!(joined.contains("footprint grew"), "{joined}");
        assert!(joined.contains("% CPU over 60s: exited threads 100%"), "{joined}");
        assert!(joined.contains("warning: footprint 1.0 GB") || joined.contains("warning: footprint 1.1 GB"), "{joined}");
        assert_eq!(events.iter().filter(|e| e.kind == Kind::Growth).count(), 2, "{joined}");
        assert!(events.iter().all(Event::is_warning));
        // Flat and idle: nothing.
        let mut w = Watcher::new(Config { report: None, ..Config::release("t") });
        for i in 0..=12u64 {
            assert!(w.observe(t0 + Duration::from_secs(i * 10), &at(50 << 20, 1.0)).is_empty());
        }
    }

    #[test]
    fn late_samples_still_fill_the_window() {
        // Sleeps that run 10 ms long leave no sample exactly 60 s back.
        let mut w = Watcher::new(Config { report: None, ..Config::release("t") });
        let t0 = Instant::now();
        let mut events = Vec::new();
        for i in 0..=12u32 {
            let t = Duration::from_millis(10_010) * i;
            events.extend(w.observe(t0 + t, &at(50 << 20, t.as_secs_f64())));
        }
        assert_eq!(events.iter().filter(|e| e.kind == Kind::Cpu).count(), 2, "{}", lines(&events));
    }

    #[test]
    fn an_exited_thread_counts_only_its_last_stretch() {
        let thread = |id, name: &str, cpu_s| Thread { id, name: name.into(), cpu_s };
        let old = Snapshot { cpu_s: 101.0, threads: vec![thread(1, "main", 1.0), thread(2, "worker", 100.0)], ..Default::default() };
        // The worker ran 6 more seconds and exited; main ran 54.
        let new = Snapshot { cpu_s: 161.0, threads: vec![thread(1, "main", 55.0)], ..Default::default() };
        assert_eq!(busiest(&old, &new, 60.0), "main 90%, exited threads 10%");
    }
}
