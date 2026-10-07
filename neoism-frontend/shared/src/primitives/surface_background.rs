//! Native, bounded decorations behind surface foreground. The host brackets
//! each window's serial paint with begin_frame/finish_frame; no idle timer.
#[cfg(test)]
use neoism_lua::EffectOptions;
use neoism_lua::{BackgroundEffect, PluginSnapshot, StylePatch, StyleSheet};
use std::{cell::RefCell, sync::Arc};
use sugarloaf::Sugarloaf;

const SURFACE_BUDGET: usize = 384;
const FRAME_BUDGET: usize = 1536;

#[derive(Default)]
struct Frame {
    snapshot: Option<Arc<PluginSnapshot>>,
    occlusions: Vec<[f32; 4]>,
    remaining: usize,
    demand: bool,
    active: bool,
}
thread_local! { static FRAME: RefCell<Frame> = RefCell::new(Frame::default()); }

/// Pack is the lowest layer; resolved plugin snapshot already contains the
/// field-wise personal Lua overlay. Resolve each layer's dotted ancestors.
pub fn resolve_style(selector: &str, styles: &StyleSheet) -> StylePatch {
    let mut style = crate::primitives::look::active_look()
        .styles
        .resolve(selector);
    style.overlay(Some(&styles.resolve(selector)));
    style
}

pub fn begin_frame(snapshot: Arc<PluginSnapshot>, occlusions: Vec<[f32; 4]>) {
    FRAME.with(|f| {
        *f.borrow_mut() = Frame {
            snapshot: Some(snapshot),
            occlusions,
            remaining: FRAME_BUDGET,
            demand: false,
            active: true,
        }
    });
}

pub fn animation_demand() -> bool {
    FRAME.with(|f| f.borrow().demand)
}

pub fn finish_frame() -> bool {
    FRAME.with(|f| {
        let mut f = f.borrow_mut();
        let demand = f.demand;
        *f = Frame::default();
        demand
    })
}

fn frame_style(frame: &Frame, selector: &str) -> StylePatch {
    let mut patch = crate::primitives::look::active_look()
        .styles
        .resolve(selector);
    if let Some(snapshot) = &frame.snapshot {
        patch.overlay(Some(&snapshot.styles.resolve(selector)));
    }
    patch
}

pub fn style(selector: &str) -> StylePatch {
    FRAME.with(|f| frame_style(&f.borrow(), selector))
}

pub fn base_color(
    selector: &str,
    theme: &crate::primitives::IdeTheme,
    fallback: [f32; 4],
) -> [f32; 4] {
    crate::customization::color_f32(
        style(selector).background.as_deref(),
        theme,
        fallback,
    )
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Spark {
    rect: [f32; 4],
    alpha: f32,
}

fn random(index: u32, salt: u32) -> f32 {
    let mut n = index.wrapping_mul(0x9e3779b9).wrapping_add(salt);
    n = (n ^ (n >> 16)).wrapping_mul(0x85ebca6b);
    n = (n ^ (n >> 13)).wrapping_mul(0xc2b2ae35);
    ((n ^ (n >> 16)) >> 8) as f32 / 16_777_216.0
}

/// Convex rounded bounds contain all four corners of the expanded footprint.
/// Keep the renderer's actual (potentially overlarge) radius during resize.
fn contained(rect: [f32; 4], inner: [f32; 4], radius: f32, fringe: f32) -> bool {
    let [x, y, w, h] = inner;
    let r = radius.max(0.0);
    let [sx, sy, sw, sh] = rect;
    [sx - fringe, sx + sw + fringe].into_iter().all(|px| {
        [sy - fringe, sy + sh + fringe].into_iter().all(|py| {
            if px < x || px > x + w || py < y || py > y + h {
                return false;
            }
            let qx = (px - (x + w * 0.5)).abs() - w * 0.5 + r;
            let qy = (py - (y + h * 0.5)).abs() - h * 0.5 + r;
            qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r <= 0.0
        })
    })
}

fn intersects(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3]
}

fn visible(
    rect: [f32; 4],
    inner: [f32; 4],
    radius: f32,
    fringe: f32,
    occlusions: &[[f32; 4]],
) -> bool {
    if !contained(rect, inner, radius, fringe) {
        return false;
    }
    let [x, y, w, h] = rect;
    let support = [x - fringe, y - fringe, w + 2.0 * fringe, h + 2.0 * fringe];
    !occlusions.iter().any(|clip| intersects(support, *clip))
}

fn geometry(
    effect: &BackgroundEffect,
    inner: [f32; 4],
    radius: f32,
    s: f32,
    time: f32,
) -> Vec<Spark> {
    let [x, y, w, h] = inner;
    let v = effect.options();
    if v.validate().is_err()
        || v.density == 0.0
        || v.opacity == 0.0
        || v.color[3] == 0.0
        || !inner.iter().all(|v| v.is_finite())
        || w <= 0.0
        || h <= 0.0
        || !s.is_finite()
        || s <= 0.0
        || !time.is_finite()
        || !radius.is_finite()
    {
        return vec![];
    }
    let time = time * v.speed;
    let mut sparks = Vec::new();
    let mut push = |rect: [f32; 4], alpha: f32| {
        if contained(rect, inner, radius, s.max(1.0))
            && alpha > 0.001
            && sparks.len() < SURFACE_BUDGET
        {
            sparks.push(Spark {
                rect,
                alpha: alpha * v.opacity,
            });
        }
    };
    match effect {
        BackgroundEffect::Stars(_) => {
            let count = ((w / s * h / s / 1800.0 * v.density).ceil() as usize).min(96);
            for i in 0..count as u32 {
                let rand = |salt: u32| random(i, salt.wrapping_add(v.seed));
                let phase = rand(31) * std::f32::consts::TAU;
                let px = x
                    + (rand(7) + time * (0.35 + rand(19) * 0.3) * s / w).rem_euclid(1.0)
                        * w;
                let py = y + (rand(11) + time * 0.12 * s / h).rem_euclid(1.0) * h;
                let twinkle = 0.5 + 0.5 * (time * (0.35 + rand(23) * 0.25) + phase).sin();
                let edge = ((px - x).min(x + w - px).min(py - y).min(y + h - py)
                    / (8.0 * s))
                    .clamp(0.0, 1.0);
                let size = (0.55 + rand(41) * 0.45) * s;
                let alpha = (0.13 + 0.23 * twinkle) * edge;
                for (sw, sh, a) in
                    [(size * 3.5, size * 3.5, alpha * 0.08), (size, size, alpha)]
                {
                    push([px - sw * 0.5, py - sh * 0.5, sw, sh], a);
                }
                if i % 11 == 0 {
                    let glint = ((twinkle - 0.92) / 0.08).max(0.0) * alpha * 0.35;
                    push([px - 1.75 * s, py - 0.175 * s, 3.5 * s, 0.35 * s], glint);
                    push([px - 0.175 * s, py - 1.75 * s, 0.35 * s, 3.5 * s], glint);
                }
            }
        }
        BackgroundEffect::Scanlines(_) => {
            // Short independent dashes, not full-width quads: rounded bounds
            // and partial occlusions can reject individual footprints safely.
            let spacing = (16.0 / v.density.max(0.1)) * s;
            let rows = (h / spacing).ceil().min(64.0) as usize;
            let cols = (w / (48.0 * s)).ceil().min(32.0) as usize;
            for row in 0..rows {
                let py = y
                    + (row as f32 * spacing
                        + random(row as u32, v.seed) * s
                        + time * 2.0 * s)
                        .rem_euclid(h);
                for col in 0..cols {
                    let px = x + col as f32 * 48.0 * s + 3.0 * s;
                    let dash_w = (x + w - px - 3.0 * s).min(42.0 * s).max(0.0);
                    if dash_w > 0.0 {
                        push([px, py, dash_w, 0.5 * s], 0.035);
                    }
                }
            }
        }
    }
    sparks
}

fn plan(
    frame: &mut Frame,
    patch: &StylePatch,
    inner: [f32; 4],
    radius: f32,
    s: f32,
    fringe: f32,
    time: f32,
    occlusions: &[[f32; 4]],
) -> Vec<(Spark, [f32; 4])> {
    if !frame.active || frame.remaining == 0 || patch.visible == Some(false) {
        return vec![];
    }
    let Some(effects) = &patch.background_effects else {
        return vec![];
    };
    let mut output = Vec::new();
    for effect in effects.iter().take(4) {
        let v = effect.options();
        for spark in geometry(effect, inner, radius, s, time) {
            if output.len() == SURFACE_BUDGET || frame.remaining == 0 {
                break;
            }
            if !visible(spark.rect, inner, radius, fringe, occlusions)
                || !visible(spark.rect, inner, radius, fringe, &frame.occlusions)
            {
                continue;
            }
            let mut color = v.color;
            color[3] *= spark.alpha;
            output.push((spark, color));
            frame.remaining -= 1;
            frame.demand |= v.speed > 0.0;
        }
    }
    output
}

/// Caller supplies real geometry, scale, paint order and foreground occlusions.
/// Static effects (speed=0) paint once without acquiring animation demand.
pub fn render(
    sugarloaf: &mut Sugarloaf,
    selector: &str,
    inner: [f32; 4],
    radius: f32,
    s: f32,
    depth: f32,
    order: u8,
    occlusions: &[[f32; 4]],
) {
    FRAME.with(|frame| {
        let mut frame = frame.borrow_mut();
        if !frame.active || frame.remaining == 0 {
            return;
        }
        let patch = frame_style(&frame, selector);
        let time = crate::cursor_style::rainbow_now_seconds();
        let fringe = s.max(2.0 / sugarloaf.scale_factor().max(0.1));
        for (spark, color) in plan(
            &mut frame, &patch, inner, radius, s, fringe, time, occlusions,
        ) {
            let [x, y, w, h] = spark.rect;
            sugarloaf.rounded_rect(None, x, y, w, h, color, depth, w.min(h) * 0.5, order);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_sparse_animated_clipped_dpi() {
        let effect = BackgroundEffect::Stars(EffectOptions::default());
        let inner = [10.0, 20.0, 600.0, 100.0];
        let a = geometry(&effect, inner, 16.0, 1.0, 42.0);
        assert!(!a.is_empty());
        assert!(a.len() < 100);
        assert_eq!(a, geometry(&effect, inner, 16.0, 1.0, 42.0));
        assert_ne!(a, geometry(&effect, inner, 16.0, 1.0, 43.0));
        for dpi in [0.5_f32, 1.0, 1.25, 2.0, 3.0] {
            let fringe = 2.0 / dpi;
            assert!(a.iter().any(|p| visible(p.rect, inner, 16.0, fringe, &[])));
            assert!(!a
                .iter()
                .any(|p| visible(p.rect, inner, 16.0, fringe, &[inner])));
        }
        for s in [0.5, 1.0, 1.5, 2.0, 3.0] {
            for (w, h) in [(20.0, 20.0), (80.0, 44.0), (700.0, 180.0)] {
                let inner = [31.5, 72.25, w * s, h * s];
                for effect in [
                    &effect,
                    &BackgroundEffect::Scanlines(EffectOptions::default()),
                ] {
                    for p in geometry(effect, inner, 18.0 * s, s, 40.0) {
                        assert!(contained(p.rect, inner, 18.0 * s, s.max(1.0)));
                    }
                }
            }
        }
        assert!(geometry(&effect, [0.0; 4], 14.0, 1.0, 0.0).is_empty());
    }

    #[test]
    fn tiny_resize_matches_unclamped_renderer_radius() {
        let inner = [0.0, 0.0, 20.0, 20.0];
        let corner = [4.0, 4.0, 0.1, 0.1];
        assert!(contained(corner, inner, 10.0, 0.0));
        assert!(!contained(corner, inner, 18.0, 0.0));
        assert!(contained([9.0, 9.0, 1.0, 1.0], inner, 18.0, 1.0));
    }

    #[test]
    fn hidden_disabled_occluded_paused_and_budgets() {
        let inner = [0.0, 0.0, 1000.0, 500.0];
        let patch = StylePatch {
            background_effects: Some(vec![
                BackgroundEffect::Stars(Default::default());
                4
            ]),
            ..Default::default()
        };
        let new_frame = || Frame {
            active: true,
            remaining: FRAME_BUDGET,
            ..Default::default()
        };
        let draw = |frame: &mut Frame, style: &StylePatch, occlusions: &[[f32; 4]]| {
            plan(frame, style, inner, 16.0, 1.0, 2.0, 42.0, occlusions)
        };
        for style in [
            StylePatch::default(),
            StylePatch {
                visible: Some(false),
                ..patch.clone()
            },
            StylePatch {
                background_effects: Some(vec![]),
                ..Default::default()
            },
            StylePatch {
                background_effects: Some(vec![BackgroundEffect::Stars(EffectOptions {
                    density: 0.0,
                    ..Default::default()
                })]),
                ..Default::default()
            },
            StylePatch {
                background_effects: Some(vec![BackgroundEffect::Stars(EffectOptions {
                    opacity: 0.0,
                    ..Default::default()
                })]),
                ..Default::default()
            },
        ] {
            let mut f = new_frame();
            assert!(draw(&mut f, &style, &[]).is_empty());
            assert!(!f.demand);
        }
        let mut f = new_frame();
        assert!(draw(&mut f, &patch, &[inner]).is_empty());
        assert!(!f.demand);
        let paused = StylePatch {
            background_effects: Some(vec![BackgroundEffect::Stars(EffectOptions {
                speed: 0.0,
                ..Default::default()
            })]),
            ..Default::default()
        };
        assert!(!draw(&mut f, &paused, &[]).is_empty());
        assert!(!f.demand);
        let mut f = new_frame();
        let mut total = 0;
        for _ in 0..20 {
            let drawn = draw(&mut f, &patch, &[]);
            assert!(drawn.len() <= SURFACE_BUDGET);
            total += drawn.len();
        }
        assert!(f.demand);
        assert!(total <= FRAME_BUDGET);
        assert_eq!(f.remaining, 0);
        let mut hidden = Frame::default();
        assert!(draw(&mut hidden, &patch, &[]).is_empty());
        assert!(!hidden.demand);
    }

    #[test]
    fn frame_demand_never_leaks_between_windows() {
        begin_frame(Arc::new(PluginSnapshot::empty()), vec![]);
        FRAME.with(|f| f.borrow_mut().demand = true);
        assert!(finish_frame());
        assert!(!animation_demand());
        begin_frame(Arc::new(PluginSnapshot::empty()), vec![]);
        assert!(!finish_frame());
    }
}
