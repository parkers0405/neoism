//! One tool row: header, then whichever body it has (`tool_message/render.rs`).

use super::text::{self, Text, width};
use super::{BODY_MAX_ROWS, BODY_ROW_H, Cx, GROUP_BODY_Y, HEADER_H, accent, compact_title, diff, files, group, shell, task_id, todos};
use crate::app::App;
use neoism_protocol::agent::HistoryMessage;
use rune::{Color, El, el};

/// A row and its height. `child` marks a composition child (archived: it shows a disclosure).
pub fn message(cx: &Cx, m: &HistoryMessage, child: bool) -> (El, f32) {
    if m.tool == "user_shell" {
        return shell::card(cx, m);
    }
    if m.composition_children.is_empty() {
        return single(cx, m, child);
    }
    let (root, root_h) = single(cx, m, child);
    let inner = cx.narrower(cx.pt(20.0));
    let mut height = root_h;
    let mut rows = vec![root];
    for c in &m.composition_children {
        // A lone read/search child reads as a one-item group, keeping its own identity.
        let compact = group::read_family(&c.tool).map(|_| HistoryMessage {
            id: c.id.clone(), title: c.title.clone(), status: c.status.clone(), mcp: c.mcp.clone(),
            ..group::read_group(std::slice::from_ref(c))
        });
        let (row, h) = message(&inner, compact.as_ref().unwrap_or(c), true);
        rows.push(row.absolute(cx.pt(20.0), height));
        height += h;
    }
    let mut stack = el().w(cx.w).h(height);
    for row in rows { stack = stack.child(row); }
    (stack, height)
}

fn single(cx: &Cx, m: &HistoryMessage, child: bool) -> (El, f32) {
    let state = &cx.app.tools;
    if group::is_todos(m) {
        if m.todos.is_empty() { return (el(), 0.0); }
        let (list, rows) = todos::list(cx, &m.todos);
        let h = cx.pt(42.0) + rows as f32 * cx.pt(todos::ROW_H);
        return (header(cx, m, Header::default()).h(h).child(list.absolute(cx.pt(30.0), cx.pt(28.0))), h);
    }
    let expanded = state.expanded(&m.id);
    let edit = diff::is_edit_tool(&m.tool);
    let unsettled_status = matches!(m.status.trim().to_ascii_lowercase().as_str(), "pending" | "running" | "streaming");
    let show_files = |open: bool| open || (edit && !unsettled_status);
    let sections = if edit || show_files(expanded) {
        diff::sections(&m.tool, &m.status, &m.title, &m.text, &m.detail)
    } else {
        Default::default()
    };
    let inline_files = edit && !sections.is_empty() && show_files(false);
    let deferred = diff::is_unsettled(&m.tool, &m.status);
    let counts = (edit && !sections.is_empty()).then(|| {
        (sections.iter().map(|s| u64::from(s.additions)).sum::<u64>(), sections.iter().map(|s| u64::from(s.deletions)).sum::<u64>())
    }).filter(|(a, d)| *a > 0 || *d > 0);
    let disclosure = !inline_files && (edit || child || m.tool == "compose");
    let mut row = header(cx, m, Header { counts, disclosure, clickable: !inline_files, expanded });
    let mut h = cx.pt(HEADER_H);
    if deferred || (!expanded && !inline_files) {
        return (row.h(h), h);
    }
    if !sections.is_empty() && show_files(expanded) {
        let (cards, cards_h) = files::cards(cx, m, &sections, cx.pt(HEADER_H));
        for card in cards { row = row.child(card); }
        h += cards_h;
        return (row.h(h), h);
    }
    // Expanded output: a `╰─` elbow, then up to twelve wrapped rows.
    let group_row = m.tool == "tool_group";
    let elbow_y = if group_row { GROUP_BODY_Y } else { 26.0 };
    row = row.child(cx.text("\u{2570}\u{2500}", 14.0).bold().color(cx.color(cx.p.fg)).at(cx.pt(28.0), cx.pt(elbow_y)));
    let (body, body_h) = if group_row { group_children(cx, m) } else { output(cx, m) };
    h = body_h.max(cx.pt(58.0));
    (row.child(body).h(h), h)
}

#[derive(Default)]
struct Header {
    counts: Option<(u64, u64)>,
    disclosure: bool,
    clickable: bool,
    expanded: bool,
}

/// Indicator, service symbol, title, counts, status label and disclosure; pressing it expands
/// the row (or, on a `task`, opens the sub-agent's conversation).
fn header(cx: &Cx, m: &HistoryMessage, spec: Header) -> El {
    let (p, w) = (cx.p, cx.w);
    let mut row = el().w(w).h(cx.pt(HEADER_H));
    row = row.child(indicator(cx, m));
    row = row.child(symbol(cx, m));

    let status = if m.status == "completed" { "" } else { m.status.as_str() };
    let status_w = if status.is_empty() { 0.0 } else { width(status, cx.pt(12.0)) + cx.pt(12.0) };
    let disclosure_w = if spec.disclosure { cx.pt(18.0) } else { 0.0 };
    let counts: Vec<(String, u32)> = spec.counts.map(|(a, d)| {
        [(a, format!("+{a}"), p.green), (d, format!("-{d}"), p.red)].into_iter().filter(|(n, ..)| *n > 0).map(|(_, l, c)| (l, c)).collect()
    }).unwrap_or_default();
    let counts_w: f32 = counts.iter().map(|(l, _)| width(l, cx.pt(13.0)) + cx.pt(12.0)).sum();
    let available = (w - cx.pt(66.0) - status_w - disclosure_w).max(0.0);
    let show_counts = !counts.is_empty() && available >= counts_w + cx.pt(36.0);
    let title_avail = available - if show_counts { counts_w } else { 0.0 };
    let title = if diff::is_streaming_patch_tool(&m.tool) { "Apply patch".to_owned() } else { compact_title(&m.title) };
    let title = cx.text(title, 15.5).fit(title_avail).color(cx.color(p.fg));
    let title_w = title.width();
    let text_x = cx.pt(42.0);
    row = row.child(title.at(text_x, cx.pt(2.0)));
    if show_counts {
        let mut x = text_x + title_w + cx.pt(12.0);
        for (label, color) in counts {
            let t = cx.text(label, 13.0).color(cx.color(color));
            let tw = t.width();
            row = row.child(t.at(x, cx.pt(4.0)));
            x += tw + cx.pt(12.0);
        }
    }
    if spec.disclosure {
        let arrow = if spec.expanded { "\u{2304}" } else { "\u{203a}" };
        row = row.child(cx.text(arrow, 12.0).color(cx.color(p.muted)).at(w - cx.pt(16.0), cx.pt(3.0)));
    }
    if !status.is_empty() {
        let x = w - cx.pt(24.0) - disclosure_w - status_w + cx.pt(12.0);
        row = row.child(cx.text(status, 12.0).color(cx.color(accent(p, &m.status))).at(x, cx.pt(4.0)));
    }
    if !spec.clickable {
        return row;
    }
    let id = m.id.clone();
    // Only the Task row navigates (`hit_rects.rs`): diff sections and group children don't.
    let task = (m.tool == "task").then(|| task_id(&m.detail, &m.text)).flatten();
    let hit = el().w(w).h(cx.pt(HEADER_H)).absolute(0.0, 0.0).on_press(move |a: &mut App| match &task {
        Some(session) => a.view_session(session),
        None => a.tools.toggle(&id),
    });
    row.child(hit)
}

/// A status dot, the orbiting four-square spinner while running, or `↳` on a follow-up task.
fn indicator(cx: &Cx, m: &HistoryMessage) -> El {
    let title = m.title.trim();
    if m.tool == "task" && (title == "Follow-up" || title.starts_with("Follow-up(")) {
        return cx.text("\u{21b3}", 15.5).color(cx.color(cx.p.muted)).at(cx.pt(1.0), cx.pt(2.0));
    }
    if matches!(m.status.as_str(), "running" | "streaming") {
        let mut spinner = el().w(cx.pt(16.0)).h(cx.pt(20.0)).absolute(0.0, 0.0);
        let phase = cx.app.tools.clock as f32 * 1.35;
        for (trail, alpha) in [1.0, 0.65, 0.4, 0.2].into_iter().enumerate() {
            let step = (phase * 8.0).floor() - trail as f32;
            let (dx, dy) = orbit(step / 8.0, cx.pt(3.0));
            spinner = spinner.child(el().w(cx.pt(2.0)).h(cx.pt(2.0)).fill(Color::WHITE.alpha(alpha))
                .absolute(cx.pt(7.0) + dx - cx.s, cx.pt(10.5) + dy - cx.s));
        }
        return spinner;
    }
    let d = cx.pt(7.0);
    el().w(d).h(d).radius(d / 2.0).fill(cx.color(accent(cx.p, &m.status))).absolute(cx.pt(3.5), cx.pt(7.0))
}

/// Position on a square orbit of half-size `half`; one lap per unit of `t`.
fn orbit(t: f32, half: f32) -> (f32, f32) {
    let p = t.rem_euclid(1.0) * 4.0;
    if p < 1.0 {
        (-half + p * 2.0 * half, -half)
    } else if p < 2.0 {
        (half, -half + (p - 1.0) * 2.0 * half)
    } else if p < 3.0 {
        (half - (p - 2.0) * 2.0 * half, half)
    } else {
        (-half, half - (p - 3.0) * 2.0 * half)
    }
}

/// The service symbol in the 18 pt gutter slot: a sub-agent's logo, an MCP server's mark, or the
/// tool's glyph.
fn symbol(cx: &Cx, m: &HistoryMessage) -> El {
    let icons = &cx.app.tools.icons;
    let image_slot = |image: &rune::Image| el().w(cx.pt(18.0)).h(cx.pt(18.0)).pixels(image.pixels()).absolute(cx.pt(20.0), cx.pt(2.0));
    if m.tool == "task" {
        if let Some(image) = icons.agent(m.agent_kind.as_deref()) { return image_slot(image); }
    }
    let mcp = m.mcp.is_some() || m.tool == "execute" || m.tool.starts_with("mcp__");
    let glyph = if mcp {
        if let Some(image) = m.mcp.as_ref().and_then(|v| icons.mcp(v)) { return image_slot(image); }
        "\u{f1e6}"
    } else {
        let tool = if m.tool == "tool_group" { m.title.split_once('(').map_or(m.tool.as_str(), |(t, _)| t) } else { m.tool.as_str() };
        tool_symbol(tool)
    };
    el().w(cx.pt(18.0)).h(cx.pt(22.0)).center().absolute(cx.pt(20.0), 0.0)
        .child(cx.text(glyph, 15.5).color(cx.color(cx.p.muted)).el())
}

pub fn tool_symbol(tool: &str) -> &'static str {
    match tool.to_ascii_lowercase().as_str() {
        "bash" | "shell" | "terminal" | "exec" => "\u{f120}",
        "read" | "read_file" | "readfile" | "view" | "cat" => "\u{2192}",
        "grep" | "glob" | "search" | "find" | "rg" | "tool_group" => "\u{f002}",
        "edit" | "write" | "apply_patch" | "applypatch" | "patch" | "multiedit" => "\u{f044}",
        "webfetch" | "websearch" => "\u{f0ac}",
        "task" => "\u{f0e8}",
        "todowrite" | "todoread" => "\u{f0ae}",
        _ => "\u{f013}",
    }
}

/// Up to twelve wrapped output rows at 58 pt (indented lines at 76 pt, muted), then
/// `... +N lines`. Height includes the 26 pt header gap.
fn output(cx: &Cx, m: &HistoryMessage) -> (El, f32) {
    let body = if m.detail.trim().is_empty() { m.text.as_str() } else { m.detail.as_str() };
    let size = cx.pt(13.0);
    let wrap_w = (cx.w - cx.pt(100.0)).max(cx.pt(40.0));
    let mut rows: Vec<(String, bool)> = Vec::new();
    let limited: String = body.split_inclusive('\n').take(BODY_MAX_ROWS).collect();
    for line in limited.lines() {
        if rows.len() >= BODY_MAX_ROWS { break; }
        let nested = line.starts_with("  ");
        let line = if nested { line.trim() } else { line.trim_end() };
        for part in text::wrap(line, wrap_w, size, BODY_MAX_ROWS - rows.len()) {
            rows.push((part, nested));
        }
    }
    if rows.is_empty() { rows.push((String::new(), false)); }
    let total_lines = body.lines().take(BODY_MAX_ROWS + 1).count().max(1);
    let extra = total_lines.saturating_sub(BODY_MAX_ROWS);
    let mut out = el().w(cx.w).h(0.0).absolute(0.0, 0.0);
    let mut y = cx.pt(26.0);
    for (line, nested) in &rows {
        let (x, color) = if *nested { (cx.pt(76.0), cx.p.muted) } else { (cx.pt(58.0), cx.p.fg) };
        out = out.child(Text::new(line.clone(), size, cx.app).color(cx.color(color)).at(x, y));
        y += cx.pt(BODY_ROW_H);
    }
    if extra > 0 {
        out = out.child(Text::new(format!("... +{extra} lines"), size, cx.app).color(cx.color(cx.p.fg)).at(cx.pt(58.0), y));
    }
    (out, cx.pt(28.0) + (rows.len() + usize::from(extra > 0)) as f32 * cx.pt(BODY_ROW_H))
}

/// A read group's children (`key\tlabel` rows are selectable and preview their output).
fn group_children(cx: &Cx, m: &HistoryMessage) -> (El, f32) {
    let size = cx.pt(13.0);
    let row_h = cx.pt(BODY_ROW_H);
    let body_x = cx.pt(58.0);
    let previews: std::collections::HashMap<&str, &str> = m.detail.lines().filter_map(|l| l.split_once('\t')).collect();
    let selected = cx.app.tools.selected_child.get(&m.id).map(String::as_str);
    let max_chars = ((cx.w / cx.pt(8.0)).floor().max(18.0)) as usize;
    let mut out = el().w(cx.w).h(0.0).absolute(0.0, 0.0);
    let mut y = cx.pt(GROUP_BODY_Y);
    let mut rows = 0;
    for line in m.text.lines() {
        rows += 1;
        let key = line.split_once('\t').map(|(k, _)| k);
        let label = line.split_once('\t').map_or(line, |(_, l)| l);
        let label: String = if label.chars().count() > max_chars {
            label.chars().take(max_chars.saturating_sub(1)).chain(['\u{2026}']).collect()
        } else {
            label.to_owned()
        };
        let is_selected = key.is_some() && key == selected;
        if let Some(key) = key {
            let (group, child) = (m.id.clone(), key.to_owned());
            let mut hit = el().w((cx.w - cx.pt(70.0)).max(cx.pt(40.0))).h(row_h).radius(cx.pt(7.0))
                .absolute(body_x - cx.pt(8.0), y - cx.pt(4.0))
                .on_press(move |a: &mut App| a.tools.toggle_child(&group, &child));
            if is_selected { hit = hit.fill(cx.color(cx.p.accent).alpha(0.16)); }
            out = out.child(hit);
        }
        out = out.child(Text::new(label, size, cx.app).color(cx.color(cx.p.muted)).at(body_x, y));
        y += row_h;
        if is_selected {
            let preview = key.and_then(|k| previews.get(k)).copied().unwrap_or("No preview available");
            let preview_size = cx.pt(12.5);
            for part in text::wrap(preview, (cx.w - cx.pt(96.0)).max(cx.pt(80.0)), preview_size, 4) {
                out = out.child(Text::new(part, preview_size, cx.app).color(cx.color(cx.p.fg)).at(body_x + cx.pt(18.0), y));
                y += row_h;
                rows += 1;
            }
        }
    }
    (out, cx.pt(GROUP_BODY_Y + 2.0) + rows.max(1) as f32 * row_h)
}
