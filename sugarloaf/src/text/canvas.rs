//! CPU counterpart of scaled UI glyph rendering. Ordinary text keeps its fast
//! unscaled blit path; canvas text samples the same atlas with bilinear filtering.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn draw_scaled_cpu(
    glyph: &super::TextInstance,
    atlas: &[u8],
    side: usize,
    buffer: &mut [u32],
    width: i32,
    height: i32,
) {
    let scale = glyph.raster_scale;
    if !scale.is_finite() || scale <= 0.0 || side == 0 {
        return;
    }
    let left = glyph.pos[0] + f32::from(glyph.bearings[0]) * scale;
    let top = glyph.pos[1] + f32::from(glyph.bearings[1]) * scale;
    let right = left + glyph.glyph_size[0] as f32 * scale;
    let bottom = top + glyph.glyph_size[1] as f32 * scale;
    let (cx0, cy0, cx1, cy1) = super::cpu_clip_bounds(glyph.clip_rect, width, height);
    let x0 = (left.floor() as i32).max(cx0).max(0);
    let x1 = (right.ceil() as i32).min(cx1).min(width);
    let y0 = (top.floor() as i32).max(cy0).max(0);
    let y1 = (bottom.ceil() as i32).min(cy1).min(height);
    let channels = if glyph.atlas == 1 { 4 } else { 1 };
    let sample = |x: i32, y: i32, channel: usize| -> f32 {
        let x = x.clamp(0, side as i32 - 1) as usize;
        let y = y.clamp(0, side as i32 - 1) as usize;
        atlas
            .get((y * side + x) * channels + channel)
            .copied()
            .unwrap_or(0) as f32
    };
    for y in y0..y1 {
        for x in x0..x1 {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            if px < left || px >= right || py < top || py >= bottom {
                continue;
            }
            let clip = glyph.clip_rect;
            if clip[2] > 0.0
                && clip[3] > 0.0
                && (px < clip[0]
                    || px >= clip[0] + clip[2]
                    || py < clip[1]
                    || py >= clip[1] + clip[3])
            {
                continue;
            }
            let sx = (x as f32 + 0.5 - left) / scale + glyph.glyph_pos[0] as f32 - 0.5;
            let sy = (y as f32 + 0.5 - top) / scale + glyph.glyph_pos[1] as f32 - 0.5;
            let bx = sx.floor() as i32;
            let by = sy.floor() as i32;
            let fx = sx - sx.floor();
            let fy = sy - sy.floor();
            let channel = |c| {
                let a = sample(bx, by, c) * (1.0 - fx) + sample(bx + 1, by, c) * fx;
                let b =
                    sample(bx, by + 1, c) * (1.0 - fx) + sample(bx + 1, by + 1, c) * fx;
                (a * (1.0 - fy) + b * fy).round().clamp(0.0, 255.0) as u8
            };
            let rgba = if channels == 4 {
                [channel(0), channel(1), channel(2), channel(3)]
            } else {
                let alpha =
                    (u32::from(channel(0)) * u32::from(glyph.color[3]) + 127) / 255;
                [
                    ((u32::from(glyph.color[0]) * alpha + 127) / 255) as u8,
                    ((u32::from(glyph.color[1]) * alpha + 127) / 255) as u8,
                    ((u32::from(glyph.color[2]) * alpha + 127) / 255) as u8,
                    alpha as u8,
                ]
            };
            let index = y as usize * width as usize + x as usize;
            if let Some(pixel) = buffer.get_mut(index) {
                *pixel = super::blend_premul_over(rgba, *pixel);
            }
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    #[test]
    fn scaling_keeps_glyph_bounds_and_clipping_in_screen_space() {
        let glyph = super::super::TextInstance {
            pos: [2.0, 2.0],
            glyph_size: [2, 2],
            raster_scale: 2.0,
            color: [255; 4],
            clip_rect: [3.0, 2.0, 2.0, 4.0],
            ..Default::default()
        };
        let mut pixels = vec![0; 64];
        super::draw_scaled_cpu(&glyph, &[255; 4], 2, &mut pixels, 8, 8);
        for y in 0..8 {
            for x in 0..8 {
                assert_eq!(
                    pixels[y * 8 + x] != 0,
                    (3..5).contains(&x) && (2..6).contains(&y)
                );
            }
        }
    }
}
