//! CoreGraphics' curve flattening (`aa_create`'s tolerances): coordinates are cast to f32,
//! then Beziers are halved in f64 until their squared second differences are within the
//! tolerance on both axes, and each accepted piece's endpoint is emitted as a line.

use super::{Cmd, Path, Point, f32p, lerp, split};

const QUAD_FLATNESS: f64 = 0.6400000000000001;
const CUBIC_FLATNESS: f64 = 0.07111111111111111;
/// Subdivision stops beyond this depth.
const MAX_DEPTH: u32 = 22;

fn cubic(p: [Point; 4], depth: u32, out: &mut Path) {
    let flat = (0..2).all(|k| {
        let a = (p[0][k] + p[2][k]) - 2.0 * p[1][k];
        let b = (p[1][k] + p[3][k]) - 2.0 * p[2][k];
        a * a <= CUBIC_FLATNESS && b * b <= CUBIC_FLATNESS
    });
    if flat || depth > MAX_DEPTH {
        out.push(Cmd::Line(p[3]));
    } else {
        let (a, b) = split(p, 0.5);
        cubic(a, depth + 1, out);
        cubic(b, depth + 1, out);
    }
}

fn quad(p: [Point; 3], depth: u32, out: &mut Path) {
    let flat = (0..2).all(|k| {
        let a = p[0][k] + p[2][k] - 2.0 * p[1][k];
        a * a <= QUAD_FLATNESS
    });
    if flat || depth > MAX_DEPTH {
        out.push(Cmd::Line(p[2]));
    } else {
        let a = lerp(p[0], p[1], 0.5);
        let b = lerp(p[1], p[2], 0.5);
        let m = lerp(a, b, 0.5);
        quad([p[0], a, m], depth + 1, out);
        quad([m, b, p[2]], depth + 1, out);
    }
}

/// `path` as moves, lines and closes only.
pub(crate) fn flatten(path: &[Cmd]) -> Path {
    let mut out = Vec::with_capacity(path.len() * 4);
    let mut cur = [0.0; 2];
    let mut first = cur;
    for c in path {
        match *c {
            Cmd::Move(p) => {
                cur = f32p(p);
                first = cur;
                out.push(Cmd::Move(cur));
            }
            Cmd::Line(p) => {
                cur = f32p(p);
                out.push(Cmd::Line(cur));
            }
            Cmd::Cubic(a, b, p) => {
                cubic([cur, f32p(a), f32p(b), f32p(p)], 0, &mut out);
                cur = f32p(p);
            }
            Cmd::Quad(a, p) => {
                quad([cur, f32p(a), f32p(p)], 0, &mut out);
                cur = f32p(p);
            }
            Cmd::Close => {
                out.push(Cmd::Close);
                cur = first;
            }
        }
    }
    out
}
