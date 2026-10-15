//! Host-neutral composer folder/attachment actions and measured picker-chip hits. Hosts own picker routing.

fn contains(rect: [f32; 4], x: f32, y: f32) -> bool {
    x >= rect[0] && y >= rect[1] && x < rect[0] + rect[2] && y < rect[1] + rect[3]
}
#[cfg(test)]
#[path = "composer_interactions_tests.rs"]
mod tests;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComposerAction {
    AttachPicture,
    ChangeDirectory,
}

#[derive(Default)]
pub struct ComposerInteractions {
    pub cwd_rect: Option<[f32; 4]>,
    pub cwd_rects: Vec<[f32; 4]>,
    status_chip_rects: Vec<Option<[f32; 4]>>,
    /// Renderer-provided overlays; covered chips must not receive clicks or pointers.
    pub occlusion_rects: Vec<[f32; 4]>,
}

/// Measured cwd ink (folder glyph + inner gap + cwd), never branch or trailing gap.
/// Mirror the footer's horizontal text clipping and carve all occluded columns.
pub fn cwd_visible_hit_rects(
    rect: [f32; 4],
    measured_width: f32,
    clip_width: f32,
    occlusions: &[[f32; 4]],
) -> Vec<[f32; 4]> {
    let [x, y, w, h] = rect;
    let width = measured_width.min(clip_width).min(w).max(0.0);
    if width <= 0.0 || h <= 0.0 {
        return Vec::new();
    }
    let mut spans = vec![(x, x + width)];
    for cut in occlusions {
        if cut[1] >= y + h || cut[1] + cut[3] <= y {
            continue;
        }
        let mut next = Vec::new();
        for (left, right) in spans {
            if cut[0] >= right || cut[0] + cut[2] <= left {
                next.push((left, right));
            } else {
                if cut[0] > left {
                    next.push((left, cut[0].min(right)));
                }
                if cut[0] + cut[2] < right {
                    next.push(((cut[0] + cut[2]).max(left), right));
                }
            }
        }
        spans = next;
    }
    spans
        .into_iter()
        .filter(|(left, right)| right > left)
        .map(|(left, right)| [left, y, right - left, h])
        .collect()
}

impl ComposerInteractions {
    pub fn clear_rects(&mut self) {
        self.cwd_rect = None;
        self.cwd_rects.clear();
        self.clear_status_chip_rects();
        self.occlusion_rects.clear();
    }

    pub fn clear_status_chip_rects(&mut self) {
        self.status_chip_rects.clear();
    }

    pub fn register_status_chip_rect(&mut self, index: usize, rect: [f32; 4]) {
        if self.status_chip_rects.len() <= index {
            self.status_chip_rects.resize(index + 1, None);
        }
        self.status_chip_rects[index] = Some(rect);
    }

    pub fn status_chip_at(&self, x: f32, y: f32) -> Option<usize> {
        if self.occlusion_rects.iter().any(|rect| contains(*rect, x, y)) {
            return None;
        }
        self.status_chip_rects
            .iter()
            .position(|rect| rect.is_some_and(|rect| contains(rect, x, y)))
    }

    /// Cursor policy only: picker actions stay on their existing host click path.
    /// Text input is deliberately not a pointer target.
    pub fn pointer_at(&self, x: f32, y: f32) -> bool {
        !self.occlusion_rects.iter().any(|rect| contains(*rect, x, y))
            && (self.action_at(x, y).is_some() || self.status_chip_at(x, y).is_some())
    }

    pub fn action_at(&self, x: f32, y: f32) -> Option<ComposerAction> {
        self.cwd_rect
            .into_iter()
            .chain(self.cwd_rects.iter().copied())
            .any(|r| x >= r[0] && y >= r[1] && x < r[0] + r[2] && y < r[1] + r[3])
            .then_some(ComposerAction::ChangeDirectory)
    }
}
