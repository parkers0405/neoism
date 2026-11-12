use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use neoism_agent_core::ProviderStreamEvent;
use tokio_stream::StreamExt;

use crate::provider;

// Allow five minutes between provider events for legitimate deep reasoning,
// while retaining the between-event watchdog and retry recovery.
const DEFAULT_PROVIDER_STREAM_IDLE_TIMEOUT_MS: u64 = 300_000;

// A host that sleeps mid-response usually wakes to a provider connection that is gone
// without a word: the peer hung up while this machine could not hear it, and a reader that
// sends nothing never finds out. The idle watchdog would get there, minutes later (its
// clock does not run while the host sleeps). After a wake, a stream that stays silent
// this long is treated as that dead connection; one that is alive delivers what the
// provider buffered at once.
const DEFAULT_PROVIDER_STREAM_WAKE_GRACE_MS: u64 = 30_000;
// The wall clock moving this far past a one-second timer is a suspended host, not a
// busy one.
const HOST_SLEEP_GAP: Duration = Duration::from_secs(30);

pub(crate) enum ProviderEventPoll {
    Event(anyhow::Result<ProviderStreamEvent>),
    End,
    Cancelled,
    TimedOut,
    /// The host slept and the stream has said nothing since it woke.
    SilentAfterWake,
}

pub(crate) fn provider_stream_wake_grace() -> Duration {
    let grace_ms = std::env::var("NEOISM_AGENT_PROVIDER_STREAM_WAKE_GRACE_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_PROVIDER_STREAM_WAKE_GRACE_MS);
    Duration::from_millis(grace_ms)
}

/// Resolves `grace` after the host is seen to wake from sleep; never, on a host that
/// stays awake. Timers here run on the monotonic clock, which stops while the host is
/// suspended (and on systems where it does not, they all fire at once on wake): either
/// way the wall clock is then far ahead of a one-second tick.
pub(crate) async fn silent_after_host_wake(
    grace: Duration,
    wall_clock: impl Fn() -> std::time::SystemTime,
) {
    silent_after_wake(grace, Duration::from_secs(1), HOST_SLEEP_GAP, wall_clock).await
}

async fn silent_after_wake(
    grace: Duration,
    tick: Duration,
    sleep_gap: Duration,
    wall_clock: impl Fn() -> std::time::SystemTime,
) {
    let mut seen = wall_clock();
    loop {
        tokio::time::sleep(tick).await;
        let now = wall_clock();
        let gap = now.duration_since(seen).unwrap_or_default();
        seen = now;
        if gap >= sleep_gap {
            break;
        }
    }
    tokio::time::sleep(grace).await;
}

pub(crate) fn provider_stream_idle_timeout() -> Duration {
    let timeout_ms = std::env::var("NEOISM_AGENT_PROVIDER_STREAM_IDLE_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_PROVIDER_STREAM_IDLE_TIMEOUT_MS);
    Duration::from_millis(timeout_ms)
}

pub(crate) async fn next_provider_stream_event(
    provider_events: &mut provider::ProviderEventStream,
    cancellation: &Arc<AtomicBool>,
    idle_timeout: Duration,
) -> ProviderEventPoll {
    next_provider_stream_event_or_wake(provider_events, cancellation, idle_timeout, None).await
}

/// `wake_grace`: how long a stream may stay silent after the host wakes from sleep before
/// it is given up on (`None`: only the idle timeout applies). Callers pass it only while
/// giving up is safe to recover from.
pub(crate) async fn next_provider_stream_event_or_wake(
    provider_events: &mut provider::ProviderEventStream,
    cancellation: &Arc<AtomicBool>,
    idle_timeout: Duration,
    wake_grace: Option<Duration>,
) -> ProviderEventPoll {
    if cancellation.load(Ordering::SeqCst) {
        return ProviderEventPoll::Cancelled;
    }
    let woke = async {
        match wake_grace {
            Some(grace) => silent_after_host_wake(grace, std::time::SystemTime::now).await,
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        event = provider_events.next() => match event {
            Some(event) => ProviderEventPoll::Event(event),
            None => ProviderEventPoll::End,
        },
        _ = wait_for_cancellation(cancellation.clone()) => ProviderEventPoll::Cancelled,
        _ = tokio::time::sleep(idle_timeout) => ProviderEventPoll::TimedOut,
        _ = woke => ProviderEventPoll::SilentAfterWake,
    }
}

pub(crate) async fn wait_for_cancellation(cancellation: Arc<AtomicBool>) {
    while !cancellation.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[cfg(test)]
mod wake_tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::SystemTime;

    const TICK: Duration = Duration::from_millis(5);
    const GRACE: Duration = Duration::from_millis(150);

    /// A wall clock the test moves by hand: it follows real time plus whatever the
    /// "host slept".
    fn clock() -> (Arc<Mutex<Duration>>, impl Fn() -> SystemTime) {
        let slept = Arc::new(Mutex::new(Duration::ZERO));
        let read = slept.clone();
        (slept, move || SystemTime::now() + *read.lock().unwrap())
    }

    #[tokio::test]
    async fn an_awake_host_never_trips_the_wake_watchdog() {
        let (_slept, read) = clock();
        let watchdog = tokio::spawn(silent_after_wake(GRACE, TICK, Duration::from_secs(30), read));
        // A silent stream on a host that is awake: both clocks agree.
        tokio::time::sleep(GRACE * 3).await;
        assert!(!watchdog.is_finished());
        watchdog.abort();
    }

    #[tokio::test]
    async fn a_stream_silent_after_a_sleep_is_given_up_on_after_the_grace() {
        let (slept, read) = clock();
        let watchdog = tokio::spawn(silent_after_wake(GRACE, TICK, Duration::from_secs(30), read));
        tokio::time::sleep(TICK * 4).await;
        // The lid closes for ten minutes: the wall clock moves, the timers do not.
        let woke = std::time::Instant::now();
        *slept.lock().unwrap() = Duration::from_secs(600);
        tokio::time::sleep(GRACE / 2).await;
        assert!(!watchdog.is_finished(), "a live stream gets its grace to speak");
        watchdog.await.unwrap();
        assert!(woke.elapsed() >= GRACE);
    }
}
