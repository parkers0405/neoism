//! Windows-only real anonymous-pipe regression tests (also runnable under Wine).
use super::*;
use std::io::{Read, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};

const TIMEOUT: Duration = Duration::from_secs(10);

// Run the entire scenario, including Drop and blocking peer I/O, behind a
// watchdog. A broken implementation fails instead of hanging the test process.
fn bounded(f: impl FnOnce() + Send + 'static) {
    let (tx, rx) = channel();
    spawn(move || {
        let _ = tx.send(catch_unwind(AssertUnwindSafe(f)));
    });
    match rx.recv_timeout(TIMEOUT).expect("pipe scenario timed out") {
        Ok(()) => {}
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

fn until(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(Instant::now() < deadline, "pipe progress timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn idle_separated_input_and_output() {
    bounded(|| {
        let (read, write) = miow::pipe::anonymous(4096).unwrap();
        let mut reader = EventedAnonRead::new(read);
        let mut writer = EventedAnonWrite::new(write);
        let poll = Poll::new().unwrap();
        poll.register(&reader, Token(1), Ready::readable(), PollOpt::edge())
            .unwrap();
        let mut events = corcovado::Events::with_capacity(8);
        for i in 0..200u16 {
            let bytes = i.to_le_bytes();
            let mut sent = 0;
            until(|| {
                sent += writer.write(&bytes[sent..]).unwrap();
                sent == bytes.len()
            });
            poll.poll(&mut events, Some(Duration::from_secs(3)))
                .unwrap();
            assert!(events.iter().any(|e| e.token() == Token(1)));
            let mut received = [0; 2];
            let mut count = 0;
            until(|| {
                count += reader.read(&mut received[count..]).unwrap();
                count == received.len()
            });
            assert_eq!(received, bytes);
            // Force the writer's empty-queue wait between inputs.
            std::thread::sleep(Duration::from_millis(2));
        }
    });
}

#[test]
fn output_backpressure_and_reader_full_queue_resume() {
    bounded(|| {
        let (read, write) = miow::pipe::anonymous(4096).unwrap();
        let mut reader = EventedAnonRead::new(read);
        let mut writer = EventedAnonWrite::new(write);
        let payload: Vec<u8> = (0..512 * 1024).map(|i| (i % 251) as u8).collect();
        let mut sent = 0;
        // Do not drain until both the reader queue and writer queue back up.
        until(|| {
            let n = writer.write(&payload[sent..]).unwrap();
            sent += n;
            let _guard = reader.inner.wait_tag.lock();
            reader.consumer.is_full() && n == 0
        });
        let mut received = Vec::new();
        let mut buf = [0; 997];
        until(|| {
            if sent < payload.len() {
                sent += writer.write(&payload[sent..]).unwrap();
            }
            let n = reader.read(&mut buf).unwrap();
            received.extend_from_slice(&buf[..n]);
            received.len() == payload.len()
        });
        assert_eq!(received, payload);
    });
}

#[test]
fn drop_during_blocked_read_and_before_read_starts() {
    bounded(|| {
        for i in 0..100 {
            let (read, _peer) = miow::pipe::anonymous(4096).unwrap();
            let reader = EventedAnonRead::new(read);
            let inner = reader.inner.clone();
            if i % 2 == 0 {
                std::thread::sleep(Duration::from_millis(2));
            }
            drop(reader);
            // Cancellation must actually retire the worker, not just detach it.
            assert_eq!(Arc::strong_count(&inner), 1);
        }
    });
}

#[test]
fn drop_during_blocked_write_and_empty_wait() {
    bounded(|| {
        for blocked in [false, true] {
            let (_peer, write) = miow::pipe::anonymous(4096).unwrap();
            let mut writer = EventedAnonWrite::new(write);
            if blocked {
                let buf = [7; 65536];
                until(|| writer.write(&buf).unwrap() == 0);
            }
            std::thread::sleep(Duration::from_millis(20));
            let inner = writer.inner.clone();
            drop(writer);
            assert_eq!(Arc::strong_count(&inner), 1);
        }
    });
}

#[test]
fn buffered_data_is_drained_before_reader_error_and_drop() {
    bounded(|| {
        let (read, mut peer) = miow::pipe::anonymous(4096).unwrap();
        let mut reader = EventedAnonRead::new(read);
        peer.write_all(b"final bytes").unwrap();
        drop(peer);
        until(|| reader.thread.as_ref().unwrap().is_finished());
        let mut buf = [0; 32];
        let n = reader.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"final bytes");
        assert_eq!(
            reader.read(&mut buf).unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
        drop(reader);
    });
}

#[test]
fn worker_errors_wake_pollers_and_drop_is_safe_after_join() {
    bounded(|| {
        let (read, peer) = miow::pipe::anonymous(4096).unwrap();
        let mut reader = EventedAnonRead::new(read);
        let poll = Poll::new().unwrap();
        poll.register(&reader, Token(1), Ready::readable(), PollOpt::edge())
            .unwrap();
        drop(peer);
        let mut events = corcovado::Events::with_capacity(8);
        poll.poll(&mut events, Some(Duration::from_secs(3)))
            .unwrap();
        assert!(events.iter().any(|e| e.token() == Token(1)));
        until(|| reader.read(&mut [0; 1]).is_err());
        stop_worker(&mut reader.thread);
        drop(reader); // Regression: Drop used to unwrap the consumed handle.

        let (peer, write) = miow::pipe::anonymous(4096).unwrap();
        let mut writer = EventedAnonWrite::new(write);
        drop(peer);
        until(|| writer.write(b"broken").is_err());
        assert!(writer.inner.readiness.readiness().is_writable());
        stop_worker(&mut writer.thread);
        drop(writer);
    });
}

#[test]
fn cancellation_retries_when_read_starts_after_cancel_and_after_deadline() {
    bounded(|| {
        for after_deadline in [false, true] {
            let (mut pipe, _peer) = miow::pipe::anonymous(4096).unwrap();
            let (enter_tx, enter_rx) = channel();
            let (done_tx, done_rx) = channel();
            let mut thread = Some(spawn(move || {
                // Model descheduling between the worker's done check and ReadFile.
                enter_rx.recv().unwrap();
                let result = pipe.read(&mut [0; 1]);
                done_tx.send(result.is_err()).unwrap();
            }));
            if after_deadline {
                let start = Instant::now();
                stop_worker(&mut thread);
                assert!(start.elapsed() < Duration::from_secs(2));
                enter_tx.send(()).unwrap();
            } else {
                spawn(move || {
                    std::thread::sleep(Duration::from_millis(30));
                    enter_tx.send(()).unwrap();
                });
                stop_worker(&mut thread);
            }
            assert!(done_rx.recv_timeout(Duration::from_secs(3)).unwrap());
            assert!(thread.is_none());
        }
    });
}

#[test]
fn drop_while_reader_waits_for_buffer_space() {
    bounded(|| {
        let (read, mut peer) = miow::pipe::anonymous(4096).unwrap();
        let reader = EventedAnonRead::new(read);
        let peer_thread = spawn(move || {
            let _ = peer.write_all(&[9; 256 * 1024]);
        });
        until(|| {
            let _guard = reader.inner.wait_tag.lock();
            reader.consumer.is_full()
        });
        let inner = reader.inner.clone();
        drop(reader);
        assert_eq!(Arc::strong_count(&inner), 1);
        peer_thread.join().unwrap();
    });
}
