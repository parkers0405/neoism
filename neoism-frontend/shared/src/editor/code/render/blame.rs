//! Zed-style CURRENT-LINE inline blame, not a gutter. Seven ems of trailing
//! padding; muted author + relative time, avatar replacing Zed's FileGit icon.
//! No layout reservation, syntax changes, or network/image decoding in paint.
use super::*;
use std::hash::{Hash, Hasher};
use sugarloaf::{ColorType, GraphicData, GraphicDataEntry, GraphicId};

pub(super) const OVERLAY_ID: usize = 0xAB10_0000;

pub(super) struct InlineLayout {
    pub rect: [f32; 4],
    pub label: String,
    pub avatar_size: f32,
    pub gap: f32,
}

/// Pure layout pass before any code glyphs are emitted, so a hover detail
/// rectangle can participate in the editor's ordinary text occlusion list.
pub(super) fn layout(
    sugarloaf: &mut Sugarloaf,
    pane: &CodePane,
    line: usize,
    rect: [f32; 4],
    opts: &DrawOpts,
) -> Option<InlineLayout> {
    let [x, y, w, h] = rect;
    let scale = opts.font_size / 14.0;
    let gap = 8.0 * scale;
    let avatar_size = (16.0 * scale).min(h - 2.0);
    if w < avatar_size + gap + 6.0 * opts.font_size {
        return None;
    }
    let label = if let Some(commit) = pane.blame.commit(line, pane.buffer.revision) {
        let now = web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        format!(
            "{}, {}",
            clean(&commit.author, 100),
            crate::editor::code::blame::relative_date(commit.timestamp, now)
        )
    } else {
        pane.blame.empty_label(pane.buffer.revision).to_owned()
    };
    let label = elide(sugarloaf, &label, opts, w - avatar_size - gap);
    if label.is_empty() {
        return None;
    }
    let width = (avatar_size + gap + sugarloaf.text_mut().measure(&label, opts)).min(w);
    Some(InlineLayout {
        rect: [x, y, width, h],
        label,
        avatar_size,
        gap,
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn inline(
    sugarloaf: &mut Sugarloaf,
    pane: &CodePane,
    line: usize,
    layout: &InlineLayout,
    ty: f32,
    opts: &DrawOpts,
    theme: &IdeTheme,
    occlusions: &[[f32; 4]],
) {
    let [x, y, _, h] = layout.rect;
    let Some(clip) =
        intersect_cursor_rect(layout.rect, opts.clip_rect.unwrap_or(layout.rect))
    else {
        return;
    };
    let dim = DrawOpts {
        color: theme.u8_alpha(theme.dim, 0.85),
        clip_rect: Some(clip),
        ..*opts
    };
    let avatar_size = layout.avatar_size;
    let image = pane
        .blame
        .commit(line, pane.buffer.revision)
        .and_then(|commit| commit.avatar)
        .and_then(|index| pane.blame.avatars.get(index as usize))
        .and_then(|pixels| register(sugarloaf, pixels));
    let icon_rect = [x, y + (h - avatar_size) * 0.5, avatar_size, avatar_size];
    if let Some(id) = image {
        paint_avatar(sugarloaf, id, icon_rect, clip, occlusions);
    } else {
        draw_icon_centered_with_occlusion(
            sugarloaf,
            x,
            icon_rect,
            "\u{f02a2}",
            &dim,
            occlusions,
            true,
        );
    }
    draw_text(
        sugarloaf,
        x + avatar_size + layout.gap,
        ty,
        &layout.label,
        &dim,
        occlusions,
    );
}

pub(super) fn clean(text: &str, max_chars: usize) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(max_chars)
        .collect()
}

fn elide(sugarloaf: &mut Sugarloaf, text: &str, opts: &DrawOpts, width: f32) -> String {
    if sugarloaf.text_mut().measure(text, opts) <= width {
        return text.into();
    }
    let mut output = text.to_owned();
    while !output.is_empty() {
        output.pop();
        let candidate = format!("{output}…");
        if sugarloaf.text_mut().measure(&candidate, opts) <= width {
            return candidate;
        }
    }
    String::new()
}

pub(super) struct TooltipLayout {
    pub rect: [f32; 4],
    lines: [String; 4],
}

/// Details appear on hover only; layout precedes code paint for occlusion.
pub(super) fn tooltip_layout(
    sugarloaf: &mut Sugarloaf,
    pane: &CodePane,
    anchor: [f32; 4],
    viewport: [f32; 4],
    opts: &DrawOpts,
    occlusions: &[[f32; 4]],
) -> Option<TooltipLayout> {
    let commit = pane
        .blame
        .commit(pane.buffer.cursor_line, pane.buffer.revision)?;
    let lines = [
        format!(
            "{} <{}>",
            clean(&commit.author, 100),
            clean(&commit.email, 150)
        ),
        clean(&commit.summary, 300),
        commit.sha.clone(),
        crate::editor::code::blame::absolute_date(commit.timestamp),
    ];
    let padding = 10.0;
    let row_h = opts.font_size * 1.5;
    let width = lines
        .iter()
        .map(|line| sugarloaf.text_mut().measure(line, opts))
        .fold(0.0, f32::max)
        .min(560.0)
        .min((viewport[2] - padding * 2.0).max(0.0))
        + padding * 2.0;
    let height = 4.0 * row_h + padding * 2.0;
    if width <= 20.0 || height > viewport[3] {
        return None;
    }
    let x = anchor[0]
        .min(viewport[0] + viewport[2] - width)
        .max(viewport[0]);
    let y = if anchor[1] + anchor[3] + height <= viewport[1] + viewport[3] {
        anchor[1] + anchor[3]
    } else {
        (anchor[1] - height).max(viewport[1])
    };
    let rect = [x, y, width, height];
    if occlusions
        .iter()
        .any(|&o| intersect_cursor_rect(rect, o).is_some())
    {
        return None;
    }
    Some(TooltipLayout { rect, lines })
}

pub(super) fn tooltip(
    sugarloaf: &mut Sugarloaf,
    layout: &TooltipLayout,
    opts: &DrawOpts,
    theme: &IdeTheme,
    occlusions: &[[f32; 4]],
) {
    let [x, y, width, height] = layout.rect;
    let padding = 10.0;
    let row_h = opts.font_size * 1.5;
    sugarloaf.rect(
        None,
        x,
        y,
        width,
        height,
        theme.f32(theme.border),
        DEPTH,
        ORDER_TEXT + 1,
    );
    sugarloaf.rect(
        None,
        x + 1.0,
        y + 1.0,
        width - 2.0,
        height - 2.0,
        theme.f32(theme.surface),
        DEPTH,
        ORDER_TEXT + 2,
    );
    let style = DrawOpts {
        color: theme.u8(theme.fg),
        clip_rect: Some(layout.rect),
        ..*opts
    };
    for (i, line) in layout.lines.iter().enumerate() {
        let label = elide(sugarloaf, line, &style, width - padding * 2.0);
        draw_text(
            sugarloaf,
            x + padding,
            y + padding + i as f32 * row_h,
            &label,
            &style,
            occlusions,
        );
    }
}

fn register(sugarloaf: &mut Sugarloaf, pixels: &[u8]) -> Option<u32> {
    if pixels.len() != 4096 {
        return None;
    }
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    pixels.hash(&mut hash);
    let mut id = 0xAB00_0000 | (hash.finish() as u32 & 0x00ff_ffff);
    // Compare the actual bytes, not just a truncated image ID: a collision
    // must NEVER display another author's face. Probe a vacant ID instead.
    while let Some(entry) = sugarloaf.image_data.get(&id) {
        if let sugarloaf::components::core::image::Data::Rgba {
            width: 32,
            height: 32,
            pixels: stored,
        } = entry.handle.data()
        {
            if stored.as_ref() == pixels {
                return Some(id);
            }
        }
        id = 0xAB00_0000 | (id.wrapping_add(1) & 0x00ff_ffff);
    }
    let keys: Vec<_> = sugarloaf
        .image_data
        .keys()
        .copied()
        .filter(|id| id & 0xff00_0000 == 0xAB00_0000)
        .collect();
    if keys.len() >= 256 {
        if let Some(key) = keys.first() {
            sugarloaf.image_data.remove(key);
        }
    }
    sugarloaf.image_data.insert(
        id,
        GraphicDataEntry::from_graphic_data(GraphicData {
            id: GraphicId::new(id as u64),
            width: 32,
            height: 32,
            color_type: ColorType::Rgba,
            pixels: pixels.to_vec(),
            is_opaque: false,
            resize: None,
            display_width: None,
            display_height: None,
            transmit_time: Instant::now(),
        }),
    );
    Some(id)
}

fn paint_avatar(
    sugarloaf: &mut Sugarloaf,
    id: u32,
    rect: [f32; 4],
    clip: [f32; 4],
    occlusions: &[[f32; 4]],
) {
    let Some(clipped) = intersect_cursor_rect(rect, clip) else {
        return;
    };
    let mut pieces = vec![clipped];
    for &occlusion in occlusions {
        let mut next = Vec::new();
        for p in pieces {
            if let Some(o) = intersect_cursor_rect(p, occlusion) {
                next.extend(
                    [
                        [p[0], p[1], p[2], o[1] - p[1]],
                        [p[0], o[1] + o[3], p[2], p[1] + p[3] - o[1] - o[3]],
                        [p[0], o[1], o[0] - p[0], o[3]],
                        [o[0] + o[2], o[1], p[0] + p[2] - o[0] - o[2], o[3]],
                    ]
                    .into_iter()
                    .filter(|r| r[2] > 0.0 && r[3] > 0.0),
                );
            } else {
                next.push(p);
            }
        }
        pieces = next;
    }
    let scale = sugarloaf.scale_factor();
    for [x, y, w, h] in pieces {
        sugarloaf.push_image_overlay(
            OVERLAY_ID,
            sugarloaf::GraphicOverlay {
                image_id: id,
                x: x * scale,
                y: y * scale,
                width: w * scale,
                height: h * scale,
                z_index: 8,
                source_rect: [
                    (x - rect[0]) / rect[2],
                    (y - rect[1]) / rect[3],
                    (x + w - rect[0]) / rect[2],
                    (y + h - rect[1]) / rect[3],
                ],
            },
        );
    }
}
