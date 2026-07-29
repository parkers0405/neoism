use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use neoism_agent_core::{event_type, EventPayload, SessionStatus};
use serde_json::json;

use crate::provider_error::ProviderError;
use crate::server_util::now_millis;
use crate::session_loop::wait_for_cancellation;
use crate::state::AppState;

const DEFAULT_MAX_RETRIES: u64 = 3;
const DEFAULT_INITIAL_DELAY_MS: u64 = 2_000;
const DEFAULT_MAX_DELAY_MS: u64 = 30_000;
const MAX_HEADER_DELAY_MS: u64 = 2_147_483_647;

pub(crate) fn max_retries() -> u64 {
    std::env::var("NEOISM_AGENT_PROVIDER_MAX_RETRIES")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_MAX_RETRIES)
}

pub(crate) fn retry_delay_ms(attempt: u64) -> u64 {
    retry_delay_ms_with_override(attempt, None)
}

pub(crate) fn retry_delay_ms_for_error(
    attempt: u64,
    error: Option<&anyhow::Error>,
) -> u64 {
    let retry_after = error
        .and_then(|error| error.downcast_ref::<ProviderError>())
        .and_then(|error| error.retry_after_ms);
    retry_delay_ms_with_override(attempt, retry_after)
}

fn retry_delay_ms_with_override(attempt: u64, retry_after_ms: Option<u64>) -> u64 {
    if let Some(retry_after_ms) = retry_after_ms {
        return retry_after_ms.min(MAX_HEADER_DELAY_MS);
    }
    let initial = std::env::var("NEOISM_AGENT_PROVIDER_RETRY_INITIAL_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_INITIAL_DELAY_MS);
    let max = std::env::var("NEOISM_AGENT_PROVIDER_RETRY_MAX_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_MAX_DELAY_MS);
    let multiplier = 1_u64
        .checked_shl(attempt.saturating_sub(1) as u32)
        .unwrap_or(u64::MAX);
    initial.saturating_mul(multiplier).min(max)
}

pub(crate) fn retryable_error(error: &anyhow::Error) -> bool {
    if let Some(provider_error) = error.downcast_ref::<ProviderError>() {
        return provider_error.retryable;
    }
    retryable_message(&error.to_string())
}

pub(crate) fn retryable_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    [
        "429",
        "500",
        "502",
        "503",
        "504",
        "rate limit",
        "too many requests",
        "temporarily unavailable",
        "temporary failure",
        "provider is overloaded",
        "overloaded",
        "timeout",
        "timed out",
        "connection closed",
        "connection reset",
        "connection refused",
        "incomplete message",
        "stream error",
        // Provider-worded transient failures that carry no status code
        // because they arrive MID-STREAM (HTTP 200, error in the body).
        // OpenAI's generic 500 reads "An error occurred while processing
        // your request. You can retry your request..." — an explicit
        // retry signal, so honor it instead of cold-stopping the turn.
        "an error occurred while processing",
        "you can retry",
        "please try again",
        "internal server error",
        "service unavailable",
        "bad gateway",
        "gateway timeout",
        "server had an error",
        "server error",
    ]
    .iter()
    .any(|pattern| lower.contains(pattern))
}

pub(crate) async fn publish_retry_status(
    state: &AppState,
    session_id: &str,
    attempt: u64,
    message: &str,
    delay_ms: u64,
) {
    let status = SessionStatus::Retry {
        attempt,
        message: message.to_string(),
        next: now_millis().saturating_add(delay_ms),
        action: None,
    };
    state
        .inner
        .statuses
        .write()
        .await
        .insert(session_id.to_string(), status.clone());
    state.publish(EventPayload::new(
        event_type::SESSION_STATUS,
        json!({ "sessionID": session_id, "status": status }),
    ));
}

pub(crate) async fn sleep_or_cancel(
    delay_ms: u64,
    cancellation: Arc<AtomicBool>,
) -> bool {
    if delay_ms == 0 {
        return !cancellation.load(Ordering::SeqCst);
    }
    let wait_cancel = cancellation.clone();
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_millis(delay_ms)) => !cancellation.load(Ordering::SeqCst),
        _ = wait_for_cancellation(wait_cancel) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryable_message_detects_transient_provider_failures() {
        assert!(retryable_message("OpenAI-compatible provider returned 429"));
        assert!(retryable_message("Provider is overloaded"));
        assert!(retryable_message("connection reset by peer"));
        assert!(!retryable_message("invalid API key"));
        assert!(!retryable_message("context window exceeded"));
    }

    #[test]
    fn retry_delay_uses_capped_exponential_backoff() {
        assert_eq!(retry_delay_ms(1), DEFAULT_INITIAL_DELAY_MS);
        assert_eq!(retry_delay_ms(2), DEFAULT_INITIAL_DELAY_MS * 2);
        assert_eq!(retry_delay_ms(10), DEFAULT_MAX_DELAY_MS);
    }

    #[test]
    fn retry_delay_prefers_provider_retry_after() {
        let error = anyhow::Error::new(ProviderError {
            provider: "test".to_string(),
            status: Some(429),
            message: "rate limit".to_string(),
            body: None,
            headers: Default::default(),
            retryable: true,
            retry_after_ms: Some(4_200),
            context_overflow: false,
        });

        assert_eq!(retry_delay_ms_for_error(1, Some(&error)), 4_200);
        assert!(retryable_error(&error));
    }
}
