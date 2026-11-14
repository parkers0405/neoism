use super::*;

fn fill(rt: &Runtime<Real>, key: &str) -> Color {
    find(rt, key).unwrap_or_else(|| panic!("no {key}")).fill
}

#[test]
fn the_chrome_follows_the_apps_own_dark_setting() {
    let mut rt = runtime_with(base().dark(|s| s.count >= 100), Some(Some("Fade".into())), true);
    assert_eq!(fill(&rt, "playground"), tok::theme(false).bg, "the real state says light");
    rt.state.count = 100;
    rt.update(|_, _| {});
    for _ in 0..60 { rt.frame(DT); }
    assert_eq!(fill(&rt, "playground"), tok::theme(true).bg, "though the system is light");
    assert_eq!(find(&rt, "playground-theme").and_then(|e| e.label).as_deref(), Some("Dark appearance"), "and the stage starts dark with it");
}

#[test]
fn without_it_the_chrome_follows_the_system() {
    let mut rt = runtime(Some(Some("Fade".into())));
    rt.state.count = 100;
    rt.update(|_, _| {});
    for _ in 0..60 { rt.frame(DT); }
    assert_eq!(fill(&rt, "playground"), tok::theme(false).bg);
}
