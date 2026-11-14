use super::*;
use crate::time::Instant;

#[test]
fn trace_annotations_from_hidden_poll_survive_idle_catch_up_once() {
    let mut rt = Runtime::new(false, |_, _: &mut Cx<bool>| crate::el(), (100.0, 100.0), Env::default());
    let (trace, buffer) = rune_motion::MotionTrace::in_memory(&rune_motion::TraceHeader {
        launch: "hidden".into(), width: 100.0, height: 100.0, hz: 60.0, scale: 1.0,
    });
    rt.scene.set_trace(Some(trace));
    rt.trace_channels = Some(Default::default());
    rt.poll = Some(|pending, _, tx| {
        if !std::mem::take(pending) { return false; }
        tx.trace_event("hidden:ready");
        tx.trace_channel("shell.width", "position", 40.0);
        true
    });
    rt.frame(0.0);
    rt.state = true;
    let commits = rt.stats.commits;
    rt.poll_hidden();
    assert_eq!(rt.stats.commits, commits, "hidden polling must not lay out or render");
    rt.frame_split(1.0, 0.0);
    rt.frame(0.02);
    assert_eq!(rt.events, ["hidden:ready"]);
    assert_eq!(rt.trace_channels.as_ref().unwrap()["shell.width"], ("position", 40.0));
    rt.scene.take_trace().unwrap().flush().unwrap();
    let lines: Vec<Value> = buffer.contents().lines().map(|s| serde_json::from_str(s).unwrap()).collect();
    assert_eq!(lines.iter().filter(|line| line["ev"] == json!(["hidden:ready"])).count(), 1);
}

#[test]
fn trace_annotations_reach_both_writers_once() {
    fn view(changed: &bool, _: &mut Cx<bool>) -> El {
        let (class, host) = if *changed { ("other", "other-host") } else { ("gpui", "pill") };
        crate::el().key("root").trace_namespace(class, host)
            .child(crate::el().key("generation-7").trace_name("chip:Area").size(30.0, 20.0))
            .child(crate::el().key("fixed").trace_name("fixed").trace_namespace("gpui", "pill"))
    }
    let mut rt = Runtime::new(false, view, (100.0, 100.0), Env { scale: 1.0, system_dark: true });
    let (trace, buffer) = rune_motion::MotionTrace::in_memory(&rune_motion::TraceHeader {
        launch: "pill".into(), width: 100.0, height: 100.0, hz: 60.0, scale: 1.0,
    });
    rt.scene.set_trace(Some(trace));
    rt.trace_channels = Some(Default::default());
    let path = std::env::temp_dir().join(format!("rune-annotations-{}.jsonl", std::process::id()));
    {
        let mut trace = Trace::open(&path, rt.viewport, 60.0, 1.0);
        rt.update(|_, tx| {
            assert!(tx.tracing());
            tx.trace_event(format_args!("lab:{}", "Start"));
            tx.trace_channel("shell.width", "position", 30.0);
        });
        // A second transaction before the frame must retain the first event.
        rt.update(|_, tx| {
            tx.trace_event("lab:Ready");
            tx.trace_channel("shell.width", "position", 40.0);
        });
        rt.frame(0.0);
        trace.frame(&mut rt, 0.0, 0.0);
        rt.frame(0.02);
        trace.frame(&mut rt, 20.0, 0.0);
        rt.update(|s, _| *s = true);
        rt.frame(0.02);
        trace.frame(&mut rt, 40.0, 0.0);
    }
    rt.scene.take_trace().unwrap().flush().unwrap();
    let tooling = std::fs::read_to_string(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    for text in [tooling, buffer.contents()] {
        let lines: Vec<Value> = text.lines().map(|s| serde_json::from_str(s).unwrap()).collect();
        let chip = lines[1]["m"].as_array().unwrap().iter().find(|m| m["nm"] == "chip:Area").unwrap();
        assert_eq!(chip["cls"], "gpui");
        assert_eq!(chip["host"], "pill");
        assert_eq!(lines[1]["ev"], json!(["lab:Start", "lab:Ready"]));
        let channel = lines[1]["cm"].as_array().unwrap().iter().find(|m| m["name"] == "shell.width").unwrap();
        assert_eq!(channel["kind"], "position");
        assert!(lines[1]["c"].as_array().unwrap().iter().any(|c| c[0] == channel["id"] && c[1].as_f64() == Some(40.0)));
        for field in ["cm", "c", "ev"] { assert!(lines[2][field].is_null(), "duplicate {field}"); }
        let updated = lines[3]["m"].as_array().unwrap();
        let renamed = updated.iter().find(|m| m["nm"] == "chip:Area").unwrap();
        assert_eq!(renamed["id"], chip["id"]);
        assert_eq!(renamed["cls"], "other");
        assert_eq!(renamed["host"], "other-host");
        assert!(!updated.iter().any(|m| m["nm"] == "fixed"), "descendant override must survive parent namespace changes");
    }
}

#[test]
fn native_traces_add_target_and_present_times_without_changing_other_fields() {
    struct Presents(std::sync::Mutex<Vec<crate::frame::present_time::Presented>>);
    impl crate::frame::present_time::Source for Presents {
        fn take(&self, out: &mut Vec<crate::frame::present_time::Presented>) { out.append(&mut self.0.lock().unwrap()); }
    }
    let view = |_: &bool, _: &mut Cx<bool>| crate::el().key("box").size(20.0, 20.0);
    let run = |name: &str, timed: bool| {
        let mut rt = Runtime::new(false, view, (100.0, 100.0), Env { scale: 1.0, system_dark: true });
        let path = std::env::temp_dir().join(format!("rune-present-{name}-{}.jsonl", std::process::id()));
        let start = Instant::now();
        let source = std::sync::Arc::new(Presents(Default::default()));
        {
            let mut trace = Trace::open_clock(&path, rt.viewport, 120.0, 1.0, "wall");
            if timed { trace.record_presents(source.clone(), start); }
            for k in 0..3u64 {
                rt.frame(1.0 / 120.0);
                let timing = FrameTiming { target_ms: timed.then_some(k as f64 * 8.333), drawable: timed.then_some(k + 1), continuous: k > 0 };
                trace.frame_timed(&mut rt, k as f64 * 8.0, 0.0, timing);
                let at = start + std::time::Duration::from_micros(20_000 + k * 8_333);
                source.0.lock().unwrap().push((k + 1, Some(at)));
            }
        }
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        text
    };
    let plain = run("plain", false);
    let timed = run("timed", true);
    let strip = |text: &str| -> Vec<Value> {
        text.lines().map(|l| {
            let mut v: Value = serde_json::from_str(l).unwrap();
            for key in ["target_ms", "present_ms", "present_dt_ms"] { v.as_object_mut().unwrap().remove(key); }
            if let Some(q) = v.get_mut("quality") { q["cpu_ms"] = 0.into(); }
            v
        }).collect()
    };
    assert_eq!(strip(&plain), strip(&timed), "present timing only adds fields");
    assert!(!plain.contains("present_ms") && !plain.contains("target_ms"), "untimed frames keep the old format");
    let frames: Vec<Value> = timed.lines().skip(1).map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(frames.len(), 3, "every held line is written when the trace ends");
    assert_eq!(frames[1]["target_ms"], 8.333);
    assert_eq!(frames[1]["present_ms"], 28.333);
    assert_eq!(frames[1]["present_dt_ms"], 8.333);
    let summary = crate::frame::present_time::Summary::of_trace(&timed).unwrap();
    assert_eq!((summary.frames, summary.presented, summary.bunched, summary.missed), (3, 3, 0, 0));
    assert!(crate::frame::present_time::Summary::of_trace(&plain).is_none());
}

#[test]
fn marks_are_inert_events_that_times_can_name() {
    let script = parse_script(json!([
        {"t": 0.5, "type": "tap", "x": 1, "y": 1},
        {"t": 3.2, "type": "mark", "name": "end"},
        {"t": 1.0, "type": "mark", "name": "end-card"},
    ]));
    let marks: Vec<_> = script.iter().filter_map(|e| match &e.kind { Kind::Mark(n) => Some((n.as_str(), e.t)), _ => None }).collect();
    assert_eq!(marks, [("end-card", 1.0), ("end", 3.2)]);
    assert!(matches!(script[4].public().event, ScriptEvent::Mark(ref n) if n == "end"));

    assert_eq!(mark_time("mark:end"), Some("end".into()));
    assert_eq!(mark_time("mark:end+0.5"), Some("end+0.5".into()));
    assert_eq!(mark_time("1.5"), None);

    let args = |v: &[&str]| Args::parse_from(v.iter().map(|s| (*s).into()).collect());
    let timeline = |v: &[&str]| Timeline::of(&args(v), &script);
    let snap = timeline(&["--snap", "a.png", "--at", "mark:end+0.5"]).unwrap();
    assert_eq!((snap.snap_frame, snap.frames), (222, 0));
    let video = timeline(&["--video", "a.mp4", "--start", "mark:end-card"]).unwrap();
    assert_eq!((video.start_frame, video.frames, video.last_frame), (60, 133, 192), "auto runs to the end mark");
    let auto = timeline(&["--frames", "1", "--duration", "auto"]).unwrap();
    assert_eq!(auto.frames, 1, "--frames wins over --duration");
    let seconds = timeline(&["--video", "a.mp4", "--duration", "2", "--fps", "30"]).unwrap();
    assert_eq!((seconds.frames, seconds.last_frame), (60, 59));
    let err = timeline(&["--snap", "a.png", "--at", "mark:intro"]).unwrap_err();
    assert_eq!(err, "no mark \"intro\" in the script (marks: end-card, end)");
    let unmarked = Timeline::of(&args(&["--video", "a.mp4"]), &parse_script(json!([{"t": 0.5, "type": "tap", "x": 1, "y": 1}]))).unwrap();
    assert_eq!(unmarked.frames, 96, "without an end mark, one second after the last event (the tap's release at 0.58 s)");
}

/// Headless runs publish the process's `headless_run`, so tests that render one take turns.
static HEADLESS: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn mark_names_win_over_offsets_and_offsets_must_be_complete() {
    let script = parse_script(json!([
        {"t": 2.0, "type": "mark", "name": "step-1"},
        {"t": 4.0, "type": "mark", "name": "end-card"},
        {"t": 6.0, "type": "mark", "name": "end+0.5"},
        {"t": 8.0, "type": "mark", "name": "step"},
        {"t": 10.0, "type": "mark", "name": "end"},
    ]));
    let resolve = |flag: &str, value: &str| {
        let args = Args::parse_from(["--snap", "a.png", "--fps", "1000", flag, value].map(String::from).into());
        Timeline::of(&args, &script)
    };
    for (value, frame) in [("mark:step-1", 2000), ("mark:step-1+0.25", 2250),
        ("mark:step-1-0.5", 1500), ("mark:end+0.5", 6000), ("mark:end-card-0.25", 3750),
        ("mark:end-1e-3", 9999), ("mark:end+1e-3", 10001)] {
        assert_eq!(resolve("--at", value).unwrap().snap_frame, frame, "{value}");
        assert_eq!(resolve("--start", value).unwrap().start_frame, frame, "{value}");
    }
    for value in ["mark:end+0.5s", "mark:end-", "mark:end+NaN", "mark:end+inf", "mark:end+1e999"] {
        let error = resolve("--at", value).unwrap_err();
        assert!(error.contains("invalid mark time") && error.contains("marks: step-1, end-card, end+0.5, step, end"), "{error}");
    }
    let args = Args::parse_from(["--at", "mark:end", "--at", "1", "--start", "mark:step", "--start", "0"].map(String::from).into());
    let timeline = Timeline::of(&args, &script).unwrap();
    assert_eq!((timeline.snap_frame, timeline.start_frame), (60, 0), "the last flag wins");
}

#[test]
fn duration_and_out_select_headless_png_output_unless_native() {
    let _turn = HEADLESS.lock().unwrap_or_else(|e| e.into_inner());
    let args = |v: &[&str]| Args::parse_from(v.iter().map(|s| (*s).into()).collect());
    for flags in [vec!["--out", "frames"], vec!["--duration", "auto"], vec!["--duration", "auto", "--out", "frames"]] {
        assert!(args(&flags).headless());
        let mut native = flags;
        native.push("--native");
        assert!(!args(&native).headless());
    }
    let dir = std::env::temp_dir().join(format!("rune-duration-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("script.json");
    std::fs::write(&script, r#"[{"t": 0.1, "type": "mark", "name": "end"}]"#).unwrap();
    let spec = || AppSpec::new("duration", (8.0, 8.0), || (), |_, _| crate::el());
    for automatic in [false, true] {
        let out = dir.join(if automatic { "explicit" } else { "default" });
        let mut flags = vec!["--script", script.to_str().unwrap(), "--out", out.to_str().unwrap(), "--fps", "30", "--scale", "1"];
        if automatic { flags.extend(["--duration", "auto"]); }
        let args = args(&flags);
        assert!(args.headless());
        headless(crate::App::new(spec()), args);
        let mut files: Vec<_> = std::fs::read_dir(out).unwrap().map(|e| e.unwrap().file_name()).collect();
        files.sort();
        assert_eq!(files, ["frame_0000.png", "frame_0001.png", "frame_0002.png", "frame_0003.png"]);
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn headless_runs_report_each_frame_to_on_frame_encoded() {
    let _turn = HEADLESS.lock().unwrap_or_else(|e| e.into_inner());
    let snap = std::env::temp_dir().join(format!("rune-encoded-{}.png", std::process::id()));
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let log = seen.clone();
    let app = crate::App::new(AppSpec::new("encoded", (8.0, 8.0), || (), |_, _| crate::el())).on_frame_encoded(move |f| {
        log.borrow_mut().push((f.seq(), f.outcome(), f.window().map(str::to_owned), f.refresh_hz(), f.started().is_some(), f.update_ms(), f.encode_ms()));
    });
    headless(app, Args { snap: Some(snap.clone()), at: Some(0.1), fps: 30.0, scale: Some(1.0), ..Default::default() });
    std::fs::remove_file(snap).unwrap();
    let seen = seen.borrow();
    assert_eq!(seen.len(), 4, "frames 0 to 3 at 30 fps: {seen:?}");
    for (i, (seq, outcome, window, hz, started, update, encode)) in seen.iter().enumerate() {
        assert_eq!((*seq, *outcome, window.as_deref(), *hz, *started), (i as u64, crate::FrameOutcome::Drawn, Some("main"), Some(30.0), true));
        assert!(update.is_some_and(|ms| ms >= 0.0) && encode.is_some_and(|ms| ms >= 0.0));
    }
}

#[test]
fn single_window_snapshots_honor_transparency_and_physical_size() {
    let _turn = HEADLESS.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("rune-window-options-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let render = |name: &str, options: crate::WindowOptions, size: Option<(f32, f32)>, legacy: bool| {
        let spec = AppSpec::new("options", (20.0, 12.0), || (), |_, _| crate::el().w_full().h_full()
            .child(crate::el().absolute(2.0, 2.0).size(8.0, 8.0).fill(crate::oklcha(0.6, 0.2, 30.0, 0.5))));
        let app = crate::App::new(spec).window_options(options);
        let snap = dir.join(format!("{name}.png"));
        let args = Args { snap: Some(snap.clone()), scale: Some(2.0), size, ..Default::default() };
        if legacy { headless_configured(app, args, false); } else { headless(app, args); }
        (std::fs::read(&snap).unwrap(), image::open(snap).unwrap().into_rgba8())
    };
    let plain = render("plain", crate::WindowOptions::default(), None, false);
    let legacy = render("legacy", crate::WindowOptions::default(), None, true);
    assert_eq!(plain.0, legacy.0, "default windows retain byte-identical PNG output");
    assert_eq!(plain.1.dimensions(), (40, 24));
    assert!(plain.1.pixels().all(|p| p[3] == 255));
    let options = crate::WindowOptions { transparent: true, physical_size: Some((80, 60)), ..Default::default() };
    let transparent = render("transparent", options.clone(), None, false).1;
    assert_eq!(transparent.dimensions(), (80, 60));
    assert_eq!(transparent.get_pixel(0, 0)[3], 0);
    assert!((127..=128).contains(&transparent.get_pixel(8, 8)[3]));
    let overridden = render("override", options, Some((16.0, 10.0)), false).1;
    assert_eq!(overridden.dimensions(), (32, 20), "--size overrides the physical-size option");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn window_resizes_move_the_headless_viewport_on_the_virtual_clock() {
    let _turn = HEADLESS.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("rune-window-resize-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("script.json");
    std::fs::write(&script, r#"[{"t": 0.1, "type": "tap", "key": "grow"}]"#).unwrap();
    let viewport = |name: &str, instant: bool, at: &str| {
        let spec = AppSpec::new("resize", (300.0, 400.0), || false, |_, _| crate::el().key("root").w_full().h_full()
            .child(crate::el().key("grow").size(50.0, 50.0).on_press_tx(|instant: &mut bool, tx| {
                if *instant { tx.resize_window_instant((300.0, 600.0)) } else { tx.resize_window((300.0, 600.0)) }
            })));
        let tree = dir.join(format!("{name}.json"));
        let args = ["--script", script.to_str().unwrap(), "--scale", "1", "--fps", "60", "--at", at, "--tree", tree.to_str().unwrap()];
        headless(crate::App::new(spec).init(move || instant), Args::parse_from(args.map(String::from).into()));
        let tree: Value = serde_json::from_str(&std::fs::read_to_string(tree).unwrap()).unwrap();
        let size = tree["viewport"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect::<Vec<_>>();
        (size[0], size[1])
    };
    assert_eq!(viewport("before", false, "0.1"), (300.0, 400.0));
    // The press lands with the release at 0.18 s, on frame 11; as on macOS, frame 12 moves.
    assert_eq!(viewport("released", false, "0.1834"), (300.0, 400.0));
    let (_, first) = viewport("first", false, "0.2");
    assert!(first > 400.0, "moves the frame after the press: {first}");
    let (w, h) = viewport("moving", false, "0.3");
    assert!(w == 300.0 && h > 400.0 && h < 600.0, "mid-spring at 0.3 s: {h}");
    assert_eq!(viewport("moving-again", false, "0.3"), (w, h), "the virtual clock is deterministic");
    assert_eq!(viewport("landed", false, "2"), (300.0, 600.0));
    assert_eq!(viewport("instant", true, "0.3"), (300.0, 600.0));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_scripted_resize_steps_the_viewport_with_the_hand() {
    let _turn = HEADLESS.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("rune-script-resize-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("script.json");
    std::fs::write(&script, r#"[{"t": 0.1, "type": "resize", "dw": -100, "dh": -40, "dur": 0.5}]"#).unwrap();
    let viewport = |at: &str| {
        let spec = AppSpec::new("resize", (300.0, 400.0), || (), |_, _| crate::el().key("root").w_full().h_full());
        let tree = dir.join(format!("{at}.json"));
        let args = ["--script", script.to_str().unwrap(), "--scale", "1", "--fps", "60", "--at", at, "--tree", tree.to_str().unwrap()];
        headless(crate::App::new(spec), Args::parse_from(args.map(String::from).into()));
        let tree: Value = serde_json::from_str(&std::fs::read_to_string(tree).unwrap()).unwrap();
        let size = tree["viewport"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect::<Vec<_>>();
        (size[0], size[1])
    };
    assert_eq!(viewport("0.05"), (300.0, 400.0));
    let (w, h) = viewport("0.35");
    assert!(w < 300.0 && w > 200.0 && h < 400.0 && h > 360.0, "mid-resize: {w}x{h}");
    assert_eq!(viewport("1"), (200.0, 360.0));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn fit_content_height_follows_the_content() {
    let _turn = HEADLESS.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("rune-fit-content-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("script.json");
    std::fs::write(&script, r#"[{"t": 0.1, "type": "tap", "key": "more"}]"#).unwrap();
    let viewport = |name: &str, at: &str| {
        let spec = AppSpec::new("fit", (300.0, 400.0), || 1usize, |rows, _| crate::col().key("root").w_full()
            .child(crate::el().key("more").size(50.0, 50.0).on_press(|rows: &mut usize| *rows += 1))
            .children((0..*rows).map(|i| crate::el().key(format!("row{i}")).size(100.0, 40.0))));
        let tree = dir.join(format!("{name}.json"));
        let args = ["--script", script.to_str().unwrap(), "--scale", "2", "--fps", "60", "--at", at, "--tree", tree.to_str().unwrap()];
        headless(crate::App::new(spec).fit_content_height(), Args::parse_from(args.map(String::from).into()));
        let tree: Value = serde_json::from_str(&std::fs::read_to_string(tree).unwrap()).unwrap();
        let size = tree["viewport"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect::<Vec<_>>();
        (size[0], size[1])
    };
    assert_eq!(viewport("open", "0"), (300.0, 90.0), "opens at the content's height");
    let (_, h) = viewport("moving", "0.3");
    assert!(h > 90.0 && h < 130.0, "follows on the change's spring: {h}");
    assert_eq!(viewport("landed", "2"), (300.0, 130.0));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn snapshots_leave_the_playground_out_unless_asked() {
    let _turn = HEADLESS.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("rune-playground-snap-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    fn view(n: &u32, _: &mut Cx<u32>) -> crate::El {
        crate::col().w_full().h_full().padding(8.0).child(crate::text(format!("count {n}")))
    }
    let render = |name: &str, playground: bool, open: Option<Option<String>>| {
        let spec = AppSpec::new("snap", (240.0, 160.0), || 3, view);
        let mut app = crate::App::new(spec);
        if playground { app = app.playground(crate::Playground::new().enabled(true).bar(true)); }
        let snap = dir.join(format!("{name}.png"));
        headless(app, Args { snap: Some(snap.clone()), scale: Some(2.0), playground: open, ..Default::default() });
        std::fs::read(&snap).unwrap()
    };
    let plain = render("plain", false, None);
    assert!(plain == render("with", true, None), "a playground with a bar snapshots as the app alone");
    assert!(plain != render("open", true, Some(None)), "--playground shows it");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn apps_read_the_headless_run_from_init() {
    let _turn = HEADLESS.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("rune-run-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("script.json");
    std::fs::write(&script, r#"[{"t": 0.5, "type": "tap", "x": 4, "y": 5}, {"t": 0.25, "type": "type", "text": "hi", "every": 0.1}]"#).unwrap();
    fn init() -> usize {
        let run = headless_run().expect("published before init");
        assert_eq!((run.size(), run.scale(), run.fps()), ((37.0, 23.0), 1.0, 30.0));
        run.events().len()
    }
    let spec = AppSpec::new("run", (37.0, 23.0), init, |_, _| crate::el());
    let tree = dir.join("tree.json");
    let args = ["--script", script.to_str().unwrap(), "--scale", "1", "--fps", "30", "--tree", tree.to_str().unwrap()];
    headless(crate::App::new(spec), Args::parse_from(args.map(String::from).into()));
    let run = headless_run().unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(run.script(), Some(script.as_path()));
    let summary: Vec<String> = run.events().iter().map(|e| match &e.event {
        ScriptEvent::Key { key: KeyEvent { key: Key::Text(c), .. }, repeat: false } => format!("{} key {c}", e.t),
        ScriptEvent::Pointer { action, at } => format!("{} {action:?} {at:?}", e.t),
        other => panic!("unexpected {other:?}"),
    }).collect();
    assert_eq!(summary, ["0.25 key h", "0.35 key i", "0.5 Move At(4.0, 5.0)", "0.5 Down At(4.0, 5.0)", "0.58 Up FromDown(0.0, 0.0)"]);
    assert_eq!((run.script_end(), run.frames(), run.start()), (0.58, 0, 0.0));
    // A tree is taken one second after the script, at the nearest frame.
    assert!((run.end() - 47.0 / 30.0).abs() < 1e-9, "{}", run.end());
}

#[test]
fn video_streams_every_frame_into_ffmpeg() {
    let _turn = HEADLESS.lock().unwrap_or_else(|e| e.into_inner());
    if std::process::Command::new("ffprobe").arg("-version").output().is_err() {
        eprintln!("skipped: ffmpeg and ffprobe aren't installed");
        return;
    }
    let dir = std::env::temp_dir().join(format!("rune-video-{}", std::process::id()));
    for (extension, codec, width) in [("mp4", "h264", 32), ("mov", "prores", 31), ("webm", "vp9", 32)] {
        let path = dir.join(format!("tiny.{extension}"));
        let out = dir.join(format!("{extension}-frames"));
        let snap = dir.join(format!("{extension}-last.png"));
        let spec = AppSpec::new("video", (31.0, 20.0), || (), |_, _| crate::el().w_full().h_full()
            .child(crate::el().absolute(0.0, 0.0).size(10.0, 10.0).fill(crate::oklch(0.6, 0.2, 30.0))));
        let args = ["--video", path.to_str().unwrap(), "--frames", "12", "--scale", "1", "--fps", "30",
            "--out", out.to_str().unwrap(), "--every", "3", "--snap", snap.to_str().unwrap(), "--at", "0.3"];
        let app = crate::App::new(spec).window_options(crate::WindowOptions {
            transparent: extension == "mov", ..Default::default()
        });
        headless(app, Args::parse_from(args.map(String::from).into()));
        let probe = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-count_frames", "-select_streams", "v:0",
                "-show_entries", "stream=codec_name,width,height,nb_read_frames,r_frame_rate", "-of", "json"])
            .arg(&path).output().unwrap();
        assert!(probe.status.success(), "{}", String::from_utf8_lossy(&probe.stderr));
        let probe: Value = serde_json::from_slice(&probe.stdout).unwrap();
        let stream = &probe["streams"][0];
        assert_eq!(stream["codec_name"], codec);
        assert_eq!(stream["width"], width, "420 formats pad odd sizes; ProRes does not");
        assert_eq!(stream["height"], 20);
        assert_eq!(stream["r_frame_rate"], "30/1");
        assert_eq!(stream["nb_read_frames"], "12", "--every must not sample video");
        if extension == "mov" {
            let decoded = std::process::Command::new("ffmpeg")
                .args(["-v", "error", "-i"]).arg(&path)
                .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgba", "-"]).output().unwrap();
            assert!(decoded.status.success(), "{}", String::from_utf8_lossy(&decoded.stderr));
            assert_eq!(decoded.stdout.len(), 31 * 20 * 4);
            assert_eq!(decoded.stdout[(15 * 31 + 20) * 4 + 3], 0, "transparent background survives ProRes encoding");
            assert_eq!(decoded.stdout[(5 * 31 + 5) * 4 + 3], 255);
        }
        let mut files: Vec<_> = std::fs::read_dir(&out).unwrap().map(|e| e.unwrap().file_name()).collect();
        files.sort();
        assert_eq!(files, ["frame_0000.png", "frame_0003.png", "frame_0006.png", "frame_0009.png"]);
        assert_eq!(std::fs::read(snap).unwrap(), std::fs::read(out.join("frame_0009.png")).unwrap());
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn desktop_composites_each_window_at_its_frame_with_a_shadow() {
    let _turn = HEADLESS.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("rune-desktop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let backdrop = dir.join("backdrop.png");
    ::image::RgbaImage::from_pixel(240, 160, ::image::Rgba([128, 128, 128, 255])).save(&backdrop).unwrap();
    let script = dir.join("script.json");
    // A click on the back window gives it the keyboard and brings it to the front.
    std::fs::write(&script, r#"[{"t": 0.1, "type": "tap", "x": 45, "y": 35}]"#).unwrap();
    let fill = || AppSpec::new("fill", (1.0, 1.0), || crate::Color::TRANSPARENT, |c, _| crate::el().w_full().h_full().fill(*c));
    let red = crate::Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };
    let blue = crate::Color { r: 0.0, g: 0.0, b: 1.0, a: 1.0 };
    let render = |name: &str| {
        let snap = dir.join(name);
        let args = ["--desktop", backdrop.to_str().unwrap(), "--script", script.to_str().unwrap(),
            "--scale", "2", "--at", "0.5", "--snap", snap.to_str().unwrap()];
        crate::run_windows_with_args(crate::RunOptions::default(), args.map(String::from), move |displays| {
            assert_eq!(displays[0].size, (240, 160), "one display the backdrop's size");
            [crate::WindowSpec::new("back", fill()).state(red).options(crate::WindowOptions {
                position: Some((40, 40)), physical_size: Some((100, 60)), ..Default::default()
            }), crate::WindowSpec::new("panel", fill()).state(blue).configured_options(crate::WindowOptions {
                borderless: true, position: Some((160, 100)), physical_size: Some((40, 40)), ..Default::default()
            }.macos(crate::MacWindowOptions::default().shadow(false)))]
        });
        std::fs::read(snap).unwrap()
    };
    let bytes = render("a.png");
    assert_eq!(bytes, render("b.png"), "byte-identical across runs");
    let png = ::image::load_from_memory(&bytes).unwrap().into_rgba8();
    let px = |x: u32, y: u32| png.get_pixel(x, y).0;
    assert_eq!(png.dimensions(), (240, 160));
    // The back window covers points (20, 20) to (70, 50); the panel (80, 50) to (100, 70).
    let (red, blue) = (|p: [u8; 4]| p[0] > 245 && p[1] < 10 && p[2] < 10, |p: [u8; 4]| p[2] > 245 && p[0] < 10 && p[1] < 10);
    assert!(red(px(90, 70)) && red(px(40, 70)) && red(px(139, 70)) && red(px(90, 40)) && red(px(90, 99)), "the window's pixels land at its frame");
    assert!(!red(px(39, 70)) && !red(px(140, 70)) && !red(px(90, 100)), "and stop at its edges");
    assert!(blue(px(160, 100)) && blue(px(199, 139)) && blue(px(180, 120)));
    assert!(!red(px(40, 40)), "a titled window's corners are rounded");
    let below = px(90, 106);
    assert!(below[0] < 112 && below[0] == below[2], "a soft shadow under the window: {below:?}");
    assert_eq!(px(180, 146), [128, 128, 128, 255], "no shadow when the window asks for none");
    assert_eq!(px(4, 156), [128, 128, 128, 255], "the backdrop elsewhere");
    let run = headless_run().unwrap();
    assert_eq!(run.desktop(), Some(backdrop.as_path()));
    let frames = run.windows_at(0.5);
    let back = frames.iter().find(|f| f.key == "back").unwrap();
    assert_eq!((back.frame, back.visible, back.focused), ([20.0, 20.0, 50.0, 30.0], true, true));
    assert!(run.window_frames().iter().any(|f| f.key == "panel" && f.focused && f.t == 0.0), "the topmost window starts with the keyboard");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A `--desktop` window whose key handler does `act` to another window.
struct Pair { name: &'static str, color: crate::Color, other: Option<crate::AppHandle<Pair>>, act: fn(&crate::AppHandle<Pair>, &mut crate::Tx) }
/// The windows `Pair`'s key handler ran in, in order.
static KEYED: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());
const RED: crate::Color = crate::Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };
const GREEN: crate::Color = crate::Color { r: 0.0, g: 1.0, b: 0.0, a: 1.0 };
const BLUE: crate::Color = crate::Color { r: 0.0, g: 0.0, b: 1.0, a: 1.0 };

fn pair(name: &'static str, color: crate::Color, other: Option<crate::AppHandle<Pair>>, act: fn(&crate::AppHandle<Pair>, &mut crate::Tx), options: crate::WindowOptions) -> (crate::WindowSpec<Pair>, crate::AppHandle<Pair>) {
    let spec = AppSpec {
        on_key: Some(|p: &mut Pair, _: &crate::KeyEvent, tx: &mut crate::Tx| {
            KEYED.lock().unwrap().push(p.name);
            if let Some(other) = &p.other { (p.act)(other, tx); }
            true
        }),
        ..AppSpec::new("pair", (1.0, 1.0), || Pair { name: "", color: crate::Color::TRANSPARENT, other: None, act: |_, _| {} },
            |p, _| crate::el().w_full().h_full().fill(p.color))
    };
    let app = crate::App::new(spec).init(move || Pair { name, color, other, act });
    let handle = app.handle();
    (crate::WindowSpec::from_app(name, app).options(options), handle)
}

/// Runs `windows` over a grey 120x80-point backdrop at scale 2: the snap at `at`, and the run.
fn desktop_case(name: &str, script: &str, at: f64, extra: &[&str], windows: impl FnOnce() -> Vec<crate::AnyWindowSpec> + 'static) -> (::image::RgbaImage, &'static HeadlessRun) {
    let dir = std::env::temp_dir().join(format!("rune-desktop-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let backdrop = dir.join("backdrop.png");
    ::image::RgbaImage::from_pixel(240, 160, ::image::Rgba([128, 128, 128, 255])).save(&backdrop).unwrap();
    let (script_path, snap) = (dir.join("script.json"), dir.join("snap.png"));
    std::fs::write(&script_path, script).unwrap();
    KEYED.lock().unwrap().clear();
    let at = at.to_string();
    let mut args = vec!["--desktop", backdrop.to_str().unwrap(), "--script", script_path.to_str().unwrap(),
        "--scale", "2", "--fps", "10", "--at", &at, "--snap", snap.to_str().unwrap()];
    args.extend_from_slice(extra);
    crate::run_windows_with_args(crate::RunOptions::default(), args.into_iter().map(String::from).collect::<Vec<_>>(), move |_| windows());
    let png = ::image::open(&snap).unwrap().into_rgba8();
    std::fs::remove_dir_all(&dir).unwrap();
    (png, headless_run().unwrap())
}

/// Window pixels at (20, 20) to (120, 80) of the output, and at (160, 40) to (220, 100).
fn left() -> crate::WindowOptions { crate::WindowOptions { position: Some((20, 20)), physical_size: Some((100, 60)), ..Default::default() } }
fn right() -> crate::WindowOptions { crate::WindowOptions { position: Some((160, 40)), physical_size: Some((60, 60)), ..Default::default() } }
fn focus_of(run: &HeadlessRun, t: f64, key: &str) -> (bool, bool) {
    let f = run.windows_at(t).into_iter().find(|f| f.key == key).unwrap();
    (f.visible, f.focused)
}

#[test]
fn desktop_frame_shows_what_a_handler_did_to_another_window_that_frame() {
    let _turn = HEADLESS.lock().unwrap_or_else(|e| e.into_inner());
    // The launcher's Return updates and focuses the notes window and closes the launcher;
    // the frame of that key shows the new note in the focused window.
    let (png, run) = desktop_case("stale", r#"[{"t": 0.1, "type": "key", "name": "Enter"}]"#, 0.1, &[], || {
        let (notes, handle) = pair("notes", RED, None, |_, _| {}, left());
        let (launcher, _) = pair("launcher", BLUE, Some(handle), |notes, tx| {
            notes.update(|p, tx| { p.color = GREEN; tx.instant(); });
            notes.focus();
            tx.close_window();
        }, right());
        vec![notes.boxed(), launcher.boxed()]
    });
    assert_eq!(*KEYED.lock().unwrap(), ["launcher"]);
    let p = png.get_pixel(70, 50).0;
    assert!(p[1] > 245 && p[0] < 10, "the note changed in this frame: {p:?}");
    assert_eq!((focus_of(&run, 0.1, "notes"), focus_of(&run, 0.1, "launcher")), ((true, true), (false, false)));
}

#[test]
fn desktop_window_closing_itself_on_blur_closes_in_that_frame() {
    let _turn = HEADLESS.lock().unwrap_or_else(|e| e.into_inner());
    // Return in `a` focuses `b`, which blurs `a`, whose on_focus closes it.
    let script = r#"[{"t": 0.1, "type": "tap", "x": 30, "y": 25}, {"t": 0.2, "type": "key", "name": "Enter"}]"#;
    let (png, run) = desktop_case("blur", script, 0.2, &[], || {
        let (b, handle) = pair("b", BLUE, None, |_, _| {}, right());
        let (a, _) = pair("a", RED, Some(handle), |b, _| b.focus(), left());
        vec![a.on_focus(|_, focused, tx| if !focused { tx.close_window() }).boxed(), b.boxed()]
    });
    assert_eq!(focus_of(&run, 0.1, "a"), (true, true));
    assert_eq!((focus_of(&run, 0.2, "a"), focus_of(&run, 0.2, "b")), ((false, false), (true, true)));
    assert_eq!(png.get_pixel(40, 50).0, [128, 128, 128, 255], "a is gone from the frame");

    // The last shown window closes; losing the keyboard, it opens `c`, which shows at once.
    let (png, run) = desktop_case("blur-open", r#"[{"t": 0.1, "type": "key", "name": "Enter"}]"#, 0.1, &[], || {
        let (c, handle) = pair("c", BLUE, None, |_, _| {}, crate::WindowOptions { visible: false, ..right() });
        let (a, _) = pair("a", RED, Some(handle), |_, tx| tx.close_window(), left());
        let a = a.on_focus(|p, focused, _| if !focused && let Some(c) = &p.other { c.open() });
        vec![c.boxed(), a.boxed()]
    });
    assert_eq!((focus_of(&run, 0.1, "a"), focus_of(&run, 0.1, "c")), ((false, false), (true, true)));
    let p = png.get_pixel(190, 70).0;
    assert!(p[2] > 245 && p[0] < 10, "c draws in that frame: {p:?}");
}

#[test]
fn desktop_click_on_the_backdrop_deactivates_the_app_until_it_refocuses() {
    let _turn = HEADLESS.lock().unwrap_or_else(|e| e.into_inner());
    // As on macOS, a click on the desktop activates another app: no window has focus and
    // keys go nowhere until the app is active again.
    let script = r#"[{"t": 0.1, "type": "tap", "x": 110, "y": 75}, {"t": 0.2, "type": "key", "name": "Enter"},
        {"t": 0.3, "focus": true}, {"t": 0.4, "type": "key", "name": "Enter"}]"#;
    let dir = std::env::temp_dir().join(format!("rune-desktop-tree-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let tree = dir.join("tree.json");
    let (_, run) = desktop_case("inactive", script, 0.2, &["--tree", tree.to_str().unwrap()], || {
        vec![pair("a", RED, None, |_, _| {}, left()).0.boxed()]
    });
    let tree: Value = serde_json::from_str(&std::fs::read_to_string(&tree).unwrap()).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(tree["windows"][0]["focused"], json!(false), "--tree reports no focus while inactive");
    assert_eq!(focus_of(&run, 0.0, "a"), (true, true));
    assert_eq!(focus_of(&run, 0.2, "a"), (true, false));
    assert!(KEYED.lock().unwrap().is_empty(), "keys while inactive go to another app");

    let (_, run) = desktop_case("inactive", script, 0.5, &[], || vec![pair("a", RED, None, |_, _| {}, left()).0.boxed()]);
    assert_eq!(focus_of(&run, 0.3, "a"), (true, true));
    assert_eq!(*KEYED.lock().unwrap(), ["a"], "only the key after refocusing lands");
}

#[test]
fn desktop_click_through_window_never_takes_the_keyboard() {
    let _turn = HEADLESS.lock().unwrap_or_else(|e| e.into_inner());
    // `a` opens and focuses a click-through HUD; `a` keeps the keyboard.
    let script = r#"[{"t": 0.1, "type": "key", "name": "Enter"}, {"t": 0.2, "type": "key", "name": "Enter"}]"#;
    let (_, run) = desktop_case("hud", script, 0.3, &[], || {
        let hud = crate::WindowOptions { visible: false, click_through: true, borderless: true, ..right() };
        let (hud, handle) = pair("hud", BLUE, None, |_, _| {}, hud);
        let (a, _) = pair("a", RED, Some(handle), |hud, _| { hud.open(); hud.focus(); }, left());
        vec![a.boxed(), hud.boxed()]
    });
    assert_eq!((focus_of(&run, 0.1, "a"), focus_of(&run, 0.1, "hud")), ((true, true), (true, false)));
    assert_eq!(*KEYED.lock().unwrap(), ["a", "a"]);
}

/// The reopen hooks that ran: window, `has_visible_windows`.
static REOPENED: std::sync::Mutex<Vec<(&'static str, bool)>> = std::sync::Mutex::new(Vec::new());

/// Reopen hooks for `closer`: each records the reopen.
fn record(p: &mut Pair, visible: bool, _: &mut crate::Tx) -> bool { REOPENED.lock().unwrap().push((p.name, visible)); false }
fn handle(p: &mut Pair, visible: bool, tx: &mut crate::Tx) -> bool { record(p, visible, tx); true }
/// Opens the other window and leaves the rest to the default.
fn open_other(p: &mut Pair, visible: bool, tx: &mut crate::Tx) -> bool {
    if let Some(other) = &p.other { other.open(); }
    record(p, visible, tx)
}

/// A `keep_running` window that closes on any key, with `hook` as its `on_reopen`.
fn closer_with(name: &'static str, color: crate::Color, other: Option<crate::AppHandle<Pair>>,
    hook: fn(&mut Pair, bool, &mut crate::Tx) -> bool, options: crate::ConfiguredWindowOptions) -> (crate::AnyWindowSpec, crate::AppHandle<Pair>) {
    let spec = AppSpec {
        on_key: Some(|_: &mut Pair, _: &crate::KeyEvent, tx: &mut crate::Tx| { tx.close_window(); true }),
        ..AppSpec::new("closer", (1.0, 1.0), || Pair { name: "", color: crate::Color::TRANSPARENT, other: None, act: |_, _| {} },
            |p, _| crate::el().w_full().h_full().fill(p.color))
    };
    let app = crate::App::new(spec).init(move || Pair { name, color, other, act: |_, _| {} }).keep_running().on_reopen(hook);
    let handle = app.handle();
    (crate::WindowSpec::from_app(name, app).configured_options(options).boxed(), handle)
}

/// `closer_with` that records reopens, handling them when `handles`.
fn closer(name: &'static str, color: crate::Color, handles: bool, options: crate::ConfiguredWindowOptions) -> crate::AnyWindowSpec {
    closer_with(name, color, None, if handles { handle } else { record }, options).0
}

/// Runs `windows` on a headless desktop with `script`, as `desktop_case` does.
fn reopen_case(script: &str, accessory: bool, windows: impl FnOnce() -> Vec<crate::AnyWindowSpec> + 'static) -> &'static HeadlessRun {
    REOPENED.lock().unwrap().clear();
    let dir = std::env::temp_dir().join(format!("rune-reopen-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let backdrop = dir.join("backdrop.png");
    ::image::RgbaImage::from_pixel(240, 160, ::image::Rgba([128, 128, 128, 255])).save(&backdrop).unwrap();
    let (script_path, tree) = (dir.join("script.json"), dir.join("tree.json"));
    std::fs::write(&script_path, script).unwrap();
    let args = ["--desktop", backdrop.to_str().unwrap(), "--script", script_path.to_str().unwrap(),
        "--scale", "2", "--fps", "10", "--tree", tree.to_str().unwrap()];
    let options = crate::RunOptions { accessory, ..Default::default() };
    crate::run_windows_with_args(options, args.map(String::from), move |_| windows());
    std::fs::remove_dir_all(&dir).unwrap();
    headless_run().unwrap()
}

#[test]
fn desktop_reopen_shows_the_window_closed_last_with_the_keyboard() {
    let _turn = HEADLESS.lock().unwrap_or_else(|e| e.into_inner());
    // Enter closes `b` (on top, with the keyboard), then `a`; reopening brings back `a`.
    let script = r#"[{"t": 0.1, "type": "key", "name": "Enter"}, {"t": 0.2, "type": "key", "name": "Enter"},
        {"t": 0.3, "reopen": true}]"#;
    let run = reopen_case(script, false, || vec![closer("a", RED, false, left().macos(Default::default())),
        closer("b", BLUE, false, right().macos(Default::default()))]);
    assert_eq!((focus_of(run, 0.2, "a"), focus_of(run, 0.2, "b")), ((false, false), (false, false)));
    assert_eq!((focus_of(run, 0.3, "a"), focus_of(run, 0.3, "b")), ((true, true), (false, false)));
    assert_eq!(*REOPENED.lock().unwrap(), [("a", false), ("b", false)], "every hook runs, before the default");
    assert!(run.events().iter().any(|e| matches!(e.event, ScriptEvent::Reopen)));
}

#[test]
fn desktop_reopen_with_a_window_showing_changes_nothing() {
    let _turn = HEADLESS.lock().unwrap_or_else(|e| e.into_inner());
    let script = r#"[{"t": 0.1, "type": "key", "name": "Enter"}, {"t": 0.2, "reopen": true}]"#;
    let run = reopen_case(script, false, || vec![closer("a", RED, false, left().macos(Default::default())),
        closer("b", BLUE, false, right().macos(Default::default()))]);
    assert_eq!((focus_of(run, 0.2, "a"), focus_of(run, 0.2, "b")), ((true, true), (false, false)));
    assert_eq!(*REOPENED.lock().unwrap(), [("a", true), ("b", true)]);
}

#[test]
fn desktop_reopen_default_sees_what_the_hooks_did() {
    let _turn = HEADLESS.lock().unwrap_or_else(|e| e.into_inner());
    // `a` closes, then `b`, so `b` would come back; but `b`'s hook opens `a` and leaves
    // the rest to the default, which then finds a window showing.
    let script = r#"[{"t": 0.1, "type": "tap", "x": 30, "y": 25}, {"t": 0.2, "type": "key", "name": "Enter"},
        {"t": 0.3, "type": "key", "name": "Enter"}, {"t": 0.4, "reopen": true}]"#;
    let run = reopen_case(script, false, || {
        let (a, handle) = closer_with("a", RED, None, record, left().macos(Default::default()));
        let (b, _) = closer_with("b", BLUE, Some(handle), open_other, right().macos(Default::default()));
        vec![a, b]
    });
    assert_eq!((focus_of(run, 0.3, "a"), focus_of(run, 0.3, "b")), ((false, false), (false, false)), "both closed");
    assert_eq!((focus_of(run, 0.4, "a"), focus_of(run, 0.4, "b")), ((true, true), (false, false)));
    assert_eq!(*REOPENED.lock().unwrap(), [("a", false), ("b", false)]);
}

#[test]
fn desktop_reopen_hook_that_handles_it_skips_the_default() {
    let _turn = HEADLESS.lock().unwrap_or_else(|e| e.into_inner());
    let script = r#"[{"t": 0.1, "type": "key", "name": "Enter"}, {"t": 0.2, "reopen": true}]"#;
    let run = reopen_case(script, false, || vec![closer("a", RED, true, left().macos(Default::default()))]);
    assert_eq!(focus_of(run, 0.2, "a"), (false, false));
    assert_eq!(*REOPENED.lock().unwrap(), [("a", false)]);
}

#[test]
fn desktop_reopen_never_brings_back_a_launcher_or_an_accessory_app_window() {
    let _turn = HEADLESS.lock().unwrap_or_else(|e| e.into_inner());
    // The launcher closes last, so a launcher-blind rule would pick it; `a` comes back.
    let launcher = || {
        let mut options = crate::WindowOptions::launcher_panel();
        let (position, physical_size) = (right().position, right().physical_size);
        options.options = crate::WindowOptions { visible: true, position, physical_size, ..options.options };
        closer("launcher", BLUE, false, options)
    };
    let script = r#"[{"t": 0.1, "type": "tap", "x": 30, "y": 25}, {"t": 0.2, "type": "key", "name": "Enter"},
        {"t": 0.3, "type": "tap", "x": 95, "y": 35}, {"t": 0.4, "type": "key", "name": "Enter"}, {"t": 0.5, "reopen": true}]"#;
    let run = reopen_case(script, false, move || vec![closer("a", RED, false, left().macos(Default::default())), launcher()]);
    assert_eq!((focus_of(run, 0.4, "a"), focus_of(run, 0.4, "launcher")), ((false, false), (false, false)), "both closed");
    assert_eq!((focus_of(run, 0.5, "a"), focus_of(run, 0.5, "launcher")), ((true, true), (false, false)));

    // A launcher alone stays hidden.
    let run = reopen_case(r#"[{"t": 0.1, "type": "tap", "x": 95, "y": 35}, {"t": 0.2, "type": "key", "name": "Enter"},
        {"t": 0.3, "reopen": true}]"#, false, move || vec![launcher()]);
    assert_eq!(focus_of(run, 0.3, "launcher"), (false, false));

    // An accessory app's hooks run, but nothing comes back.
    let script = r#"[{"t": 0.1, "type": "key", "name": "Enter"}, {"t": 0.2, "reopen": true}]"#;
    let run = reopen_case(script, true, || vec![closer("a", RED, false, left().macos(Default::default()))]);
    assert_eq!(focus_of(run, 0.2, "a"), (false, false));
    assert_eq!(*REOPENED.lock().unwrap(), [("a", false)]);
}

#[test]
fn size_and_scale_must_be_positive() {
    assert_eq!(parse_size("400x300.5"), Ok((400.0, 300.5)));
    assert_eq!(parse_scale("1.5"), Ok(1.5));
    for bad in ["0x0", "400x0", "-1x10", "NaNx10", "infx10", "400", "ax3"] { assert!(parse_size(bad).is_err(), "{bad}"); }
    for bad in ["0", "-2", "NaN", "inf", "x"] { assert!(parse_scale(bad).is_err(), "{bad}"); }
}

#[test]
fn keycaps_draw_over_frames_but_never_enter_the_tree() {
    let _turn = HEADLESS.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("rune-keys-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("script.json");
    std::fs::write(&script, r#"[{"t": 0.1, "type": "type", "text": "hi"}, {"t": 0.3, "type": "tap", "x": 150, "y": 30}]"#).unwrap();
    let run = |name: &str, extra: &[&str]| {
        let spec = AppSpec::new("keys", (200.0, 120.0), || (), |_, _| crate::el().key("root").w_full().h_full().fill(crate::oklch(0.9, 0.0, 0.0)));
        let (snap, tree) = (dir.join(format!("{name}.png")), dir.join(format!("{name}.json")));
        let mut args = vec!["--script", script.to_str().unwrap(), "--scale", "1", "--at", "0.5",
            "--snap", snap.to_str().unwrap(), "--tree", tree.to_str().unwrap()];
        args.extend_from_slice(extra);
        headless(crate::App::new(spec), Args::parse_from(args.into_iter().map(String::from).collect()));
        (std::fs::read(snap).unwrap(), std::fs::read_to_string(tree).unwrap())
    };
    let plain = run("plain", &[]);
    let keys = run("keys", &["--keys", "top-left", "--pointer"]);
    let light = run("light", &["--keys", "top-left", "--pointer", "--keys-theme", "light"]);
    let dark = run("dark", &["--keys", "top-left", "--pointer", "--keys-theme", "dark"]);
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(plain.1, keys.1, "the overlay never enters the tree");
    assert_ne!(plain.0, keys.0, "but it draws over the frame");
    assert_eq!(keys.0, light.0, "auto caps follow the app's light background");
    assert_ne!(light.0, dark.0);

    use crate::input::keycaps::Corner;
    let args = |v: &[&str]| Args::parse_from(v.iter().map(|s| (*s).into()).collect());
    assert_eq!(args(&["--keys", "--snap", "a.png"]).keys, Some(Corner::BottomLeft), "the corner is optional");
    assert_eq!(args(&["--keys", "top"]).keys, Some(Corner::Top));
    assert_eq!(args(&["--snap", "a.png"]).keys, None);
}

#[test]
fn script_chords_only_fire_global_handlers_even_when_the_window_is_key() {
    let script = parse_script(json!([{"t": 0.1, "type": "shortcut", "chord": "cmd+space"}, {"t": 0.2, "shortcut": "cmd+k"}]));
    assert!(matches!(&script[0].kind, Kind::Shortcut(c) if *c == "cmd+space".parse().unwrap()));
    assert!(matches!(&script[1].kind, Kind::Shortcut(c) if *c == "cmd+k".parse().unwrap()));

    let _turn = HEADLESS.lock().unwrap_or_else(|e| e.into_inner());
    static SEEN: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let dir = std::env::temp_dir().join(format!("rune-chords-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("script.json");
    SEEN.lock().unwrap().clear();
    std::fs::write(&path, r#"[{"focus": true}, {"t": 0.1, "type": "shortcut", "chord": "cmd+k"},
        {"t": 0.2, "type": "shortcut", "chord": "cmd+space"}, {"t": 0.3, "shortcut": "cmd+k"},
        {"t": 0.4, "focus": false}, {"t": 0.5, "type": "shortcut", "chord": "cmd+space"},
        {"t": 0.6, "type": "key", "name": "Escape"}]"#).unwrap();
    let spec = AppSpec {
        on_key: Some(|_, ev, _| { SEEN.lock().unwrap().push(format!("on_key {:?}", ev.key)); true }),
        ..AppSpec::new("chords", (40.0, 30.0), || (), |_, _| crate::el())
    };
    let app = crate::App::new(spec)
        .global_shortcut("cmd+k", |_, _| SEEN.lock().unwrap().push("global cmd+k".into()))
        .global_shortcut("cmd+space", |_, _| SEEN.lock().unwrap().push("global cmd+space".into()));
    let tree = dir.join("tree.json");
    headless(app, Args::parse_from(["--script", path.to_str().unwrap(), "--tree", tree.to_str().unwrap()].map(String::from).into()));
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(*SEEN.lock().unwrap(), ["global cmd+k", "global cmd+space", "global cmd+k", "global cmd+space", "on_key Escape"]);
}

#[test]
fn native_trace_does_not_select_virtual_rendering() {
    let args = |v: &[&str]| Args::parse_from(v.iter().map(|s| (*s).into()).collect());
    assert!(args(&["--trace", "trace.jsonl", "--frames", "120"]).headless());
    assert!(!args(&["--native", "--trace", "trace.jsonl", "--frames", "120"]).headless());
    assert!(args(&["--native", "--snap", "snap.png"]).headless());
}

#[test]
fn a_script_alone_replays_headless_unless_native() {
    // A window never replays a script, so `--script` alone renders offscreen like `--frames`.
    let args = |v: &[&str]| Args::parse_from(v.iter().map(|s| (*s).into()).collect());
    assert!(args(&["--script", "s.json"]).headless());
    assert!(!args(&["--native", "--script", "s.json"]).headless());
    assert!(!args(&["--theme", "light"]).headless());
}

#[test]
fn tree_and_trace_serialize_effects_and_remove_channels() {
    fn view(show: &bool, _: &mut Cx<bool>) -> El {
        crate::el().key("root").size(100.0, 100.0).children(show.then(||
            crate::el().key("pill").size(40.0, 40.0).opacity(0.5)
                .live_scale(&crate::live::Live::new(0.8))
                .transition(crate::Transition::blur(4.0))))
    }
    let mut rt = Runtime::new(true, view, (100.0, 100.0), Env { scale: 1.0, system_dark: true });
    rt.frame(0.0);
    let tree = tree_json(&rt, 0.0);
    let pill = tree["elements"].as_array().unwrap().iter().find(|e| e["key"] == "pill").unwrap();
    assert_eq!(pill["opacity"], 0.5);
    assert!((pill["scale"].as_f64().unwrap() - 0.8).abs() < 1e-6);
    assert_eq!(pill["blur"], 0.0);
    let path = std::env::temp_dir().join(format!("rune-effects-{}.jsonl", std::process::id()));
    {
        let mut trace = Trace::open(&path, rt.viewport, 60.0, 1.0);
        trace.frame(&mut rt, 0.0, 0.0);
        rt.state = false; rt.invalidate(); rt.frame(0.0); rt.frame(0.05);
        let tree = tree_json(&rt, 0.05);
        let pill = tree["elements"].as_array().unwrap().iter().find(|e| e["key"] == "pill").unwrap();
        assert_eq!(pill["exiting"], true);
        assert!(tree["elements"].as_array().unwrap().iter().filter(|e| e["key"] != "pill").all(|e| e.get("exiting").is_none()));
        trace.frame(&mut rt, 50.0, 0.0);
        rt.frame(3.0);
        trace.frame(&mut rt, 3050.0, 0.0);
    }
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    let lines: Vec<Value> = text.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    let blur = lines[1]["cm"].as_array().unwrap().iter()
        .find(|m| m["name"].as_str().unwrap().ends_with("pill.blur")).unwrap()["id"].as_u64().unwrap();
    let values = lines[2]["c"].as_array().unwrap();
    assert!(values.iter().any(|v| v[0] == blur && v[1].as_f64().unwrap() > 0.0));
    assert!(lines[3]["cg"].as_array().unwrap().contains(&json!(blur)));
    assert_eq!(lines[1]["e"][0].as_array().unwrap().len(), 8, "element row schema stays compatible");
}

#[test]
fn trace_all_names_unkeyed_elements_and_reports_what_animated() {
    fn view(wide: &bool, _: &mut Cx<bool>) -> El {
        crate::el().key("root").size(100.0, 100.0)
            .children([crate::el().size(if *wide { 80.0 } else { 20.0 }, 20.0)])
    }
    assert!(Args::parse_from(["--trace-all"].map(String::from).into()).trace_all);
    let lines = |all: bool| {
        let mut rt = Runtime::new(false, view, (100.0, 100.0), Env { scale: 1.0, system_dark: true });
        rt.frame(0.0);
        let path = std::env::temp_dir().join(format!("rune-trace-all-{all}-{}.jsonl", std::process::id()));
        {
            let mut trace = Trace::open(&path, rt.viewport, 60.0, 1.0);
            trace.all = all;
            trace.frame(&mut rt, 0.0, 0.0);
            rt.state = true; rt.invalidate(); rt.frame(0.0); rt.frame(0.05);
            trace.frame(&mut rt, 50.0, 0.0);
        }
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        text.lines().map(|line| serde_json::from_str(line).unwrap()).collect::<Vec<Value>>()
    };
    let keyed = lines(false);
    assert_eq!(keyed[1]["m"].as_array().unwrap().len(), 1, "only the keyed root without --trace-all");
    let all = lines(true);
    let metas = all[1]["m"].as_array().unwrap();
    let root = metas.iter().find(|m| m["nm"] == "root").unwrap();
    let child = metas.iter().find(|m| m["nm"] != "root").expect("unkeyed child traced");
    assert_eq!(child["pv"], root["id"], "parent link to the root");
    assert_eq!(all[2]["quality"]["why"], json!(["tracks"]));
    let tk = all[2]["tk"].as_array().unwrap();
    assert!(tk.iter().any(|t| t[0] == child["id"] && t[1] == "frame" && t[2] == 1), "{tk:?}");
}

#[test]
fn the_window_title_starts_from_the_spec_and_follows_set_title() {
    fn view(_: &u32, _: &mut Cx<u32>) -> El {
        crate::el().size(100.0, 100.0).on_press_tx(|n: &mut u32, tx| { *n += 1; tx.set_title(format!("Notes {}", *n % 2)); })
    }
    let spec = AppSpec::new("Notes", (100.0, 100.0), || 0, view);
    let mut rt = runtime_for(&spec, 0, (100.0, 100.0), Env { scale: 1.0, system_dark: true }, std::sync::Arc::new(|| {}));
    rt.frame(0.0);
    assert_eq!(tree_json(&rt, 0.0)["title"], "Notes");
    assert_eq!(rt.window_title.take_change(), None, "the window opens with the spec's title");
    let press = |rt: &mut Runtime<u32>, t| { rt.pointer_down(50.0, 50.0); rt.pointer_up(50.0, 50.0); rt.frame(t); };
    press(&mut rt, 0.1);
    assert_eq!(tree_json(&rt, 0.1)["title"], "Notes 1");
    assert_eq!(rt.window_title.take_change(), Some("Notes 1"));
    assert_eq!(rt.window_title.take_change(), None);
    rt.update(|_, tx| tx.set_title("Notes 1"));
    assert_eq!(rt.window_title.take_change(), None, "the same title again is no change");
    assert!(!rt.update_if(|_, tx| { tx.set_title("Draft"); false }), "the shell shows a title without a frame");
    assert_eq!(rt.window_title.take_change(), Some("Draft"));
}

#[path = "playground_cli_tests.rs"]
mod playground_cli;

#[test]
fn a_trace_says_how_far_scrolling_carries_an_element_when_that_changes() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        crate::col().key("scroll").size(100.0, 100.0).scroll(crate::ScrollSpec { stick_bottom: false })
            .child(crate::el().key("row").w_full().h(400.0))
    }
    let mut rt = Runtime::new((), view, (100.0, 100.0), Env { scale: 1.0, system_dark: true });
    rt.frame(0.0);
    let path = std::env::temp_dir().join(format!("rune-trace-scrolled-{}.jsonl", std::process::id()));
    {
        let mut trace = Trace::open(&path, rt.viewport, 60.0, 1.0);
        trace.frame(&mut rt, 0.0, 0.0);
        rt.update(|_, tx| tx.scroll_to_instant("scroll", 0.0, 30.0));
        rt.frame(0.02);
        trace.frame(&mut rt, 20.0, 0.0);
        trace.frame(&mut rt, 40.0, 0.0);
    }
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    let lines: Vec<Value> = text.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    let row = lines[1]["m"].as_array().unwrap().iter().find(|m| m["nm"].as_str().unwrap().ends_with("row")).unwrap()["id"].clone();
    assert!(lines[1].get("sc").is_none(), "nothing scrolled yet");
    assert_eq!(lines[2]["sc"], json!([[row, 0.0, 30.0]]));
    assert!(lines[3].get("sc").is_none(), "only when it changes");
}
