//! Wordmark timing and glyph outlines for the reusable UI pixel-fire effect.
use super::animation::{AnimationFrame, MASKS, map_point};
use crate::ui::fire::PixelFire;
use crate::ui::theme::rgb;
use rune::Shape;
use std::sync::OnceLock;

pub fn draw(age: f32, letters: &[([f32; 4], f32)], canvas: &mut Shape, offset: [f32; 2], colors: [u32; 5], frame: Option<AnimationFrame>) {
    if !age.is_finite() || age < 0.0 { return; }
    static FIRE: OnceLock<[PixelFire; 6]> = OnceLock::new();
    let flames = FIRE.get_or_init(|| std::array::from_fn(|letter| {
        let profile: [Option<usize>; 14] = std::array::from_fn(|column| {
            MASKS[letter].iter().position(|row| row & (1 << (6 - column / 2)) != 0).map(|row| row * 2)
        });
        PixelFire::new(&profile, letter as u32)
    }));
    for (letter, &(rect, _)) in letters.iter().take(6).enumerate() {
        if let Some(frame) = frame.filter(|f| !f.show_original()) {
            flames[letter].cells(age, colors, |x, y, color| {
                let column = x as usize / 2;
                let Some(row) = MASKS[letter].iter().position(|mask| mask & (1 << (6 - column)) != 0) else { return };
                let (left, top) = (x * 15.625, -250.0 + y * 15.625);
                let corners = [[left - 0.6, top - 0.6], [left + 16.225, top - 0.6],
                    [left + 16.225, top + 16.225], [left - 0.6, top + 16.225]];
                if let Some((quad, alpha)) = frame.attached_quad(letter, row, column, corners, rect) {
                    let points: Vec<_> = quad.map(|p| [p[0] - offset[0], p[1] - offset[1]]).into();
                    *canvas = std::mem::take(canvas).fill(points, rgb(color).alpha(alpha));
                }
            });
            continue;
        }
        let origin = map_point([0.0, -250.0], letter, [0.0, 0.0, 1.0], rect);
        let end = map_point([15.625, -234.375], letter, [0.0, 0.0, 1.0], rect);
        flames[letter].paint(canvas, [origin[0] - offset[0], origin[1] - offset[1]],
            [end[0] - origin[0], end[1] - origin[1]], age, 1.0, colors);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flame_layer_keeps_drawing_without_an_active_click_animation() {
        let letters = [([0.0, 0.0, 100.0, 96.0], 0.0); 6];
        for seconds in [0.0, 30.0, 3600.0] {
            let mut shape = Shape::new();
            draw(seconds, &letters, &mut shape, [0.0, 0.0], crate::ui::fire::CLASSIC, None);
            assert_ne!(shape, Shape::new(), "flames disappeared at {seconds}s");
        }
    }
}