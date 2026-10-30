//! Paint real buffer segments in a host-owned (possibly discontinuous) layout.
pub use super::gitdiff::build_review_diff;
pub use super::types::{ProjectedCodeRow, ProjectedCodeSpec};
use super::{feed::styled_runs_with_syntax, layout::*, types::*};
use crate::{primitives::ide_theme::IdeTheme, syntax::syn_color};
use neoism_paint::text::DrawOpts;

/// Text/selection/caret only. The host owns backgrounds, gutters and scrolling.
/// Refresh syntax once for the whole buffer, never render a full pane per row.
pub fn render_projected_code(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    pane: &mut CodePane,
    spec: &ProjectedCodeSpec,
    theme: &IdeTheme,
    occlusions: &[[f32; 4]],
) {
    let opts = DrawOpts {
        font_size: spec.font_size,
        color: theme.u8(theme.fg),
        clip_rect: Some(spec.viewport),
        ..DrawOpts::default()
    };
    let cell = sugarloaf.text_mut().measure("0", &opts).max(1.0);
    pane.geometry = CodePaneGeometry {
        rect: spec.viewport,
        text_x: spec.text_x,
        cell_w: cell,
        row_h: spec.row_height,
        scroll_x: spec.scroll_x,
        projected_rows: Some(spec.rows.clone()),
        ..CodePaneGeometry::default()
    };
    pane.cursor_rect = None;
    pane.highlight.refresh(&pane.buffer, pane.language);
    let clips = visible_clips(spec.viewport, occlusions);
    let mut cached_line = usize::MAX;
    let mut runs = Vec::new();
    let caret_row =
        projected_caret_row(&spec.rows, pane.buffer.cursor_line, pane.buffer.cursor_col);
    for (row_index, row) in spec.rows.iter().enumerate() {
        if row.y + spec.row_height <= spec.viewport[1]
            || row.y >= spec.viewport[1] + spec.viewport[3]
        {
            continue;
        }
        let Some(line) = pane.buffer.lines.get(row.source_line) else {
            continue;
        };
        if row.byte_start > row.byte_end
            || line.get(row.byte_start..row.byte_end).is_none()
        {
            continue;
        }
        let segment = &line[row.byte_start..row.byte_end];
        let px = |byte| {
            spec.text_x - spec.scroll_x
                + (row.indent_cols
                    + display_col_for_byte(
                        segment,
                        byte - row.byte_start,
                        TAB_DISPLAY_WIDTH,
                    )) as f32
                    * cell
        };
        let ty = row.y + ((spec.row_height - spec.font_size * 1.2) * 0.5).max(0.0);
        if let Some((start, end)) = pane.buffer.selection_on_line(row.source_line) {
            let a = start.max(row.byte_start);
            let b = end.min(row.byte_end);
            if a < b {
                paint_rect(
                    sugarloaf,
                    [px(a), row.y, px(b) - px(a), spec.row_height],
                    theme.f32_alpha(theme.accent, 0.28),
                    &clips,
                );
            }
        }
        if cached_line != row.source_line {
            cached_line = row.source_line;
            runs = styled_runs_with_syntax(
                line,
                pane.highlight.line_runs(row.source_line),
                pane.language,
                pane.buffer.selection_on_line(row.source_line),
                &[],
                &[],
            );
        }
        for run in &runs {
            let start = run.start.max(row.byte_start);
            let end = run.end.min(row.byte_end);
            if start >= end || run.concealed {
                continue;
            }
            let col =
                display_col_for_byte(segment, start - row.byte_start, TAB_DISPLAY_WIDTH);
            let text = expand_tabs_from(&line[start..end], col, TAB_DISPLAY_WIDTH);
            let run_opts = DrawOpts {
                color: syn_color(run.token, theme, false),
                ..opts
            };
            paint_text(sugarloaf, px(start), ty, &text, run_opts, &clips);
        }
        // Trimmed wrap whitespace has no glyphs: keep its real source caret
        // visible at the nearest fragment edge without inventing editable bytes.
        let col = pane.buffer.cursor_col.clamp(row.byte_start, row.byte_end);
        if spec.focused && caret_row == Some(row_index) {
            let block =
                pane.cursor_shape() == neoism_terminal_types::ansi::CursorShape::Block;
            // Host cursor shapes scale within a full cell (the beam is a
            // fraction of its width). Publishing an already-thinned rect makes
            // native Insert almost invisible: the host thins it a second time.
            let rect = projected_source_cursor_rect(spec, row, line, col, cell);
            pane.cursor_rect = visible_cursor_rect(rect, spec.viewport, occlusions);
            if !pane.caret_drawn_by_host {
                let paint_rect_width = if block { cell } else { 2.0 };
                paint_rect(
                    sugarloaf,
                    [rect[0], rect[1], paint_rect_width, rect[3]],
                    theme.f32(theme.accent),
                    &clips,
                );
                if block {
                    if let Some(c) = line
                        .get(col..)
                        .and_then(|s| s.chars().next())
                        .filter(|c| !c.is_whitespace())
                    {
                        paint_text(
                            sugarloaf,
                            px(col),
                            ty,
                            &c.to_string(),
                            DrawOpts {
                                color: theme.u8(theme.bg),
                                ..opts
                            },
                            &clips,
                        );
                    }
                }
            }
        }
    }
}

fn visible_cursor_rect(
    rect: [f32; 4],
    viewport: [f32; 4],
    occlusions: &[[f32; 4]],
) -> Option<[f32; 4]> {
    let clipped = intersection(rect, viewport)?;
    // Native TrailCursor has no clip regions: do not publish a cell under chrome.
    (!occlusions
        .iter()
        .any(|cut| intersection(clipped, *cut).is_some()))
    .then_some(clipped)
}

fn projected_source_cursor_rect(
    spec: &ProjectedCodeSpec,
    row: &ProjectedCodeRow,
    line: &str,
    column: usize,
    cell: f32,
) -> [f32; 4] {
    let column = column.clamp(row.byte_start, row.byte_end);
    let segment = &line[row.byte_start..row.byte_end];
    let x = spec.text_x - spec.scroll_x
        + (row.indent_cols
            + display_col_for_byte(segment, column - row.byte_start, TAB_DISPLAY_WIDTH))
            as f32
            * cell;
    projected_cursor_rect(
        x,
        row.y,
        cell,
        (spec.font_size * 1.2).min(spec.row_height),
        spec.row_height,
    )
}

fn projected_cursor_rect(
    x: f32,
    row_y: f32,
    cell: f32,
    height: f32,
    row_height: f32,
) -> [f32; 4] {
    [x, row_y + (row_height - height) * 0.5, cell, height]
}

fn projected_caret_row(
    rows: &[ProjectedCodeRow],
    line: usize,
    col: usize,
) -> Option<usize> {
    rows.iter()
        .enumerate()
        .filter(|(_, row)| row.source_line == line)
        .min_by_key(|(_, row)| {
            let distance = row
                .byte_start
                .saturating_sub(col)
                .max(col.saturating_sub(row.byte_end));
            (distance, std::cmp::Reverse(row.byte_start))
        })
        .map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projected_caret_keeps_full_cell_geometry_across_vim_modes() {
        use neoism_terminal_types::ansi::CursorShape;
        let mut pane = CodePane::new("/repo/a.rs".into(), "hello\n");
        pane.input_mode = CodeInputMode::Vim;
        let viewport = [20.0, 10.0, 200.0, 80.0];
        let expected = Some([40.0, 22.0, 10.0, 16.0]);
        for (mode, shape) in [
            (CodeMode::Normal, CursorShape::Block),
            (CodeMode::Insert, CursorShape::Beam),
            (CodeMode::Normal, CursorShape::Block),
        ] {
            pane.buffer.mode = mode;
            pane.cursor_rect = intersection(
                projected_cursor_rect(40.0, 20.0, 10.0, 16.0, 20.0),
                viewport,
            );
            assert_eq!(pane.cursor_rect, expected);
            assert_eq!(pane.cursor_shape(), shape);
        }
        assert!(
            intersection(
                projected_cursor_rect(40.0, 100.0, 10.0, 16.0, 20.0),
                viewport,
            )
            .is_none(),
            "offscreen carets must not be clamped into spacer rows"
        );
    }

    #[test]
    fn source_cursor_cell_tracks_utf8_tabs_wrap_indent_and_horizontal_scroll_in_all_modes(
    ) {
        use neoism_terminal_types::ansi::CursorShape;
        let mut pane = CodePane::new("/repo/a.rs".into(), "hidden\naé\tz\n");
        pane.input_mode = CodeInputMode::Vim;
        let row = ProjectedCodeRow {
            source_line: 1,
            y: 40.0,
            byte_start: 1,
            byte_end: 5,
            indent_cols: 2,
        };
        let spec = ProjectedCodeSpec {
            viewport: [0.0, 0.0, 200.0, 100.0],
            text_x: 20.0,
            scroll_x: 10.0,
            row_height: 20.0,
            font_size: 10.0,
            rows: vec![row],
            focused: true,
        };
        for (mode, shape) in [
            (CodeMode::Normal, CursorShape::Block),
            (CodeMode::Insert, CursorShape::Beam),
            (CodeMode::Visual, CursorShape::Block),
        ] {
            pane.buffer.mode = mode;
            for (column, expected_x) in [(1, 30.0), (3, 40.0), (4, 70.0), (5, 80.0)] {
                pane.buffer.set_cursor_position(1, column, false);
                let index = projected_caret_row(
                    &spec.rows,
                    pane.buffer.cursor_line,
                    pane.buffer.cursor_col,
                )
                .unwrap();
                assert_eq!(
                    projected_source_cursor_rect(
                        &spec,
                        &spec.rows[index],
                        &pane.buffer.lines[1],
                        pane.buffer.cursor_col,
                        10.0
                    ),
                    [expected_x, 44.0, 10.0, 12.0]
                );
                assert_eq!(pane.cursor_shape(), shape);
            }
        }
        assert_eq!(projected_caret_row(&spec.rows, 0, 0), None);
    }

    fn geometry() -> CodePaneGeometry {
        CodePaneGeometry {
            rect: [0.0, 0.0, 400.0, 200.0],
            text_x: 20.0,
            cell_w: 10.0,
            row_h: 20.0,
            projected_rows: Some(vec![
                ProjectedCodeRow {
                    source_line: 2,
                    y: 20.0,
                    byte_start: 0,
                    byte_end: 7,
                    indent_cols: 0,
                },
                ProjectedCodeRow {
                    source_line: 5,
                    y: 100.0,
                    byte_start: 2,
                    byte_end: 6,
                    indent_cols: 0,
                },
            ]),
            ..CodePaneGeometry::default()
        }
    }

    #[test]
    fn exact_projection_rejects_old_only_and_hunk_gaps() {
        let g = geometry();
        let lines = vec![
            "unused".into(),
            "unused".into(),
            "aé\t界".into(),
            "unused".into(),
            "unused".into(),
            "αβxy".into(),
        ];
        assert_eq!(g.projected_hit_position(&lines, 20.0, 20.0), Some((2, 0)));
        assert_eq!(g.projected_hit_position(&lines, 30.0, 25.0), Some((2, 1)));
        assert_eq!(g.projected_hit_position(&lines, 40.0, 25.0), Some((2, 3)));
        for y in [0.0, 40.0, 60.0, 99.0, 120.0, 199.0] {
            assert_eq!(g.projected_hit_position(&lines, 20.0, y), None);
            let (line, _) = g.hit_position(&lines, 20.0, y);
            assert!(line == 2 || line == 5, "never fabricate spacer source rows");
        }
        assert_eq!(g.projected_hit_position(&lines, 20.0, 200.0), None);
    }

    #[test]
    fn segment_byte_columns_scroll_and_stale_ranges() {
        let mut g = geometry();
        let lines = vec![
            String::new(),
            String::new(),
            "aé\t界".into(),
            String::new(),
            String::new(),
            "αβxy".into(),
        ];
        assert_eq!(g.projected_hit_position(&lines, 20.0, 105.0), Some((5, 2)));
        assert_eq!(g.projected_hit_position(&lines, 30.0, 105.0), Some((5, 4)));
        assert_eq!(g.projected_hit_position(&lines, 390.0, 105.0), Some((5, 6)));
        g.projected_rows.as_mut().unwrap()[1].indent_cols = 3;
        assert_eq!(g.projected_hit_position(&lines, 20.0, 105.0), Some((5, 2)));
        assert_eq!(g.projected_hit_position(&lines, 60.0, 105.0), Some((5, 4)));
        g.projected_rows.as_mut().unwrap()[1].indent_cols = 0;
        g.scroll_x = 10.0;
        assert_eq!(g.projected_hit_position(&lines, 20.0, 105.0), Some((5, 4)));
        g.projected_rows.as_mut().unwrap()[1].byte_start = 1;
        assert_eq!(g.projected_hit_position(&lines, 20.0, 105.0), None);
        assert_eq!(g.projected_hit_position(&[], 20.0, 25.0), None);
    }

    #[test]
    fn fragment_tabs_restart_at_fragment_origin_after_synthetic_indent() {
        let g = CodePaneGeometry {
            rect: [0.0, 0.0, 200.0, 20.0],
            cell_w: 10.0,
            row_h: 20.0,
            projected_rows: Some(vec![ProjectedCodeRow {
                source_line: 0,
                y: 0.0,
                byte_start: 1,
                byte_end: 3,
                indent_cols: 2,
            }]),
            ..CodePaneGeometry::default()
        };
        let lines = vec!["a\tz".into()];
        assert_eq!(g.projected_hit_position(&lines, 10.0, 10.0), Some((0, 1)));
        assert_eq!(g.projected_hit_position(&lines, 50.0, 10.0), Some((0, 1)));
        assert_eq!(g.projected_hit_position(&lines, 60.0, 10.0), Some((0, 2)));
    }

    #[test]
    fn caret_survives_trimmed_whitespace_and_empty_lines() {
        let rows = vec![
            ProjectedCodeRow {
                source_line: 3,
                y: 0.0,
                byte_start: 0,
                byte_end: 4,
                indent_cols: 0,
            },
            ProjectedCodeRow {
                source_line: 3,
                y: 20.0,
                byte_start: 8,
                byte_end: 12,
                indent_cols: 3,
            },
            ProjectedCodeRow {
                source_line: 4,
                y: 40.0,
                byte_start: 0,
                byte_end: 0,
                indent_cols: 0,
            },
        ];
        assert_eq!(projected_caret_row(&rows, 3, 5), Some(0));
        assert_eq!(projected_caret_row(&rows, 3, 7), Some(1));
        assert_eq!(projected_caret_row(&rows, 3, 12), Some(1));
        assert_eq!(projected_caret_row(&rows, 4, 0), Some(2));
        assert_eq!(projected_caret_row(&rows, 2, 0), None);
    }

    #[test]
    fn host_caret_never_paints_over_a_hover_island() {
        let viewport = [0.0, 0.0, 300.0, 100.0];
        let cell = [220.0, 44.0, 10.0, 16.0];
        assert_eq!(visible_cursor_rect(cell, viewport, &[]), Some(cell));
        assert_eq!(
            visible_cursor_rect(cell, viewport, &[[200.0, 40.0, 90.0, 24.0]]),
            None
        );
        assert_eq!(
            visible_cursor_rect(cell, viewport, &[[225.0, 40.0, 50.0, 24.0]]),
            None
        );
        assert_eq!(
            visible_cursor_rect(cell, viewport, &[[240.0, 40.0, 50.0, 24.0]]),
            Some(cell)
        );
    }

    #[test]
    fn clipped_paint_never_covers_occlusions() {
        let clips = visible_clips([0.0, 0.0, 100.0, 100.0], &[[20.0, 20.0, 60.0, 60.0]]);
        assert_eq!(clips.iter().map(|r| r[2] * r[3]).sum::<f32>(), 6400.0);
        assert!(clips
            .iter()
            .all(|r| intersection(*r, [20.0, 20.0, 60.0, 60.0]).is_none()));
    }
}

fn paint_text(
    s: &mut dyn crate::primitives::paint::Paint,
    x: f32,
    y: f32,
    text: &str,
    opts: DrawOpts,
    clips: &[[f32; 4]],
) {
    for clip in clips {
        s.text_mut().draw(
            x,
            y,
            text,
            &DrawOpts {
                clip_rect: Some(*clip),
                ..opts
            },
        );
    }
}
fn paint_rect(s: &mut dyn crate::primitives::paint::Paint, rect: [f32; 4], color: [f32; 4], clips: &[[f32; 4]]) {
    for clip in clips {
        if let Some([x, y, w, h]) = intersection(rect, *clip) {
            s.rect(None, x, y, w, h, color, 0.0, 8);
        }
    }
}
fn intersection(a: [f32; 4], b: [f32; 4]) -> Option<[f32; 4]> {
    let x = a[0].max(b[0]);
    let y = a[1].max(b[1]);
    let w = (a[0] + a[2]).min(b[0] + b[2]) - x;
    let h = (a[1] + a[3]).min(b[1] + b[3]) - y;
    (w > 0.0 && h > 0.0).then_some([x, y, w, h])
}
fn visible_clips(viewport: [f32; 4], occlusions: &[[f32; 4]]) -> Vec<[f32; 4]> {
    let mut clips = vec![viewport];
    for cut in occlusions {
        clips = clips
            .into_iter()
            .flat_map(|r| {
                let Some([x, y, w, h]) = intersection(r, *cut) else {
                    return vec![r];
                };
                vec![
                    [r[0], r[1], r[2], y - r[1]],
                    [r[0], y + h, r[2], r[1] + r[3] - y - h],
                    [r[0], y, x - r[0], h],
                    [x + w, y, r[0] + r[2] - x - w, h],
                ]
                .into_iter()
                .filter(|r| r[2] > 0.0 && r[3] > 0.0)
                .collect()
            })
            .collect();
    }
    clips
}
