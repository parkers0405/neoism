//! Writes SVG plots of every preset (position and velocity over time), a retarget scenario, a
//! presence enter/exit/revive, and screen-space vs parent-space paths into `demo/motion/`.
//!
//! `cargo run --example curves [out-dir]`

use std::fmt::Write as _;

use rune_motion::prelude::*;
use rune_motion::{Keyframes, Prop, PropClass, Transition};

const BLUE: &str = "#2f6fec";
const ORANGE: &str = "#e8833a";
const GREY: &str = "#9aa3ad";
const GREEN: &str = "#2fa35a";
const RED: &str = "#d64545";
const PURPLE: &str = "#8a5cd6";

struct Series {
    points: Vec<(f32, f32)>,
    color: &'static str,
    dashed: bool,
    label: &'static str,
}

struct Panel {
    title: String,
    x: (f32, f32),
    y: (f32, f32),
    series: Vec<Series>,
    marks: Vec<f32>,
    equal_axes: bool,
}

fn svg(panels: &[Panel], cols: usize, pw: f32, ph: f32, title: &str) -> String {
    let rows = panels.len().div_ceil(cols);
    // Square canvas so Quick Look thumbnails (qlmanage -t) don't crop it.
    let side = (cols as f32 * pw).max(rows as f32 * ph + 40.0);
    let (w, h) = (side, side);
    let mut s = String::new();
    let _ = write!(
        s,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" font-family="-apple-system, Helvetica, sans-serif"><rect width="100%" height="100%" fill="white"/><text x="16" y="26" font-size="16" font-weight="600" fill="#222">{title}</text>"##
    );
    for (i, p) in panels.iter().enumerate() {
        let (ox, oy) = ((i % cols) as f32 * pw, (i / cols) as f32 * ph + 40.0);
        let (l, t, r, b) = (ox + 44.0, oy + 28.0, ox + pw - 14.0, oy + ph - 26.0);
        let (x0, mut x1, y0, mut y1) = (p.x.0, p.x.1, p.y.0, p.y.1);
        if p.equal_axes {
            let span = (x1 - x0).max(y1 - y0);
            x1 = x0 + span;
            y1 = y0 + span;
        }
        let px = |x: f32| l + (x - x0) / (x1 - x0) * (r - l);
        let py = |y: f32| if p.equal_axes { t + (y - y0) / (y1 - y0) * (b - t) } else { b - (y - y0) / (y1 - y0) * (b - t) };
        let _ = write!(s, r##"<rect x="{l}" y="{t}" width="{}" height="{}" fill="#fafbfc" stroke="#e3e6ea"/>"##, r - l, b - t);
        let _ = write!(s, r##"<text x="{l}" y="{}" font-size="12" font-weight="600" fill="#333">{}</text>"##, t - 9.0, p.title);
        if !p.equal_axes {
            for v in [0.0f32, 1.0] {
                if v >= y0 && v <= y1 {
                    let _ = write!(s, r##"<line x1="{l}" x2="{r}" y1="{0}" y2="{0}" stroke="#d5d9de" stroke-dasharray="3 3"/><text x="{1}" y="{2}" font-size="9" fill="#888" text-anchor="end">{v}</text>"##, py(v), l - 4.0, py(v) + 3.0);
                }
            }
            let _ = write!(s, r##"<text x="{r}" y="{}" font-size="9" fill="#888" text-anchor="end">{:.2} s</text>"##, b + 14.0, x1);
        }
        for m in &p.marks {
            let _ = write!(s, r##"<line x1="{0}" x2="{0}" y1="{t}" y2="{b}" stroke="{RED}" stroke-width="1" stroke-dasharray="2 3"/>"##, px(*m));
        }
        for (k, se) in p.series.iter().enumerate() {
            let mut d = String::new();
            for (j, &(x, y)) in se.points.iter().enumerate() {
                let _ = write!(d, "{}{:.2},{:.2} ", if j == 0 { "M" } else { "L" }, px(x), py(y));
            }
            let dash = if se.dashed { r#" stroke-dasharray="4 3""# } else { "" };
            let _ = write!(s, r#"<path d="{d}" fill="none" stroke="{}" stroke-width="1.8"{dash}/>"#, se.color);
            if !se.label.is_empty() {
                let ly = t + 12.0 + k as f32 * 12.0;
                let _ = write!(s, r#"<text x="{}" y="{ly}" font-size="9" fill="{}" text-anchor="end">{}</text>"#, r - 4.0, se.color, se.label);
            }
        }
    }
    s.push_str("</svg>");
    s
}

fn normalized(v: Vec<(f32, f32)>) -> Vec<(f32, f32)> {
    let m = v.iter().map(|p| p.1.abs()).fold(1e-6, f32::max);
    v.into_iter().map(|(t, y)| (t, y / m)).collect()
}

fn spring_panel(name: &str, s: Spring, t_max: f32) -> Panel {
    let n = 300;
    let (mut x, mut v) = (Vec::new(), Vec::new());
    for i in 0..=n {
        let t = t_max * i as f32 / n as f32;
        let (p, q) = s.sample(0.0, 0.0, 1.0, t);
        x.push((t, p));
        v.push((t, q));
    }
    Panel {
        title: format!("{name}  {}/{}", s.duration, s.bounce),
        x: (0.0, t_max),
        y: (-0.45, 1.25),
        series: vec![
            Series { points: x, color: BLUE, dashed: false, label: "position" },
            Series { points: normalized(v), color: ORANGE, dashed: true, label: "velocity (norm.)" },
        ],
        marks: vec![s.duration],
        equal_axes: false,
    }
}

fn curve_panel(name: &str, anim: Anim, from: f32, to: f32, t_max: f32) -> Panel {
    const X: Prop<f32> = Prop::custom(64, PropClass::Other);
    let mut st = AnimationStore::new();
    let id = NodeId(1);
    st.set(id, X, from);
    st.animate(id, X, to, anim);
    let (mut x, mut v) = (Vec::new(), Vec::new());
    for i in 0..=300 {
        let t = t_max * i as f32 / 300.0;
        st.set_time(t as f64);
        x.push((t, st.sample(id, X).unwrap()));
        v.push((t, st.sample_velocity(id, X).unwrap()));
    }
    Panel {
        title: name.to_string(),
        x: (0.0, t_max),
        y: (-0.45, 1.25),
        series: vec![
            Series { points: x, color: BLUE, dashed: false, label: "position" },
            Series { points: normalized(v), color: ORANGE, dashed: true, label: "velocity (norm.)" },
        ],
        marks: vec![],
        equal_axes: false,
    }
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/../../demo/motion").into());
    std::fs::create_dir_all(&out).unwrap();
    let write = |name: &str, s: String| {
        let path = format!("{out}/{name}");
        std::fs::write(&path, s).unwrap();
        println!("wrote {path}");
    };

    // 1. Kit's tokens (enter and exit) and SwiftUI's presets.
    let mut panels = Vec::new();
    for (name, pair) in motion::ALL {
        panels.push(spring_panel(name, pair.enter, 1.2));
        panels.push(spring_panel(&format!("{name} exit"), pair.exit, 1.2));
    }
    write("tokens.svg", svg(&panels, 4, 300.0, 170.0, "Kit motion tokens: position (blue), velocity (orange), duration (red)"));

    let panels = vec![
        spring_panel(".smooth", Spring::SMOOTH, 1.5),
        spring_panel(".snappy", Spring::SNAPPY, 1.5),
        spring_panel(".bouncy", Spring::BOUNCY, 1.5),
        spring_panel("overdamped", Spring::new(0.5, -0.5), 1.5),
        spring_panel("interactive", Spring::interactive(), 1.5),
        spring_panel("very bouncy", Spring::new(0.5, 0.7), 1.5),
    ];
    write("swiftui.svg", svg(&panels, 3, 300.0, 170.0, "SwiftUI springs"));

    // 2. Other curves.
    let panels = vec![
        curve_panel("CSS ease", Curve::tween(0.6, Easing::EASE).into(), 0.0, 1.0, 0.8),
        curve_panel("Kit CSS enter 150 ms", motion::CSS_ENTER.into(), 0.0, 1.0, 0.3),
        curve_panel("Kit CSS exit 110 ms", motion::CSS_EXIT.into(), 0.0, 1.0, 0.3),
        curve_panel("overshoot bezier", Curve::tween(0.5, Easing::OVERSHOOT).into(), 0.0, 1.0, 0.7),
        curve_panel("steps(4)", Curve::tween(0.6, Easing::Steps(4, rune_motion::StepPosition::End)).into(), 0.0, 1.0, 0.8),
        curve_panel("linear() piecewise", Curve::tween(0.6, Easing::Piecewise(&[[0.0, 0.0], [0.3, 0.7], [0.6, 0.9], [1.0, 1.0]])).into(), 0.0, 1.0, 0.8),
        {
            const X: Prop<f32> = Prop::custom(64, PropClass::Gesture);
            let mut st = AnimationStore::new();
            st.set(NodeId(1), X, 0.0);
            st.release(NodeId(1), X, 2.0, Release::Decay(Decay::IOS_NORMAL));
            let (mut x, mut v) = (Vec::new(), Vec::new());
            for i in 0..=300 {
                let t = 3.0 * i as f32 / 300.0;
                st.set_time(t as f64);
                x.push((t, st.sample(NodeId(1), X).unwrap()));
                v.push((t, st.sample_velocity(NodeId(1), X).unwrap()));
            }
            Panel {
                title: "decay 0.998/ms (fling v=2)".into(),
                x: (0.0, 3.0),
                y: (-0.1, 1.25),
                series: vec![
                    Series { points: x, color: BLUE, dashed: false, label: "position" },
                    Series { points: normalized(v), color: ORANGE, dashed: true, label: "velocity" },
                ],
                marks: vec![],
                equal_axes: false,
            }
        },
        {
            const X: Prop<f32> = Prop::custom(64, PropClass::Other);
            let mut st = AnimationStore::new();
            st.set(NodeId(1), X, 0.0);
            let kf = Keyframes::new().linear(0.4, 0.15).cubic(1.1, 0.2).cubic(0.8, 0.15).spring(1.0, motion::POP);
            st.play_keyframes(NodeId(1), X, &kf);
            let (mut x, mut v) = (Vec::new(), Vec::new());
            for i in 0..=300 {
                let t = 1.0 * i as f32 / 300.0;
                st.tick_at(t as f64);
                x.push((t, st.get(NodeId(1), X).unwrap()));
                v.push((t, st.velocity(NodeId(1), X).unwrap()));
            }
            Panel {
                title: "keyframes: linear, cubic, cubic, spring".into(),
                x: (0.0, 1.0),
                y: (-0.45, 1.25),
                series: vec![
                    Series { points: x, color: BLUE, dashed: false, label: "position" },
                    Series { points: normalized(v), color: ORANGE, dashed: true, label: "velocity" },
                ],
                marks: vec![0.15, 0.35, 0.5],
                equal_axes: false,
            }
        },
    ];
    write("curves.svg", svg(&panels, 4, 300.0, 170.0, "Tweens, decay and keyframes"));

    // 3. Retargeting mid-flight: merge (springs) and additive (tweens), with the naive restart.
    let retarget = |anim: Anim, naive: bool| {
        const X: Prop<f32> = Prop::custom(64, PropClass::Other);
        let mut st = AnimationStore::new();
        st.set(NodeId(1), X, 0.0);
        st.animate(NodeId(1), X, 1.0, anim);
        let plan = [(0.15f64, 0.2f32), (0.32, 1.1), (0.55, 0.6)];
        let (mut x, mut v) = (Vec::new(), Vec::new());
        let mut k = 0;
        for i in 0..=480 {
            let t = i as f64 / 400.0;
            st.set_time(t);
            if k < plan.len() && t >= plan[k].0 {
                if naive {
                    let cur = st.sample(NodeId(1), X).unwrap();
                    st.set(NodeId(1), X, cur);
                }
                st.animate(NodeId(1), X, plan[k].1, anim);
                k += 1;
            }
            x.push((t as f32, st.sample(NodeId(1), X).unwrap()));
            v.push((t as f32, st.sample_velocity(NodeId(1), X).unwrap()));
        }
        (x, normalized(v))
    };
    let mut panels = Vec::new();
    for (name, anim) in [
        ("spring merge (SwiftUI): keeps x and v", Anim::from(motion::REFLOW)),
        ("tween additive (Core Animation)", Curve::ease_in_out(0.35).into()),
        ("spring, bounce .3", Anim::from(Spring::new(0.4, 0.3))),
    ] {
        let (x, v) = retarget(anim, false);
        let (nx, nv) = retarget(anim, true);
        panels.push(Panel {
            title: name.into(),
            x: (0.0, 1.2),
            y: (-1.1, 1.3),
            series: vec![
                Series { points: nx, color: GREY, dashed: false, label: "naive restart (v = 0)" },
                Series { points: nv, color: GREY, dashed: true, label: "" },
                Series { points: x, color: BLUE, dashed: false, label: "position" },
                Series { points: v, color: ORANGE, dashed: true, label: "velocity" },
            ],
            marks: vec![0.15, 0.32, 0.55],
            equal_axes: false,
        });
    }
    write("retarget.svg", svg(&panels, 3, 380.0, 220.0, "Retargeting at the red lines: position and velocity stay continuous"));

    // 4. Presence: blur-replace enter, exit, revive mid-exit.
    let mut scene = Scene::new();
    scene.insert(NodeId(9), NodeSpec::new(Rect::new(0.0, 0.0, 1.0, 1.0)), Mount::Initial);
    scene.frame_at(0.0);
    let id = NodeId(1);
    let spec = NodeSpec::new(Rect::new(0.0, 0.0, 200.0, 40.0)).transition(Transition::blur_replace());
    scene.insert(id, spec, Mount::Animated);
    let (mut op, mut bl, mut sc) = (Vec::new(), Vec::new(), Vec::new());
    let mut last = None;
    for i in 0..=480 {
        let t = i as f64 / 240.0;
        scene.set_time(t);
        if i == 168 {
            scene.remove(id);
        }
        if i == 220 {
            scene.insert(id, spec, Mount::Animated);
        }
        if i == 400 {
            scene.remove(id);
        }
        let out = scene.frame();
        if let Some(p) = out.changed.iter().find(|p| p.node == id) {
            last = Some(*p);
        }
        if out.exited.contains(&id) {
            last = None;
        }
        if let Some(p) = last {
            op.push((t as f32, p.opacity));
            bl.push((t as f32, p.blur / Transition::blur_replace().insertion.blur));
            sc.push((t as f32, (p.transform.scale.x - 0.96) / 0.04));
        }
    }
    let presence = Panel {
        title: "blurReplace: enter, exit, revive mid-exit, exit".into(),
        x: (0.0, 2.0),
        y: (-0.1, 1.15),
        series: vec![
            Series { points: op, color: BLUE, dashed: false, label: "opacity" },
            Series { points: bl, color: PURPLE, dashed: false, label: "blur / 4 px" },
            Series { points: sc, color: GREEN, dashed: true, label: "(scale − .96) / .04" },
        ],
        marks: vec![0.7, 220.0 / 240.0, 400.0 / 240.0],
        equal_axes: false,
    };

    // 5. Paths: a child moving inside a moving parent.
    let path = |screen: bool| {
        let mut s = Scene::new();
        let (p, c) = (NodeId(10), NodeId(11));
        s.insert(p, NodeSpec::new(Rect::new(0.0, 0.0, 400.0, 400.0)), Mount::Initial);
        s.insert(c, NodeSpec::new(Rect::new(10.0, 10.0, 50.0, 50.0)).parent(p), Mount::Initial);
        s.frame_at(0.0);
        s.set_frame(p, Rect::new(200.0, 100.0, 400.0, 400.0), motion::GLIDE);
        let la = LayoutAnim::new(motion::DRIFT);
        s.set_frame(c, Rect::new(100.0, 200.0, 50.0, 50.0), if screen { la.screen() } else { la });
        let mut pts = Vec::new();
        for i in 0..=360 {
            let out = s.frame_at(i as f64 / 120.0);
            if let Some(q) = out.changed.iter().find(|q| q.node == c) {
                pts.push((q.rect.x, q.rect.y));
            }
        }
        pts
    };
    let paths = Panel {
        title: "child path in root space (parent GLIDE, child DRIFT)".into(),
        x: (0.0, 320.0),
        y: (0.0, 320.0),
        series: vec![
            Series { points: path(false), color: GREY, dashed: false, label: "parent space: dog-leg" },
            Series { points: path(true), color: BLUE, dashed: false, label: "Travel::Screen: straight" },
        ],
        marks: vec![],
        equal_axes: true,
    };
    write("presence-and-paths.svg", svg(&[presence, paths], 2, 420.0, 330.0, "Presence and travel"));
}
