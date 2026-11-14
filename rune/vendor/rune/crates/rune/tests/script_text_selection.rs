//! `--script` drags, double-clicks and triple-clicks selectable text, and `--tree` reports each
//! element's selected text.
use rune::*;

const CMD: &str = "opencode serve --port 4096";
const PARA: &str = "first line\nsecond line\nthird";

fn view(_: &(), _: &mut Cx<()>) -> El {
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    col().w_full().h_full().padding(10.0).gap(10.0)
        .child(text(CMD).key("cmd").font(style.mono()).self_start().selectable(true))
        .child(text(PARA).key("para").font(style).self_start().selectable(true))
}

/// Runs `events` and returns each keyed element's selected text.
fn run(events: &str, name: &str) -> Vec<(String, String)> {
    let dir = std::env::temp_dir().join(format!("rune-script-select-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (script, tree) = (dir.join("script.json"), dir.join("tree.json"));
    std::fs::write(&script, events).unwrap();
    App::new(AppSpec::new("select", (300.0, 200.0), || (), view))
        .args(["--script", script.to_str().unwrap(), "--tree", tree.to_str().unwrap()].map(String::from))
        .run();
    let tree: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&tree).unwrap()).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    tree["elements"].as_array().unwrap().iter()
        .filter_map(|e| Some((e["key"].as_str()?.to_owned(), e["selected"].as_str()?.to_owned())))
        .collect()
}

fn selected(key: &str, text: &str) -> Vec<(String, String)> { vec![(key.to_owned(), text.to_owned())] }

#[test]
fn script_events_select_text() {
    // Events aim at the element's centre: inside "serve", and on the middle line.
    // A drag from there past the end selects the rest of the line.
    let dragged = run(r#"[{"t": 0.1, "type": "drag", "key": "cmd", "dx": 150, "dy": 0}]"#, "drag");
    let [(key, text)] = &dragged[..] else { panic!("{dragged:?}") };
    assert!(key == "cmd" && text.ends_with("e --port 4096") && CMD[9..].ends_with(text.as_str()), "{text:?}");
    assert_eq!(run(r#"[{"t": 0.1, "type": "double-click", "key": "cmd"}]"#, "double"), selected("cmd", "serve"));
    assert_eq!(run(r#"[{"t": 0.1, "type": "triple-click", "key": "para"}]"#, "triple"), selected("para", "second line\n"));
}
