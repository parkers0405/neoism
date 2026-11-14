use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[test]
fn jobs_run_off_the_callers_thread_in_order_and_wait_sees_them_done() {
    let mut worker = PresentWorker::spawn().expect("a thread");
    let caller = std::thread::current().id();
    let log = Arc::new(Mutex::new(Vec::new()));
    for i in 0..4 {
        let log = log.clone();
        worker.run(move || {
            std::thread::sleep(Duration::from_millis(5));
            assert_ne!(std::thread::current().id(), caller);
            log.lock().unwrap().push(i);
        });
    }
    worker.wait();
    assert_eq!(*log.lock().unwrap(), [0, 1, 2, 3]);
}

#[test]
fn run_returns_before_its_job_finishes() {
    let mut worker = PresentWorker::spawn().expect("a thread");
    let (release, gate) = std::sync::mpsc::channel::<()>();
    let done = Arc::new(AtomicUsize::new(0));
    let flag = done.clone();
    worker.run(move || {
        gate.recv().unwrap();
        flag.store(1, Ordering::SeqCst);
    });
    assert_eq!(done.load(Ordering::SeqCst), 0, "the caller went on while the job waited");
    release.send(()).unwrap();
    worker.wait();
    assert_eq!(done.load(Ordering::SeqCst), 1);
}

#[test]
fn dropping_the_worker_finishes_the_job_in_flight() {
    let done = Arc::new(AtomicUsize::new(0));
    let flag = done.clone();
    let mut worker = PresentWorker::spawn().expect("a thread");
    worker.run(move || {
        std::thread::sleep(Duration::from_millis(20));
        flag.store(1, Ordering::SeqCst);
    });
    drop(worker);
    assert_eq!(done.load(Ordering::SeqCst), 1);
}

#[test]
fn a_panicking_job_panics_on_the_owners_thread() {
    let mut worker = PresentWorker::spawn().expect("a thread");
    worker.run(|| panic!("device lost"));
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| worker.wait()));
    let message = caught.expect_err("the panic came back").downcast::<&str>().map(|m| *m);
    assert_eq!(message.ok(), Some("device lost"));
    let ran = Arc::new(AtomicUsize::new(0));
    let flag = ran.clone();
    worker.run(move || { flag.store(1, Ordering::SeqCst); });
    worker.wait();
    assert_eq!(ran.load(Ordering::SeqCst), 1, "later jobs run on the caller's thread");
}
