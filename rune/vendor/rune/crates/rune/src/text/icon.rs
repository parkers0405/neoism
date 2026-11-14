//! Icons: a subset of SVG (path, rect, circle, ellipse, line, polyline, polygon; fill/stroke, caps, joins, evenodd,
//! `opacity`, `fill-opacity`, `stroke-opacity`; groups, whose paint attributes their elements
//! inherit and whose `opacity` fades them as one; `transform` on groups and elements). What
//! `defs`, `clipPath`, `mask` and the like hold doesn't paint.

use swash::zeno::{Cap, Fill, Join, Mask, Stroke, Transform};

use super::{Atlas, Rasterized, svg_path};

/// An SVG icon body as found in `@opencode/ui`'s catalog: elements inside a square viewBox,
/// drawn in its element's colour. A body that names its own colours (`fill="#…"`, gradients),
/// such as a file-type badge, draws in them instead, as full SVG (`use`, clips, gradients).
#[derive(Debug)]
pub struct Icon {
    pub name: &'static str,
    pub view: f32,
    pub body: &'static str,
}

/// What draws in place of each icon: an app's alternative icon set (`set_icon_swap`).
type Swap = fn(&Icon) -> Option<&'static Icon>;

static SWAP: std::sync::RwLock<Option<Swap>> = std::sync::RwLock::new(None);

/// Draws each icon `swap` gives another for as that one, and every icon as itself with `None`:
/// an alternative icon set an app's setting turns on, such as Desktop's New icons. Every window
/// draws its next frame with it; an icon keeps its element's size unless [set_icon_swap_size]
/// gives swapped ones another.
pub fn set_icon_swap(swap: Option<Swap>) {
    *SWAP.write().unwrap_or_else(std::sync::PoisonError::into_inner) = swap;
}

/// The size a swapped icon's element takes for the size it's asked for (`set_icon_swap_size`).
type SwapSize = fn(f32) -> f32;

static SWAP_SIZE: std::sync::RwLock<Option<SwapSize>> = std::sync::RwLock::new(None);

/// Sizes the elements of the icons the swap draws others for: `size` takes the points asked for
/// and gives the element's, such as Desktop's New icons, drawn on a 16 pt grid, which stay 16 pt
/// where the icons they replace shrink to 14. Elements made after the call take it, and views
/// build again (every `memo` included) once it or the swap changes. Icons the swap leaves alone
/// keep their size.
pub fn set_icon_swap_size(size: Option<SwapSize>) {
    *SWAP_SIZE.write().unwrap_or_else(std::sync::PoisonError::into_inner) = size;
}

/// `icon`, or what the icon swap draws in its place.
pub(crate) fn swapped(icon: &'static Icon) -> &'static Icon {
    let swap = *SWAP.read().unwrap_or_else(std::sync::PoisonError::into_inner);
    swap.and_then(|f| f(icon)).unwrap_or(icon)
}

/// The points an element for `icon` takes when asked for `size` (`set_icon_swap_size`).
pub(crate) fn swapped_size(icon: &'static Icon, size: f32) -> f32 {
    let Some(grow) = *SWAP_SIZE.read().unwrap_or_else(std::sync::PoisonError::into_inner) else { return size };
    if std::ptr::eq(swapped(icon), icon) { size } else { grow(size) }
}

/// Which icon swap and swap size are on, for memos to build again when either changes.
pub(crate) fn swap_state() -> (usize, usize) {
    (swap_id(), SWAP_SIZE.read().unwrap_or_else(std::sync::PoisonError::into_inner).map_or(0, |f| f as usize))
}

/// Which icon swap is on, for a cached layer's paint state.
pub(crate) fn swap_id() -> usize {
    SWAP.read().unwrap_or_else(std::sync::PoisonError::into_inner).map_or(0, |f| f as usize)
}

fn attr<'a>(el: &'a str, name: &str) -> Option<&'a str> {
    let mut rest = el;
    loop {
        let i = rest.find(name)?;
        let before = rest[..i].chars().last();
        let after = &rest[i + name.len()..];
        if before.is_some_and(|c| c.is_whitespace()) && after.starts_with("=\"") {
            let v = &after[2..];
            return Some(&v[..v.find('"')?]);
        }
        rest = &rest[i + name.len()..];
    }
}

fn num(el: &str, name: &str) -> f32 {
    attr(el, name).and_then(|v| v.trim_end_matches("px").parse().ok()).unwrap_or(0.0)
}

/// SVG path data for one icon element, in viewBox units.
fn element_path(el: &str) -> Option<String> {
    let tag = el.trim_start_matches('<').split_whitespace().next()?;
    Some(match tag {
        "path" => attr(el, "d")?.to_owned(),
        "rect" => {
            let (x, y, w, h) = (num(el, "x"), num(el, "y"), num(el, "width"), num(el, "height"));
            let r = num(el, "rx").min(w / 2.0).min(h / 2.0);
            if r <= 0.0 {
                format!("M{x} {y}h{w}v{h}h{}Z", -w)
            } else {
                format!(
                    "M{} {y}h{}a{r} {r} 0 0 1 {r} {r}v{}a{r} {r} 0 0 1 {} {r}h{}a{r} {r} 0 0 1 {} {}v{}a{r} {r} 0 0 1 {r} {}Z",
                    x + r,
                    w - 2.0 * r,
                    h - 2.0 * r,
                    -r,
                    -(w - 2.0 * r),
                    -r,
                    -r,
                    -(h - 2.0 * r),
                    -r
                )
            }
        }
        "circle" | "ellipse" => {
            let (cx, cy) = (num(el, "cx"), num(el, "cy"));
            let (rx, ry) = if tag == "circle" { (num(el, "r"), num(el, "r")) } else { (num(el, "rx"), num(el, "ry")) };
            format!(
                "M{} {cy}a{rx} {ry} 0 1 0 {} 0a{rx} {ry} 0 1 0 {} 0Z",
                cx - rx,
                2.0 * rx,
                -2.0 * rx
            )
        }
        "line" => format!("M{} {}L{} {}", num(el, "x1"), num(el, "y1"), num(el, "x2"), num(el, "y2")),
        "polyline" | "polygon" => {
            let p = numbers(attr(el, "points")?);
            let mut d: String =
                p.as_chunks::<2>().0.iter().enumerate().map(|(i, [x, y])| format!("{}{x} {y}", if i == 0 { 'M' } else { 'L' })).collect();
            if tag == "polygon" {
                d.push('Z');
            }
            d
        }
        _ => return None,
    })
}

/// Which way an icon's drawing faces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Facing {
    /// As drawn.
    Upright,
    /// Mirrored left to right: a directional icon in a right-to-left element (`El::mirror_in_rtl`).
    Mirrored,
}

pub(super) fn rasterize_icon(icon: &Icon, px: u32, facing: Facing, atlas: &mut Atlas) -> Rasterized {
    if px == 0 {
        return Rasterized::Empty;
    }
    if px + 4 > super::ATLAS_SIZE {
        return Rasterized::Unplaced;
    }
    let mut cov = icon_coverage(icon, px);
    if facing == Facing::Mirrored {
        cov.chunks_exact_mut(px as usize).for_each(<[u8]>::reverse);
    }
    if !cov.iter().any(|&c| c != 0) {
        return Rasterized::Empty;
    }
    // Pad by one transparent texel on every side so sub-pixel, scaled and turned quads can
    // expand by a texel and filter out to zero without clipping or sampling a neighbour.
    let stride = (px + 2) as usize;
    let mut padded = vec![0u8; stride * stride];
    for row in 0..px as usize {
        let src = &cov[row * px as usize..(row + 1) * px as usize];
        let dst = (row + 1) * stride + 1;
        padded[dst..dst + px as usize].copy_from_slice(src);
    }
    match atlas.put(px + 2, px + 2, &padded, 0, 0) {
        Some(g) => Rasterized::Placed(g),
        None => Rasterized::Unplaced,
    }
}

/// What an open element hands the ones inside it: its transform to device pixels, whether they
/// paint at all, and the group whose paint attributes they inherit.
struct Scope<'a> {
    ctm: Transform,
    paints: bool,
    group: Option<&'a str>,
    /// It fades what it holds as one (`opacity` on a group): drawn into its own layer.
    layer: bool,
}

/// Paint attributes SVG passes from a group to the elements inside it.
const INHERITED: [&str; 8] =
    ["fill", "stroke", "stroke-width", "stroke-linecap", "stroke-linejoin", "fill-rule", "fill-opacity", "stroke-opacity"];

/// `icon`'s coverage at `px` × `px` device pixels, row-major, as the glyph atlas stores it.
pub fn icon_coverage(icon: &Icon, px: u32) -> Vec<u8> {
    let s = px as f32 / icon.view;
    let mut acc = vec![0u8; (px * px) as usize];
    // Each open group with an `opacity`: what it holds so far, and that opacity.
    let mut layers: Vec<(Vec<u8>, f32)> = Vec::new();
    let mut buf = vec![0u8; (px * px) as usize];
    let root = Scope { ctm: Transform::scale(s, s), paints: true, group: None, layer: false };
    let mut scopes: Vec<Scope> = Vec::new();
    for raw in icon.body.split('>') {
        let el = raw.trim();
        if el.is_empty() || el.starts_with("<!") || el.starts_with("<?") {
            continue;
        }
        if el.starts_with("</") {
            if scopes.pop().is_some_and(|sc| sc.layer)
                && let Some((top, alpha)) = layers.pop()
            {
                union(layers.last_mut().map_or(&mut acc, |l| &mut l.0), &top, alpha);
            }
            continue;
        }
        let tag = el.trim_start_matches('<').split_whitespace().next().unwrap_or("").trim_end_matches('/');
        let open = !el.ends_with('/');
        let up = scopes.last().unwrap_or(&root);
        let ctm = attr(el, "transform").map_or(up.ctm, |t| then(transform(t), up.ctm));
        let paints = up.paints && attr(el, "display") != Some("none") && !NON_RENDERING.contains(&tag);
        if tag == "g" || tag == "a" || tag == "svg" {
            if open {
                let alpha = opacity(attr(el, "opacity"));
                let layer = paints && alpha < 1.0;
                if layer {
                    layers.push((vec![0u8; (px * px) as usize], alpha));
                }
                scopes.push(Scope { ctm, paints, group: Some(el), layer });
            }
            continue;
        }
        if open {
            scopes.push(Scope { ctm, paints: false, group: None, layer: false });
        }
        if !paints {
            continue;
        }
        let Some(d) = element_path(el) else { continue };
        let d = svg_path::parse(&d);
        // The element's own attribute, else the innermost group's that sets it.
        let get = |name: &str| {
            attr(el, name).or_else(|| {
                scopes.iter().rev().filter_map(|sc| sc.group).find_map(|g| attr(g, name)).filter(|_| INHERITED.contains(&name))
            })
        };
        let stroke = get("stroke").is_some_and(|v| v != "none");
        let fill = get("fill").map_or(!stroke, |v| v != "none");
        let whole = opacity(attr(el, "opacity"));
        let into = layers.last_mut().map_or(&mut acc, |l| &mut l.0);
        let mut paint = |style: swash::zeno::Style<'_>, alpha: f32| {
            buf.iter_mut().for_each(|b| *b = 0);
            Mask::new(d.as_slice()).style(style).transform(Some(ctm)).size(px, px).render_into(&mut buf, None);
            union(into, &buf, alpha);
        };
        if fill {
            let rule = if get("fill-rule") == Some("evenodd") { Fill::EvenOdd } else { Fill::NonZero };
            paint(rule.into(), whole * opacity(get("fill-opacity")));
        }
        if stroke {
            let w = get("stroke-width").and_then(|v| v.trim_end_matches("px").parse().ok()).unwrap_or(1.0f32);
            // The transform scales the width with the geometry.
            let mut st = Stroke::new(w);
            st.cap(match get("stroke-linecap") {
                Some("round") => Cap::Round,
                Some("square") => Cap::Square,
                _ => Cap::Butt,
            });
            st.join(match get("stroke-linejoin") {
                Some("round") => Join::Round,
                Some("bevel") => Join::Bevel,
                _ => Join::Miter,
            });
            paint(st.into(), whole * opacity(get("stroke-opacity")));
        }
    }
    // A group left open still ends at the body's end.
    while let Some((top, alpha)) = layers.pop() {
        union(layers.last_mut().map_or(&mut acc, |l| &mut l.0), &top, alpha);
    }
    acc
}

/// Elements whose contents are referenced, not drawn where they stand.
const NON_RENDERING: [&str; 12] = [
    "defs", "clipPath", "mask", "symbol", "linearGradient", "radialGradient", "pattern", "marker", "filter", "title",
    "desc", "style",
];

/// Coverage `top` at `alpha` over `acc`, as overlapping ink adds up.
fn union(acc: &mut [u8], top: &[u8], alpha: f32) {
    for (a, &b) in acc.iter_mut().zip(top) {
        let (fa, fb) = (*a as u32, (b as f32 * alpha).round() as u32);
        *a = (fa + fb - fa * fb / 255).min(255) as u8;
    }
}

/// An opacity attribute (`0.5` or `50%`), clamped; 1 when absent or unreadable.
fn opacity(v: Option<&str>) -> f32 {
    let Some(v) = v.map(str::trim) else { return 1.0 };
    let n = match v.strip_suffix('%') {
        Some(p) => p.trim().parse::<f32>().map(|p| p / 100.0),
        None => v.parse::<f32>(),
    };
    n.map_or(1.0, |n| n.clamp(0.0, 1.0))
}

/// An SVG `transform` list as one transform from the element's units to its parent's: the
/// rightmost function applies first, as SVG composes them.
fn transform(list: &str) -> Transform {
    let mut m = Transform::IDENTITY;
    for item in list.split(')') {
        let Some((name, args)) = item.split_once('(') else { continue };
        let a = numbers(args);
        let at = |i: usize| a.get(i).copied().unwrap_or(0.0);
        let t = match name.trim().trim_start_matches(',').trim() {
            "matrix" if a.len() == 6 => Transform::new(a[0], a[1], a[2], a[3], a[4], a[5]),
            "translate" => Transform::translation(at(0), at(1)),
            "scale" => Transform::scale(at(0), a.get(1).copied().unwrap_or(at(0))),
            "rotate" => {
                let (sin, cos) = at(0).to_radians().sin_cos();
                let (cx, cy) = (at(1), at(2));
                // translate(cx cy) rotate(a) translate(-cx -cy)
                Transform::new(cos, sin, -sin, cos, cx - cos * cx + sin * cy, cy - sin * cx - cos * cy)
            }
            "skewX" => Transform::new(1.0, 0.0, at(0).to_radians().tan(), 1.0, 0.0, 0.0),
            "skewY" => Transform::new(1.0, at(0).to_radians().tan(), 0.0, 1.0, 0.0, 0.0),
            _ => continue,
        };
        m = then(t, m);
    }
    m
}

/// `a`, then `b`. zeno's `Transform::then` mixes up its off-diagonal terms (a skew under a
/// translate moves the wrong way), so compose here as `Transform::transform_point` reads them.
fn then(a: Transform, b: Transform) -> Transform {
    Transform::new(
        a.xx * b.xx + a.xy * b.yx,
        a.xx * b.xy + a.xy * b.yy,
        a.yx * b.xx + a.yy * b.yx,
        a.yx * b.xy + a.yy * b.yy,
        a.x * b.xx + a.y * b.yx + b.x,
        a.x * b.xy + a.y * b.yy + b.y,
    )
}

/// The numbers in a transform's arguments or a `points` list, which commas, spaces or a sign
/// separate.
fn numbers(s: &str) -> Vec<f32> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev = ' ';
    for c in s.chars() {
        let starts = (c == '-' || c == '+') && !matches!(prev, 'e' | 'E');
        if c == ',' || c.is_whitespace() || (starts && !cur.is_empty()) {
            out.extend(cur.parse::<f32>().ok());
            cur.clear();
        }
        if !(c == ',' || c.is_whitespace()) {
            cur.push(c);
        }
        prev = c;
    }
    out.extend(cur.parse::<f32>().ok());
    out
}


#[cfg(test)]
#[path = "icon_tests.rs"]
mod tests;
