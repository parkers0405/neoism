//! Concentric icon, vector shape, box fill and heading text inside an anchored card over a
//! scaled stage (`t1193`). Tapping `"stage"` zooms to `1.6×` and drifts diagonally on springs;
//! tapping `"badge"` swaps its inner `canvas` disc for the matching `Icon` disc with no shift.
//!
//! Headless trace and motion audit (`scripts/motion-audit/run.sh` records both):
//!   cargo run -p rune --example one_snap -- --script scripts/motion-audit/scenarios/one-snap.json --trace t.jsonl --trace-all
use rune::{
    Align, AppSpec, Color, Cx, El, Icon, Shape, Side, Spring, TextStyle, canvas, col, el, icon,
    oklch, row, text,
};

static RING_ICON: Icon = Icon {
    name: "ring",
    view: 16.0,
    body: r#"<circle cx="8" cy="8" r="6.5" stroke="currentColor" stroke-width="1.5" fill="none"/>"#,
};

static DISC_ICON: Icon = Icon {
    name: "disc",
    view: 16.0,
    body: r#"<circle cx="8" cy="8" r="4" fill="currentColor"/>"#,
};

struct State {
    zoomed: bool,
    swapped: bool,
}

fn disc_shape(color: Color) -> Shape {
    let pts: Vec<[f32; 2]> = (0..64)
        .map(|k| {
            let a = std::f32::consts::TAU * k as f32 / 64.0;
            [8.0 + 4.0 * a.cos(), 8.0 + 4.0 * a.sin()]
        })
        .collect();
    Shape::new().fill(pts, color)
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let accent = oklch(0.63, 0.22, 27.0);
    let inner = if s.swapped {
        icon(&DISC_ICON, 16.0).key("disc").color(accent).absolute(0.0, 0.0)
    } else {
        canvas(disc_shape(accent)).key("disc").size(16.0, 16.0).absolute(0.0, 0.0)
    };
    let badge = el()
        .key("badge")
        .size(16.0, 16.0)
        .child(icon(&RING_ICON, 16.0).key("ring").color(Color::WHITE).absolute(0.0, 0.0))
        .child(inner);

    let zoom = if s.zoomed { 1.6 } else { 1.0 };
    let drift = if s.zoomed { [9.35, 6.65] } else { [0.0, 0.0] };
    let stage = col()
        .key("stage")
        .absolute(24.0, 20.0)
        .size(180.0, 96.0)
        .radius(10.0)
        .fill(oklch(0.26, 0.02, 250.0))
        .scale(zoom)
        .scale_motion(Spring::new(0.45, 0.0))
        .origin(0.0, 0.0)
        .on_press(|s: &mut State| s.zoomed = !s.zoomed)
        .child(
            el().key("anchor")
                .absolute(18.2, 14.35)
                .size(28.0, 8.0)
                .offset(drift[0], drift[1])
                .offset_motion(Spring::new(0.55, 0.0)),
        );

    let hud = row()
        .key("hud")
        .padding(8.0)
        .gap(8.0)
        .items_center()
        .radius(9.0)
        .fill(oklch(0.32, 0.02, 250.0))
        .border(1.0, oklch(0.44, 0.02, 250.0))
        .child(badge.on_press_tx(|s: &mut State, tx| {
            s.swapped = !s.swapped;
            tx.instant();
        }))
        .child(
            text(if s.swapped { "Icon disc" } else { "Path disc" })
                .key("title")
                .font(TextStyle::new(15.0, 600.0))
                .color(Color::WHITE),
        )
        .anchored("anchor", Side::Bottom, Align::Start, 4.1);

    col()
        .w_full()
        .h_full()
        .fill(oklch(0.18, 0.01, 250.0))
        .children([stage, hud])
}

fn main() {
    rune::run(AppSpec::new(
        "One snap",
        (360.0, 240.0),
        || State { zoomed: false, swapped: false },
        view,
    ));
}
