//! Paint-only helpers; neither motion nor reveal clocks originate in the view.
use super::*;
use crate::panels::agent_pane::text_reveal::{PaintScope, Projection};

pub(super) struct PaintTheme<'a> {
    pub theme: &'a IdeTheme,
    pub opacity: f32,
}
impl std::ops::Deref for PaintTheme<'_> {
    type Target = IdeTheme;
    fn deref(&self) -> &IdeTheme {
        self.theme
    }
}
impl PaintTheme<'_> {
    pub fn u8(&self, color: u32) -> [u8; 4] {
        self.u8_alpha(color, 1.0)
    }
    pub fn u8_alpha(&self, color: u32, alpha: f32) -> [u8; 4] {
        self.theme.u8_alpha(color, alpha * self.opacity)
    }
    pub fn f32(&self, color: u32) -> [f32; 4] {
        self.f32_alpha(color, 1.0)
    }
    pub fn f32_alpha(&self, color: u32, alpha: f32) -> [f32; 4] {
        self.theme.f32_alpha(color, alpha * self.opacity)
    }
}

pub(super) fn projection(rows: Rc<Vec<ToolWrappedRow>>) -> Rc<Projection> {
    let mut p = Projection::default();
    for row in rows.iter() {
        p.push(&row.text, &row.text);
    }
    p.retain_layout(rows);
    Rc::new(p)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn prepare_body_reveal(
    sugarloaf: &mut Sugarloaf,
    pane: &mut impl AgentToolPane,
    id: &str,
    rows: Rc<Vec<ToolWrappedRow>>,
    width: f32,
    opts: &DrawOpts,
    limit: usize,
    preview: bool,
) -> Option<PaintScope> {
    let state = pane.tool_text_reveal_state()?;
    if !state.has_body(id) {
        return None;
    }
    let identity = Rc::as_ptr(&rows) as usize;
    let current = state
        .cached_projection(id, identity)
        .unwrap_or_else(|| projection(rows));
    // Ingestion coalesces updates to one sharp baseline and the latest source.
    // Only that first baseline needs wrapping; current rows already own layout.
    for update in state.pending(id) {
        let p = if update.at.is_none() {
            let wrap = if preview {
                tool_preview_wrapped_rows
            } else {
                tool_wrapped_rows
            };
            projection(wrap(sugarloaf, &update.text, width, opts, limit))
        } else {
            current.clone()
        };
        state.apply_projection(id, &p, update.at);
    }
    Some(state.paint(id, identity, current))
}

/// Subtract overlays rather than considering a partly covered rectangle invisible.
pub(super) fn visible_ink(rect: [f32; 4], clip: [f32; 4], cuts: &[[f32; 4]]) -> bool {
    let Some(rect) = intersect_rect(rect, clip) else {
        return false;
    };
    let mut regions = vec![rect];
    for cut in cuts {
        let mut next = Vec::new();
        for r in regions {
            if let Some(i) = intersect_rect(r, *cut) {
                for part in [
                    [r[0], r[1], r[2], i[1] - r[1]],
                    [r[0], i[1] + i[3], r[2], r[1] + r[3] - i[1] - i[3]],
                    [r[0], i[1], i[0] - r[0], i[3]],
                    [i[0] + i[2], i[1], r[0] + r[2] - i[0] - i[2], i[3]],
                ] {
                    if part[2] > 0.0 && part[3] > 0.0 {
                        next.push(part);
                    }
                }
            } else {
                next.push(r);
            }
        }
        regions = next;
        if regions.is_empty() {
            return false;
        }
    }
    !regions.is_empty()
}

/// A transparent first frame still owns its fade's next frame. Geometry, not
/// current alpha, decides whether that future ink can be seen.
pub(super) fn active_visible(
    sample: crate::panels::agent_pane::tool_motion::ToolMotionSample,
    rect: [f32; 4],
    clip: [f32; 4],
    cuts: &[[f32; 4]],
) -> bool {
    sample
        .deadline
        .is_some_and(|end| end > web_time::Instant::now())
        && visible_ink(rect, clip, cuts)
}

/// Centered icons can have substantial empty padding in their layout slot.
/// Use emitted glyph bounds (even transparent glyphs), not that slot, to avoid
/// owning a deadline when only blank header space is uncovered.
pub(super) fn active_glyphs(
    sample: crate::panels::agent_pane::tool_motion::ToolMotionSample,
    sugarloaf: &mut Sugarloaf,
    first: usize,
    clip: [f32; 4],
    cuts: &[[f32; 4]],
) -> bool {
    if !sample
        .deadline
        .is_some_and(|end| end > web_time::Instant::now())
    {
        return false;
    }
    let scale = sugarloaf.scale_factor().max(f32::EPSILON);
    sugarloaf.text_mut().instances()[first..]
        .iter()
        .any(|glyph| active_glyph_ink(sample, glyph, scale, clip, cuts))
}

fn active_glyph_ink(
    sample: crate::panels::agent_pane::tool_motion::ToolMotionSample,
    glyph: &sugarloaf::text::TextInstance,
    scale: f32,
    clip: [f32; 4],
    cuts: &[[f32; 4]],
) -> bool {
    let raster_scale = if glyph.raster_scale > 0.0 {
        glyph.raster_scale
    } else {
        1.0
    };
    let rect = [
        (glyph.pos[0] + glyph.bearings[0] as f32 * raster_scale) / scale,
        (glyph.pos[1] + glyph.bearings[1] as f32 * raster_scale) / scale,
        glyph.glyph_size[0] as f32 * raster_scale / scale,
        glyph.glyph_size[1] as f32 * raster_scale / scale,
    ];
    let glyph_clip = if glyph.clip_rect[2] > 0.0 {
        intersect_rect(clip, glyph.clip_rect.map(|v| v / scale))
    } else {
        Some(clip)
    };
    glyph_clip.is_some_and(|clip| active_visible(sample, rect, clip, cuts))
}

pub(super) fn status_blend(has_outgoing: bool, progress: f32) -> (f32, f32) {
    let incoming = if has_outgoing {
        progress.clamp(0.0, 1.0)
    } else {
        1.0
    };
    (incoming, 1.0 - incoming)
}

pub(super) fn status_width(incoming: f32, outgoing: f32, progress: f32) -> f32 {
    outgoing + (incoming - outgoing) * progress.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zero_opacity_future_ink_owns_the_next_frame_only_when_visible_and_unexpired() {
        use crate::panels::agent_pane::tool_motion::{ToolMotionSample, ToolMotionState};
        use std::time::Duration;
        use web_time::Instant;
        let sample = ToolMotionSample {
            opacity: 0.0,
            deadline: Some(Instant::now() + Duration::from_secs(1)),
            ..ToolMotionSample::default()
        };
        let clip = [0.0, 0.0, 100.0, 30.0];
        let ink = [5.0, 5.0, 10.0, 10.0];
        let mut state = ToolMotionState::default();
        if active_visible(sample, ink, clip, &[]) {
            state.mark_visible(sample);
        }
        assert!(state.is_animating_for(None));
        state.begin_frame();
        for (rect, cuts) in [([5.0, 31.0, 10.0, 10.0], &[][..]), (ink, &[clip][..])] {
            if active_visible(sample, rect, clip, cuts) {
                state.mark_visible(sample);
            }
            assert!(!state.is_animating_for(None));
        }
        let expired = ToolMotionSample {
            deadline: Some(Instant::now() - Duration::from_secs(1)),
            ..sample
        };
        assert!(!active_visible(expired, ink, clip, &[]));
        assert!(!active_visible(ToolMotionSample::default(), ink, clip, &[]));
        assert!(!active_visible(sample, [5.0, 5.0, 0.0, 10.0], clip, &[]));
    }
    #[test]
    fn transparent_icon_ink_owns_an_empty_title_header_but_blank_padding_does_not() {
        use crate::panels::agent_pane::tool_motion::ToolMotionSample;
        let sample = ToolMotionSample {
            opacity: 0.0,
            deadline: Some(web_time::Instant::now() + std::time::Duration::from_secs(1)),
            ..ToolMotionSample::default()
        };
        let glyph = sugarloaf::text::TextInstance {
            pos: [40.0, 0.0],
            bearings: [6, 8],
            glyph_size: [16, 20],
            color: [255, 255, 255, 0],
            ..Default::default()
        };
        let clip = [0.0, 0.0, 100.0, 30.0];
        assert!(active_glyph_ink(sample, &glyph, 2.0, clip, &[]));
        // The actual icon spans x=23..31, y=4..14. Cover only that ink,
        // leaving most of its header slot blank and unobscured.
        assert!(!active_glyph_ink(
            sample,
            &glyph,
            2.0,
            clip,
            &[[23.0, 4.0, 8.0, 10.0]]
        ));
        assert!(!active_glyph_ink(
            sample,
            &glyph,
            2.0,
            [0.0, 0.0, 20.0, 30.0],
            &[]
        ));
        assert!(!active_glyph_ink(
            sample,
            &sugarloaf::text::TextInstance::default(),
            2.0,
            clip,
            &[]
        ));
    }
    #[test]
    fn completion_blends_outgoing_orbit_and_incoming_dot_without_restarting() {
        assert_eq!(status_blend(false, 0.0), (1.0, 0.0));
        assert_eq!(status_blend(true, 0.0), (0.0, 1.0));
        assert_eq!(status_blend(true, 0.5), (0.5, 0.5));
        assert_eq!(status_blend(true, 1.0), (1.0, 0.0));
        for progress in [0.1, 0.3, 0.8, 0.9999] {
            let (incoming, outgoing) = status_blend(true, progress);
            assert_eq!(incoming + outgoing, 1.0);
            assert_eq!(status_blend(true, progress), (incoming, outgoing));
        }
    }
    #[test]
    fn completion_width_shrinks_without_a_final_jump() {
        assert_eq!(status_width(0.0, 60.0, 0.0), 60.0);
        assert_eq!(status_width(0.0, 60.0, 0.5), 30.0);
        assert_eq!(status_width(0.0, 60.0, 1.0), 0.0);
        assert!((status_width(0.0, 60.0, 0.9999) - 0.0).abs() < 0.01);
    }
    #[test]
    fn partial_rows_visible_but_overscan_and_occlusion_do_not_own_motion() {
        let clip = [0.0, 0.0, 100.0, 30.0];
        assert!(visible_ink([5.0, 28.0, 40.0, 18.0], clip, &[]));
        assert!(!visible_ink([5.0, 31.0, 40.0, 18.0], clip, &[]));
        assert!(!visible_ink([5.0, 5.0, 40.0, 18.0], clip, &[clip]));
        assert!(!visible_ink(
            clip,
            clip,
            &[[0.0, 0.0, 50.0, 30.0], [50.0, 0.0, 50.0, 30.0]]
        ));
        assert!(visible_ink(clip, clip, &[[0.0, 0.0, 50.0, 30.0]]));
    }
    #[test]
    fn projection_reflow_keeps_canonical_identity_and_owns_rows() {
        let rows = Rc::new(vec![ToolWrappedRow {
            text: "first second".into(),
            nested: false,
        }]);
        let p = projection(rows.clone());
        let wrapped = projection(Rc::new(vec![
            ToolWrappedRow {
                text: "first".into(),
                nested: false,
            },
            ToolWrappedRow {
                text: "second".into(),
                nested: false,
            },
        ]));
        assert_eq!(p.canonical, wrapped.canonical);
        assert_eq!(p.lines[&(rows[0].text.as_ptr() as usize)], 0);
        assert_eq!(Rc::strong_count(&rows), 2);
    }
}
