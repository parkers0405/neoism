#![allow(dead_code)]

use rune::runtime::ElementInfo;
use rune::{Key, KeyEvent, Mods, Runtime};

pub fn find<S: 'static>(rt: &Runtime<S>, key: &str) -> ElementInfo {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("{key}"))
}

pub fn centre(e: &ElementInfo) -> (f32, f32) {
    (e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0)
}

pub fn tick<S: 'static>(rt: &mut Runtime<S>) {
    rt.frame(1.0 / 60.0);
}

pub fn wait<S: 'static>(rt: &mut Runtime<S>) {
    rt.frame(1.0);
}

pub fn click<S: 'static>(rt: &mut Runtime<S>, (x, y): (f32, f32)) {
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    tick(rt);
}

pub fn drag<S: 'static>(rt: &mut Runtime<S>, (x0, y0): (f32, f32), (x1, y1): (f32, f32)) {
    rt.pointer_move(x0, y0);
    rt.pointer_down(x0, y0);
    tick(rt);
    for k in 1..=6 {
        let f = k as f32 / 6.0;
        rt.pointer_move(x0 + (x1 - x0) * f, y0 + (y1 - y0) * f);
        tick(rt);
    }
    rt.pointer_up(x1, y1);
    tick(rt);
}

pub fn command<S: 'static>(rt: &mut Runtime<S>, c: char) {
    rt.key(KeyEvent { key: Key::Command(c), mods: Mods { cmd: true, ..Mods::default() } });
    tick(rt);
}

/// Centres (points) of the glyphs drawn inside element `key`, in reading order by line.
pub fn glyphs<S: 'static>(rt: &Runtime<S>, key: &str) -> Vec<(f32, f32)> {
    glyphs_at(rt, &find(rt, key).path)
}

/// [`glyphs`] for the element at `path`.
pub fn glyphs_at<S: 'static>(rt: &Runtime<S>, path: &str) -> Vec<(f32, f32)> {
    let e = rt.elements().into_iter().find(|e| e.path == path).unwrap_or_else(|| panic!("{path}"));
    let mut out: Vec<(f32, f32)> = rt
        .instances
        .iter()
        .filter(|i| i.params[1] == rune::render::KIND_GLYPH)
        .map(|i| ((i.rect[0] + i.rect[2] / 2.0) / 2.0, (i.rect[1] + i.rect[3] / 2.0) / 2.0))
        .filter(|&(x, y)| x >= e.rect[0] && x <= e.rect[0] + e.rect[2] && y >= e.rect[1] && y <= e.rect[1] + e.rect[3])
        .collect();
    // Rows by ink centre, a new row when the centre jumps by more than a third of a line: a
    // tall glyph's centre sits a little higher than a short one's in every rasterizer.
    out.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut row = (0, f32::NEG_INFINITY);
    let mut rows: Vec<(usize, (f32, f32))> = out
        .into_iter()
        .map(|g| {
            if g.1 - row.1 > 6.0 {
                row = (row.0 + 1, g.1);
            }
            (row.0, g)
        })
        .collect();
    rows.sort_by(|a, b| (a.0, a.1.0).partial_cmp(&(b.0, b.1.0)).unwrap());
    rows.into_iter().map(|r| r.1).collect()
}
