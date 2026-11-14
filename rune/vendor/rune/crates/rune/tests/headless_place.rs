use rune::*;

static HANDLE: std::sync::OnceLock<AppHandle<()>> = std::sync::OnceLock::new();

fn view(_: &(), _: &mut Cx<()>) -> El { el() }

fn placed(app: App<()>, name: &str) -> serde_json::Value {
    let dir = std::env::temp_dir().join(format!("rune-headless-place-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (script, tree) = (dir.join("desktop.json"), dir.join("tree.json"));
    std::fs::write(&script, r#"{
        "displays": [{"id": 1, "frame": [0, 0, 1512, 982], "scale": 2, "primary": true},
                     {"id": 2, "frame": [1512, 0, 1920, 1080], "scale": 1}],
        "pointer": [2000, 400]
    }"#).unwrap();
    app.args(["--script", script.to_str().unwrap(), "--tree", tree.to_str().unwrap()].map(String::from)).run();
    let tree: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&tree).unwrap()).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    tree["placed"].clone()
}

#[test]
fn headless_place_resolves_against_scripted_displays() {
    let app = App::new(AppSpec::new("place", (600.0, 80.0), || (), view));
    app.handle().place(DisplayTarget::UnderPointer,
        OverlayPlacement::Edge { edge: ScreenEdge::Top, margin: 160.0, align: Align::Center, inset: 0.0 });
    assert_eq!(placed(app, "handle"), serde_json::json!({"t": 0.0, "display": 2, "frame": [2172.0, 160.0, 600.0, 80.0]}));

    // poll runs inside the snapshot frame; its placement must still be recorded.
    fn poll(_: &mut (), _: f64, _: &mut Tx) -> bool {
        HANDLE.get().unwrap().place(DisplayTarget::Primary, OverlayPlacement::FullScreen);
        false
    }
    let app = App::new(AppSpec { poll: Some(poll), ..AppSpec::new("poll", (600.0, 80.0), || (), view) });
    HANDLE.set(app.handle()).ok().unwrap();
    assert_eq!(placed(app, "poll"), serde_json::json!({"t": 0.0, "display": 1, "frame": [0.0, 0.0, 1512.0, 982.0]}));
}
