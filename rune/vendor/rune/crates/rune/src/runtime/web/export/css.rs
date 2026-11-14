//! CSS for an exported element: its Taffy style as flexbox, its paint and its text.

use std::fmt::Write;

use taffy::style::{CompactLength, Display, FlexDirection, FlexWrap, Overflow, Position, Style};

use super::escape;
use crate::color::Color;
use crate::el::{GradientGeometry, RadialGradient, TextSpec, Visual};

pub(super) type Decls = Vec<(&'static str, String)>;

fn num(v: f32) -> String {
    let r = (v * 100.0).round() / 100.0;
    if r == r.trunc() { format!("{}", r as i64) } else { format!("{r}") }
}

fn px(v: f32) -> String {
    if v == 0.0 { "0".into() } else { format!("{}px", num(v)) }
}

/// A length, percentage or `auto`; `None` for what CSS can't take as is (calc, fr).
fn len(raw: CompactLength) -> Option<String> {
    match raw.tag() {
        CompactLength::LENGTH_TAG => Some(px(raw.value())),
        CompactLength::PERCENT_TAG => Some(format!("{}%", num(raw.value() * 100.0))),
        CompactLength::AUTO_TAG => Some("auto".into()),
        _ => None,
    }
}

/// A Rust variant name in CSS's spelling: `SpaceBetween` to `space-between`.
fn kebab(debug: String) -> String {
    let mut out = String::new();
    for (k, c) in debug.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if k > 0 { out.push('-'); }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

pub(super) fn color(c: Color) -> String {
    let hex = c.hex();
    match hex.strip_suffix("ff") {
        Some(opaque) => opaque.to_owned(),
        None => hex,
    }
}

/// The element's box: what differs from a Rune element's defaults, which the page's base rule
/// (`display: flex; position: relative; box-sizing: border-box`) already sets.
pub(super) fn layout(s: &Style, root: bool) -> Decls {
    let mut d: Decls = Vec::new();
    let push_len = |d: &mut Decls, prop: &'static str, raw: CompactLength, default: &str| {
        if let Some(v) = len(raw).filter(|v| v != default) { d.push((prop, v)); }
    };
    match s.display {
        Display::Flex => {}
        Display::Grid => d.push(("display", "grid".into())),
        Display::None => d.push(("display", "none".into())),
        _ => d.push(("display", "block".into())),
    }
    if s.flex_direction != FlexDirection::Row { d.push(("flex-direction", kebab(format!("{:?}", s.flex_direction)))); }
    if s.flex_wrap != FlexWrap::NoWrap { d.push(("flex-wrap", kebab(format!("{:?}", s.flex_wrap)))); }
    let align = |keyword: String, safe: bool| if safe { format!("safe {}", kebab(keyword)) } else { kebab(keyword) };
    let safe = |debug: String| debug == "Safe";
    if format!("{:?}", s.align_items.keyword) != "Normal" {
        d.push(("align-items", align(format!("{:?}", s.align_items.keyword), safe(format!("{:?}", s.align_items.safety)))));
    }
    if let Some(a) = s.align_self { d.push(("align-self", align(format!("{:?}", a.keyword), safe(format!("{:?}", a.safety))))); }
    if format!("{:?}", s.justify_content.keyword) != "Normal" {
        d.push(("justify-content", align(format!("{:?}", s.justify_content.keyword), safe(format!("{:?}", s.justify_content.safety)))));
    }
    if format!("{:?}", s.align_content.keyword) != "Normal" {
        d.push(("align-content", align(format!("{:?}", s.align_content.keyword), false)));
    }
    push_len(&mut d, "row-gap", s.gap.height.into_raw(), "0");
    push_len(&mut d, "column-gap", s.gap.width.into_raw(), "0");
    if s.flex_grow != 0.0 { d.push(("flex-grow", num(s.flex_grow))); }
    if s.flex_shrink != 1.0 { d.push(("flex-shrink", num(s.flex_shrink))); }
    push_len(&mut d, "flex-basis", s.flex_basis.into_raw(), "auto");
    if matches!(s.position, Position::Absolute | Position::Fixed) {
        d.push(("position", "absolute".into()));
        push_len(&mut d, "left", s.inset.left.into_raw(), "auto");
        push_len(&mut d, "right", s.inset.right.into_raw(), "auto");
        push_len(&mut d, "top", s.inset.top.into_raw(), "auto");
        push_len(&mut d, "bottom", s.inset.bottom.into_raw(), "auto");
    }
    if root {
        // The root fills the page's width and grows with its content.
        d.push(("width", "100%".into()));
    } else {
        push_len(&mut d, "width", s.size.width.into_raw(), "auto");
        push_len(&mut d, "height", s.size.height.into_raw(), "auto");
        push_len(&mut d, "min-width", s.min_size.width.into_raw(), "auto");
        push_len(&mut d, "min-height", s.min_size.height.into_raw(), "auto");
        push_len(&mut d, "max-width", s.max_size.width.into_raw(), "auto");
        push_len(&mut d, "max-height", s.max_size.height.into_raw(), "auto");
    }
    if let Some(r) = s.aspect_ratio { d.push(("aspect-ratio", num(r))); }
    let sides = |r: &taffy::geometry::Rect<CompactLength>| [r.top, r.right, r.bottom, r.left].map(len);
    let shorthand = |vals: [Option<String>; 4]| -> Option<String> {
        let v: Vec<String> = vals.into_iter().map(|v| v.unwrap_or_else(|| "0".into())).collect();
        if v.iter().all(|x| x == "0") { return None; }
        Some(if v[0] == v[2] && v[1] == v[3] {
            if v[0] == v[1] { v[0].clone() } else { format!("{} {}", v[0], v[1]) }
        } else {
            v.join(" ")
        })
    };
    let raw = |r: taffy::geometry::Rect<CompactLength>| r;
    let margin = raw(taffy::geometry::Rect {
        left: s.margin.left.into_raw(), right: s.margin.right.into_raw(), top: s.margin.top.into_raw(), bottom: s.margin.bottom.into_raw(),
    });
    let padding = raw(taffy::geometry::Rect {
        left: s.padding.left.into_raw(), right: s.padding.right.into_raw(), top: s.padding.top.into_raw(), bottom: s.padding.bottom.into_raw(),
    });
    let border = raw(taffy::geometry::Rect {
        left: s.border.left.into_raw(), right: s.border.right.into_raw(), top: s.border.top.into_raw(), bottom: s.border.bottom.into_raw(),
    });
    if let Some(v) = shorthand(sides(&margin)) { d.push(("margin", v)); }
    if let Some(v) = shorthand(sides(&padding)) { d.push(("padding", v)); }
    if let Some(v) = shorthand(sides(&border)) { d.push(("border-width", v)); }
    let overflow = |o: Overflow| match o {
        Overflow::Visible => None,
        Overflow::Clip => Some("clip"),
        Overflow::Hidden => Some("hidden"),
        Overflow::Scroll => Some("auto"),
    };
    if let Some(v) = overflow(s.overflow.x) { d.push(("overflow-x", v.into())); }
    if let Some(v) = overflow(s.overflow.y) { d.push(("overflow-y", v.into())); }
    d
}

/// Fill, gradients, border, corners, shadows, opacity, blurs and soft clips, for an element
/// `size` points big.
pub(super) fn paint(v: &Visual, size: (f32, f32), d: &mut Decls) {
    // Rune paints the fill, then the vertical gradient, then the radial or angled one: CSS lists
    // background layers top first, with the colour last.
    let mut layers: Vec<String> = Vec::new();
    if let Some(r) = v.cold.radial.as_deref() { layers.push(gradient(r, size)); }
    if let Some(g) = v.cold.gradient.as_deref() { layers.push(format!("linear-gradient({},{})", color(g[0]), color(g[1]))); }
    if v.fill.a > 0.0 { layers.push(color(v.fill)); }
    if !layers.is_empty() { d.push(("background", layers.join(","))); }
    if let Some(clip) = v.cold.clip { mask(clip, d); }
    let r = v.radii();
    if r.iter().any(|&c| c > 0.0) {
        let value = if r.iter().all(|&c| c == r[0]) { px(r[0]) } else { r.map(px).join(" ") };
        d.push(("border-radius", value));
    }
    let laid_out_border = d.iter().any(|(p, _)| *p == "border-width");
    let mut shadows: Vec<String> = Vec::new();
    if v.border_width() > 0.0 && v.border().a > 0.0 {
        if laid_out_border {
            d.push(("border-style", "solid".into()));
            d.push(("border-color", color(v.border())));
        } else if let Some([t, r, b, l]) = v.cold.border_sides {
            for (x, y, w) in [(0.0, t, t), (-r, 0.0, r), (0.0, -b, b), (l, 0.0, l)] {
                if w > 0.0 { shadows.push(format!("inset {} {} 0 0 {}", px(x), px(y), color(v.border()))); }
            }
        } else {
            shadows.push(format!("inset 0 0 0 {} {}", px(v.border_width()), color(v.border())));
        }
    }
    for s in &v.cold.shadows {
        let inset = if s.inset { "inset " } else { "" };
        shadows.push(format!("{inset}{} {} {} {} {}", px(s.x), px(s.y), px(s.blur), px(s.spread), color(s.color)));
    }
    if !shadows.is_empty() { d.push(("box-shadow", shadows.join(","))); }
    let opacity = v.opacity * v.layer_opacity;
    if opacity < 0.999 { d.push(("opacity", num(opacity))); }
    if v.cold.clip.is_some() || v.cold.clip_radius.is_some() {
        if !d.iter().any(|(p, _)| p.starts_with("overflow")) { d.push(("overflow", "hidden".into())); }
    }
    if v.cold.backdrop_blur > 0.0 { d.push(("backdrop-filter", format!("blur({})", px(v.cold.backdrop_blur)))); }
    if v.blur > 0.0 { d.push(("filter", format!("blur({})", px(v.blur)))); }
}

/// A radial gradient's radii as percentages of the box, so it scales with the box between the
/// widths the page was taken at.
fn gradient(r: &RadialGradient, (w, h): (f32, f32)) -> String {
    let stops: Vec<String> = r.stops.iter().map(|&(t, c)| format!("{} {}%", color(c), num(t * 100.0))).collect();
    match r.geometry {
        GradientGeometry::Radial { center, radius } => {
            let pct = |r: f32, side: f32| if side > 0.0 { format!("{}%", num(r / side * 100.0)) } else { px(r) };
            format!("radial-gradient({} {} at {}% {}%,{})", pct(radius[0], w), pct(radius[1], h),
                num(center[0] * 100.0), num(center[1] * 100.0), stops.join(","))
        }
        GradientGeometry::Linear { angle } => format!("linear-gradient({}deg,{})", num(angle.to_degrees()), stops.join(",")),
    }
}

/// A soft clip as a mask: one ramp per axis, intersected. Rune eases a positive width and ramps a
/// negative one linearly; CSS ramps both, which meet at the middle of the fade.
fn mask([l, t, r, b]: [f32; 4], d: &mut Decls) {
    let ramp = |to: &str, a: f32, z: f32| -> Option<String> {
        if a == 0.0 && z == 0.0 { return None; }
        let side = |w: f32, from_end: bool| -> Vec<String> {
            let at = |p: f32| if from_end { format!("calc(100% - {})", px(p)) } else { px(p) };
            let edge = if from_end { "100%" } else { "0" };
            let mut s = vec![format!("transparent {edge}"), format!("#000 {}", at(w.abs()))];
            if from_end { s.reverse(); }
            s
        };
        let mut stops = if a != 0.0 { side(a, false) } else { vec!["#000 0".to_owned()] };
        stops.extend(if z != 0.0 { side(z, true) } else { vec!["#000 100%".to_owned()] });
        Some(format!("linear-gradient(to {to},{})", stops.join(",")))
    };
    let layers: Vec<String> = [ramp("right", l, r), ramp("bottom", t, b)].into_iter().flatten().collect();
    if layers.is_empty() { return; }
    let image = layers.join(",");
    d.push(("-webkit-mask-image", image.clone()));
    d.push(("mask-image", image));
    if layers.len() == 2 {
        d.push(("-webkit-mask-composite", "source-in".into()));
        d.push(("mask-composite", "intersect".into()));
    }
}

fn family(f: crate::tok::Family) -> String {
    match f {
        crate::tok::Family::Sans => "var(--rune-sans)".into(),
        crate::tok::Family::Mono | crate::tok::Family::SystemMono => "var(--rune-mono)".into(),
        crate::tok::Family::Named(name) => format!("\"{}\"", escape(name)),
    }
}

/// The text's font, colour, alignment and wrapping.
pub(super) fn text(spec: &TextSpec, d: &mut Decls) {
    let s = spec.style;
    if !d.iter().any(|(p, _)| *p == "display") { d.push(("display", "block".into())); }
    d.push(("font-family", family(s.family)));
    d.push(("font-size", px(s.size)));
    d.push(("font-weight", num(s.weight)));
    d.push(("line-height", px(if s.line_height > 0.0 { s.line_height } else { s.size * 1.25 })));
    if s.tracking != 0.0 { d.push(("letter-spacing", px(s.tracking))); }
    if s.italic { d.push(("font-style", "italic".into())); }
    d.push(("color", color(spec.color)));
    match spec.align {
        crate::el::Align::Start => {}
        crate::el::Align::Center => d.push(("text-align", "center".into())),
        crate::el::Align::End => d.push(("text-align", "right".into())),
    }
    if spec.justify { d.push(("text-align", "justify".into())); }
    if spec.truncate {
        d.push(("white-space", "pre".into()));
        d.push(("overflow", "hidden".into()));
        d.push(("text-overflow", "ellipsis".into()));
    } else {
        d.push(("white-space", if spec.wrap { "pre-wrap" } else { "pre" }.into()));
        if spec.wrap_anywhere { d.push(("overflow-wrap", "anywhere".into())); }
    }
}

/// The text with its spans (weight, size, colour, code backgrounds, underlines) and its links.
pub(super) fn text_html(spec: &TextSpec) -> String {
    let content = &spec.content;
    let mut cuts: Vec<usize> = vec![0, content.len()];
    for s in &spec.spans { cuts.extend([s.start as usize, s.end as usize]); }
    for l in &spec.links { cuts.extend([l.start as usize, l.end as usize]); }
    cuts.retain(|&c| c <= content.len() && content.is_char_boundary(c));
    cuts.sort_unstable();
    cuts.dedup();
    let mut out = String::new();
    let mut open_link: Option<usize> = None;
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let link = spec.links.iter().position(|l| (l.start as usize) <= a && b <= l.end as usize);
        if link != open_link {
            if open_link.is_some() { out.push_str("</a>"); }
            if let Some(k) = link { let _ = write!(out, "<a href=\"{}\">", escape(&spec.links[k].id)); }
            open_link = link;
        }
        let piece = escape(&content[a..b]);
        let mut style = String::new();
        if let Some(s) = spec.spans.iter().rev().find(|s| (s.start as usize) <= a && b <= s.end as usize) {
            if let Some(w) = s.weight { let _ = write!(style, "font-weight:{};", num(w)); }
            if s.mono { style.push_str("font-family:var(--rune-mono);"); }
            if let Some(z) = s.size { let _ = write!(style, "font-size:{};", px(z)); }
            if let Some(c) = s.color { let _ = write!(style, "color:{};", color(c)); }
            if let Some(c) = s.bg { let _ = write!(style, "background:{};border-radius:3px;padding:0 {};", color(c), px(s.bg_pad)); }
            if let Some(c) = s.underline { let _ = write!(style, "text-decoration:underline {};", color(c)); }
            if s.italic { style.push_str("font-style:italic;"); }
        }
        if style.is_empty() {
            out.push_str(&piece);
        } else {
            let _ = write!(out, "<span style=\"{}\">{piece}</span>", style.trim_end_matches(';'));
        }
    }
    if open_link.is_some() { out.push_str("</a>"); }
    out
}

/// A shape as inline SVG: fills as paths, strokes as polylines, stretched like Rune's.
pub(super) fn svg(shape: &crate::Shape, size: (f32, f32)) -> String {
    let [w, h] = shape.view_box.unwrap_or([size.0, size.1]);
    let mut out = format!(
        "<svg viewBox=\"0 0 {} {}\" preserveAspectRatio=\"none\" width=\"100%\" height=\"100%\" aria-hidden=\"true\" focusable=\"false\" shape-rendering=\"crispEdges\">",
        num(w), num(h),
    );
    let points = |pts: &[[f32; 2]]| pts.iter().map(|p| format!("{},{}", num(p[0]), num(p[1]))).collect::<Vec<_>>().join(" ");
    // Consecutive fills of one colour share a path.
    let mut fill: Option<(Color, String)> = None;
    let flush = |out: &mut String, fill: &mut Option<(Color, String)>| {
        if let Some((c, d)) = fill.take() { let _ = write!(out, "<path fill=\"{}\" d=\"{d}\"/>", color(c)); }
    };
    for op in &shape.ops {
        match op {
            crate::shape::Op::Fill { points: pts, color: c } => {
                if fill.as_ref().is_some_and(|(fc, _)| fc != c) { flush(&mut out, &mut fill); }
                let d = &mut fill.get_or_insert_with(|| (*c, String::new())).1;
                let _ = write!(d, "M{}Z", points(pts).replace(' ', "L"));
            }
            crate::shape::Op::Stroke { points: pts, closed, width, cap, color: c } => {
                flush(&mut out, &mut fill);
                let el = if *closed { "polygon" } else { "polyline" };
                let cap = if *cap == crate::shape::LineCap::Round { "round" } else { "butt" };
                let _ = write!(out, "<{el} fill=\"none\" stroke=\"{}\" stroke-width=\"{}\" stroke-linecap=\"{cap}\" stroke-linejoin=\"round\" vector-effect=\"non-scaling-stroke\" points=\"{}\"/>",
                    color(*c), num(*width), points(pts));
            }
        }
    }
    flush(&mut out, &mut fill);
    out.push_str("</svg>");
    out
}
