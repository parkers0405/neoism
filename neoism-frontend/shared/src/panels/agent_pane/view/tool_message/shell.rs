//! User-entered commands stay open in history, unlike model tool previews.
use super::*;
use unicode_segmentation::UnicodeSegmentation;

type RowCache = VecDeque<(String, u64, u32, u32, Rc<Vec<String>>)>;
thread_local! { static ROWS: RefCell<RowCache> = RefCell::new(VecDeque::new()); }
const HEADER: f32 = 58.0;
const ROW: f32 = 20.0;
const VISIBLE_ROWS: usize = 16;

fn rows(paint: &mut dyn crate::primitives::paint::Paint, message: &impl AgentToolMessage, width: f32, s: f32) -> Rc<Vec<String>> {
    let hash = hash_value(&message.detail());
    let width = (width - 32.0 * s).max(1.0);
    if let Some(rows) = ROWS.with(|cache| cache.borrow().iter().find(|entry|
        entry.0 == message.id() && entry.1 == hash && entry.2 == width.to_bits() && entry.3 == s.to_bits()).map(|entry| entry.4.clone())) {
        return rows;
    }
    let opts = DrawOpts { font_size: 13.0 * s, ..DrawOpts::default() };
    let mut rows = Vec::new();
    // Preserve blank lines/indentation and wrap whole graphemes, not words.
    for line in message.detail().split('\n') {
        let mut row = String::new();
        let mut used = 0.0;
        for glyph in line.trim_end_matches('\r').graphemes(true) {
            let glyph = if glyph == "\t" { "    " } else { glyph };
            let advance = super::super::draw::measure_text_cached(paint, glyph, &opts);
            if used + advance > width && !row.is_empty() {
                rows.push(std::mem::take(&mut row));
                used = 0.0;
            }
            row.push_str(glyph);
            used += advance;
        }
        rows.push(row);
    }
    let rows = Rc::new(rows);
    ROWS.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.retain(|entry| entry.0 != message.id());
        cache.push_back((message.id().into(), hash, width.to_bits(), s.to_bits(), rows.clone()));
        while cache.len() > 16 { cache.pop_front(); }
    });
    rows
}

pub(super) fn height(paint: &mut dyn crate::primitives::paint::Paint, message: &impl AgentToolMessage, width: f32, s: f32) -> f32 {
    (HEADER + 16.0 + ROW * rows(paint, message, width, s).len().min(VISIBLE_ROWS).max(1) as f32) * s
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render(paint: &mut dyn crate::primitives::paint::Paint, pane: &mut impl AgentToolPane,
    x: f32, y: f32, w: f32, h: f32, message: &impl AgentToolMessage, theme: &IdeTheme, s: f32,
    clip: [f32; 4], occlusions: &[[f32; 4]]) -> f32 {
    let Some(clip) = intersect_rect([x, y, w, h], clip) else { return h; };
    let panel = theme.panel_text_theme();
    // Match Home's terminal transcript: open background and horizontal rules only.
    for line_y in [y, y+h-s] {
        draw_rect_clipped(paint, [x, line_y, w, s], theme.f32_alpha(theme.fg, 0.1), ORDER_PANEL, clip);
    }
    let accent = match message.status() { "error" | "interrupted" => theme.red, "completed" => theme.muted, _ => theme.accent };
    let header_clip = intersect_rect([x+16.0*s, y, (w-32.0*s).max(0.0), HEADER*s], clip);
    if let Some(header_clip) = header_clip {
        let opts = opts_with_clip(DrawOpts { font_size: 13.0*s, color: theme.u8(theme.fg), ..DrawOpts::default() }, header_clip).unwrap();
        let (directory, status) = message.text().rsplit_once("  |  ").unwrap_or((message.text(), ""));
        let status_opts = DrawOpts { font_size: 11.7*s, color: theme.u8(accent), ..opts.clone() };
        let status_width = super::super::draw::measure_text_cached(paint, status, &status_opts);
        let show_status = status_width+48.0*s < header_clip[2];
        let directory_opts = DrawOpts { color: theme.u8(theme.muted), ..status_opts.clone() };
        let directory = crate::primitives::truncate_to_fit(directory,
            (header_clip[2]-if show_status { status_width+16.0*s } else { 0.0 }).max(0.0), paint, &directory_opts);
        draw_text_clipped(paint, x+16.0*s, y+13.0*s, &directory, &directory_opts, occlusions);
        if show_status {
            draw_text_clipped(paint, x+w-16.0*s-status_width, y+13.0*s, status, &status_opts, occlusions);
        }
        let command = crate::primitives::truncate_to_fit(&message.title().replace('\n', " "), header_clip[2], paint, &opts);
        draw_text_clipped(paint, x+16.0*s, y+34.0*s, &command, &opts, occlusions);
    }
    let body = [x+16.0*s, y+(HEADER+8.0)*s, (w-32.0*s).max(0.0), (h-(HEADER+16.0)*s).max(0.0)];
    let Some(body_clip) = intersect_rect(body, clip) else { return h; };
    let rows = rows(paint, message, w, s);
    let row_h = ROW*s;
    let full_height = rows.len() as f32*row_h;
    let max_scroll = (full_height-body[3]).max(0.0);
    let key = format!("shell:{}", message.id());
    let scroll = pane.diff_scroll_offset(&key, max_scroll);
    // Wheel capture must remain available during the timeline's interaction-settle period.
    if max_scroll > 0.0 { pane.register_diff_scroll_rect(key, [x, body[1], w, body[3]], max_scroll); }
    let opts = opts_with_clip(DrawOpts { font_size: 13.0*s, color: panel.u8(panel.fg), ..DrawOpts::default() }, body_clip).unwrap();
    let first = ((body_clip[1]-body[1]+scroll)/row_h).floor().max(0.0) as usize;
    let end = (((body_clip[1]+body_clip[3]-body[1]+scroll)/row_h).ceil().max(0.0) as usize).min(rows.len());
    for index in first..end {
        let line_y = body[1]+index as f32*row_h-scroll;
        let line = &rows[index];
        let rect = [body[0], line_y, body[2], row_h];
        // Fractional scaling can round the estimated row range one row past
        // the clip edge. Only register/paint rows that actually intersect it.
        let Some(visible_rect) = intersect_rect(rect, body_clip) else { continue; };
        if !pane.suppress_tool_interactions() && !occlusions.iter().any(|cut| intersect_rect(rect, *cut).is_some()) {
            let stops = measured_caret_stops(paint, line, &opts, body[0]);
            let selected = pane.register_selectable_line_with_caret_stops(line, visible_rect, &stops);
            if let Some((left, right)) = pane.selectable_line_highlight(selected) {
                draw_rect_clipped(paint, [left, line_y, (right-left).max(1.0), row_h], theme.f32_alpha(theme.accent, 0.22), ORDER_PANEL+2, body_clip);
            }
        }
        draw_text_clipped(paint, body[0], line_y, line, &opts, occlusions);
    }
    if max_scroll > 0.0 {
        let thumb = (body[3]*body[3]/full_height).max(18.0*s).min(body[3]);
        draw_rounded_rect_clipped(paint, [x+w-7.0*s, body[1]+(body[3]-thumb)*scroll/max_scroll, 3.0*s, thumb],
            theme.f32_alpha(theme.muted, 0.5), 1.5*s, ORDER_PANEL+3, clip);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::agent_pane::api_mapping::part_block;
    use crate::primitives::paint_record::{PaintCommand, RecordingPainter};

    #[test]
    fn direct_shell_fractional_clip_boundary_does_not_register_an_invisible_row() {
        let output=(0..30).map(|i|format!("line{i:02}")).collect::<Vec<_>>().join("\n");
        let message=part_block(&serde_json::json!({"id":"fractional-shell","type":"tool","tool":"bash",
            "state":{"status":"completed","input":{"command":"print lines"},"output":output,
                "metadata":{"directShell":true,"cwd":"/project","exitCode":0}}})).unwrap();
        let mut paint=RecordingPainter::new("monospace",400,vec![]);
        let mut pane=NeoismAgentPane::new();
        let h=height(&mut paint,&message,400.0,0.8);
        render(&mut paint,&mut pane,0.0,203.6,400.0,h,&message,&IdeTheme::by_name("pastel_dark"),0.8,[0.0,0.0,400.0,650.0],&[]);
        let commands=paint.borrowed_commands();
        let output_rows=commands.iter().filter_map(|command|match command {
            PaintCommand::Text{text,..} if text.starts_with("line")=>Some(text.as_str()),_=>None,
        }).collect::<Vec<_>>();
        assert_eq!(output_rows.len(),16);
        assert_eq!(output_rows.last(),Some(&"line15"));
    }

    #[test]
    fn direct_shell_block_keeps_output_open_and_scrolls_to_the_last_line() {
        let output = (0..100).map(|i| format!("  line {i}")).collect::<Vec<_>>().join("\n");
        let message = part_block(&serde_json::json!({"id":"shell-test", "type":"tool", "tool":"bash",
            "state":{"status":"completed", "input":{"command":"print lines"}, "output":output,
                "metadata":{"directShell":true,"cwd":"/project","exitCode":0}}})).unwrap();
        for name in ["pastel_dark", "ayu_light"] {
            let theme = IdeTheme::by_name(name);
            let mut paint = RecordingPainter::new("monospace", 400, vec![]);
            let mut pane = NeoismAgentPane::new();
            let h = super::super::render::measure_tool_message_height(&mut paint, &pane, &message, 400.0, 1.0, false, None).unwrap();
            assert_eq!(h, HEADER+16.0+ROW*VISIBLE_ROWS as f32);
            render(&mut paint, &mut pane, 0.0, 0.0, 400.0, h, &message, &theme, 1.0, [0.0,0.0,400.0,h], &[]);
            let rules: Vec<_> = paint.borrowed_commands().iter().filter_map(|c| match c {
                PaintCommand::Rect { rect, radii, .. } if rect[2] > 3.0 => Some((*rect, *radii)),
                _ => None,
            }).collect();
            assert_eq!(rules, vec![([0.0,0.0,400.0,1.0], [0.0;4]), ([0.0,h-1.0,400.0,1.0], [0.0;4])]);
            assert!(paint.borrowed_commands().iter().any(|c| matches!(c, PaintCommand::Text {text,..} if text == "print lines")));
            assert!(paint.borrowed_commands().iter().any(|c| matches!(c, PaintCommand::Text {text,..} if text == "  line 0")));
            assert_eq!(pane.scroll_diff_at(100.0, 100.0, 10000.0), Some(true));
            let mut paint = RecordingPainter::new("monospace", 400, vec![]);
            render(&mut paint, &mut pane, 0.0, 0.0, 400.0, h, &message, &theme, 1.0, [0.0,0.0,400.0,h], &[]);
            assert!(paint.borrowed_commands().iter().any(|c| matches!(c, PaintCommand::Text {text,..} if text == "  line 99")));
            assert!(!paint.borrowed_commands().iter().any(|c| matches!(c, PaintCommand::Text {text,..} if text == "  line 0")));
        }
    }
}