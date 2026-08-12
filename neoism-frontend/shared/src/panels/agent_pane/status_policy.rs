//! Shared status/update policies for the agent pane.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueStatusDecision {
    pub count: usize,
    pub preview: Option<String>,
    pub should_enter_thinking: bool,
    pub started_at: Option<u64>,
}

pub fn queue_status_decision(
    count: usize,
    preview: Option<String>,
    started_at: Option<u64>,
    is_streaming: bool,
) -> QueueStatusDecision {
    QueueStatusDecision {
        count,
        preview,
        should_enter_thinking: started_at.is_some() && !is_streaming,
        started_at,
    }
}

/// Turn a raw provider retry message into one short status-row reason.
///
/// Provider bodies are often full sentences (and frequently repeat
/// "please try again"). The timeline status already says "Retrying", so keep
/// only the useful cause and cap unknown messages before they can crowd the
/// composer.
pub fn compact_retry_reason(message: &str) -> Option<String> {
    let message = message.split_whitespace().collect::<Vec<_>>().join(" ");
    let message = message.trim();
    if message.is_empty() {
        return None;
    }
    let lower = message.to_ascii_lowercase();
    let known = if lower.contains("overloaded") || lower.contains("at capacity") {
        Some("Provider overloaded")
    } else if lower.contains("rate limit")
        || lower.contains("too many requests")
        || lower.contains("429")
    {
        Some("Rate limited")
    } else if lower.contains("timed out") || lower.contains("timeout") {
        Some("Provider timeout")
    } else if lower.contains("connection reset")
        || lower.contains("connection closed")
        || lower.contains("stream error")
    {
        Some("Connection interrupted")
    } else if lower.contains("service unavailable") || lower.contains("503") {
        Some("Service unavailable")
    } else {
        None
    };
    if let Some(known) = known {
        return Some(known.to_string());
    }

    const MAX_CHARS: usize = 72;
    let mut compact = message
        .trim_end_matches(|ch: char| matches!(ch, '.' | '!' | '?'))
        .to_string();
    if compact.chars().count() > MAX_CHARS {
        compact = compact.chars().take(MAX_CHARS - 1).collect();
        compact.push('…');
    }
    Some(compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_queue_status_preserves_authoritative_count_and_preview() {
        assert_eq!(
            queue_status_decision(3, Some("queued prompt".to_string()), None, false),
            QueueStatusDecision {
                count: 3,
                preview: Some("queued prompt".to_string()),
                should_enter_thinking: false,
                started_at: None,
            }
        );
    }

    #[test]
    fn active_queue_status_preserves_prompt_and_enters_thinking_when_idle() {
        assert_eq!(
            queue_status_decision(2, Some("next prompt".to_string()), Some(1234), false),
            QueueStatusDecision {
                count: 2,
                preview: Some("next prompt".to_string()),
                should_enter_thinking: true,
                started_at: Some(1234),
            }
        );
    }

    #[test]
    fn active_queue_status_does_not_replace_existing_streaming_state() {
        let decision = queue_status_decision(1, None, Some(1234), true);
        assert_eq!(decision.count, 1);
        assert!(!decision.should_enter_thinking);
    }

    #[test]
    fn retry_reason_compacts_provider_noise() {
        assert_eq!(
            compact_retry_reason(
                "Our servers are currently overloaded. Please try again later."
            )
            .as_deref(),
            Some("Provider overloaded")
        );
        assert_eq!(
            compact_retry_reason("  connection   reset by peer  ").as_deref(),
            Some("Connection interrupted")
        );
        assert_eq!(compact_retry_reason("   "), None);
    }
}
