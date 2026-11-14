//! Two-edge 1/64 device-pixel snapping.
//!
//! Snap both edges, then derive the size. Snapping origin and size separately makes a stationary
//! edge flicker by one quantum as the other edge moves.

use crate::value::Rect;

/// Rounds `v` points to the 1/64 device-pixel grid at `scale` device pixels per point.
#[inline]
pub fn grid(v: f32, scale: f32) -> f32 {
    let q = 64.0 * scale;
    (v * q).round() / q
}

/// Snaps a rectangle's four edges to the 1/64 px grid and derives its size from them.
#[inline]
pub fn snap_rect(r: Rect, scale: f32) -> Rect {
    snap_edges(r.edges(), scale)
}

/// Snaps edges given directly (left, top, right, bottom).
#[inline]
pub fn snap_edges(e: [f32; 4], scale: f32) -> Rect {
    Rect::from_edges(grid(e[0], scale), grid(e[1], scale), grid(e[2], scale), grid(e[3], scale))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stationary_bottom_edge_never_moves() {
        // Top edge animates continuously; bottom stays at 200.3. Two-edge snapping keeps the bottom
        // exactly fixed, where snapping (y, h) separately would wobble it.
        let bottom = 200.3f32;
        let fixed = grid(bottom, 2.0);
        let mut separate = std::collections::HashSet::new();
        for i in 0..1000 {
            let top = 10.0 + i as f32 * 0.0137;
            let r = snap_rect(Rect::from_edges(0.0, top, 10.0, bottom), 2.0);
            assert_eq!(r.y + r.h, fixed);
            let (y, h) = (grid(top, 2.0), grid(bottom - top, 2.0));
            separate.insert((y + h).to_bits());
        }
        assert!(separate.len() > 1, "the naive way should wobble, proving the test bites");
    }
}
