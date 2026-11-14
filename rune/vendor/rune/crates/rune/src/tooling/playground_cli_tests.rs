//! `--playground NAME` with `--snap`, `--out`, `--video`, `--script`, `--variant` and
//! `--crop KEY`, headless and the same every run.

use super::*;

#[derive(Default)]
struct Pad {
    x: f32,
    size: f32,
    green: bool,
}

fn pad(p: &Pad, _: &mut Cx<Pad>) -> crate::El {
    let fill = if p.green { crate::oklch(0.7, 0.2, 145.0) } else { crate::oklch(0.6, 0.2, 30.0) };
    crate::el().w_full().h_full().child(crate::el().key("puck").absolute(p.x, 10.0).size(p.size, p.size).fill(fill))
}

static SCENES: &[(&str, fn(&mut Pad))] = &[("Motion/Slide", |p| p.size = 20.0)];

fn app() -> crate::App<Pad> {
    let playground = crate::Playground::<Pad>::new()
        .enabled(true)
        .trigger("Slide", 'r', "Slide", |_, play| play.after(0.3, |p, _| p.x = 60.0))
        .trigger("Slide", 'g', "Green", |p, _| p.green = true)
        .variant("Slide", "Small", |p| p.size = 20.0)
        .variant("Slide", "Big", |p| p.size = 40.0);
    let mut spec = AppSpec::new("pad", (960.0, 240.0), Pad::default, pad);
    spec.scenes = SCENES;
    crate::App::new(spec).playground(playground)
}

fn run(dir: &std::path::Path, name: &str, args: &[&str]) -> std::path::PathBuf {
    let out = dir.join(name);
    std::fs::create_dir_all(&out).unwrap();
    let snap = out.join("snap.png");
    let mut argv: Vec<String> = ["--playground", "Motion/Slide", "--scale", "2", "--fps", "30", "--snap", snap.to_str().unwrap()].map(String::from).into();
    argv.extend(args.iter().map(|a| a.replace("{out}", out.to_str().unwrap())));
    headless(app(), Args::parse_from(argv));
    out
}

fn bytes(dir: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
    files.sort();
    files.into_iter().flat_map(|p| if p.is_dir() { bytes(&p) } else { vec![(p.file_name().unwrap().to_string_lossy().into(), std::fs::read(&p).unwrap())] }).collect()
}

fn png_size(path: &std::path::Path) -> (u32, u32) {
    let b = std::fs::read(path).unwrap();
    let be = |i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    (be(16), be(20))
}

#[test]
fn a_playground_scene_renders_its_script_the_same_every_run() {
    let _turn = HEADLESS.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("rune-playground-cli-{}", std::process::id()));
    let script = dir.join("script.json");
    std::fs::create_dir_all(&dir).unwrap();
    // G's trigger runs at 0.5 s on the logical clock, after the slide 'r' started on choosing.
    std::fs::write(&script, r#"[{"t": 0.5, "type": "type", "text": "g"}, {"t": 1.2, "type": "mark", "name": "end"}]"#).unwrap();
    let video = std::process::Command::new("ffprobe").arg("-version").output().is_ok();
    let mut args = vec!["--script", script.to_str().unwrap(), "--at", "1.0", "--out", "{out}/frames", "--every", "6"];
    if video { args.extend(["--video", "{out}/clip.mp4"]); }
    let first = bytes(&run(&dir, "first", &args));
    let second = bytes(&run(&dir, "second", &args));
    assert!(first.iter().any(|(n, _)| n == "snap.png") && first.iter().any(|(n, _)| n.starts_with("frame_")));
    assert_eq!(first.iter().any(|(n, _)| n == "clip.mp4"), video);
    for ((name, a), (_, b)) in first.iter().zip(&second) {
        assert!(a == b, "{name} differs between runs");
    }
    assert_eq!(first.len(), second.len());
    let still = bytes(&run(&dir, "still", &["--at", "1.0"]));
    let start = bytes(&run(&dir, "start", &["--at", "0"]));
    let snap = |files: &[(String, Vec<u8>)]| files.iter().find(|(n, _)| n == "snap.png").unwrap().1.clone();
    assert!(snap(&first) != snap(&still), "the script's trigger key changed the scene");
    assert!(snap(&still) != snap(&start), "choosing the scene played its 'r' motion");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn variant_and_crop_pick_a_storys_variant_and_crop_the_snap_to_an_element() {
    let _turn = HEADLESS.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("rune-playground-crop-{}", std::process::id()));
    let parsed = Args::parse_from(["--playground", "--snap", "s.png", "--crop", "1,2,3,4", "--crop", "puck", "--variant", "Big"].map(String::from).into());
    assert_eq!((parsed.crops, parsed.crop_key.as_deref(), parsed.variant.as_deref()), (vec![[1.0, 2.0, 3.0, 4.0]], Some("puck"), Some("Big")));
    let small = run(&dir, "small", &["--variant", "1", "--crop", "puck"]);
    let big = run(&dir, "big", &["--variant", "Big", "--crop", "puck"]);
    let again = run(&dir, "again", &["--variant", "big", "--crop", "puck"]);
    assert_eq!(png_size(&small.join("snap.png")), (40, 40), "the puck's 20 pt at 2x");
    assert_eq!(png_size(&big.join("snap.png")), (80, 80), "--variant, by name");
    assert!(bytes(&big) == bytes(&again), "the same every run");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn whole_window_snaps_the_list_and_side_column_and_crops_to_their_keys() {
    let _turn = HEADLESS.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("rune-playground-whole-{}", std::process::id()));
    let stage = run(&dir, "stage", &["--at", "0"]);
    let whole = run(&dir, "whole", &["--at", "0", "--whole-window"]);
    let again = run(&dir, "again", &["--at", "0", "--whole-window"]);
    let group = run(&dir, "group", &["--at", "0", "--whole-window", "--crop", "playground-group-0"]);
    let side = run(&dir, "side", &["--at", "0", "--whole-window", "--crop", "playground-side"]);
    let rgba = |d: &std::path::Path| image::open(d.join("snap.png")).unwrap().to_rgba8();
    let (stage_px, whole_px) = (rgba(&stage), rgba(&whole));
    assert_eq!(stage_px.dimensions(), (988, 480), "by default the stage only: 960 - 200 - 264 - 2 pt, at 2x");
    assert_eq!(whole_px.dimensions(), (1920, 480), "--whole-window: the whole window");
    // The stage sits after the 200 pt list and its 1 pt separator.
    assert!(image::imageops::crop_imm(&whole_px, 402, 0, 988, 480).to_image() == stage_px, "the same stage inside the whole window");
    let varied = |x: u32, w: u32| {
        let region = image::imageops::crop_imm(&whole_px, x, 0, w, 480).to_image();
        region.pixels().any(|p| p != region.get_pixel(0, 0))
    };
    assert!(varied(0, 400) && varied(1392, 528), "the list and the side column are drawn");
    assert!(bytes(&whole) == bytes(&again), "the same every run");
    assert_eq!(png_size(&group.join("snap.png")), (368, 48), "a group header: the list's 184 pt by 24 pt, at 2x");
    assert_eq!(png_size(&side.join("snap.png")), (528, 480), "the side column's 264 pt");
    std::fs::remove_dir_all(dir).unwrap();
}
