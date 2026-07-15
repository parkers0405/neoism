use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use neoism_agent_core::ProviderStreamEvent;
use tokio_stream::StreamExt;

use crate::provider;

const DEFAULT_PROVIDER_STREAM_IDLE_TIMEOUT_MS: u64 = 120_000;

pub(crate) enum ProviderEventPoll {
    Event(anyhow::Result<ProviderStreamEvent>),
    End,
    Cancelled,
    TimedOut,
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
    if cancellation.load(Ordering::SeqCst) {
        return ProviderEventPoll::Cancelled;
    }
    tokio::select! {
        event = provider_events.next() => match event {
            Some(event) => ProviderEventPoll::Event(event),
            None => ProviderEventPoll::End,
        },
        _ = wait_for_cancellation(cancellation.clone()) => ProviderEventPoll::Cancelled,
        _ = tokio::time::sleep(idle_timeout) => ProviderEventPoll::TimedOut,
    }
}

pub(crate) async fn wait_for_cancellation(cancellation: Arc<AtomicBool>) {
    while !cancellation.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
