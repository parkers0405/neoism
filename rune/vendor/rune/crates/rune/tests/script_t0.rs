//! `--script` events at t = 0 run once the first frame is laid out, so they find elements by
//! key and hit them by position like later events.
use rune::*;

#[derive(Default)]
struct State { taps: Vec<&'static str>, typed: String }

fn view(s: &State, _: &mut Cx<State>) -> El {
    col().w_full().h_full()
        .child(el().key("a").size(100.0, 40.0).on_press(|s: &mut State| s.taps.push("a")))
        .child(el().key("b").size(100.0, 40.0).on_press(|s: &mut State| s.taps.push("b")))
        .child(text(format!("taps {} typed {}", s.taps.join(","), s.typed)).key("out"))
}

fn on_key(s: &mut State, k: &KeyEvent, _: &mut Tx) -> bool {
    if let Key::Text(t) = &k.key { s.typed.push_str(t); }
    true
}

#[test]
fn script_events_at_t_zero_see_the_first_layout() {
    let dir = std::env::temp_dir().join(format!("rune-script-t0-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (script, tree) = (dir.join("script.json"), dir.join("tree.json"));
    // An event without "t" is at 0 too.
    std::fs::write(&script, r#"[
        {"t": 0.0, "type": "click", "key": "a"},
        {"type": "type", "text": "x"},
        {"t": 0.2, "type": "tap", "x": 50, "y": 60}
    ]"#).unwrap();
    let spec = AppSpec { on_key: Some(on_key), ..AppSpec::new("t0", (200.0, 200.0), State::default, view) };
    App::new(spec)
        .args(["--script", script.to_str().unwrap(), "--tree", tree.to_str().unwrap()].map(String::from))
        .run();
    let tree: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&tree).unwrap()).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    let out = tree["elements"].as_array().unwrap().iter().find(|e| e["key"] == "out").unwrap();
    assert_eq!(out["text"], "taps a,b typed x");
}
