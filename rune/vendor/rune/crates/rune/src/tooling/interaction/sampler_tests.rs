use std::sync::Arc;

use super::cause::{Cause, cause};
use super::stack::Frame;
use super::watchdog::{Sampled, Stacks};

fn frame(name: &str, app: bool) -> Frame { Frame { name: name.into(), app } }

fn sampled(frames: &[(&str, bool)], waiting: bool, cpu_us: u64, at_ms: u64) -> Sampled {
    Sampled { frames: frames.iter().map(|&(n, a)| frame(n, a)).collect(), waiting, cpu_us, at_ns: at_ms * 1_000_000 }
}

#[test]
fn stacks_name_the_innermost_app_frame_most_samples_were_in() {
    let sleep = [("__semwait_signal (libsystem_kernel)", false), ("std::thread::sleep", false), ("app::save", true), ("app::main", true)];
    let layout = [("taffy::compute", true), ("app::view", true)];
    let s = Stacks::of(&[sampled(&sleep, true, 0, 0), sampled(&layout, false, 1000, 4), sampled(&sleep, true, 1000, 8), sampled(&sleep, true, 2000, 12)]).unwrap();
    assert_eq!((s.taken, s.waiting), (4, 3));
    let hot: Vec<(&str, u32)> = s.hot.iter().map(|(n, c)| (&**n, *c)).collect();
    assert_eq!(hot, [("app::save", 3), ("taffy::compute", 1)]);
    assert_eq!(s.stack.first().map(|f| &**f), Some("__semwait_signal (libsystem_kernel)"), "the hottest sample's stack, innermost first");
    assert!((s.cpu_share.unwrap() - 2.0 / 12.0).abs() < 1e-9);
    assert!(Stacks::of(&[]).is_none());
    assert_eq!(Stacks::of(&[sampled(&layout, false, 0, 0)]).unwrap().cpu_share, None, "one sample has no stretch");
}

fn stacks(taken: u32, waiting: u32, cpu_share: Option<f64>) -> Stacks {
    Stacks { taken, waiting, cpu_share, hot: Vec::<(Arc<str>, u32)>::new(), stack: Vec::new() }
}

#[test]
fn a_busy_machine_takes_the_blame_only_when_the_app_was_ready_to_run() {
    let slow = 16.7;
    assert_eq!(cause(10.0, slow, Some(9.0), None, false), Cause::Queued, "a quick frame that reached the screen late waited outside it");
    assert_eq!(cause(40.0, slow, Some(36.0), None, true), Cause::Cpu, "computing is the app's, busy machine or not");
    assert_eq!(cause(40.0, slow, Some(1.0), Some(&stacks(7, 6, None)), true), Cause::Blocked, "sleeping is the app's, busy machine or not");
    assert_eq!(cause(40.0, slow, Some(1.0), None, false), Cause::Blocked, "little CPU on an idle machine: it waited on something");
    assert_eq!(cause(40.0, slow, Some(1.0), None, true), Cause::SchedulerWait, "little CPU on a busy machine");
    assert_eq!(cause(40.0, slow, Some(4.0), Some(&stacks(7, 0, None)), false), Cause::SchedulerWait, "ready to run all along, yet hardly ran");
    assert_eq!(cause(40.0, slow, None, Some(&stacks(7, 1, Some(0.9))), false), Cause::Cpu, "the samples' CPU share stands in for the clock");
    assert_eq!(cause(40.0, slow, None, None, false), Cause::Cpu);
    assert_eq!(cause(40.0, slow, None, None, true), Cause::SchedulerWait);
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod mac {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};

    use super::super::stack::{Raw, Symbols, Target, demangled, library};

    #[inline(never)]
    fn spin_here_for_the_sampler(stop: &AtomicBool) {
        while !stop.load(Ordering::Relaxed) { std::hint::spin_loop(); }
    }

    /// Runs `body` on a thread, samples it from this one, and stops it.
    fn sample_a_thread(body: impl FnOnce(Arc<AtomicBool>, mpsc::Receiver<()>) + Send + 'static) -> (Raw, Vec<String>, Vec<bool>) {
        let stop = Arc::new(AtomicBool::new(false));
        let (ready, target) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let flag = stop.clone();
        let worker = std::thread::spawn(move || {
            ready.send(Target::current().unwrap()).unwrap();
            body(flag, wait);
        });
        let target: Target = target.recv().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        let raw = target.sample(|| true).expect("sampled");
        stop.store(true, Ordering::Relaxed);
        let _ = release.send(());
        worker.join().unwrap();
        let mut symbols = Symbols::default();
        let frames: Vec<_> = raw.pcs[..raw.len].iter().enumerate().map(|(i, &pc)| symbols.frame(pc, i == 0)).collect();
        let names = frames.iter().map(|f| f.name.to_string()).collect();
        let app = frames.iter().map(|f| f.app).collect();
        (raw, names, app)
    }

    #[test]
    fn the_sampler_names_the_function_a_running_thread_is_in() {
        let (raw, names, app) = sample_a_thread(|stop, _| spin_here_for_the_sampler(&stop));
        assert!(!raw.waiting, "a spinning thread is running");
        assert!(raw.cpu_us >= 10_000, "it spun for 30 ms before the sample: {} µs", raw.cpu_us);
        let at = names.iter().position(|n| n.contains("spin_here_for_the_sampler")).unwrap_or_else(|| panic!("{names:#?}"));
        assert!(app[at], "the test binary's own function is app code");
        assert!(raw.len > at + 1, "the walk goes on past it to its callers: {names:#?}");
    }

    #[test]
    fn the_sampler_sees_a_waiting_thread_wait() {
        let (raw, names, _) = sample_a_thread(|_, wait| { let _ = wait.recv(); });
        assert!(raw.waiting, "a thread in recv waits: {names:#?}");
    }

    #[test]
    fn a_sample_that_no_longer_applies_is_dropped() {
        let stop = Arc::new(AtomicBool::new(false));
        let (ready, got) = mpsc::channel();
        let flag = stop.clone();
        let worker = std::thread::spawn(move || {
            ready.send(Target::current().unwrap()).unwrap();
            spin_here_for_the_sampler(&flag);
        });
        let target = got.recv().unwrap();
        assert!(target.sample(|| false).is_none());
        stop.store(true, Ordering::Relaxed);
        worker.join().unwrap();
    }

    #[test]
    fn names_are_demangled_and_the_standard_library_isnt_the_app() {
        assert_eq!(demangled("__RNvCs1mieHSqJ4qC_1m11local_thing"), "m::local_thing");
        assert_eq!(demangled("__ZN3std6thread5sleep17h0123456789abcdefE"), "std::thread::sleep");
        assert_eq!(demangled("__semwait_signal"), "__semwait_signal");
        assert!(library("std::thread::sleep") && library("<core::ops::Range<usize> as Iterator>::next"));
        assert!(!library("<app::Save as core::ops::FnOnce>::call_once"));
    }
}
