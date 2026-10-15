use web_time::Instant;

use sugarloaf::{ColorType, GraphicData, GraphicDataEntry, GraphicId, Sugarloaf};

use crate::panels::agent_pane::state::NeoismWordmarkState;
use crate::primitives::wordmark_animation::WordmarkAnimation;

use super::draw::push_image_overlay_clipped;
use super::{
    DEPTH, LETTER_COUNT, LETTER_HOVER_LIFT, LETTER_HOVER_RATE, LETTER_HOVER_SCALE,
    LETTER_SHIMMER_AMP, LETTER_SHIMMER_PERIOD, ORDER_CARET, OVERLAY_PANEL_ID,
    WORDMARK_IMAGE_ID, WORDMARK_PNG,
};

pub trait WordmarkState {
    fn set_rect(&mut self, rect: [f32; 4]);
    fn frame_delta_seconds(&mut self) -> f32;
    fn animation_mut(&mut self) -> &mut WordmarkAnimation;
    fn hover_mut(&mut self) -> &mut [f32; LETTER_COUNT];
}

#[macro_export]
macro_rules! neoism_ui_impl_wordmark_state {
    ($state:ty, $now:path) => {
        impl $crate::panels::agent_pane::view::wordmark::WordmarkState for $state {
            fn set_rect(&mut self, rect: [f32; 4]) {
                self.rect = Some(rect);
            }

            fn frame_delta_seconds(&mut self) -> f32 {
                let now = $now();
                let dt = self
                    .last_frame_at
                    .map(|previous| {
                        now.saturating_duration_since(previous)
                            .as_secs_f32()
                            .clamp(0.0, 0.10)
                    })
                    .unwrap_or(0.0);
                self.last_frame_at = Some(now);
                dt
            }

            fn animation_mut(
                &mut self,
            ) -> &mut $crate::primitives::wordmark_animation::WordmarkAnimation {
                &mut self.animation
            }

            fn hover_mut(&mut self) -> &mut [f32; 6] {
                &mut self.hover
            }
        }
    };
}

neoism_ui_impl_wordmark_state!(NeoismWordmarkState, Instant::now);

/// Tiny in-file HSL-to-RGBA8 helper used by agent animations.
pub fn hsl_to_u8_simple(h: f32, s: f32, l: f32) -> [u8; 4] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let h_prime = h.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - ((h_prime % 2.0) - 1.0).abs());
    let (r1, g1, b1) = match h_prime as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    [
        ((r1 + m) * 255.0).clamp(0.0, 255.0) as u8,
        ((g1 + m) * 255.0).clamp(0.0, 255.0) as u8,
        ((b1 + m) * 255.0).clamp(0.0, 255.0) as u8,
        255,
    ]
}

pub fn format_elapsed(seconds: f32) -> String {
    let seconds = seconds.max(0.0);
    if seconds < 60.0 {
        format!("{seconds:.1}s")
    } else if seconds < 3600.0 {
        let minutes = (seconds / 60.0).floor() as u32;
        let rem = (seconds - minutes as f32 * 60.0).max(0.0);
        format!("{minutes}m {rem:.0}s")
    } else {
        let hours = (seconds / 3600.0).floor() as u32;
        let minutes = ((seconds - hours as f32 * 3600.0) / 60.0).floor() as u32;
        format!("{hours}h {minutes}m")
    }
}

#[cfg(test)]
mod tests {
    use super::format_elapsed;

    #[test]
    fn elapsed_formats_hours_instead_of_unbounded_minutes() {
        assert_eq!(format_elapsed(3_600.0), "1h 0m");
        assert_eq!(format_elapsed(5_460.0), "1h 31m");
    }
}

/// Letter tint cycle the wordmark pixels were last uploaded with
/// (empty = never). The source PNG is a white glyph; tinting to the
/// pack's `[wordmark] colors` (or the active theme's `fg`) keeps the
/// agent home legible on light themes, as on the terminal splash.
/// The per-window `image_data.contains_key` check below still forces
/// the first upload into each window.
static WORDMARK_TINT: std::sync::RwLock<Vec<u32>> = std::sync::RwLock::new(Vec::new());

pub fn register_wordmark(sugarloaf: &mut Sugarloaf) -> bool {
    let tint =
        crate::primitives::look::wordmark_colors_or(crate::chrome::active_ide_theme().fg);
    let unchanged = WORDMARK_TINT
        .read()
        .map(|last| *last == tint)
        .unwrap_or(false);
    if unchanged && sugarloaf.image_data.contains_key(&WORDMARK_IMAGE_ID) {
        return true;
    }
    let img = match image_rs::load_from_memory(WORDMARK_PNG) {
        Ok(i) => i.to_rgba8(),
        Err(_) => return false,
    };
    let (width, height) = img.dimensions();
    let mut pixels = img.into_raw();
    crate::primitives::look::tint_wordmark_pixels(
        &mut pixels,
        width as usize,
        LETTER_COUNT,
        &tint,
    );
    let entry = GraphicDataEntry::from_graphic_data(GraphicData {
        id: GraphicId::new(WORDMARK_IMAGE_ID as u64),
        width: width as usize,
        height: height as usize,
        color_type: ColorType::Rgba,
        pixels,
        is_opaque: false,
        resize: None,
        display_width: None,
        display_height: None,
        transmit_time: Instant::now(),
    });
    sugarloaf.image_data.insert(WORDMARK_IMAGE_ID, entry);
    if let Ok(mut last) = WORDMARK_TINT.write() {
        *last = tint;
    }
    true
}

#[allow(clippy::too_many_arguments)]
pub fn render_wordmark(
    sugarloaf: &mut Sugarloaf,
    state: &mut impl WordmarkState,
    rect: [f32; 4],
    clip: [f32; 4],
    now_seconds: f32,
    mouse: Option<(f32, f32)>,
    z_index: i32,
    occlusion_rects: &[[f32; 4]],
) {
    if !register_wordmark(sugarloaf) {
        return;
    }
    let [x, y, w, h] = rect;
    state.set_rect(rect);
    let dt = state.frame_delta_seconds();
    let animation = state.animation_mut().frame();
    let tint =
        crate::primitives::look::wordmark_colors_or(crate::chrome::active_ide_theme().fg);
    let letter_w = w / LETTER_COUNT as f32;
    for i in 0..LETTER_COUNT {
        let lx = x + i as f32 * letter_w;
        let target = mouse
            .map(|(mx, my)| mx >= lx && mx <= lx + letter_w && my >= y && my <= y + h)
            .unwrap_or(false) as u8 as f32;
        let alpha = 1.0 - (-LETTER_HOVER_RATE * dt).exp();
        let hover = state.hover_mut();
        hover[i] += (target - hover[i]) * alpha;
    }

    let scale = sugarloaf.scale_factor();
    for i in 0..LETTER_COUNT {
        let hover = state.hover_mut()[i].clamp(0.0, 1.0);
        let shimmer_phase = (now_seconds / LETTER_SHIMMER_PERIOD + i as f32 * 0.16)
            * std::f32::consts::TAU;
        let shimmer = shimmer_phase.sin() * LETTER_SHIMMER_AMP;
        let extra_scale = 1.0 + hover * LETTER_HOVER_SCALE + shimmer;
        let lift = -hover * LETTER_HOVER_LIFT * h;
        let center_x = x + (i as f32 + 0.5) * letter_w;
        let center_y = y + h * 0.5 + lift;
        let lw = letter_w * extra_scale;
        let lh = h * extra_scale;
        let lx = center_x - lw * 0.5;
        let ly = center_y - lh * 0.5;
        let u0 = i as f32 / LETTER_COUNT as f32;
        let u1 = (i as f32 + 1.0) / LETTER_COUNT as f32;

        if animation.as_ref().is_none_or(|frame| frame.show_original()) {
            let glow = 1.04 + hover * 0.05;
            push_image_overlay_clipped(
                sugarloaf,
                OVERLAY_PANEL_ID,
                WORDMARK_IMAGE_ID,
                [
                    center_x - lw * glow * 0.5,
                    center_y - lh * glow * 0.5,
                    lw * glow,
                    lh * glow,
                ],
                [u0, 0.0, u1, 1.0],
                z_index - 1,
                scale,
                occlusion_rects,
            );
            push_image_overlay_clipped(
                sugarloaf,
                OVERLAY_PANEL_ID,
                WORDMARK_IMAGE_ID,
                [lx, ly, lw, lh],
                [u0, 0.0, u1, 1.0],
                z_index,
                scale,
                occlusion_rects,
            );
        }
        if let Some(frame) = animation.as_ref() {
            frame.draw_letter(
                sugarloaf,
                i,
                [lx, ly, lw, lh],
                crate::chrome::active_ide_theme().f32_alpha(tint[i % tint.len()], 1.0),
                clip,
                occlusion_rects,
                DEPTH,
                ORDER_CARET,
            );
        }
    }
}
