//! `--script` taps a link (`El::link`) by its id, as it taps an element by key. A wrapped link
//! is tapped on its first line.
use rune::*;

const TEXT: &str = "see docs or a very long link that wraps";

fn at(part: &str) -> std::ops::Range<usize> {
    let i = TEXT.find(part).unwrap();
    i..i + part.len()
}

fn view(opened: &Vec<String>, _: &mut Cx<Vec<String>>) -> El {
    col().w_full().h_full().padding(10.0).gap(10.0)
        .child(rich(TEXT, Vec::new()).key("message").font(TextStyle::new(14.0, 400.0).lh(20.0)).w(120.0)
            .link(Link::new(at("docs"), "https://docs.dev/a"))
            .link(Link::new(at("a very long link"), "long"))
            .on_link(|s: &mut Vec<String>, id, _| s.push(id.to_owned())))
        .child(text(format!("opened {}", opened.join(","))).key("opened"))
}

#[test]
fn script_taps_links_by_id() {
    let dir = std::env::temp_dir().join(format!("rune-script-links-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (script, tree) = (dir.join("script.json"), dir.join("tree.json"));
    std::fs::write(&script, r#"[
        {"t": 0.1, "type": "tap", "key": "https://docs.dev/a"},
        {"t": 0.3, "type": "tap", "key": "long"}
    ]"#).unwrap();
    App::new(AppSpec::new("links", (300.0, 200.0), Vec::new, view))
        .args(["--script", script.to_str().unwrap(), "--tree", tree.to_str().unwrap()].map(String::from))
        .run();
    let tree: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&tree).unwrap()).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    let opened = tree["elements"].as_array().unwrap().iter().find(|e| e["key"] == "opened").unwrap();
    assert_eq!(opened["text"], "opened https://docs.dev/a,long");
}
