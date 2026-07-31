use crate::layout::ContextDimension;
use neoism_backend::sugarloaf::text::DrawOpts;
use neoism_backend::sugarloaf::Sugarloaf;

/// Confirm-before-quit dialog. Restyled from the old Rio black-box
/// tooltip into a neoism-style centered card: a dim scrim, a rounded
/// panel with a hairline border + accent strip, a bold title, and the
/// confirm / cancel key hints (confirm in accent, cancel dimmed).
#[inline]
pub fn screen(
    sugarloaf: &mut Sugarloaf,
    context_dimension: &ContextDimension,
    heading_content: &str,
    confirm_content: &str,
    quit_content: &str,
) {
    let layout = sugarloaf.window_size();
    let scale = context_dimension.dimension.scale;
    let win_w = layout.width / scale;
    let win_h = layout.height / scale;

    // Theme-independent neoism dark palette so the dialog reads well
    // over any pack / wallpaper.
    let scrim = [0.0, 0.0, 0.0, 0.5];
    let panel_bg = [0.07, 0.07, 0.09, 1.0];
    let border = [0.24, 0.24, 0.30, 1.0];
    let accent_f = [0.31, 0.67, 1.0, 1.0];
    let fg = [236, 236, 242, 255];
    let dim = [152, 152, 162, 255];
    let accent = [92, 176, 255, 255];

    // Dim the workspace behind the card.
    sugarloaf.rect(None, 0.0, 0.0, win_w, win_h, scrim, 0.0, 20);

    // Centered rounded card.
    let box_w = 440.0_f32.min(win_w - 48.0).max(280.0);
    let box_h = 118.0;
    let box_x = (win_w - box_w) / 2.0;
    let box_y = (win_h - box_h) / 2.0;

    // Border ring, then the fill on top.
    sugarloaf.rounded_rect(
        None,
        box_x - 1.0,
        box_y - 1.0,
        box_w + 2.0,
        box_h + 2.0,
        border,
        0.0,
        13.0,
        20,
    );
    sugarloaf.rounded_rect(None, box_x, box_y, box_w, box_h, panel_bg, 0.0, 12.0, 21);
    // Left accent strip.
    sugarloaf.rounded_rect(
        None,
        box_x,
        box_y + 14.0,
        3.0,
        box_h - 28.0,
        accent_f,
        0.0,
        1.5,
        22,
    );

    let pad = 26.0;
    let ui = sugarloaf.text_mut();

    // Title.
    let title_opts = DrawOpts {
        font_size: 16.0,
        color: fg,
        bold: true,
        ..DrawOpts::default()
    };
    ui.draw(box_x + pad, box_y + 42.0, heading_content, &title_opts);

    // Key hints — confirm (accent) then cancel (dim), spaced apart.
    let hint_y = box_y + box_h - 32.0;
    let confirm_opts = DrawOpts {
        font_size: 13.0,
        color: accent,
        bold: true,
        ..DrawOpts::default()
    };
    let confirm_w = ui.draw(box_x + pad, hint_y, confirm_content, &confirm_opts);
    let quit_opts = DrawOpts {
        font_size: 13.0,
        color: dim,
        ..DrawOpts::default()
    };
    ui.draw(
        box_x + pad + confirm_w + 18.0,
        hint_y,
        quit_content,
        &quit_opts,
    );
}
