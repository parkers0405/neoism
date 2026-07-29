/// Wave 7G: the "who's here" roster — one colored dot (peer initial)
/// per remote collaborator on THIS document, tucked into the pane's
/// top-right corner like the scrollbar. Hovering a dot shows the
/// collaborator's display name beside the row; clicking it scrolls to
/// their cursor line (hit rects registered here, consumed by
/// `MarkdownPane::roster_jump_at` via the desktop mouse bridge).
// Kept for reference: the "who's here" roster moved to the top-chrome peer
// cluster, so this in-document dot column is no longer drawn.
#[allow(dead_code)]
fn draw_markdown_roster(
    sugarloaf: &mut Sugarloaf,
    pane: &mut MarkdownPane,
    rect: [f32; 4],
    theme: &IdeTheme,
    mouse: Option<[f32; 2]>,
    clip: [f32; 4],
    font_scale: f32,
) {
    use crate::editor::markdown::roster::{
        markdown_roster_dot_rects, markdown_roster_entries, ROSTER_DOT_DIAMETER,
        ROSTER_DOT_GAP, ROSTER_MARGIN_RIGHT, ROSTER_MARGIN_TOP,
    };

    let entries = markdown_roster_entries(&pane.remote_cursors);
    if entries.is_empty() {
        return;
    }
    let [x, y, w, _h] = rect;
    let dots = markdown_roster_dot_rects(
        entries.len(),
        x + w - ROSTER_MARGIN_RIGHT,
        y + ROSTER_MARGIN_TOP,
        ROSTER_DOT_DIAMETER,
        ROSTER_DOT_GAP,
    );

    let mut hovered: Option<usize> = None;
    for (ix, (entry, dot)) in entries.iter().zip(dots.iter()).enumerate() {
        let [dot_x, dot_y, dot_w, dot_h] = *dot;
        // The peer's deterministic pixel-plasma orb replaces the old
        // colored-circle-with-initial badge — one presence identity
        // everywhere (seed = display name), over a face-pile ring so the
        // pile stays legible, animated on the shared process clock.
        let ring = theme.f32(theme.bg);
        let ring_pad = 1.0;
        draw_rounded_rect_clipped(
            sugarloaf,
            clip,
            dot_x - ring_pad,
            dot_y - ring_pad,
            dot_w + 2.0 * ring_pad,
            dot_h + 2.0 * ring_pad,
            (dot_w + 2.0 * ring_pad) * 0.5,
            ring,
            DEPTH,
            ORDER_TEXT + 3,
        );
        draw_presence_orb_clipped(
            sugarloaf,
            clip,
            &entry.name,
            dot_x,
            dot_y,
            dot_w,
            crate::editor::crdt::presence_orb_now_seconds(),
            DEPTH,
            ORDER_TEXT + 4,
        );
        pane.register_roster_rect(*dot, entry.line);
        if mouse.is_some_and(|[mx, my]| {
            mx >= dot_x && mx <= dot_x + dot_w && my >= dot_y && my <= dot_y + dot_h
        }) {
            hovered = Some(ix);
        }
    }

    // Hovered dot: the display name rides to the LEFT of the roster
    // row (never over the dots), same flag style as the caret tag.
    let Some(ix) = hovered else {
        return;
    };
    let entry = &entries[ix];
    let name = entry.name.trim();
    if name.is_empty() {
        return;
    }
    let name_opts = DrawOpts {
        font_size: (markdown_font(16.0, font_scale) * 0.58).max(8.0),
        color: theme.u8(theme.bg),
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let name_w = sugarloaf.text_mut().measure(name, &name_opts);
    let tag_h = name_opts.font_size + 6.0;
    let row_left = dots.first().map(|dot| dot[0]).unwrap_or(x + w);
    let tag_x = row_left - name_w - 16.0;
    let tag_y = y + ROSTER_MARGIN_TOP + (ROSTER_DOT_DIAMETER - tag_h) * 0.5;
    let color = roster_entry_color(entry);
    draw_rounded_rect_clipped(
        sugarloaf,
        clip,
        tag_x,
        tag_y,
        name_w + 10.0,
        tag_h,
        3.0,
        color,
        DEPTH,
        ORDER_TEXT + 3,
    );
    sugarloaf
        .text_mut()
        .draw(tag_x + 5.0, tag_y + 3.0, name, &name_opts);
}

/// Dot/flag fill for a roster entry: the peer's broadcast color, or
/// the live rainbow color for rainbow-preset peers.
#[allow(dead_code)]
fn roster_entry_color(
    entry: &crate::editor::markdown::roster::MarkdownRosterEntry,
) -> [f32; 4] {
    if entry.rainbow {
        let c = crate::cursor_style::rainbow_color_f32(
            crate::cursor_style::rainbow_now_seconds(),
        );
        [c[0], c[1], c[2], 0.95]
    } else {
        [
            entry.color[0] as f32 / 255.0,
            entry.color[1] as f32 / 255.0,
            entry.color[2] as f32 / 255.0,
            0.95,
        ]
    }
}
