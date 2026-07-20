// Card / header / tab strip painter for the Zed-style Extensions panel.
// Geometry constants are exposed so 1.3 (interaction) and 1.4 (tab
// integration) can hit-test against the same numbers without re-deriving
// layout.

use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::primitives::geom::intersect_rect;
use crate::primitives::ide_theme::IdeTheme;
use crate::primitives::{draw_text_with_occlusion, truncate_to_fit};

use super::state::{
    ExtensionStatus, ExtensionTab, NeoismExtensionsPane, RowAction, RowHit,
};

pub(crate) const HEADER_HEIGHT: f32 = 56.0;
pub(crate) const SEARCH_HEIGHT: f32 = 36.0;
pub(crate) const TAB_STRIP_HEIGHT: f32 = 32.0;
pub(crate) const CARD_HEIGHT: f32 = 92.0;
pub(crate) const CARD_GAP: f32 = 6.0;
pub(crate) const HORIZONTAL_PAD: f32 = 24.0;

const DEPTH: f32 = 0.0;
// All paint orders kept BELOW `panels::status_line::ORDER_BG` (16) so the
// status bar's own background covers anything our panel emits beneath it
// — same convention the markdown pane follows (its orders sit in 3–8).
const ORDER_BG: u8 = 3;
const ORDER_CARD: u8 = 4;
const ORDER_CHIP: u8 = 5;
const ORDER_BUTTON: u8 = 5;
const ORDER_PROGRESS: u8 = 6;

const CARD_RADIUS: f32 = 8.0;
const SEARCH_RADIUS: f32 = 8.0;
const BUTTON_RADIUS: f32 = 6.0;
const CHIP_RADIUS: f32 = 4.0;

const BUTTON_W: f32 = 100.0;
const BUTTON_H: f32 = 32.0;

// Tab strip labels in display order. We only ship MCP and LSP — no
// themes, snippets, etc. that we don't actually install through this
// pipeline.
const TAB_ORDER: &[(ExtensionTab, &str)] = &[
    (ExtensionTab::McpServers, "MCP Servers"),
    (ExtensionTab::LanguageServers, "Language Servers"),
    (ExtensionTab::Formatters, "Formatters"),
    (ExtensionTab::Linters, "Linters"),
    (ExtensionTab::TreeSitterParsers, "Syntax Parsers"),
    (ExtensionTab::Kernels, "Kernels"),
];

const FILTER_LABELS: [&str; 3] = ["All", "Installed", "Not Installed"];

pub(crate) fn render(
    pane: &mut NeoismExtensionsPane,
    sugarloaf: &mut Sugarloaf,
    rect: [f32; 4],
    theme: &IdeTheme,
    scale: f32,
    mouse: Option<(f32, f32)>,
    occlusion_rects: &[[f32; 4]],
) {
    let [x, y, w, h] = rect;
    if w <= 8.0 || h <= 8.0 {
        return;
    }

    pane.row_hits.clear();
    pane.tab_pill_rects.clear();
    pane.filter_pill_rects = [[0.0; 4]; 3];
    pane.search_input_rect = [0.0; 4];

    let s = scale.clamp(0.75, 2.0);
    let clip = Some(rect);

    sugarloaf.rect(None, x, y, w, h, theme.f32(theme.bg), DEPTH, ORDER_BG);

    let pad_x = HORIZONTAL_PAD * s;
    let content_x = x + pad_x;
    let content_w = (w - pad_x * 2.0).max(160.0);
    let mut cursor_y = y + 18.0 * s;

    cursor_y = draw_header(
        pane,
        sugarloaf,
        content_x,
        cursor_y,
        content_w,
        theme,
        s,
        mouse,
        clip,
        occlusion_rects,
    );
    cursor_y += 8.0 * s;

    cursor_y = draw_search_and_filters(
        pane,
        sugarloaf,
        content_x,
        cursor_y,
        content_w,
        theme,
        s,
        mouse,
        clip,
        occlusion_rects,
    );
    cursor_y += 10.0 * s;

    cursor_y = draw_tab_strip(
        pane,
        sugarloaf,
        content_x,
        cursor_y,
        content_w,
        theme,
        s,
        clip,
        occlusion_rects,
    );
    cursor_y += 10.0 * s;

    // Card list region. Cards live below the chrome; their clip rect is
    // the remaining viewport so off-screen cards don't paint text into
    // the header.
    let list_top = cursor_y;
    let list_bottom = y + h;
    let list_clip = Some([x, list_top, w, (list_bottom - list_top).max(0.0)]);

    draw_card_list(
        pane,
        sugarloaf,
        content_x,
        list_top,
        content_w,
        list_bottom,
        theme,
        s,
        mouse,
        list_clip,
        occlusion_rects,
    );

    // Language dropdown — painted LAST so it floats above the card
    // list and the rest of the chrome. Geometry uses the trigger rect
    // captured during `draw_tab_strip`; we just need to position it
    // below the trigger.
    if pane.language_picker_open() {
        draw_language_dropdown(pane, sugarloaf, theme, s, mouse, clip, occlusion_rects);
    } else {
        pane.language_option_rects.clear();
    }
}

fn draw_language_dropdown(
    pane: &mut NeoismExtensionsPane,
    sugarloaf: &mut Sugarloaf,
    theme: &IdeTheme,
    s: f32,
    mouse: Option<(f32, f32)>,
    clip: Option<[f32; 4]>,
    occlusion_rects: &[[f32; 4]],
) {
    let trigger = pane.language_trigger_rect;
    if trigger[2] <= 0.0 || trigger[3] <= 0.0 {
        return;
    }
    let row_h = 24.0 * s;
    let pad_x = 12.0 * s;
    let search_h = 28.0 * s;
    let pad_y = 6.0 * s;
    let max_visible_rows = 9_usize;

    let item_opts = DrawOpts {
        font_size: 12.0 * s,
        color: theme.u8(theme.fg),
        clip_rect: clip,
        ..DrawOpts::default()
    };
    let muted_opts = DrawOpts {
        font_size: 12.0 * s,
        color: theme.u8(theme.dim),
        clip_rect: clip,
        ..DrawOpts::default()
    };
    let placeholder_opts = DrawOpts {
        font_size: 12.0 * s,
        color: theme.u8(theme.muted),
        clip_rect: clip,
        ..DrawOpts::default()
    };
    let check_opts = DrawOpts {
        font_size: 11.0 * s,
        color: theme.u8(theme.accent),
        clip_rect: clip,
        ..DrawOpts::default()
    };
    let search_glyph_opts = DrawOpts {
        font_size: 12.0 * s,
        color: theme.u8(theme.muted),
        clip_rect: clip,
        ..DrawOpts::default()
    };

    // Filter the option list with the search query each frame — cheap
    // (max ~50 langs) and avoids stale state if the user typed.
    let options: Vec<(String, String)> = pane
        .filtered_language_options()
        .into_iter()
        .map(|lang| {
            if lang.is_empty() {
                (String::new(), "All languages".to_string())
            } else {
                (lang.clone(), lang)
            }
        })
        .collect();

    // Width: enough for the widest visible row + the leading checkmark
    // gutter, clamped so the panel doesn't grow ridiculous on
    // small-but-many-langs tabs.
    let mut max_label_w = 0.0_f32;
    for (_, label) in &options {
        let w = sugarloaf.text_mut().measure(label, &item_opts);
        if w > max_label_w {
            max_label_w = w;
        }
    }
    let panel_w = (max_label_w + pad_x * 2.0 + 18.0 * s)
        .max(trigger[2])
        .max(220.0 * s)
        .min(320.0 * s);

    // Height: fixed search bar + N visible rows (capped). The actual
    // option list might be longer; the rest is reachable via wheel.
    let visible_rows = options.len().min(max_visible_rows);
    let list_h = (visible_rows as f32) * row_h;
    let panel_h = search_h + pad_y * 2.0 + list_h + pad_y;
    let panel_x = (trigger[0] + trigger[2] - panel_w).max(8.0 * s);
    let panel_y = trigger[1] + trigger[3] + 4.0 * s;
    let panel_rect = [panel_x, panel_y, panel_w, panel_h];
    pane.language_panel_rect = panel_rect;

    // Opaque background + outline so the cards underneath don't bleed
    // through. We don't use the modal pipeline; this is just a
    // sugarloaf-clipped overlay layered on top.
    draw_rounded_rect_clipped(
        sugarloaf,
        panel_rect,
        theme.f32(theme.bg),
        SEARCH_RADIUS * s,
        ORDER_PROGRESS,
        panel_rect,
    );
    paint_outline_clipped(
        sugarloaf,
        panel_rect,
        theme.f32(theme.border),
        s,
        panel_rect,
    );

    // Search input row at the top.
    let search_rect = [
        panel_x + pad_y,
        panel_y + pad_y,
        panel_w - pad_y * 2.0,
        search_h,
    ];
    pane.language_search_rect = search_rect;
    draw_rounded_rect_clipped(
        sugarloaf,
        search_rect,
        theme.f32(theme.surface),
        SEARCH_RADIUS * s,
        ORDER_PROGRESS,
        panel_rect,
    );
    // Accent ring when the dropdown's mini-search owns focus (it auto-
    // focuses on open), matching the main search box's focus cue.
    let lang_focused = pane.language_search_focused;
    paint_outline_clipped(
        sugarloaf,
        search_rect,
        if lang_focused {
            theme.f32(theme.accent)
        } else {
            theme.f32(theme.border)
        },
        s,
        panel_rect,
    );
    // Magnifier glyph (nerd-font fa-search).
    draw_text_with_occlusion(
        sugarloaf,
        search_rect[0] + 8.0 * s,
        search_rect[1] + (search_h - 12.0 * s) * 0.5,
        "\u{f002}",
        &search_glyph_opts,
        occlusion_rects,
    );
    let search_text_x = search_rect[0] + 8.0 * s + 16.0 * s;
    let search_text_y = search_rect[1] + (search_h - 12.0 * s) * 0.5;
    let mut lang_caret_x = search_text_x;
    if pane.language_search_query.is_empty() {
        draw_text_with_occlusion(
            sugarloaf,
            search_text_x,
            search_text_y,
            "Filter languages\u{2026}",
            &placeholder_opts,
            occlusion_rects,
        );
    } else {
        let query_w = sugarloaf
            .text_mut()
            .measure(&pane.language_search_query, &item_opts);
        draw_text_with_occlusion(
            sugarloaf,
            search_text_x,
            search_text_y,
            &pane.language_search_query,
            &item_opts,
            occlusion_rects,
        );
        lang_caret_x = search_text_x + query_w;
    }
    if lang_focused {
        let caret_w = (1.5 * s).max(1.0);
        let caret_h = 14.0 * s;
        let caret_y = search_rect[1] + (search_h - caret_h) * 0.5;
        let caret_max_x = search_rect[0] + search_rect[2] - caret_w - 4.0 * s;
        draw_rounded_rect_clipped(
            sugarloaf,
            [lang_caret_x.min(caret_max_x), caret_y, caret_w, caret_h],
            theme.f32(theme.accent),
            0.0,
            ORDER_PROGRESS + 1,
            panel_rect,
        );
    }

    // Option list region.
    let list_top = search_rect[1] + search_h + pad_y;
    let list_bottom = list_top + list_h;
    let list_clip = match crate::primitives::geom::intersect_rect(
        panel_rect,
        [panel_rect[0], list_top, panel_rect[2], list_h],
    ) {
        Some(r) => r,
        None => panel_rect,
    };

    // Row text needs its OWN DrawOpts.clip_rect = list_clip (not the
    // panel's outer clip) so scrolled partial rows don't bleed text up
    // into the search bar / chrome above. We clone the base opts and
    // swap the clip_rect.
    let row_item_opts = DrawOpts {
        clip_rect: Some(list_clip),
        ..item_opts
    };
    let row_muted_opts = DrawOpts {
        clip_rect: Some(list_clip),
        ..muted_opts
    };
    let row_check_opts = DrawOpts {
        clip_rect: Some(list_clip),
        ..check_opts
    };

    let content_h = (options.len() as f32) * row_h;
    let max_scroll = (content_h - list_h).max(0.0);
    pane.clamp_language_scroll(max_scroll);
    let scroll = pane.language_scroll_top;

    pane.language_option_rects.clear();
    let selected = pane.selected_language().map(|s| s.to_string());
    let mut cursor_y = list_top - scroll;
    for (lang_key, label) in &options {
        let row_rect = [panel_x, cursor_y, panel_w, row_h];
        // Skip rows fully off-screen (still register the rect for hit
        // testing when even partly visible so the user can click them).
        let visible = cursor_y + row_h > list_top - 0.5 && cursor_y < list_bottom + 0.5;
        if !visible {
            cursor_y += row_h;
            continue;
        }
        let hovered = mouse.is_some_and(|(mx, my)| point_in_rect(mx, my, row_rect));
        if hovered {
            draw_rect_clipped_local(
                sugarloaf,
                row_rect,
                theme.f32(theme.hover),
                ORDER_PROGRESS,
                list_clip,
            );
        }
        let is_selected = match (&selected, lang_key.as_str()) {
            (None, "") => true,
            (Some(cur), other) if !other.is_empty() => cur.eq_ignore_ascii_case(other),
            _ => false,
        };
        if is_selected {
            draw_text_with_occlusion(
                sugarloaf,
                panel_x + pad_x - 4.0 * s,
                cursor_y + (row_h - 11.0 * s) * 0.5,
                "\u{f00c}",
                &row_check_opts,
                occlusion_rects,
            );
        }
        let opts = if lang_key.is_empty() {
            &row_muted_opts
        } else {
            &row_item_opts
        };
        draw_text_with_occlusion(
            sugarloaf,
            panel_x + pad_x + 14.0 * s,
            cursor_y + (row_h - 12.0 * s) * 0.5,
            label,
            opts,
            occlusion_rects,
        );

        // Hit-rect for clicks: intersect with the visible list region
        // so a row scrolled half off-screen can't be clicked on its
        // hidden portion (which sits behind the search bar — clicking
        // there should focus the search input, not select the row).
        let hit_rect = crate::primitives::geom::intersect_rect(row_rect, list_clip)
            .unwrap_or(row_rect);
        pane.language_option_rects
            .push((hit_rect, lang_key.clone()));
        cursor_y += row_h;
    }

    // Scroll indicator hairline — thin track on the right edge of the
    // list region with a thumb whose height/position mirrors the
    // viewport-to-content ratio. No drag wiring yet; wheel only.
    if max_scroll > 0.0 {
        let style = crate::primitives::look::scrollbar_style();
        let track_w = style.width_or(3.0) * s;
        let track_x = panel_x + panel_w - track_w - 3.0 * s;
        let track_rect = [track_x, list_top, track_w, list_h];
        let radius = style.radius(track_w, 0.5);
        // This site draws a themed track by default; overrides keep
        // its 0.5 alpha so the hairline stays subtle.
        if let Some(track_color) =
            style.track_or(Some(theme.f32_alpha(theme.border, 0.5)))
        {
            draw_rounded_rect_clipped(
                sugarloaf,
                track_rect,
                track_color,
                radius,
                ORDER_PROGRESS,
                panel_rect,
            );
        }
        let thumb_h = (list_h * list_h / content_h).max(style.min_thumb_or(16.0) * s);
        let thumb_y = list_top + (scroll / max_scroll) * (list_h - thumb_h).max(0.0);
        draw_rounded_rect_clipped(
            sugarloaf,
            [track_x, thumb_y, track_w, thumb_h],
            style.thumb_or(theme.f32(theme.muted)),
            radius,
            ORDER_PROGRESS,
            panel_rect,
        );
    }
}

fn draw_rect_clipped_local(
    sugarloaf: &mut Sugarloaf,
    rect: [f32; 4],
    color: [f32; 4],
    order: u8,
    clip: [f32; 4],
) {
    let Some([x, y, w, h]) = intersect_rect(rect, clip) else {
        return;
    };
    sugarloaf.rect(None, x, y, w, h, color, DEPTH, order);
}

fn draw_header(
    _pane: &mut NeoismExtensionsPane,
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    _w: f32,
    theme: &IdeTheme,
    s: f32,
    _mouse: Option<(f32, f32)>,
    clip: Option<[f32; 4]>,
    occlusion_rects: &[[f32; 4]],
) -> f32 {
    let row_h = HEADER_HEIGHT * s;
    let title_opts = DrawOpts {
        font_size: 22.0 * s,
        color: theme.u8(theme.fg),
        bold: true,
        clip_rect: clip,
        ..DrawOpts::default()
    };
    draw_text_with_occlusion(
        sugarloaf,
        x,
        y + (row_h - 22.0 * s) * 0.5,
        "Extensions",
        &title_opts,
        occlusion_rects,
    );

    y + row_h
}

fn draw_search_and_filters(
    pane: &mut NeoismExtensionsPane,
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    w: f32,
    theme: &IdeTheme,
    s: f32,
    mouse: Option<(f32, f32)>,
    clip: Option<[f32; 4]>,
    occlusion_rects: &[[f32; 4]],
) -> f32 {
    let row_h = SEARCH_HEIGHT * s;

    // Filter pills hug the right edge; search input takes the remaining
    // space minus a small gutter.
    let pill_opts = DrawOpts {
        font_size: 12.0 * s,
        color: theme.u8(theme.fg),
        clip_rect: clip,
        ..DrawOpts::default()
    };
    let mut pill_widths = [0.0_f32; 3];
    for (i, label) in FILTER_LABELS.iter().enumerate() {
        pill_widths[i] = sugarloaf.text_mut().measure(label, &pill_opts) + 18.0 * s;
    }
    let pill_gap = 6.0 * s;
    let pills_total: f32 = pill_widths.iter().sum::<f32>() + pill_gap * 2.0;
    let pills_x = x + w - pills_total;
    let gutter = 12.0 * s;
    let search_w = (pills_x - gutter - x).max(160.0);

    // Search input.
    let search_rect = [x, y, search_w, row_h];
    pane.search_input_rect = search_rect;
    sugarloaf.rounded_rect(
        None,
        x,
        y,
        search_w,
        row_h,
        theme.f32(theme.surface),
        DEPTH,
        SEARCH_RADIUS * s,
        ORDER_CARD,
    );
    // Focus ring: accent outline when the search box owns keyboard
    // focus, plain border otherwise — the visible "cursor is here" cue
    // the user expects when focus jumps to search (auto-focus / `/` /
    // Cmd+F).
    let focused = pane.search_focused();
    let outline_color = if focused {
        theme.f32(theme.accent)
    } else {
        theme.f32(theme.border)
    };
    paint_outline(sugarloaf, search_rect, outline_color, s);

    let glyph_opts = DrawOpts {
        font_size: 13.0 * s,
        color: theme.u8(theme.muted),
        clip_rect: clip,
        ..DrawOpts::default()
    };
    let glyph_x = x + 10.0 * s;
    draw_text_with_occlusion(
        sugarloaf,
        glyph_x,
        y + (row_h - 13.0 * s) * 0.5,
        "\u{f002}",
        &glyph_opts,
        occlusion_rects,
    );

    let text_x = x + 30.0 * s;
    let text_y = y + (row_h - 13.0 * s) * 0.5;
    let text_max = (search_w - 40.0 * s).max(40.0);
    // Caret tracks the end of the rendered query (start of box when
    // empty) so it visibly "moves with focus" as the user types.
    let mut caret_x = text_x;
    if pane.search_query.is_empty() {
        let placeholder_opts = DrawOpts {
            font_size: 13.0 * s,
            color: theme.u8(theme.muted),
            clip_rect: clip,
            ..DrawOpts::default()
        };
        let placeholder = truncate_to_fit(
            "Search extensions\u{2026}",
            text_max,
            sugarloaf,
            &placeholder_opts,
        );
        draw_text_with_occlusion(
            sugarloaf,
            text_x,
            text_y,
            &placeholder,
            &placeholder_opts,
            occlusion_rects,
        );
    } else {
        let query_opts = DrawOpts {
            font_size: 13.0 * s,
            color: theme.u8(theme.fg),
            clip_rect: clip,
            ..DrawOpts::default()
        };
        let query = truncate_to_fit(&pane.search_query, text_max, sugarloaf, &query_opts);
        let query_w = sugarloaf.text_mut().measure(&query, &query_opts);
        draw_text_with_occlusion(
            sugarloaf,
            text_x,
            text_y,
            &query,
            &query_opts,
            occlusion_rects,
        );
        caret_x = text_x + query_w;
    }

    // Static caret (no blink clock on this page) — a thin accent bar at
    // the insertion point whenever the search box is focused. Clamped
    // inside the box so a full query can't push it past the edge.
    if focused {
        let caret_w = (1.5 * s).max(1.0);
        let caret_h = 16.0 * s;
        let caret_y = y + (row_h - caret_h) * 0.5;
        let caret_max_x = x + search_w - caret_w - 2.0 * s;
        sugarloaf.rect(
            None,
            caret_x.min(caret_max_x),
            caret_y,
            caret_w,
            caret_h,
            theme.f32(theme.accent),
            DEPTH,
            ORDER_PROGRESS,
        );
    }

    // Filter pills. Active is filled in accent, inactive outlined.
    let mut pill_x = pills_x;
    for (i, label) in FILTER_LABELS.iter().enumerate() {
        let pw = pill_widths[i];
        let pill_rect = [pill_x, y, pw, row_h];
        pane.filter_pill_rects[i] = pill_rect;

        let is_active = filter_index(pane.filter) == i;
        let hovered = mouse.is_some_and(|(mx, my)| point_in_rect(mx, my, pill_rect));
        let fill = if is_active {
            theme.f32(theme.accent)
        } else if hovered {
            theme.f32(theme.hover)
        } else {
            theme.f32(theme.surface)
        };
        sugarloaf.rounded_rect(
            None,
            pill_x,
            y,
            pw,
            row_h,
            fill,
            DEPTH,
            BUTTON_RADIUS * s,
            ORDER_CARD,
        );
        if !is_active {
            paint_outline(sugarloaf, pill_rect, theme.f32(theme.border), s);
        }

        let label_color = if is_active {
            theme.u8(theme.bg)
        } else {
            theme.u8(theme.dim)
        };
        let label_opts = DrawOpts {
            font_size: 12.0 * s,
            color: label_color,
            clip_rect: clip,
            ..DrawOpts::default()
        };
        let label_w = sugarloaf.text_mut().measure(label, &label_opts);
        draw_text_with_occlusion(
            sugarloaf,
            pill_x + (pw - label_w) * 0.5,
            y + (row_h - 12.0 * s) * 0.5,
            label,
            &label_opts,
            occlusion_rects,
        );

        pill_x += pw + pill_gap;
    }

    y + row_h
}

fn draw_tab_strip(
    pane: &mut NeoismExtensionsPane,
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    w: f32,
    theme: &IdeTheme,
    s: f32,
    clip: Option<[f32; 4]>,
    occlusion_rects: &[[f32; 4]],
) -> f32 {
    let row_h = TAB_STRIP_HEIGHT * s;
    let gap = 24.0 * s;
    let underline_h = 2.0 * s;
    let base_opts = DrawOpts {
        font_size: 13.0 * s,
        color: theme.u8(theme.muted),
        clip_rect: clip,
        ..DrawOpts::default()
    };
    let active_opts = DrawOpts {
        font_size: 13.0 * s,
        color: theme.u8(theme.fg),
        bold: true,
        clip_rect: clip,
        ..DrawOpts::default()
    };

    // Language trigger sits at the RIGHT of the tab row when the
    // active tab supports per-language filtering. We lay it out first
    // so the tabs know how much horizontal space they can consume
    // without overlapping it.
    let lang_pill_w = if pane.tab_supports_language_filter() {
        let label = lang_pill_label(pane);
        let label_w = sugarloaf
            .text_mut()
            .measure(&label, &base_opts)
            .max(40.0 * s);
        let chevron_w = sugarloaf.text_mut().measure("\u{f078}", &base_opts);
        let pad_x = 10.0 * s;
        let gap_chev = 6.0 * s;
        label_w + chevron_w + gap_chev + pad_x * 2.0
    } else {
        pane.language_trigger_rect = [0.0; 4];
        0.0
    };
    let tab_avail = (w - lang_pill_w - 12.0 * s).max(0.0);

    let mut tab_x = x;
    let row_y = y;
    let text_y = row_y + (row_h - 13.0 * s) * 0.5;

    for (tab, label) in TAB_ORDER {
        let is_active = pane.active_tab == *tab;
        let opts = if is_active { &active_opts } else { &base_opts };
        let label_w = sugarloaf.text_mut().measure(label, opts);

        // Stop laying out tabs that would spill past the container; the
        // right-edge gutter would clip them mid-glyph and 1.3 has no
        // overflow scroller yet. `tab_avail` reserves room for the
        // language trigger on the right when it's visible.
        if tab_x + label_w > x + tab_avail {
            break;
        }

        let pill_rect = [tab_x, row_y, label_w, row_h];
        pane.tab_pill_rects.push((pill_rect, *tab));

        draw_text_with_occlusion(sugarloaf, tab_x, text_y, label, opts, occlusion_rects);
        if is_active {
            sugarloaf.rect(
                None,
                tab_x,
                row_y + row_h - underline_h,
                label_w,
                underline_h,
                theme.f32(theme.accent),
                DEPTH,
                ORDER_CARD,
            );
        }
        tab_x += label_w + gap;
    }

    // Language trigger. Always right-aligned to the tab row so users
    // never have to hunt for it when the tab list grows.
    if pane.tab_supports_language_filter() && lang_pill_w > 0.0 {
        let pill_x = x + w - lang_pill_w;
        let pill_h = (row_h - 8.0 * s).max(18.0 * s);
        let pill_y = row_y + (row_h - pill_h) * 0.5;
        let pill_rect = [pill_x, pill_y, lang_pill_w, pill_h];
        pane.language_trigger_rect = pill_rect;

        let hovered = false; // hover refresh on next frame; cheap to skip
        let fill = if pane.language_picker_open() {
            theme.f32(theme.accent)
        } else if hovered {
            theme.f32(theme.hover)
        } else {
            theme.f32(theme.surface)
        };
        draw_rounded_rect_clipped(
            sugarloaf,
            pill_rect,
            fill,
            BUTTON_RADIUS * s,
            ORDER_CARD,
            list_clip_for(clip, pill_rect),
        );
        paint_outline_clipped(
            sugarloaf,
            pill_rect,
            theme.f32(theme.border),
            s,
            list_clip_for(clip, pill_rect),
        );

        let label_text = lang_pill_label(pane);
        let label_color = if pane.language_picker_open() {
            theme.u8(theme.bg)
        } else {
            theme.u8(theme.fg)
        };
        let label_opts = DrawOpts {
            font_size: 12.0 * s,
            color: label_color,
            clip_rect: clip,
            ..DrawOpts::default()
        };
        let chev_opts = DrawOpts {
            font_size: 10.0 * s,
            color: label_color,
            clip_rect: clip,
            ..DrawOpts::default()
        };
        let label_w = sugarloaf.text_mut().measure(&label_text, &label_opts);
        let chev_w = sugarloaf.text_mut().measure("\u{f078}", &chev_opts);
        let pad_x = 10.0 * s;
        draw_text_with_occlusion(
            sugarloaf,
            pill_x + pad_x,
            pill_y + (pill_h - 12.0 * s) * 0.5,
            &label_text,
            &label_opts,
            occlusion_rects,
        );
        draw_text_with_occlusion(
            sugarloaf,
            pill_x + lang_pill_w - pad_x - chev_w,
            pill_y + (pill_h - 10.0 * s) * 0.5 + 1.0 * s,
            "\u{f078}",
            &chev_opts,
            occlusion_rects,
        );
        let _ = label_w;
    }

    // Faint separator under the strip; mirrors Zed's hairline.
    sugarloaf.rect(
        None,
        x,
        row_y + row_h,
        w,
        1.0_f32.max(s),
        theme.f32_alpha(theme.border, 0.7),
        DEPTH,
        ORDER_CARD,
    );

    row_y + row_h
}

fn lang_pill_label(pane: &NeoismExtensionsPane) -> String {
    match pane.selected_language() {
        Some(lang) => lang.to_string(),
        None => "All languages".to_string(),
    }
}

/// Tighten an optional outer clip to a rect — used so painting the
/// language trigger never bleeds past its own pill bounds.
fn list_clip_for(outer: Option<[f32; 4]>, rect: [f32; 4]) -> [f32; 4] {
    match outer.and_then(|o| intersect_rect(o, rect)) {
        Some(c) => c,
        None => rect,
    }
}

fn draw_card_list(
    pane: &mut NeoismExtensionsPane,
    sugarloaf: &mut Sugarloaf,
    x: f32,
    list_top: f32,
    w: f32,
    list_bottom: f32,
    theme: &IdeTheme,
    s: f32,
    mouse: Option<(f32, f32)>,
    list_clip: Option<[f32; 4]>,
    occlusion_rects: &[[f32; 4]],
) {
    let card_h = CARD_HEIGHT * s;
    let gap = CARD_GAP * s;
    let row_advance = card_h + gap;
    let mut card_y = list_top - pane.scroll_top;

    let visible_indices = pane.visible_entries();
    // visible_position is the on-screen row (used for selection
    // highlight + hit Focus payload). entry_idx points back into
    // `pane.entries` for content lookup.
    for (visible_position, &entry_idx) in visible_indices.iter().enumerate() {
        let entry = &pane.entries[entry_idx];
        let card_top = card_y;
        let card_bottom = card_top + card_h;

        // Cache focus hit even when off-screen so keyboard nav can
        // find the row geometry; only skip the paint pass.
        let focus_rect = [x, card_top, w, card_h];
        pane.row_hits.push(RowHit {
            rect: focus_rect,
            action: RowAction::Focus(visible_position),
        });

        if card_bottom < list_top || card_top > list_bottom {
            card_y += row_advance;
            continue;
        }

        let is_selected = pane.selected_index == visible_position;
        let card_fill = if is_selected {
            theme.f32(theme.hover)
        } else {
            theme.f32(theme.surface)
        };
        // Clip rect for the card list region. The card's fill,
        // outline, and inner widgets all get intersected against this
        // so a card scrolling past the tab strip / search bar gets cut
        // off instead of bleeding into the chrome above.
        let clip_rect = list_clip.unwrap_or([x, list_top, w, list_bottom - list_top]);
        draw_rounded_rect_clipped(
            sugarloaf,
            [x, card_top, w, card_h],
            card_fill,
            CARD_RADIUS * s,
            ORDER_CARD,
            clip_rect,
        );
        paint_outline_clipped(
            sugarloaf,
            [x, card_top, w, card_h],
            theme.f32(theme.border),
            s,
            clip_rect,
        );

        draw_card_body(
            &mut pane.row_hits,
            sugarloaf,
            entry,
            visible_position,
            x,
            card_top,
            w,
            card_h,
            theme,
            s,
            mouse,
            list_clip,
            occlusion_rects,
        );

        card_y += row_advance;
    }

    // Track total content height for scroll clamping. Only visible
    // (post-filter) rows contribute, so the scroll bar tracks what
    // the user can actually scroll through.
    let total = (visible_indices.len() as f32) * row_advance;
    pane.set_content_height(total);
    pane.set_viewport_height(list_bottom - list_top);
    // Keyboard scroll-follow places the selected row in this same
    // scaled space, so hand it the scaled advance we just drew with.
    pane.set_list_row_advance(row_advance);
}

#[allow(clippy::too_many_arguments)]
fn draw_card_body(
    row_hits: &mut Vec<RowHit>,
    sugarloaf: &mut Sugarloaf,
    entry: &super::state::ExtensionEntry,
    idx: usize,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    theme: &IdeTheme,
    s: f32,
    mouse: Option<(f32, f32)>,
    clip: Option<[f32; 4]>,
    occlusion_rects: &[[f32; 4]],
) {
    let pad = 14.0 * s;
    let inner_x = x + pad;
    let button_w = BUTTON_W * s;
    let button_h = BUTTON_H * s;
    let button_x = x + w - pad - button_w;
    let button_y = y + (h - button_h) * 0.5;
    let inner_w = (button_x - 12.0 * s - inner_x).max(60.0);

    // Row 1: name + version + category chips.
    let name_opts = DrawOpts {
        font_size: 15.0 * s,
        color: theme.u8(theme.fg),
        bold: true,
        clip_rect: clip,
        ..DrawOpts::default()
    };
    let version_opts = DrawOpts {
        font_size: 12.0 * s,
        color: theme.u8(theme.dim),
        clip_rect: clip,
        ..DrawOpts::default()
    };
    let chip_label_opts = DrawOpts {
        font_size: 11.0 * s,
        color: theme.u8(theme.dim),
        clip_rect: clip,
        ..DrawOpts::default()
    };
    let row1_y = y + 14.0 * s;
    let name_max = (inner_w * 0.55).max(80.0);
    let name_text = truncate_to_fit(&entry.name, name_max, sugarloaf, &name_opts);
    let name_w = sugarloaf.text_mut().measure(&name_text, &name_opts);
    draw_text_with_occlusion(
        sugarloaf,
        inner_x,
        row1_y,
        &name_text,
        &name_opts,
        occlusion_rects,
    );

    let mut cursor = inner_x + name_w + 8.0 * s;
    if !entry.version.is_empty() {
        let v_text = format!("v{}", entry.version.trim_start_matches('v'));
        draw_text_with_occlusion(
            sugarloaf,
            cursor,
            row1_y + 2.0 * s,
            &v_text,
            &version_opts,
            occlusion_rects,
        );
        cursor += sugarloaf.text_mut().measure(&v_text, &version_opts) + 10.0 * s;
    }

    // Chips. Stop drawing when we'd intrude on the button area; the
    // tail is silently dropped — chip overflow is fine for v1.
    let chip_pad_x = 6.0 * s;
    let chip_pad_y = 2.0 * s;
    let chip_h = 11.0 * s + chip_pad_y * 2.0;
    let chip_y = row1_y - chip_pad_y - 1.0 * s;
    let chip_gap = 6.0 * s;
    let chip_limit_x = button_x - 12.0 * s;
    // Categories first (e.g. "MCP", "LSP"), then language tags
    // (e.g. "Rust", "Python") so the most identifying chip leads.
    let chip_iter = entry.categories.iter().chain(entry.languages.iter());
    let chip_clip = clip.unwrap_or([x, y, w, h]);

    // Leading LSP source badge: where the engine resolves this server's
    // binary. Resolvable sources read green, missing binaries red, and a
    // package without a registered runtime adapter yellow.
    if let Some(source) = entry.lsp_source.as_deref() {
        let badge_color = match source {
            "missing" => theme.red,
            "adapter required" => theme.yellow,
            _ => theme.green,
        };
        let badge_opts = DrawOpts {
            font_size: 11.0 * s,
            color: theme.u8(badge_color),
            clip_rect: clip,
            ..DrawOpts::default()
        };
        let lw = sugarloaf.text_mut().measure(source, &badge_opts);
        let chip_w = lw + chip_pad_x * 2.0;
        if cursor + chip_w <= chip_limit_x {
            draw_rounded_rect_bordered_clipped(
                sugarloaf,
                [cursor, chip_y, chip_w, chip_h],
                theme.f32(theme.surface),
                theme.f32(badge_color),
                CHIP_RADIUS * s,
                ORDER_CHIP,
                s,
                chip_clip,
            );
            draw_text_with_occlusion(
                sugarloaf,
                cursor + chip_pad_x,
                chip_y + chip_pad_y + 0.5 * s,
                source,
                &badge_opts,
                occlusion_rects,
            );
            cursor += chip_w + chip_gap;
        }
    }

    for chip in chip_iter {
        let lw = sugarloaf.text_mut().measure(chip, &chip_label_opts);
        let chip_w = lw + chip_pad_x * 2.0;
        if cursor + chip_w > chip_limit_x {
            break;
        }
        draw_rounded_rect_bordered_clipped(
            sugarloaf,
            [cursor, chip_y, chip_w, chip_h],
            theme.f32(theme.surface),
            theme.f32(theme.border),
            CHIP_RADIUS * s,
            ORDER_CHIP,
            s,
            chip_clip,
        );
        draw_text_with_occlusion(
            sugarloaf,
            cursor + chip_pad_x,
            chip_y + chip_pad_y + 0.5 * s,
            chip,
            &chip_label_opts,
            occlusion_rects,
        );
        cursor += chip_w + chip_gap;
    }

    // Row 2: description.
    let desc_opts = DrawOpts {
        font_size: 12.0 * s,
        color: theme.u8(theme.dim),
        clip_rect: clip,
        ..DrawOpts::default()
    };
    let row2_y = y + 38.0 * s;
    let desc_max = (button_x - 12.0 * s - inner_x).max(60.0);
    let desc_text = truncate_to_fit(&entry.description, desc_max, sugarloaf, &desc_opts);
    draw_text_with_occlusion(
        sugarloaf,
        inner_x,
        row2_y,
        &desc_text,
        &desc_opts,
        occlusion_rects,
    );

    // Row 3/4: downloads under the description, author at the bottom.
    let footer_opts = DrawOpts {
        font_size: 11.0 * s,
        color: theme.u8(theme.muted),
        clip_rect: clip,
        ..DrawOpts::default()
    };
    let downloads_short = entry.downloads.map(format_thousands).unwrap_or_default();

    let author_y = y + h - 18.0 * s;
    let gap = 6.0 * s;

    // LEFT cluster: github icon leading the downloads count.
    let github_glyph = "\u{f09b}";
    let gh_w = sugarloaf.text_mut().measure(github_glyph, &footer_opts);
    draw_text_with_occlusion(
        sugarloaf,
        inner_x,
        author_y,
        github_glyph,
        &footer_opts,
        occlusion_rects,
    );
    if !downloads_short.is_empty() {
        draw_text_with_occlusion(
            sugarloaf,
            inner_x + gh_w + gap,
            author_y,
            &downloads_short,
            &footer_opts,
            occlusion_rects,
        );
    }

    // Overflow "…" menu stays on the far right.
    let ellipsis_glyph = "\u{f141}";
    let ell_w = sugarloaf.text_mut().measure(ellipsis_glyph, &footer_opts);
    let ell_x = (button_x - 12.0 * s - ell_w).max(inner_x);
    draw_text_with_occlusion(
        sugarloaf,
        ell_x,
        author_y,
        ellipsis_glyph,
        &footer_opts,
        occlusion_rects,
    );

    // Install / Uninstall / Installing button.
    let button_rect = [button_x, button_y, button_w, button_h];
    let button_hovered = mouse.is_some_and(|(mx, my)| point_in_rect(mx, my, button_rect));
    paint_install_button(
        sugarloaf,
        button_rect,
        &entry.status,
        theme,
        s,
        button_hovered,
        clip,
        occlusion_rects,
    );
    // Hit-rect for 1.3's click dispatch. Pushed after the row Focus
    // entry so click-resolution can prefer the button before falling
    // back to row-level focus.
    if !matches!(entry.status, ExtensionStatus::BuiltIn) {
        row_hits.push(RowHit {
            rect: button_rect,
            action: RowAction::ToggleInstall(entry.id.clone()),
        });
    }

    let _ = idx;
}

#[allow(clippy::too_many_arguments)]
fn paint_install_button(
    sugarloaf: &mut Sugarloaf,
    rect: [f32; 4],
    status: &ExtensionStatus,
    theme: &IdeTheme,
    s: f32,
    hovered: bool,
    clip: Option<[f32; 4]>,
    occlusion_rects: &[[f32; 4]],
) {
    let [bx, by, bw, bh] = rect;

    match status {
        ExtensionStatus::BuiltIn => {
            let btn_clip = clip.unwrap_or(rect);
            draw_rounded_rect_clipped(
                sugarloaf,
                [bx, by, bw, bh],
                theme.f32(theme.bg),
                BUTTON_RADIUS * s,
                ORDER_BUTTON,
                btn_clip,
            );
            paint_outline_clipped(sugarloaf, rect, theme.f32(theme.border), s, btn_clip);
            let label = install_button_label(status);
            let opts = DrawOpts {
                font_size: 11.0 * s,
                color: theme.u8(theme.dim),
                bold: true,
                clip_rect: clip,
                ..DrawOpts::default()
            };
            let lw = sugarloaf.text_mut().measure(label, &opts);
            draw_text_with_occlusion(
                sugarloaf,
                bx + (bw - lw) * 0.5,
                by + (bh - 11.0 * s) * 0.5,
                label,
                &opts,
                occlusion_rects,
            );
        }
        ExtensionStatus::NotInstalled => {
            // Filled grey button to match the Zed screenshot. We pick
            // theme.hover so the button reads as raised against the
            // surface-colored card without leaning on theme.accent
            // (which would clash with the active filter pill).
            let fill = if hovered {
                theme.f32(theme.border)
            } else {
                theme.f32(theme.hover)
            };
            let btn_clip = clip.unwrap_or(rect);
            draw_rounded_rect_clipped(
                sugarloaf,
                [bx, by, bw, bh],
                fill,
                BUTTON_RADIUS * s,
                ORDER_BUTTON,
                btn_clip,
            );
            paint_outline_clipped(sugarloaf, rect, theme.f32(theme.border), s, btn_clip);
            let label = install_button_label(status);
            let opts = DrawOpts {
                font_size: 12.0 * s,
                color: theme.u8(theme.fg),
                bold: true,
                clip_rect: clip,
                ..DrawOpts::default()
            };
            let lw = sugarloaf.text_mut().measure(label, &opts);
            draw_text_with_occlusion(
                sugarloaf,
                bx + (bw - lw) * 0.5,
                by + (bh - 12.0 * s) * 0.5,
                label,
                &opts,
                occlusion_rects,
            );
        }
        ExtensionStatus::Installed { version: _ } => {
            let fill = if hovered {
                theme.f32(theme.hover)
            } else {
                theme.f32(theme.bg)
            };
            let btn_clip = clip.unwrap_or(rect);
            draw_rounded_rect_clipped(
                sugarloaf,
                [bx, by, bw, bh],
                fill,
                BUTTON_RADIUS * s,
                ORDER_BUTTON,
                btn_clip,
            );
            paint_outline_clipped(sugarloaf, rect, theme.f32(theme.border), s, btn_clip);
            let label = install_button_label(status);
            let opts = DrawOpts {
                font_size: 12.0 * s,
                color: theme.u8(theme.fg),
                clip_rect: clip,
                ..DrawOpts::default()
            };
            let lw = sugarloaf.text_mut().measure(label, &opts);
            draw_text_with_occlusion(
                sugarloaf,
                bx + (bw - lw) * 0.5,
                by + (bh - 12.0 * s) * 0.5,
                label,
                &opts,
                occlusion_rects,
            );
        }
        ExtensionStatus::Installing {
            percent,
            status_text,
        } => {
            let btn_clip = clip.unwrap_or(rect);
            draw_rounded_rect_clipped(
                sugarloaf,
                [bx, by, bw, bh],
                theme.f32(theme.bg),
                BUTTON_RADIUS * s,
                ORDER_BUTTON,
                btn_clip,
            );
            paint_outline_clipped(sugarloaf, rect, theme.f32(theme.border), s, btn_clip);
            if let Some(percent) = percent {
                let pct = (*percent as f32 / 100.0).clamp(0.0, 1.0);
                let fill_w = (bw * pct).max(0.0);
                if fill_w > 0.0 {
                    draw_rounded_rect_clipped(
                        sugarloaf,
                        [bx, by, fill_w, bh],
                        theme.f32_alpha(theme.accent, 0.55),
                        BUTTON_RADIUS * s,
                        ORDER_PROGRESS,
                        btn_clip,
                    );
                }
            } else {
                // No denominator (DNS, package-manager work, extraction): a
                // moving segment communicates liveness without fabricating a
                // percentage. The host already repaints continuously while an
                // install is in flight.
                let elapsed = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs_f32();
                let segment_w = bw * 0.32;
                let travel = bw + segment_w;
                let segment_x = bx - segment_w + travel * ((elapsed * 0.8) % 1.0);
                draw_rounded_rect_clipped(
                    sugarloaf,
                    [segment_x, by, segment_w, bh],
                    theme.f32_alpha(theme.accent, 0.55),
                    BUTTON_RADIUS * s,
                    ORDER_PROGRESS,
                    btn_clip,
                );
            }
            let label = installing_label(*percent, status_text, hovered);
            let opts = DrawOpts {
                font_size: 11.0 * s,
                color: theme.u8(theme.fg),
                bold: true,
                clip_rect: clip,
                ..DrawOpts::default()
            };
            let lw = sugarloaf.text_mut().measure(&label, &opts);
            draw_text_with_occlusion(
                sugarloaf,
                bx + (bw - lw) * 0.5,
                by + (bh - 11.0 * s) * 0.5,
                &label,
                &opts,
                occlusion_rects,
            );
        }
        ExtensionStatus::Uninstalling => {
            let btn_clip = clip.unwrap_or(rect);
            draw_rounded_rect_clipped(
                sugarloaf,
                [bx, by, bw, bh],
                theme.f32(theme.bg),
                BUTTON_RADIUS * s,
                ORDER_BUTTON,
                btn_clip,
            );
            paint_outline_clipped(sugarloaf, rect, theme.f32(theme.border), s, btn_clip);
            let label = install_button_label(status);
            let opts = DrawOpts {
                font_size: 11.0 * s,
                color: theme.u8(theme.dim),
                clip_rect: clip,
                ..DrawOpts::default()
            };
            let lw = sugarloaf.text_mut().measure(label, &opts);
            draw_text_with_occlusion(
                sugarloaf,
                bx + (bw - lw) * 0.5,
                by + (bh - 11.0 * s) * 0.5,
                label,
                &opts,
                occlusion_rects,
            );
        }
    }
}

fn install_button_label(status: &ExtensionStatus) -> &'static str {
    match status {
        ExtensionStatus::BuiltIn => "Built in",
        ExtensionStatus::NotInstalled => "+ Install",
        ExtensionStatus::Installed { .. } => "Uninstall",
        ExtensionStatus::Installing { .. } => "Installing...",
        ExtensionStatus::Uninstalling => "Uninstalling...",
    }
}

fn installing_label(percent: Option<u8>, status: &str, hovered: bool) -> String {
    if hovered {
        return "Cancel".to_string();
    }
    let lower = status.to_ascii_lowercase();
    let phase = if lower.contains("connect") || lower.contains("resolv") {
        "Connecting"
    } else if lower.contains("download") || lower.contains("fetch") {
        "Downloading"
    } else if lower.contains("extract") || lower.contains("unpack") {
        "Extracting"
    } else if lower.contains("link") || lower.contains("final") {
        "Finalizing"
    } else if lower.contains("start") {
        "Starting"
    } else {
        "Installing"
    };
    match percent {
        Some(percent) => format!("{phase} {percent}%"),
        None => format!("{phase}…"),
    }
}

// Four 1-px slabs masquerading as a border. Sugarloaf's rect primitive
// has no stroke variant, so this is the cheapest way to outline the
// rounded shapes already painted underneath. The corners are slightly
// squared but the eye doesn't catch it at chrome scale.
/// Corner inset for outline edges so the straight border slabs stop short of
/// the rounded fill's corners instead of poking square corners past them (the
/// "oddly has corners" artifact). Matches the common ~4px corner radius.
const OUTLINE_CORNER_INSET: f32 = 4.0;

fn paint_outline(sugarloaf: &mut Sugarloaf, rect: [f32; 4], color: [f32; 4], s: f32) {
    let [x, y, w, h] = rect;
    let t = 1.0_f32.max(s);
    let r = OUTLINE_CORNER_INSET * s;
    let hw = (w - 2.0 * r).max(0.0);
    let vh = (h - 2.0 * r).max(0.0);
    sugarloaf.rect(None, x + r, y, hw, t, color, DEPTH, ORDER_CARD);
    sugarloaf.rect(None, x + r, y + h - t, hw, t, color, DEPTH, ORDER_CARD);
    sugarloaf.rect(None, x, y + r, t, vh, color, DEPTH, ORDER_CARD);
    sugarloaf.rect(None, x + w - t, y + r, t, vh, color, DEPTH, ORDER_CARD);
}

/// Same as `paint_outline` but each of the four slabs is intersected
/// with `clip_rect` first. Used for cards that may straddle the list
/// region's top/bottom edge during scroll — without this the outline
/// would draw a hairline through the tab strip and search bar above.
fn paint_outline_clipped(
    sugarloaf: &mut Sugarloaf,
    rect: [f32; 4],
    color: [f32; 4],
    s: f32,
    clip: [f32; 4],
) {
    let [x, y, w, h] = rect;
    let t = 1.0_f32.max(s);
    let r = OUTLINE_CORNER_INSET * s;
    let hw = (w - 2.0 * r).max(0.0);
    let vh = (h - 2.0 * r).max(0.0);
    // Edge slabs inset from the corners so they don't render square corners
    // outside the rounded fill.
    for slab in [
        [x + r, y, hw, t],
        [x + r, y + h - t, hw, t],
        [x, y + r, t, vh],
        [x + w - t, y + r, t, vh],
    ] {
        if let Some([cx, cy, cw, ch]) = intersect_rect(slab, clip) {
            sugarloaf.rect(None, cx, cy, cw, ch, color, DEPTH, ORDER_CARD);
        }
    }
}

/// Draw a rounded rect clipped to `clip`. When the visible portion
/// equals the full rect the rounded corners are preserved; when the
/// rect is partly off-clip we fall back to a sharp-cornered rect for
/// the visible slice (same trick the agent pane uses for partial-row
/// cards — the eye doesn't catch the corner change at chrome scale).
fn draw_rounded_rect_clipped(
    sugarloaf: &mut Sugarloaf,
    rect: [f32; 4],
    color: [f32; 4],
    radius: f32,
    order: u8,
    clip: [f32; 4],
) {
    crate::widgets::quad::rounded_rect_clipped(
        sugarloaf, clip, None, rect, color, DEPTH, radius, order, 0.5,
    );
}

/// Draw a rounded rect with a border that FOLLOWS the corner radius. The
/// old approach (rounded fill + four straight outline slabs) left the square
/// outline corners poking past the rounded fill — the "oddly has corners"
/// artifact. Here the border is a full-size rounded rect and the fill is an
/// inset rounded rect drawn on top, so the visible border is a rounded ring.
fn draw_rounded_rect_bordered_clipped(
    sugarloaf: &mut Sugarloaf,
    rect: [f32; 4],
    fill: [f32; 4],
    border: [f32; 4],
    radius: f32,
    order: u8,
    s: f32,
    clip: [f32; 4],
) {
    let [x, y, w, h] = rect;
    let t = 1.0_f32.max(s);
    draw_rounded_rect_clipped(sugarloaf, rect, border, radius, order, clip);
    draw_rounded_rect_clipped(
        sugarloaf,
        [x + t, y + t, (w - 2.0 * t).max(0.0), (h - 2.0 * t).max(0.0)],
        fill,
        (radius - t).max(0.0),
        order,
        clip,
    );
}

fn filter_index(filter: super::state::ExtensionFilter) -> usize {
    use super::state::ExtensionFilter;
    match filter {
        ExtensionFilter::All => 0,
        ExtensionFilter::Installed => 1,
        ExtensionFilter::NotInstalled => 2,
    }
}

fn format_thousands(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len + len.saturating_sub(1) / 3);
    // Walk left-to-right. A comma goes before any digit whose remaining
    // count is a multiple of 3 (excluding the very first position).
    // `remaining = len - i` is always non-negative; the prior impl
    // computed `i - first` with both as `usize` and underflowed for any
    // count where `len % 3 == 2` (e.g. 2-, 8-, 11-digit numbers).
    for (i, b) in bytes.iter().enumerate() {
        let remaining = len - i;
        if i > 0 && remaining % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

fn point_in_rect(x: f32, y: f32, rect: [f32; 4]) -> bool {
    x >= rect[0] && y >= rect[1] && x <= rect[0] + rect[2] && y <= rect[1] + rect[3]
}

#[cfg(test)]
mod tests {
    use super::super::state::{ExtensionEntry, ExtensionStatus, NeoismExtensionsPane};

    fn entry(id: &str, status: ExtensionStatus) -> ExtensionEntry {
        ExtensionEntry {
            id: id.to_string(),
            name: format!("Extension {id}"),
            version: "1.2.3".into(),
            description: "An example MCP server with a longer description.".into(),
            author: "Test Author <test@example.com>".into(),
            downloads: Some(5_509_987),
            categories: vec!["mcp".into(), "tools".into()],
            languages: Vec::new(),
            status,
            repository_url: Some("https://example.com/repo".into()),
            lsp_source: None,
        }
    }

    #[test]
    fn constructs_with_mixed_statuses() {
        let mut pane = NeoismExtensionsPane::new();
        pane.set_entries(vec![
            entry("a", ExtensionStatus::NotInstalled),
            entry(
                "b",
                ExtensionStatus::Installing {
                    percent: Some(42),
                    status_text: "downloading".into(),
                },
            ),
            entry(
                "c",
                ExtensionStatus::Installed {
                    version: "1.2.3".into(),
                },
            ),
            entry("d", ExtensionStatus::Uninstalling),
        ]);
        assert_eq!(pane.entries().len(), 4);
    }

    #[test]
    fn format_thousands_groups() {
        assert_eq!(super::format_thousands(0), "0");
        assert_eq!(super::format_thousands(999), "999");
        assert_eq!(super::format_thousands(1_000), "1,000");
        assert_eq!(super::format_thousands(5_509_987), "5,509,987");
        assert_eq!(super::format_thousands(1_000_000_000), "1,000,000,000");
    }

    #[test]
    fn install_button_label_uses_text_safe_marker() {
        assert_eq!(
            super::install_button_label(&ExtensionStatus::BuiltIn),
            "Built in"
        );
        assert_eq!(
            super::install_button_label(&ExtensionStatus::NotInstalled),
            "+ Install"
        );
        assert_eq!(
            super::install_button_label(&ExtensionStatus::Installed {
                version: "1.2.3".into()
            }),
            "Uninstall"
        );
        assert_eq!(
            super::install_button_label(&ExtensionStatus::Uninstalling),
            "Uninstalling..."
        );
    }

    #[test]
    fn install_progress_label_distinguishes_known_and_unknown_progress() {
        assert_eq!(
            super::installing_label(None, "connecting to GitHub", false),
            "Connecting…"
        );
        assert_eq!(
            super::installing_label(Some(42), "downloading 12 MiB", false),
            "Downloading 42%"
        );
        assert_eq!(
            super::installing_label(Some(42), "downloading 12 MiB", true),
            "Cancel"
        );
    }
}
