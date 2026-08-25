use std::collections::BTreeMap;
use std::fmt;

use reqwest::header::HeaderMap;
use reqwest::StatusCode;
use serde_json::Value;

#[derive(Clone, Debug)]
pub(crate) struct ProviderError {
    pub(crate) provider: String,
    pub(crate) status: Option<u16>,
    pub(crate) message: String,
    pub(crate) body: Option<String>,
    pub(crate) retryable: bool,
    pub(crate) retry_after_ms: Option<u64>,
    pub(crate) context_overflow: bool,
}

impl ProviderError {
    pub(crate) fn from_response(
        provider: impl Into<String>,
        status: StatusCode,
        headers: &HeaderMap,
        body: String,
    ) -> Self {
        let provider = provider.into();
        let headers = headers_to_map(headers);
        let retry_after_ms = retry_after_ms(&headers);
        let message = response_message(&body)
            .unwrap_or_else(|| format!("{provider} provider returned {status}"));
        let context_overflow =
            is_context_overflow(&message) || is_context_overflow(&body);
        let retryable = !context_overflow && is_retryable_status(status);
        Self {
            provider,
            status: Some(status.as_u16()),
            message,
            body: (!body.trim().is_empty()).then_some(body),
            retryable,
            retry_after_ms,
            context_overflow,
        }
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(status) = self.status {
            write!(
                f,
                "{} provider returned {status}: {}",
                self.provider, self.message
            )
        } else {
            write!(f, "{} provider error: {}", self.provider, self.message)
        }
    }
}

impl std::error::Error for ProviderError {}

pub(crate) fn retry_after_ms(headers: &BTreeMap<String, String>) -> Option<u64> {
    if let Some(value) = headers.get("retry-after-ms") {
        if let Ok(ms) = value.trim().parse::<f64>() {
            if ms.is_finite() && ms >= 0.0 {
                return Some(ms.ceil() as u64);
            }
        }
    }
    if let Some(value) = headers.get("retry-after") {
        if let Ok(seconds) = value.trim().parse::<f64>() {
            if seconds.is_finite() && seconds >= 0.0 {
                return Some((seconds * 1_000.0).ceil() as u64);
            }
        }
    }
    None
}

fn is_retryable_status(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS
        || status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::BAD_GATEWAY
        || status == StatusCode::SERVICE_UNAVAILABLE
        || status == StatusCode::GATEWAY_TIMEOUT
        || status.as_u16() >= 500
}

fn headers_to_map(headers: &HeaderMap) -> BTreeMap<String, String> {
    headers
        .iter()
        .filter_map(|(key, value)| {
            Some((
                key.as_str().to_ascii_lowercase(),
                value.to_str().ok()?.to_string(),
            ))
        })
        .collect()
}

fn response_message(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    string_at(&value, &["error", "message"])
        .or_else(|| string_at(&value, &["message"]))
        .or_else(|| string_at(&value, &["error"]))
}

fn string_at(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current.as_str().map(ToString::to_string)
}

fn is_context_overflow(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    (lower.contains("context") && lower.contains("window"))
        || lower.contains("maximum context")
        || lower.contains("context length")
        || lower.contains("too many tokens")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_error_parses_retry_after_ms() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after-ms", "1250".parse().unwrap());

        let error = ProviderError::from_response(
            "OpenAI",
            StatusCode::TOO_MANY_REQUESTS,
            &headers,
            r#"{"error":{"message":"rate limit"}}"#.to_string(),
        );

        assert!(error.retryable);
        assert_eq!(error.retry_after_ms, Some(1250));
        assert_eq!(error.message, "rate limit");
        assert_eq!(error.status, Some(429));
    }

    #[test]
    fn provider_error_does_not_retry_context_overflow() {
        let error = ProviderError::from_response(
            "OpenAI",
            StatusCode::BAD_REQUEST,
            &HeaderMap::new(),
            r#"{"error":{"message":"exceeds the context window"}}"#.to_string(),
        );

        assert!(!error.retryable);
        assert!(error.context_overflow);
    }
}
