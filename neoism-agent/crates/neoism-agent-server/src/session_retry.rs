use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use neoism_agent_core::{event_type, EventPayload, SessionStatus};
use serde_json::json;

use crate::provider_error::ProviderError;
use crate::server_util::now_millis;
use crate::session_loop::wait_for_cancellation;
use crate::state::AppState;

// Transient provider errors (5xx, "an error occurred… you can retry", stream
// resets) should keep retrying so a run doesn't visibly STOP on a blip — the
// user just sees a brief "retrying" status and it continues. A blip recovers
// on the first attempt or two; only a sustained outage walks the full ladder
// (~2.5 min with the backoff below), after which the error is genuinely fatal
// and surfaces. Override with NEOISM_AGENT_PROVIDER_MAX_RETRIES.
const DEFAULT_MAX_RETRIES: u64 = 8;
const DEFAULT_INITIAL_DELAY_MS: u64 = 1_500;
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
        // A context-overflow never recovers by retrying — it must surface.
        if provider_error.context_overflow {
            return false;
        }
        // Retry on a retryable STATUS code, OR when the provider's own error
        // TEXT says it's transient ("an error occurred… you can retry",
        // "overloaded", worded 5xx, etc). Previously this returned early on the
        // status flag alone, so an OpenAI 200-with-error-body (no retryable
        // status) or a message-only 5xx slipped straight to a hard stop — the
        // "retry never fired" bug. Consult the message and raw body too.
        return provider_error.retryable
            || retryable_message(&provider_error.message)
            || provider_error
                .body
                .as_deref()
                .is_some_and(retryable_message);
    }
    error.chain().any(|cause| {
        retryable_message(&cause.to_string())
            || cause.downcast_ref::<reqwest::Error>().is_some_and(|error| {
                error.is_timeout()
                    || error.is_connect()
                    || error.is_request()
                    || error.is_body()
                    // Reqwest classifies failures while decoding a response
                    // body separately from raw body I/O. A network change can
                    // truncate an encoded streaming response and surface as
                    // `Kind::Decode` ("error decoding response body"). Treat
                    // that like the other transient transport failures so the
                    // provider step is reset and safely re-streamed.
                    || error.is_decode()
            })
    })
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
    fn retryable_error_inspects_wrapped_transport_causes() {
        let inner = anyhow::anyhow!("connection reset by peer");
        let error =
            inner.context("failed to send OpenAI OAuth Responses streaming request");

        assert!(retryable_error(&error));
    }

    #[tokio::test]
    async fn retryable_error_accepts_reqwest_response_decode_failures() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test HTTP server");
        let address = listener.local_addr().expect("test server address");
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept test request");
            let mut request = [0_u8; 1024];
            let _ = socket.read(&mut request).await.expect("read test request");
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 1\r\nconnection: close\r\n\r\n{",
                )
                .await
                .expect("write malformed JSON response");
        });

        let response = reqwest::get(format!("http://{address}"))
            .await
            .expect("receive test response");
        let decode_error = response
            .json::<serde_json::Value>()
            .await
            .expect_err("malformed JSON must fail response decoding");
        assert!(decode_error.is_decode());
        assert!(retryable_error(&anyhow::Error::new(decode_error)));

        server.await.expect("test HTTP server task");
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
            retryable: true,
            retry_after_ms: Some(4_200),
            context_overflow: false,
        });

        assert_eq!(retry_delay_ms_for_error(1, Some(&error)), 4_200);
        assert!(retryable_error(&error));
    }
}
