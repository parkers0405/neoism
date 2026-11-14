//! Rune's text against CoreText (`rune::text::pixel_compare`).
//!
//! `coretext_compare [--set inter|sf|mono] [--smoothing on|off] [--blend srgb|linear]
//! [--against smoothed|unsmoothed] [--out DIR]`: prints every case's numbers, by default in the
//! platform's font smoothing and its blending, against the CoreText rendering that mode matches;
//! `--out` also writes Rune's sheets and heatmaps.
//! `coretext_compare shot SHOT.png [--pair light|dark] [--smoothing ...]`: Rune's paragraph
//! against a 2× browser screenshot of the same page.
//! `coretext_compare export DIR` and `pack DIR`: the two halves of
//! `scripts/coretext-reference.sh`.
use rune::FontSmoothing;
use rune::text::pixel_compare::{self as pc, Image, Options, Reference, Set};
use std::path::{Path, PathBuf};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let smoothing = flag("--smoothing").map(|m| match m.as_str() {
        "on" => FontSmoothing::On,
        "off" => FontSmoothing::Off,
        other => die(&format!("--smoothing {other}: on or off")),
    });
    let blend = flag("--blend").map(|b| b == "srgb");
    match args.first().map(String::as_str) {
        Some("export") => return export(Path::new(args.get(1).expect("export DIR"))),
        Some("pack") => return pack(Path::new(args.get(1).expect("pack DIR"))),
        Some("shot") => {
            let shot = Image::read_png(Path::new(args.get(1).expect("shot SHOT.png"))).unwrap_or_else(|e| die(&e));
            let name = flag("--pair").unwrap_or_else(|| "light".into());
            let pair = *pc::PAIRS.iter().find(|p| p.name == name).unwrap_or_else(|| die("--pair: light, dark, white or blue"));
            let mode = smoothing.unwrap_or(FontSmoothing::Off);
            let ([dx, dy], s) = pc::compare_shot(&shot, pair, mode, blend.unwrap_or(true)).unwrap_or_else(|e| die(&e));
            println!(
                "{mode:?} vs {}: shift ({dx}, {dy}) px  differ {:.2}%  >8 {:.2}%  max {}  meanabs {:.3}  ink {:.3}",
                args[1], 100.0 * s.differing_share(), 100.0 * s.over8_share(), s.max, s.meanabs(), s.ink_ratio()
            );
            return;
        }
        _ => {}
    }
    let set = match flag("--set").as_deref() {
        None | Some("inter") => Set::Inter,
        Some("sf") => Set::Sf,
        Some("mono") => Set::Mono,
        Some(other) => die(&format!("--set {other}: inter, sf or mono")),
    };
    let reference = flag("--against").map(|r| if r == "unsmoothed" { Reference::Unsmoothed } else { Reference::Smoothed });
    let options = Options { set, smoothing, srgb_blend: blend, reference };
    let out = flag("--out").map(PathBuf::from);
    let rows = pc::run(options, |pair, scale, rune: &Image, reference: &Image| {
        let Some(dir) = &out else { return };
        std::fs::create_dir_all(dir).unwrap();
        rune.write_png(&dir.join(format!("rune-{}-{scale}x.png", pair.name))).unwrap();
        pc::heatmap(rune, reference).write_png(&dir.join(format!("heat-{}-{scale}x.png", pair.name))).unwrap();
    })
    .unwrap_or_else(|e| die(&e));
    print!("{}", pc::table(&options, &rows));
}

fn die(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(1);
}

fn export(dir: &Path) {
    for (set, name) in [(Set::Inter, "inter"), (Set::Sf, "sf"), (Set::Mono, "mono")] {
        let sub = dir.join(name);
        for scale in pc::SCALES {
            match pc::export(set, scale) {
                Ok(json) => {
                    std::fs::create_dir_all(&sub).unwrap();
                    std::fs::write(sub.join(format!("cases-{scale}x.json")), serde_json::to_vec(&json).unwrap()).unwrap();
                }
                Err(e) => eprintln!("skipping {}: {e}", set.name()),
            }
        }
    }
}

/// Recompresses the sheets Swift drew in `dir/<set>` into each set's directory and records the
/// layout they were drawn from.
fn pack(dir: &Path) {
    for (set, name) in [(Set::Inter, "inter"), (Set::Sf, "sf"), (Set::Mono, "mono")] {
        let Ok(entries) = std::fs::read_dir(dir.join(name)) else { continue };
        std::fs::create_dir_all(set.dir()).unwrap();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "png") {
                let image = Image::read_png(&path).unwrap_or_else(|e| die(&e));
                image.write_green_png(&set.dir().join(entry.file_name())).unwrap_or_else(|e| die(&e));
            }
        }
        std::fs::write(set.dir().join("layout.txt"), pc::layout_file(set).unwrap()).unwrap();
    }
}
