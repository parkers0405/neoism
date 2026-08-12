#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UsageSnapshot {
    pub input: u64,
    pub output: u64,
    pub reasoning: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total: u64,
    pub cost_micros: u64,
    pub context_limit: Option<u64>,
}

pub fn usage_summary_label(usage: UsageSnapshot, _total_cost_micros: u64) -> String {
    // Context percent only — the price already lives in the side panel's
    // usage details; repeating it next to the input was noise.
    if let Some(limit) = usage.context_limit.filter(|limit| *limit > 0) {
        let percent = usage_percent(usage.total, limit);
        return format!("{percent}%");
    }
    format_token_short(usage.total)
}

pub fn usage_detail_lines(
    usage: UsageSnapshot,
    total_cost_micros: u64,
    model: &str,
) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(limit) = usage.context_limit.filter(|limit| *limit > 0) {
        lines.push(format!(
            "Context {}%   {} / {} tokens",
            usage_percent(usage.total, limit),
            format_count(usage.total),
            format_count(limit)
        ));
    } else {
        lines.push(format!("Context {} tokens", format_count(usage.total)));
    }
    // The latest turn reflects the current provider/auth route. If it is free
    // (OAuth subscription or a zero-cost model), do not surface paid estimates
    // retained from older turns that used different or stale catalog metadata.
    if usage.cost_micros > 0 {
        lines.push(format!("Total price {}", format_cost(total_cost_micros)));
        lines.push(format!("Last turn {}", format_cost(usage.cost_micros)));
    }
    lines.push(format!("Input {}", format_count(usage.input)));
    lines.push(format!("Output {}", format_count(usage.output)));
    lines.push(format!("Reasoning {}", format_count(usage.reasoning)));
    lines.push(format!("Cache read {}", format_count(usage.cache_read)));
    lines.push(format!("Cache write {}", format_count(usage.cache_write)));
    if !model.is_empty() {
        lines.push(format!("Model {model}"));
    }
    lines
}

fn usage_percent(total: u64, limit: u64) -> u64 {
    if limit == 0 {
        return 0;
    }
    ((total as f64 / limit as f64) * 100.0).round().max(0.0) as u64
}

fn format_cost(micros: u64) -> String {
    let dollars = micros as f64 / 1_000_000.0;
    if dollars < 0.01 {
        format!("${dollars:.6}")
    } else {
        format!("${dollars:.4}")
    }
}

fn format_token_short(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}m", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}k", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

fn format_count(value: u64) -> String {
    let s = value.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    let first = s.len() % 3;
    for (ix, ch) in s.chars().enumerate() {
        if ix > 0 && (ix % 3) == first {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(total: u64, cost_micros: u64, limit: Option<u64>) -> UsageSnapshot {
        UsageSnapshot {
            input: 12_345,
            output: 2_000,
            reasoning: 800,
            cache_read: 1_000_000,
            cache_write: 42,
            total,
            cost_micros,
            context_limit: limit,
        }
    }

    #[test]
    fn summary_prefers_context_percent_when_limit_exists() {
        assert_eq!(
            usage_summary_label(usage(32_000, 10_000, Some(128_000)), 25_000),
            // Price intentionally omitted — it lives in the side panel's
            // usage details, not next to the input.
            "25%"
        );
    }

    #[test]
    fn summary_falls_back_to_short_token_count_without_limit() {
        assert_eq!(
            usage_summary_label(usage(1_250_000, 250_000, None), 2_500_000),
            "1.2m"
        );
    }

    #[test]
    fn detail_lines_match_agent_usage_chip_copy() {
        assert_eq!(
            usage_detail_lines(usage(32_000, 10_000, Some(128_000)), 25_000, "gpt-x"),
            vec![
                "Context 25%   32,000 / 128,000 tokens",
                "Total price $0.0250",
                "Last turn $0.0100",
                "Input 12,345",
                "Output 2,000",
                "Reasoning 800",
                "Cache read 1,000,000",
                "Cache write 42",
                "Model gpt-x",
            ]
        );
    }

    #[test]
    fn detail_lines_omit_zero_price_for_subscription_models() {
        assert_eq!(
            usage_detail_lines(usage(32_000, 0, Some(128_000)), 0, "gpt-5.5"),
            vec![
                "Context 25%   32,000 / 128,000 tokens",
                "Input 12,345",
                "Output 2,000",
                "Reasoning 800",
                "Cache read 1,000,000",
                "Cache write 42",
                "Model gpt-5.5",
            ]
        );
    }

    #[test]
    fn detail_lines_omit_stale_price_after_switching_to_subscription_auth() {
        let lines =
            usage_detail_lines(usage(32_000, 0, Some(400_000)), 256_900, "gpt-5.6-sol");

        assert!(lines.iter().all(|line| !line.contains("price")));
        assert!(lines.iter().all(|line| !line.contains("Last turn")));
    }
}
