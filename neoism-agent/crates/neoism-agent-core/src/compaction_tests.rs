use super::*;
use serde_json::json;

#[test]
fn compaction_defaults_and_safety_ceiling() {
    let policy = CompactionConfig::default();
    assert!(policy.enabled());
    assert_eq!(policy.threshold(200_000, 168_000), 130_000);
    assert_eq!(policy.threshold(200_000, 100_000), 100_000);
    let policy: CompactionConfig =
        serde_json::from_value(json!({"threshold-percent": 100})).unwrap();
    assert_eq!(policy.threshold(200_000, 190_000), 180_000);
    assert_eq!(policy.threshold(0, 0), 0);
}

#[test]
fn compaction_percent_is_validated() {
    for value in [json!(-1), json!(0), json!(100.1), json!("65")] {
        assert!(serde_json::from_value::<CompactionConfig>(
            json!({"threshold-percent": value})
        )
        .is_err());
    }
    for value in [1.0, 65.5, 100.0] {
        let policy: CompactionConfig =
            serde_json::from_value(json!({"threshold-percent": value})).unwrap();
        assert_eq!(policy.threshold_percent, Some(value));
    }
}

#[test]
fn compaction_overlay_preserves_inheritance_and_explicit_false() {
    let mut policy: CompactionConfig = serde_json::from_value(json!({
        "auto": true, "threshold-percent": 65, "buffer": 12000, "keep": {"tokens": 8000}
    }))
    .unwrap();
    let overlay: CompactionConfig = serde_json::from_value(json!({
        "auto": false, "threshold-percent": 50, "keep": {"tokens": 0}
    }))
    .unwrap();
    policy.overlay(&overlay);
    assert!(!policy.enabled());
    assert_eq!(policy.threshold_percent, Some(50.0));
    assert_eq!(policy.buffer, Some(12000));
    assert_eq!(policy.keep.tokens, Some(0));
    policy.overlay(&serde_json::from_value(json!({"auto": true})).unwrap());
    assert!(policy.enabled());
}

#[test]
fn compaction_round_trips_at_all_configuration_scopes() {
    let value = json!({
        "compaction": {"threshold-percent": 65},
        "provider": {"test": {"models": {"model": {"compaction": {"threshold-percent": 75}}}}},
        "agent": {"explore": {"compaction": {"auto": false}}}
    });
    let config: AgentConfigDocument = serde_json::from_value(value).unwrap();
    assert_eq!(config.compaction.threshold_percent, Some(65.0));
    assert_eq!(
        config.provider["test"].models["model"]
            .compaction
            .threshold_percent,
        Some(75.0)
    );
    assert!(!config.agent["explore"].compaction.enabled());
    let value = serde_json::to_value(config).unwrap();
    assert_eq!(value["compaction"]["threshold-percent"], json!(65.0));
}
