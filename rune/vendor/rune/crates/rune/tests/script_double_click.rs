//! A `double-click` script event drives `on_double_press` on the virtual clock, and `on_press`
//! still runs for each of its presses.
use rune::*;

#[derive(Default)]
struct State { presses: u32, doubles: u32 }

fn view(s: &State, _: &mut Cx<State>) -> El {
    col().w_full().h_full()
        .child(el().key("row").size(100.0, 40.0)
            .on_press(|s: &mut State| s.presses += 1)
            .on_double_press(|s: &mut State| s.doubles += 1))
        .child(text(format!("presses {} doubles {}", s.presses, s.doubles)).key("out"))
}

#[test]
fn a_double_click_event_runs_on_double_press() {
    let dir = std::env::temp_dir().join(format!("rune-script-double-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (script, tree) = (dir.join("script.json"), dir.join("tree.json"));
    std::fs::write(&script, r#"[
        {"t": 0.1, "type": "double-click", "key": "row"},
        {"t": 1.0, "type": "click", "key": "row"},
        {"t": 2.0, "type": "click", "key": "row"}
    ]"#).unwrap();
    App::new(AppSpec::new("double", (200.0, 200.0), State::default, view))
        .args(["--script", script.to_str().unwrap(), "--tree", tree.to_str().unwrap()].map(String::from))
        .run();
    let tree: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&tree).unwrap()).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    let out = tree["elements"].as_array().unwrap().iter().find(|e| e["key"] == "out").unwrap();
    // Two separate clicks a second apart are not a pair.
    assert_eq!(out["text"], "presses 4 doubles 1");
}
