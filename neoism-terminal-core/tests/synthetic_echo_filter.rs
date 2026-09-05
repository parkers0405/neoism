use neoism_terminal_core::handler::SyntheticEchoFilter;

#[test]
fn exact_synthetic_echo_is_removed_across_fragments_but_prompt_survives() {
    let mut filter = SyntheticEchoFilter::default();
    filter.expect_command(b"cd -- '/tmp/a b'\n");
    assert!(filter.filter(b"cd -- '").is_empty());
    assert!(filter.filter(b"/tmp/a b'\r").is_empty());
    assert_eq!(filter.filter(b"\n\x1b]7;file:///tmp/a%20b\x07prompt"), b"\x1b]7;file:///tmp/a%20b\x07prompt");
}

#[test]
fn mismatch_flushes_prefix_and_never_hides_user_output() {
    let mut filter = SyntheticEchoFilter::default();
    filter.expect_command(b"cd /expected\n");
    assert!(filter.filter(b"cd /").is_empty());
    assert_eq!(filter.filter(b"user typed\r\n"), b"cd /user typed\r\n");
    assert_eq!(filter.filter(b"cd /expected\r\n"), b"cd /expected\r\n");
}

#[test]
fn exact_lf_echo_is_removed_without_hiding_trailing_output() {
    let mut filter = SyntheticEchoFilter::default();
    filter.expect_command(b"cd /tmp\n");
    assert_eq!(filter.filter(b"cd /tmp\nprompt"), b"prompt");
}