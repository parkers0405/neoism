//! `rune::extra_args()` is ready before any `App` or window exists.

const CHILD: &str = "RUNE_EXTRA_ARGS_CHILD";

/// Runs in a child process with Rune and app flags after libtest's own: nothing has parsed
/// the arguments yet when it asks for its extras.
#[test]
fn child_reads_extras_before_run() {
    if std::env::var_os(CHILD).is_none() { return; }
    let extra = rune::extra_args();
    assert!(extra.ends_with(&["--doc".to_string(), "a.txt".to_string()]), "{extra:?}");
    assert!(!extra.iter().any(|a| a == "--scale" || a == "2"), "Rune's own flags are not extras: {extra:?}");
}

#[test]
fn extra_args_are_ready_before_any_window() {
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["child_reads_extras_before_run", "--exact", "--", "--scale", "2", "--doc", "a.txt"])
        .env(CHILD, "1")
        .status()
        .unwrap();
    assert!(status.success(), "the child read its extras before run");
}
