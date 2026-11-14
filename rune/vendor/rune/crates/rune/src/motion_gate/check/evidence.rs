//! Evidence for a finding: a dump of the tracks around it and a contact strip of its frames.

use super::paint::{self, CHANNELS, Paint};
use super::{Recording, Snap};

fn paint_line(p: &Paint) -> String {
    let mut out = String::new();
    for ch in CHANNELS {
        let v = paint::value(p, ch.name);
        let quiet = matches!(v.as_str(), "none" | "0.0" | "#00000000") || v.ends_with("00") && v.len() == 9;
        if !quiet { out += &format!(" {}={v}", ch.name); }
    }
    out += &format!(" z={}", p.rank);
    if p.intent != 0 { out += &format!(" intent={:#x}", p.intent); }
    if p.instant { out += " instant"; }
    out
}

/// Frames `from..to` of every track whose path contains `filter`, one line per frame:
/// `frame rect visible opacity flags` and what it paints. For reading a finding's
/// neighbourhood.
pub fn dump(rec: &Recording, filter: &str, from: usize, to: usize) -> String {
    let mut out = String::new();
    for t in rec.tracks.iter().filter(|t| t.path.contains(filter)) {
        out += &format!("{}\n", t.path);
        for (j, (k, s)) in t.frames.iter().enumerate().filter(|(_, f)| f.0 >= from && f.0 < to) {
            out += &format!("  {k:4} [{:7.1} {:7.1} {:6.1} {:6.1}] vis [{:6.1} {:6.1} {:6.1} {:6.1}] op {:.3} sc {:.3} {:02b}{}{}\n",
                s.rect[0], s.rect[1], s.rect[2], s.rect[3], s.vis[0], s.vis[1], s.vis[2], s.vis[3], s.op, s.scale, s.flags,
                if rec.instant.get(*k) == Some(&true) { " INSTANT" } else { "" },
                t.paint.get(j).and_then(Option::as_ref).map_or(String::new(), paint_line));
        }
    }
    out
}

const Z: f32 = 0.25;
const SWATCH: u32 = 10;

/// A contact strip of frames `at-4..=at+4`: the composited window (see `raster`), every
/// element's visible rect as an outline over it, as strong as its opacity, `focus`'s in red.
/// Under each frame, a swatch per colour `focus` paints (fill, border, text, shadow, gradient,
/// outline), so the colour that broke shows frame by frame. Secondary evidence: the findings are
/// the gate.
pub fn sheet(rec: &Recording, at: usize, focus: &str) -> ::image::RgbImage {
    let (w, h) = ((rec.size.0 * Z) as u32, (rec.size.1 * Z) as u32);
    let frames: Vec<usize> = (at.saturating_sub(4)..=at + 4).filter(|&k| k < rec.frames).collect();
    let gap = 6;
    let hot = |path: &str| !focus.is_empty() && path.contains(focus);
    // The colours the focus paints anywhere in the strip.
    let used: Vec<usize> = (0..paint::COLORS).filter(|&c| rec.tracks.iter().filter(|t| hot(&t.path))
        .any(|t| frames.iter().any(|&k| t.paint_at(k).is_some_and(|p| p.colors[c].a > 0.02)))).collect();
    let strip = used.len() as u32 * (SWATCH + 2);
    let bg = ::image::Rgb([16, 18, 22]);
    let mut img = ::image::RgbImage::from_pixel((w + gap) * frames.len() as u32, h + strip + 4, bg);
    let srgb = |c: crate::color::Color| [c.r, c.g, c.b].map(|v| (crate::color::srgb_encode(v.clamp(0.0, 1.0)) * 255.0).round() as u8);
    for (col, &k) in frames.iter().enumerate() {
        let x0 = col as u32 * (w + gap);
        if let Some(g) = rec.grids.get(k).and_then(|g| g.as_deref()) {
            for py in 0..h {
                for px in 0..w {
                    let (cx, cy) = (((px as f32 / Z) / super::raster::CELL) as usize, ((py as f32 / Z) / super::raster::CELL) as usize);
                    if cx < g.w && cy < g.h { img.put_pixel(x0 + px, py, ::image::Rgb(srgb(g.color(cy * g.w + cx)))); }
                }
            }
        }
        if k == at {
            for x in x0..x0 + w { img.put_pixel(x, 0, ::image::Rgb([255, 210, 80])); }
        }
        for t in &rec.tracks {
            let Some(s) = t.at(k) else { continue };
            outline(&mut img, x0, (w, h), s, hot(&t.path));
        }
        let focus_paint = rec.tracks.iter().filter(|t| hot(&t.path)).find_map(|t| t.paint_at(k));
        for (row, &c) in used.iter().enumerate() {
            let Some(p) = focus_paint else { continue };
            let y = h + 4 + row as u32 * (SWATCH + 2);
            // Over a checker of black and white, so alpha shows.
            for py in 0..SWATCH {
                for px in 0..w.min(80) {
                    let under = if (px / 5 + py / 5) % 2 == 0 { crate::color::Color::BLACK } else { crate::color::Color::WHITE };
                    let [r, g, b, _] = crate::color::over(p.colors[c].premul(1.0), under.premul(1.0));
                    img.put_pixel(x0 + px, y + py, ::image::Rgb(srgb(crate::color::Color { r, g, b, a: 1.0 })));
                }
            }
        }
    }
    img
}

fn outline(img: &mut ::image::RgbImage, x0: u32, (w, h): (u32, u32), s: &Snap, hot: bool) {
    let a = s.op.clamp(0.0, 1.0) * if hot { 1.0 } else { 0.5 };
    let ink = if hot { [255.0, 70.0, 70.0] } else { [200.0, 205.0, 215.0] };
    let [x, y, rw, rh] = s.vis.map(|v| v * Z);
    if rw < 1.0 || rh < 1.0 { return; }
    let mut plot = |px: f32, py: f32| {
        let (px, py) = (px as u32, py as u32);
        if px >= w || py >= h { return; }
        let p = img.get_pixel_mut(x0 + px, py);
        for c in 0..3 { p.0[c] = (p.0[c] as f32 * (1.0 - a) + ink[c] * a) as u8; }
    };
    for i in 0..rw as u32 { plot(x + i as f32, y); plot(x + i as f32, y + rh - 1.0); }
    for i in 0..rh as u32 { plot(x, y + i as f32); plot(x + rw - 1.0, y + i as f32); }
}
