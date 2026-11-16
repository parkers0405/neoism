//! Exercise the production parser without optional upstream shader/font unit-test fixtures.
#[path = "../src/frame/present_time/summary.rs"]
mod summary;
use summary::Summary;

#[test]
fn sustained_443_presentations_with_one_double_interval_do_not_hide_the_miss() {
    // Shape of the parent's 2200–5900 ms capture: overwhelmingly 8.333 ms with one
    // 16.667 ms interval. Metadata is explicit; never infer refresh from that median.
    let mut lines = vec![r#"{"hz":60,"clock":"wall"}"#.to_owned()];
    let mut at = 2200.0;
    for i in 0..443 {
        let dt = if i == 220 { 16.667 } else { 8.333 };
        if i > 0 { at += dt; }
        let mut frame = serde_json::json!({"t": at, "present_ms": at, "refresh_hz": 120});
        if i > 0 { frame["present_dt_ms"] = dt.into(); }
        lines.push(frame.to_string());
    }
    let s = Summary::of_trace(&lines.join("\n")).unwrap();
    assert_eq!((s.presented, s.missed), (443, 1));
    let interval = s.interval.unwrap();
    assert_eq!((interval.p50, interval.p95, interval.max), (8.333, 8.333, 16.667));
}

#[test]
fn live_120hz_overrides_startup_60hz_and_detects_one_missed_interval() {
    let trace = r#"{"hz":60,"clock":"wall"}
{"t":0,"refresh_hz":60,"present_ms":10}
{"t":8.333,"refresh_hz":120,"present_ms":18.333,"present_dt_ms":8.333}
{"t":25,"refresh_hz":120,"present_ms":35,"present_dt_ms":16.667}"#;
    let s = Summary::of_trace(trace).unwrap();
    assert_eq!(s.hz, (120.0, 120.0));
    assert_eq!(s.missed, 1);
    assert!(s.to_string().contains("at 120 Hz"));
    assert!(s.to_string().contains("1 missed (>12.50 ms)"));
}

#[test]
fn cadence_changes_use_each_frames_period_not_the_final_or_fastest_rate() {
    let trace = r#"{"hz":60}
{"t":0,"refresh_hz":120,"present_ms":10,"present_dt_ms":16.667}
{"t":16,"refresh_hz":60,"present_ms":26.667,"present_dt_ms":16.667}
{"t":50,"refresh_hz":60,"present_ms":60,"present_dt_ms":33.333}
{"t":58,"refresh_hz":120,"present_ms":68.333,"present_dt_ms":8.333}"#;
    let s = Summary::of_trace(trace).unwrap();
    assert_eq!(s.hz, (60.0, 120.0));
    assert_eq!(s.missed, 2);
    assert!(s.to_string().contains("60–120 Hz (per-frame)"));
    assert!(s.to_string().contains("2 missed (>1.5 per-frame refresh periods)"));
}

#[test]
fn legacy_header_fallback_and_invalid_metadata_remain_supported() {
    for metadata in ["", ",\"refresh_hz\":null", ",\"refresh_hz\":0", ",\"refresh_hz\":-120"] {
        let trace = format!("{{\"hz\":120}}\n{{\"t\":0,\"present_ms\":20,\"present_dt_ms\":16.667{metadata}}}");
        assert_eq!(Summary::of_trace(&trace).unwrap().missed, 1);
    }
}

#[test]
fn targets_and_refresh_are_not_substituted_for_actual_presentations() {
    assert!(Summary::of_trace("{\"hz\":60}\n{\"t\":0,\"target_ms\":8.333,\"refresh_hz\":120}").is_none());
    let s = Summary::of_trace("{\"hz\":60}\n{\"t\":0,\"present_ms\":null,\"refresh_hz\":120}").unwrap();
    assert_eq!((s.presented, s.skipped, s.missed), (0, 1, 0));
    assert!(s.interval.is_none());
}