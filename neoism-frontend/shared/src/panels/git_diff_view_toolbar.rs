//! Compact review toolbar. Layout is measured before painting; whole groups
//! disappear at narrow widths rather than clipping labels or retaining ghost hits.
use super::*;

#[cfg(test)]
include!("git_diff_view_toolbar_tests.rs");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Control {
    Fold,
    Unified,
    Split,
    Previous,
    Next,
    Stage,
    StageAll,
    Commit,
}

#[derive(Clone, Copy)]
pub(super) struct Hit {
    pub rect: Rect,
    pub control: Control,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Group {
    Fold,
    Modes,
    Totals,
    Navigation,
    Stage,
    StageAll,
    Commit,
}

struct GroupRect {
    group: Group,
    rect: Rect,
}

/// All dimensions here are physical pixels, including measured text widths.
fn layout(rect: Rect, scale: f32, widths: [f32; 7]) -> Vec<GroupRect> {
    let groups = [
        Group::Fold,
        Group::Modes,
        Group::Totals,
        Group::Navigation,
        Group::Stage,
        Group::StageAll,
        Group::Commit,
    ];
    let mut visible = [true; 7];
    let gap = 8.0 * scale;
    let available = (rect.w - 16.0 * scale).max(0.0);
    let cost = |shown: &[bool; 7]| {
        let count = shown.iter().filter(|v| **v).count();
        widths
            .iter()
            .zip(shown)
            .filter(|(_, v)| **v)
            .map(|(w, _)| *w)
            .sum::<f32>()
            + count.saturating_sub(1) as f32 * gap
    };
    // Hide secondary information first, then entire action groups. The mode
    // pair stays together and is never rendered as a misleading single toggle.
    for index in [2, 5, 6, 4, 3, 0, 1] {
        if cost(&visible) <= available {
            break;
        }
        visible[index] = false;
    }
    let left_cost = (0..2)
        .filter(|i| visible[*i])
        .map(|i| widths[i] + gap)
        .sum::<f32>();
    let right_count = (2..7).filter(|i| visible[*i]).count();
    let right_cost = (2..7)
        .filter(|i| visible[*i])
        .map(|i| widths[i])
        .sum::<f32>()
        + right_count.saturating_sub(1) as f32 * gap;
    let mut left = rect.x + 8.0 * scale;
    let mut right = (rect.x + rect.w - 8.0 * scale - right_cost).max(left + left_cost);
    let mut result = Vec::new();
    for i in 0..7 {
        if !visible[i] {
            continue;
        }
        let x = if i < 2 { &mut left } else { &mut right };
        result.push(GroupRect {
            group: groups[i],
            rect: Rect::new(*x, rect.y, widths[i], rect.h),
        });
        *x += widths[i] + gap;
    }
    result
}

impl GitDiffView {
    fn toolbar_stage_direction(&self) -> Option<bool> {
        let index = *self.indices.get(&self.path)?;
        let change = &self.sections.get(index)?.change;
        Some(change.staged && change.status != FileStatus::Mixed)
    }

    fn toolbar_enabled(&self, control: Control) -> bool {
        match control {
            Control::Unified | Control::Split => true,
            Control::Fold => !self.sections.is_empty(),
            Control::Previous | Control::Next => {
                let Some(&index) = self.indices.get(&self.path) else {
                    return !self.sections.is_empty();
                };
                if control == Control::Previous {
                    index > 0
                } else {
                    index + 1 < self.sections.len()
                }
            }
            Control::Stage => {
                !self.actions_loading && self.toolbar_stage_direction().is_some()
            }
            Control::StageAll => {
                !self.actions_loading
                    && self
                        .sections
                        .iter()
                        .any(|s| !s.change.staged || s.change.status == FileStatus::Mixed)
            }
            // Opening the editor is safe even with only worktree changes; it
            // never calls commit(), stages files, or modifies the user's draft.
            Control::Commit => !self.actions_loading && !self.sections.is_empty(),
        }
    }

    fn toolbar_activate(&mut self, control: Control) {
        if !self.toolbar_enabled(control) {
            return;
        }
        match control {
            Control::Fold => {
                let collapse = self.sections.iter().any(|s| !s.collapsed);
                for section in &mut self.sections {
                    section.collapsed = collapse;
                }
                self.pending_reveal = Some(self.path.clone());
            }
            Control::Unified | Control::Split => {
                let split = control == Control::Split;
                if self.split != split {
                    self.key("Tab");
                }
            }
            Control::Previous | Control::Next => {
                let index = match self.indices.get(&self.path).copied() {
                    Some(i) if control == Control::Previous => i.saturating_sub(1),
                    Some(i) => (i + 1).min(self.sections.len() - 1),
                    None => 0,
                };
                let path = PathBuf::from(&self.sections[index].change.path);
                self.reveal_file(&path);
            }
            Control::Stage => {
                if let Some(unstage) = self.toolbar_stage_direction() {
                    let index = self.indices[&self.path];
                    self.pending_actions.push(GitDiffViewAction::Stage {
                        root: self.root.clone(),
                        path: self.sections[index].change.path.clone(),
                        unstage,
                    });
                }
            }
            Control::StageAll => self.pending_actions.push(GitDiffViewAction::StageAll {
                root: self.root.clone(),
            }),
            Control::Commit => self.pending_actions.push(GitDiffViewAction::Commit {
                root: self.root.clone(),
            }),
        }
    }

    pub(super) fn toolbar_pointer_down(&mut self, x: f32, y: f32) -> bool {
        let Some(hit) = self
            .toolbar_hits
            .iter()
            .find(|hit| hit.rect.contains(x, y))
            .copied()
        else {
            return false;
        };
        // Disabled targets consume the click, but snapshot state is checked
        // again: an async refresh may have invalidated the painted target.
        if hit.enabled {
            self.toolbar_activate(hit.control);
        }
        true
    }

    pub(super) fn render_toolbar(
        &mut self,
        sugarloaf: &mut dyn crate::primitives::paint::Paint,
        rect: Rect,
        height: f32,
        scale: f32,
        theme: &IdeTheme,
    ) {
        self.toolbar_hits.clear();
        if height < 28.0 * scale {
            return;
        }
        let opts = DrawOpts {
            font_size: 12.0 * scale,
            color: theme.u8(theme.dim),
            clip_rect: Some([rect.x, rect.y, rect.w, height]),
            ..DrawOpts::default()
        };
        let additions = format!(
            "+{}",
            self.sections
                .iter()
                .map(|s| u64::from(s.change.additions))
                .sum::<u64>()
        );
        let deletions = format!(
            "-{}",
            self.sections
                .iter()
                .map(|s| u64::from(s.change.deletions))
                .sum::<u64>()
        );
        let stage = if self.toolbar_stage_direction() == Some(true) {
            "Unstage"
        } else {
            "Stage"
        };
        let labels = [
            additions.as_str(),
            deletions.as_str(),
            stage,
            "Stage All",
            "Commit",
        ];
        let measured = labels.map(|label| sugarloaf.text_mut().measure(label, &opts));
        let button = 26.0 * scale;
        let padding = 12.0 * scale;
        let groups = layout(
            Rect::new(rect.x, rect.y, rect.w, height),
            scale,
            [
                button,
                button * 2.0,
                measured[0] + measured[1] + 8.0 * scale,
                button * 2.0,
                measured[2] + padding,
                measured[3] + padding,
                measured[4] + padding,
            ],
        );
        let mut previous: Option<Rect> = None;
        for entry in groups {
            let r = entry.rect;
            // Separate adjacent groups; leave the flexible middle space blank.
            if let Some(prev) = previous {
                if r.x - (prev.x + prev.w) <= 9.0 * scale {
                    sugarloaf.quad(
                        None,
                        r.x - 4.0 * scale,
                        r.y + 10.0 * scale,
                        scale,
                        16.0 * scale,
                        theme.f32(theme.border),
                        [0.0; 4],
                        0.0,
                        21,
                    );
                }
            }
            previous = Some(r);
            match entry.group {
                Group::Fold => self.toolbar_button(
                    sugarloaf,
                    r,
                    Control::Fold,
                    None,
                    scale,
                    theme,
                    &opts,
                ),
                Group::Modes | Group::Navigation => {
                    let controls = if entry.group == Group::Modes {
                        [Control::Unified, Control::Split]
                    } else {
                        [Control::Previous, Control::Next]
                    };
                    for (i, control) in controls.into_iter().enumerate() {
                        self.toolbar_button(
                            sugarloaf,
                            Rect::new(r.x + i as f32 * button, r.y, button, r.h),
                            control,
                            None,
                            scale,
                            theme,
                            &opts,
                        );
                    }
                }
                Group::Totals => {
                    draw_text_with_occlusion(
                        sugarloaf,
                        r.x,
                        r.y + 10.0 * scale,
                        &additions,
                        &DrawOpts {
                            color: theme.u8(theme.green),
                            ..opts
                        },
                        &self.text_occlusions,
                    );
                    draw_text_with_occlusion(
                        sugarloaf,
                        r.x + measured[0] + 8.0 * scale,
                        r.y + 10.0 * scale,
                        &deletions,
                        &DrawOpts {
                            color: theme.u8(theme.red),
                            ..opts
                        },
                        &self.text_occlusions,
                    );
                }
                Group::Stage | Group::StageAll | Group::Commit => {
                    let (control, label) = match entry.group {
                        Group::Stage => (Control::Stage, stage),
                        Group::StageAll => (Control::StageAll, "Stage All"),
                        _ => (Control::Commit, "Commit"),
                    };
                    self.toolbar_button(
                        sugarloaf,
                        r,
                        control,
                        Some(label),
                        scale,
                        theme,
                        &opts,
                    );
                }
            }
        }
    }

    fn toolbar_button(
        &mut self,
        sugarloaf: &mut dyn crate::primitives::paint::Paint,
        rect: Rect,
        control: Control,
        label: Option<&str>,
        scale: f32,
        theme: &IdeTheme,
        opts: &DrawOpts,
    ) {
        let enabled = self.toolbar_enabled(control);
        self.toolbar_hits.push(Hit {
            rect,
            control,
            enabled,
        });
        let active = (control == Control::Split && self.split)
            || (control == Control::Unified && !self.split);
        let color = if !enabled {
            theme.muted
        } else if active {
            theme.blue
        } else {
            theme.dim
        };
        if let Some(label) = label {
            draw_text_with_occlusion(
                sugarloaf,
                rect.x + 6.0 * scale,
                rect.y + 10.0 * scale,
                label,
                &DrawOpts {
                    color: theme.u8(color),
                    ..*opts
                },
                &self.text_occlusions,
            );
            return;
        }
        // Geometric icons avoid font fallback / glyph-metric differences.
        // Respect overlay occlusion just as the text does.
        if self.text_occlusions.iter().any(|r| {
            rect.x < r[0] + r[2]
                && rect.x + rect.w > r[0]
                && rect.y < r[1] + r[3]
                && rect.y + rect.h > r[1]
        }) {
            return;
        }
        for triangle in icon_triangles(
            control,
            self.sections.iter().all(|s| s.collapsed),
            rect,
            scale,
        ) {
            sugarloaf.triangle_ordered(
                triangle[0].0,
                triangle[0].1,
                triangle[1].0,
                triangle[1].1,
                triangle[2].0,
                triangle[2].1,
                0.0,
                theme.f32(color),
                ICON_ORDER,
            );
        }
    }
}

// `line` always uses compositor order 0 (behind the order-20 review background).
// Explicitly ordered triangle strokes also avoid any icon-font dependency.
const ICON_ORDER: u8 = 22;
type IconTriangle = [(f32, f32); 3];

fn icon_triangles(
    control: Control,
    collapsed: bool,
    rect: Rect,
    scale: f32,
) -> Vec<IconTriangle> {
    let cx = rect.x + rect.w / 2.0;
    let cy = rect.y + rect.h / 2.0;
    let mut triangles = Vec::new();
    let mut line = |ax: f32, ay: f32, bx: f32, by: f32| {
        let length = (bx - ax).hypot(by - ay);
        let nx = -(by - ay) / length * 0.5;
        let ny = (bx - ax) / length * 0.5;
        let point = |x: f32, y: f32| (cx + x * scale, cy + y * scale);
        let a = point(ax + nx, ay + ny);
        let b = point(bx + nx, by + ny);
        let c = point(bx - nx, by - ny);
        let d = point(ax - nx, ay - ny);
        triangles.extend([[a, b, c], [a, c, d]]);
    };
    match control {
        Control::Unified | Control::Split => {
            line(-7.0, -6.0, 7.0, -6.0);
            line(7.0, -6.0, 7.0, 6.0);
            line(7.0, 6.0, -7.0, 6.0);
            line(-7.0, 6.0, -7.0, -6.0);
            if control == Control::Split {
                line(0.0, -6.0, 0.0, 6.0);
            } else {
                line(-4.0, -2.0, 4.0, -2.0);
                line(-4.0, 2.0, 4.0, 2.0);
            }
        }
        Control::Previous | Control::Next => {
            let d = if control == Control::Previous {
                -1.0
            } else {
                1.0
            };
            line(0.0, -6.0, 0.0, 6.0);
            line(-4.0, 2.0 * d, 0.0, 6.0 * d);
            line(0.0, 6.0 * d, 4.0, 2.0 * d);
        }
        Control::Fold => {
            let d = if !collapsed { -1.0 } else { 1.0 };
            for y in [-3.0, 3.0] {
                line(-4.0, y - 2.0 * d, 0.0, y + 2.0 * d);
                line(0.0, y + 2.0 * d, 4.0, y - 2.0 * d);
            }
        }
        _ => {}
    }
    triangles
}
