//! Original-row change runs and their review-only hover presentation.
use super::*;
use neoism_protocol::git::GitChangeBlock;

#[derive(Clone, Debug)]
pub(super) struct ChangeRun {
    pub first: usize,
    pub end: usize,
    pub block: GitChangeBlock,
}

/// Parse hunk anchors before alignment/wrapping. A zero-count range already is
/// an insertion anchor; nonempty ranges use one-based source line numbers.
fn anchor(range: &str) -> Option<usize> {
    let mut parts = range.get(1..)?.split(',');
    let start = parts.next()?.parse::<usize>().ok()?;
    let count = parts.next().unwrap_or("1").parse::<usize>().ok()?;
    Some(if count == 0 {
        start
    } else {
        start.saturating_sub(1)
    })
}

pub(super) fn change_runs(lines: &[DiffLine]) -> Vec<ChangeRun> {
    let mut runs: Vec<ChangeRun> = Vec::new();
    let (mut old, mut new) = (0, 0);
    let mut active = false;
    for (row, line) in lines.iter().enumerate() {
        if line.text.starts_with("\\ No newline at end of file") {
            if active {
                runs.last_mut().unwrap().end = row + 1;
            }
            continue;
        }
        if line.kind == DiffLineKind::Hunk {
            let mut parts = line.text.split_whitespace();
            if parts.next() == Some("@@") {
                if let (Some(a), Some(b)) =
                    (parts.next().and_then(anchor), parts.next().and_then(anchor))
                {
                    old = a;
                    new = b;
                }
            }
            active = false;
            continue;
        }
        if let Some(n) = line.old_line_number.filter(|n| *n > 0) {
            old = n as usize - 1;
        }
        if let Some(n) = line.new_line_number.filter(|n| *n > 0) {
            new = n as usize - 1;
        }
        if matches!(line.kind, DiffLineKind::Add | DiffLineKind::Remove) {
            if !active {
                runs.push(ChangeRun {
                    first: row,
                    end: row + 1,
                    block: GitChangeBlock {
                        old_start: old,
                        new_start: new,
                        old_lines: Vec::new(),
                        new_lines: Vec::new(),
                    },
                });
            }
            let run = runs.last_mut().unwrap();
            run.end = row + 1;
            let text = line.text.trim_end_matches(['\n', '\r']).to_owned();
            if line.kind == DiffLineKind::Remove {
                run.block.old_lines.push(text);
                old += 1;
            } else {
                run.block.new_lines.push(text);
                new += 1;
            }
            active = true;
        } else {
            old += 1;
            new += 1;
            active = false;
        }
    }
    runs
}

fn action_available(sections: &[FileSection], path: &str, restore: bool) -> bool {
    sections
        .iter()
        .find(|section| section.change.path == path)
        .is_some_and(|section| {
            // Git status describes disk, not unsaved canonical source edits.
            !restore || section.change.status != FileStatus::Untracked
        })
}

pub(super) struct BlockControl {
    pub path: String,
    pub block: GitChangeBlock,
    pub body: Rect,
    pub panel: Rect,
    pub stage: Rect,
    pub restore: Rect,
    pub shown: bool,
}

/// All aligned rows, including blank padding, carry an original run origin.
pub(super) fn run_rows(
    run: &ChangeRun,
    origins: &[usize],
    offsets: &[usize],
    split: bool,
) -> Option<(usize, usize)> {
    if split {
        let first = origins
            .iter()
            .position(|r| *r >= run.first && *r < run.end)?;
        let last = origins
            .iter()
            .rposition(|r| *r >= run.first && *r < run.end)?;
        Some((first, last + 1))
    } else {
        Some((*offsets.get(run.first)?, *offsets.get(run.end)?))
    }
}

pub(super) fn block_body_rect(
    first: usize,
    end: usize,
    body_y: f32,
    line_height: f32,
    scale: f32,
    clip: Rect,
) -> Option<Rect> {
    let top = body_y + diff_card::BODY_TOP_PAD * scale + first as f32 * line_height;
    intersection(
        Rect::new(clip.x, top, clip.w, (end - first) as f32 * line_height),
        clip,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(patch: &str) -> Vec<DiffLine> {
        let mut lines = Vec::new();
        crate::panels::git_diff::parse_diff_into_with_headers(
            patch.as_bytes(),
            &mut lines,
        );
        lines
    }

    #[test]
    fn cold_patch_separates_runs_by_real_context_and_hunks() {
        let rows = parse("diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,6 +1,7 @@\n same\n-old\n+new\n+extra\n middle\n-gone\n+back\n tail\n@@ -20 +21 @@\n-last\n+final\n");
        let runs = change_runs(&rows);
        assert_eq!(runs.len(), 3);
        assert_eq!(
            runs[0].block,
            GitChangeBlock {
                old_start: 1,
                new_start: 1,
                old_lines: vec!["old".into()],
                new_lines: vec!["new".into(), "extra".into()]
            }
        );
        assert_eq!(
            runs[1].block,
            GitChangeBlock {
                old_start: 3,
                new_start: 4,
                old_lines: vec!["gone".into()],
                new_lines: vec!["back".into()]
            }
        );
        assert_eq!((runs[2].block.old_start, runs[2].block.new_start), (19, 20));
        let (_, _, origins) = split_lines(&rows);
        for (index, run) in runs.iter().enumerate() {
            let (first, end) = run_rows(run, &origins, &[], true).unwrap();
            for origin in &origins[first..end] {
                assert!(*origin >= runs[index].first && *origin < runs[index].end);
            }
        }
    }

    #[test]
    fn zero_count_anchors_and_eof_metadata_preserve_exact_payload() {
        for (patch, old, new, old_lines, new_lines) in [
            ("@@ -0,0 +1,2 @@\n+first\n+second\n",0,0,vec![],vec!["first","second"]),
            ("@@ -1,2 +0,0 @@\n-first\n-second\n",0,0,vec!["first","second"],vec![]),
            ("@@ -5,0 +6 @@\n+end\n\\ No newline at end of file\n",5,5,vec![],vec!["end"]),
            ("@@ -6 +5,0 @@\n-end\n\\ No newline at end of file\n",5,5,vec!["end"],vec![]),
            ("@@ -8 +8 @@\n-old\r\n\\ No newline at end of file\n+new\r\n\\ No newline at end of file\n",7,7,vec!["old"],vec!["new"]),
        ] {
            let runs = change_runs(&parse(patch));
            assert_eq!(runs.len(),1, "{patch}");
            assert_eq!((runs[0].block.old_start,runs[0].block.new_start),(old,new));
            assert_eq!(runs[0].block.old_lines,old_lines);
            assert_eq!(runs[0].block.new_lines,new_lines);
        }
        // Metadata supplied directly (rather than omitted by the parser) must
        // not split a replacement either.
        let mut rows = parse("@@ -1 +1 @@\n-old\n+new\n");
        rows.insert(
            2,
            DiffLine {
                text: "\\ No newline at end of file".into(),
                kind: DiffLineKind::Context,
                line_number: None,
                old_line_number: None,
                new_line_number: None,
            },
        );
        assert_eq!(change_runs(&rows).len(), 1);
    }

    #[test]
    fn split_origins_include_padding_and_unified_wraps_clip_sticky_rows() {
        let rows = parse("@@ -1,3 +1,4 @@\n-a\n-b\n+x\n+y\n+z\n same\n");
        let runs = change_runs(&rows);
        let (left, right, origins) = split_lines(&rows);
        assert_eq!(left.len(), right.len());
        assert_eq!(origins, vec![0, 1, 2, 4, 5, 6]);
        let aligned = run_rows(&runs[0], &origins, &[], true).unwrap();
        assert_eq!(aligned, (1, 5));
        assert!(left[3].new_line_number.is_none());
        assert!(left[3].old_line_number.is_none());
        let offsets = vec![0, 1, 4, 6, 7, 10, 11, 12];
        assert_eq!(run_rows(&runs[0], &[], &offsets, false), Some((1, 11)));
        for scale in [1.0, 1.5, 2.0] {
            let h = 18.0 * scale;
            let clip = Rect::new(20.0, 80.0 * scale, 300.0 * scale, 70.0 * scale);
            let rect = block_body_rect(1, 11, 0.0, h, scale, clip).unwrap();
            assert_eq!(rect.y, clip.y);
            assert_eq!(rect.h, clip.h);
            assert_eq!(rect.w, clip.w);
            assert!(block_body_rect(20, 21, 0.0, h, scale, clip).is_none());
        }
    }

    fn interactive_view() -> GitDiffView {
        let mut view = GitDiffView::new("/repo".into(), "a.rs".into());
        view.set_lines(parse("@@ -1 +1 @@\n-old\n+new\n"));
        let block = view.sections[0].runs[0].block.clone();
        view.block_controls.push(BlockControl {
            path: "a.rs".into(),
            block,
            body: Rect::new(0.0, 50.0, 300.0, 18.0),
            panel: Rect::new(180.0, 50.0, 120.0, 24.0),
            stage: Rect::new(182.0, 50.0, 50.0, 24.0),
            restore: Rect::new(232.0, 50.0, 66.0, 24.0),
            shown: true,
        });
        view.edit_hits.push(EditHit {
            path: "a.rs".into(),
            line: 0,
            byte_start: 0,
            byte_end: 3,
            text: "new".into(),
            text_x: 0.0,
            scroll_x: 0.0,
            cell_w: 8.0,
            rect: Rect::new(0.0, 50.0, 300.0, 30.0),
        });
        view.source_hits.push(SourceHit {
            path: "a.rs".into(),
            line: 0,
            rect: Rect::new(180.0, 50.0, 40.0, 24.0),
        });
        view
    }

    #[test]
    fn block_buttons_precede_source_and_edit_hits_and_capture_panel_overhang() {
        let mut view = interactive_view();
        assert!(view.pointer_move(200.0, 55.0));
        assert!(view.pointer_cursor(200.0, 55.0));
        assert!(!view.text_cursor(200.0, 55.0));
        assert!(view.pointer_down(200.0, 55.0));
        assert!(view.take_edit_request().is_none());
        let block = view.sections[0].runs[0].block.clone();
        assert_eq!(
            view.take_actions(),
            vec![GitDiffViewAction::ChangeBlock {
                root: "/repo".into(),
                path: "a.rs".into(),
                block: block.clone(),
                restore: false
            }]
        );
        assert!(view.pointer_move(250.0, 72.0)); // Below the 18px body.
        assert_eq!(view.block_hover, Some(0));
        assert!(view.pointer_down(250.0, 72.0));
        assert_eq!(
            view.take_actions(),
            vec![GitDiffViewAction::ChangeBlock {
                root: "/repo".into(),
                path: "a.rs".into(),
                block,
                restore: true
            }]
        );
        assert!(view.pointer_leave());
        assert_eq!(view.block_hover, None);
        assert!(!view.block_controls[0].shown);
        assert!(!view.pointer_leave());
        assert!(view.pointer_move(20.0, 55.0)); // The other side / blank hatching.
        assert_eq!(view.block_hover, Some(0));
        assert!(view.pointer_move(20.0, 90.0));
        assert_eq!(view.block_hover, None);
    }

    #[test]
    fn island_occludes_card_and_projected_code_without_occluding_own_hits() {
        let mut view = interactive_view();
        view.set_text_occlusions(&[[0.0, 0.0, 10.0, 10.0]]);
        // Enter via the change body, before the island was painted/shown.
        view.block_controls[0].shown = false;
        view.pointer_move(100.0, 55.0);
        let p = view.block_controls[0].panel;
        let local = view.block_paint_occlusions();
        assert_eq!(local, vec![[0.0, 0.0, 10.0, 10.0], [p.x, p.y, p.w, p.h]]);
        assert_eq!(view.text_occlusions, vec![[0.0, 0.0, 10.0, 10.0]]);
        view.block_controls[0].shown = true;
        assert!(view.block_panel_at(200.0, 55.0).is_some());
        assert!(!view.occluded(200.0, 55.0));
        assert!(view.pointer_down(200.0, 55.0));
        assert!(matches!(
            view.take_actions().as_slice(),
            [GitDiffViewAction::ChangeBlock { restore: false, .. }]
        ));
        assert!(view.take_edit_request().is_none());
        view.pointer_leave();
        assert_eq!(view.block_paint_occlusions(), view.text_occlusions);
        // An overlay covering the island suppresses it, rather than adding a
        // hidden control's footprint to the underlying code's occlusions.
        view.set_text_occlusions(&[[p.x, p.y, p.w, p.h]]);
        view.pointer_move(100.0, 55.0);
        assert_eq!(view.block_paint_occlusions(), view.text_occlusions);
    }

    #[test]
    fn live_eof_markers_are_not_rows_or_alignment_boundaries() {
        let mut view = GitDiffView::new("/repo".into(), "a.rs".into());
        view.set_lines(build_review_diff("old", "new", 3, None));
        let section = &view.sections[0];
        assert_eq!(section.lines.as_ref().unwrap().len(), 3);
        assert_eq!(section.left.len(), 2); // Hunk row + aligned replacement.
        assert_eq!(section.split_origins, vec![0, 1]);
        assert_eq!(section.runs.len(), 1);
        assert_eq!(section.runs[0].block.old_lines, vec!["old"]);
        assert_eq!(section.runs[0].block.new_lines, vec!["new"]);
    }

    #[test]
    fn second_run_click_keeps_its_exact_payload_not_the_hunk_or_first_run() {
        let mut view = interactive_view();
        view.set_lines(parse("@@ -1,3 +1,3 @@\n-a\n+A\n same\n-b\n+B\n"));
        assert_eq!(view.sections[0].runs.len(), 2);
        let expected = view.sections[0].runs[1].block.clone();
        view.block_controls[0].block = expected.clone();
        assert!(view.pointer_down(250.0, 55.0));
        assert_eq!(
            view.take_actions(),
            vec![GitDiffViewAction::ChangeBlock {
                root: "/repo".into(),
                path: "a.rs".into(),
                block: expected,
                restore: true,
            }]
        );
        assert!(view.take_edit_request().is_none());
    }

    #[test]
    fn untracked_restore_is_disabled_without_disabling_stage_for_live_edits() {
        let mut view = interactive_view();
        view.sections[0].change.status = FileStatus::Untracked;
        assert!(view.pointer_cursor(200.0, 55.0));
        assert!(!view.pointer_cursor(250.0, 55.0));
        assert!(view.pointer_down(250.0, 55.0));
        assert!(view.take_actions().is_empty());
        assert!(view.take_edit_request().is_none());
        assert!(view.pointer_down(200.0, 55.0));
        assert_eq!(view.take_actions().len(), 1);
        view.sections[0].change.status = FileStatus::Staged;
        view.sections[0].change.staged = true;
        assert!(view.pointer_cursor(200.0, 55.0));
        assert!(view.pointer_down(200.0, 55.0));
        assert_eq!(view.take_actions().len(), 1);
    }

    #[test]
    fn loading_history_occlusion_and_refreshed_payload_guard_controls() {
        let mut view = interactive_view();
        view.actions_loading = true;
        assert!(view.pointer_down(200.0, 55.0));
        assert!(view.take_actions().is_empty());
        assert!(view.take_edit_request().is_none());
        view.actions_loading = false;
        view.set_text_occlusions(&[[180.0, 50.0, 120.0, 24.0]]);
        assert!(!view.pointer_down(200.0, 55.0));
        assert!(view.block_hover_at(200.0, 55.0).is_none());
        view.set_text_occlusions(&[]);
        view.commit_id = Some("history".into());
        assert!(view.block_hover_at(200.0, 55.0).is_none());
        assert!(!view.pointer_down(200.0, 55.0));
        view.commit_id = None;
        view.set_lines(parse("@@ -1 +1 @@\n-old\n+different\n"));
        assert!(!view.block_pointer_down(200.0, 55.0));
        assert!(view.take_actions().is_empty());
    }
}

impl GitDiffView {
    /// Local to underlying card/projected code; never suppress our own hits or labels.
    pub(super) fn block_paint_occlusions(&self) -> Vec<[f32; 4]> {
        let mut regions = self.text_occlusions.clone();
        if let Some(c) = self
            .header_pointer
            .and_then(|(x, y)| self.block_hover_at(x, y))
            .and_then(|i| self.block_controls.get(i))
        {
            if !self.text_occlusions.iter().any(|r| {
                intersection(c.panel, Rect::new(r[0], r[1], r[2], r[3])).is_some()
            }) {
                regions.push([c.panel.x, c.panel.y, c.panel.w, c.panel.h]);
            }
        }
        regions
    }
    pub(super) fn block_button_at(&self, x: f32, y: f32) -> Option<(usize, bool)> {
        let index = self.block_panel_at(x, y)?;
        let c = &self.block_controls[index];
        if c.stage.contains(x, y) && action_available(&self.sections, &c.path, false) {
            Some((index, false))
        } else if c.restore.contains(x, y)
            && action_available(&self.sections, &c.path, true)
        {
            Some((index, true))
        } else {
            None
        }
    }
    fn valid_block(&self, control: &BlockControl) -> bool {
        !self.is_read_only()
            && self.sections.iter().any(|s| {
                s.change.path == control.path
                    && !s.collapsed
                    && s.runs.iter().any(|r| r.block == control.block)
            })
    }

    pub(super) fn block_hover_at(&self, x: f32, y: f32) -> Option<usize> {
        if self.occluded(x, y) {
            return None;
        }
        // A visible island owns even the few pixels extending below a one-line run.
        self.block_controls
            .iter()
            .position(|c| c.shown && c.panel.contains(x, y) && self.valid_block(c))
            .or_else(|| {
                self.block_controls
                    .iter()
                    .position(|c| c.body.contains(x, y) && self.valid_block(c))
            })
    }

    pub(super) fn block_panel_at(&self, x: f32, y: f32) -> Option<usize> {
        if self.occluded(x, y) {
            return None;
        }
        self.block_controls
            .iter()
            .position(|c| c.shown && c.panel.contains(x, y) && self.valid_block(c))
    }

    pub(super) fn block_pointer_down(&mut self, x: f32, y: f32) -> bool {
        let Some(index) = self.block_panel_at(x, y) else {
            return false;
        };
        let c = &self.block_controls[index];
        let restore = c.restore.contains(x, y);
        if !self.actions_loading
            && (c.stage.contains(x, y) || restore)
            && action_available(&self.sections, &c.path, restore)
        {
            self.pending_actions.push(GitDiffViewAction::ChangeBlock {
                root: self.root.clone(),
                path: c.path.clone(),
                block: c.block.clone(),
                restore,
            });
        }
        self.edit_request = None;
        true
    }

    pub(super) fn paint_block_controls(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        scale: f32,
        theme: &IdeTheme,
    ) {
        self.block_hover = self
            .header_pointer
            .and_then(|(x, y)| self.block_hover_at(x, y));
        for (index, c) in self.block_controls.iter_mut().enumerate() {
            c.shown = self.block_hover == Some(index);
            if !c.shown {
                continue;
            }
            let p = c.panel;
            // Never paint an island through an overlay (text and shapes agree).
            if self
                .text_occlusions
                .iter()
                .any(|r| intersection(p, Rect::new(r[0], r[1], r[2], r[3])).is_some())
            {
                c.shown = false;
                continue;
            }
            sugarloaf.quad(
                None,
                p.x,
                p.y,
                p.w,
                p.h,
                theme.f32(theme.border),
                [3.0 * scale; 4],
                0.0,
                38,
            );
            sugarloaf.quad(
                None,
                p.x + scale,
                p.y + scale,
                (p.w - 2.0 * scale).max(0.0),
                (p.h - 2.0 * scale).max(0.0),
                theme.f32(theme.surface),
                [2.0 * scale; 4],
                0.0,
                39,
            );
            for (label, r, restore) in
                [("Stage", c.stage, false), ("Restore", c.restore, true)]
            {
                let enabled = !self.actions_loading
                    && action_available(&self.sections, &c.path, restore);
                if enabled && self.header_pointer.is_some_and(|(x, y)| r.contains(x, y)) {
                    sugarloaf.quad(
                        None,
                        r.x,
                        r.y,
                        r.w,
                        r.h,
                        theme.f32(theme.hover),
                        [2.0 * scale; 4],
                        0.0,
                        40,
                    );
                }
                draw_text_with_occlusion(
                    sugarloaf,
                    r.x + 6.0 * scale,
                    r.y + 5.0 * scale,
                    label,
                    &DrawOpts {
                        font_size: 12.0 * scale,
                        color: theme.u8(if !enabled { theme.dim } else { theme.fg }),
                        clip_rect: Some([r.x, r.y, r.w, r.h]),
                        ..DrawOpts::default()
                    },
                    &self.text_occlusions,
                );
            }
        }
    }
}
