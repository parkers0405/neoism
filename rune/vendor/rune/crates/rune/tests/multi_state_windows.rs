//! Windows with different state types share one app; headless tools select one by key and
//! render it with its own state.
use rune::*;

struct Review { count: u32 }
struct Settings { name: String }

fn review_view(s: &Review, _: &mut Cx<Review>) -> El { text(format!("review {}", s.count)).key("out") }
fn settings_view(s: &Settings, _: &mut Cx<Settings>) -> El { text(format!("settings {}", s.name)).key("out") }

fn review() -> App<Review> {
    App::new(AppSpec::new("Review", (400.0, 300.0), || Review { count: 0 }, review_view))
}

fn settings() -> WindowSpec<Settings> {
    WindowSpec::new("settings", AppSpec::new("Settings", (320.0, 200.0), || Settings { name: "default".into() }, settings_view))
}

/// The text and viewport of the window `run` renders with `--tree`.
fn rendered(name: &str, run: impl FnOnce(Vec<String>)) -> (String, serde_json::Value) {
    let dir = std::env::temp_dir().join(format!("rune-multi-state-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let tree = dir.join("tree.json");
    run(vec!["--tree".into(), tree.to_str().unwrap().into()]);
    let tree: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&tree).unwrap()).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    let out = tree["elements"].as_array().unwrap().iter().find(|e| e["key"] == "out").unwrap();
    (out["text"].as_str().unwrap().to_owned(), tree["viewport"].clone())
}

#[test]
fn a_factory_mixes_state_types_and_tools_render_the_selected_one() {
    for (key, text, viewport) in [("review", "review 7", [400.0, 300.0]), ("settings", "settings Kit", [320.0, 200.0])] {
        let (out, size) = rendered(&format!("factory-{key}"), |mut args| {
            args.extend(["--window".into(), key.into()]);
            run_windows_with_args(RunOptions::default(), args, |_| {
                let review = review();
                review.handle().update(|s, _| s.count = 7);
                let settings = settings();
                settings.app.handle().update(|s, _| s.name = "Kit".into());
                vec![WindowSpec::from_app("review", review).boxed(), settings.into()]
            });
        });
        assert_eq!(out, text);
        assert_eq!(size, serde_json::json!(viewport));
    }
}

#[test]
fn app_window_is_selectable_with_its_own_state_and_hidden_until_opened() {
    for (key, text, viewport) in [("main", "review 7", [400.0, 300.0]), ("settings", "settings Kit", [320.0, 200.0])] {
        let (out, size) = rendered(&format!("app-{key}"), |mut args| {
            args.extend(["--window".into(), key.into()]);
            let mut app = review();
            let hidden = WindowOptions { visible: false, ..Default::default() };
            let (settings, window) = app.window(settings().options(hidden));
            assert_eq!(window.key(), "settings");
            assert!(!settings.is_visible() && !window.is_ready());
            // Before the loop runs, both handles queue their requests.
            window.open();
            settings.update(|s, _| s.name = "Kit".into());
            app.handle().update(|s, _| s.count = 7);
            app.args(args).run();
        });
        assert_eq!(out, text);
        assert_eq!(size, serde_json::json!(viewport));
    }
}

#[test]
fn factory_windows_can_add_their_own_secondary_windows() {
    let (out, _) = rendered("nested", |mut args| {
        args.extend(["--window".into(), "settings".into()]);
        run_windows_with_args(RunOptions::default(), args, |_| {
            let mut review = review();
            let (settings, _) = review.window(settings());
            settings.update(|s, _| s.name = "nested".into());
            vec![WindowSpec::from_app("review", review)]
        });
    });
    assert_eq!(out, "settings nested");
}
