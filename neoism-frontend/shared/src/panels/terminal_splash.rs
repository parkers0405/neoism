// Splash banner: reserves blank cell rows so the shell prompt
// lands BELOW the visual splash area. Everything visible (the
// rasterised wordmark image, the tagline, the four menu
// buttons, the click fidget) is painted by `splash_overlay.rs`
// — there are no glyphs in this byte stream, only blank rows.
//
// Doing it this way means:
//   * The wordmark stays pixel-centered; it doesn't reflow when
//     chrome (file tree, etc.) opens because the overlay
//     re-anchors against the live pane rect each frame.
//   * The menu buttons can be real GUI elements (rounded boxes,
//     hover states, click ripples) instead of dim terminal text.
//   * As soon as the user runs a command, the cells scroll up
//     normally and the overlay self-resets — the splash leaves
//     gracefully like pokemon-colorscripts.
//
// Line terminators are CRLF: feeding bytes straight into the
// parser bypasses the PTY line discipline that would translate
// bare LF to CRLF for us.
//
// TODO(wave6-cutover): once the web frontend renders the splash
// overlay via the shared chrome pipeline, delete the
// `frontends/neoism/src/terminal/splash.rs` duplicate and have
// the native pane import from `neoism_ui::panels::terminal_splash`.

const NL: &str = "\r\n";

/// DESIRED wordmark rows on a comfortably-tall pane. Actual
/// rows used are computed by `adapt_layout` against the live
/// pane height so the splash never refuses to render on a
/// smaller terminal.
pub const WORDMARK_RESERVE_ROWS: usize = 5;
/// DESIRED gap rows between wordmark and menu.
pub const WORDMARK_TO_MENU_GAP_ROWS: usize = 2;
/// DESIRED menu rows.
pub const MENU_RESERVE_ROWS: usize = 16;

/// Absolute floor for each band — what we'll shrink to on a
/// tiny hyprland tile rather than refuse to render. Frame rows
/// stay 2 for normal-size panes (breathing room between splash
/// and prompt) but the floors themselves are tight enough that
/// even a 7-row pane still gets a (compressed) splash.
const MIN_WORDMARK_ROWS: usize = 2;
const MIN_GAP_ROWS: usize = 0;
const MIN_MENU_ROWS: usize = 4;
const FRAME_ROWS: usize = 2; // 1 leading + 1 trailing blank
const MIN_TOTAL_ROWS: usize = MIN_WORDMARK_ROWS + MIN_GAP_ROWS + MIN_MENU_ROWS; // frame optional below this

/// Desired total height — used as the upper bound for the
/// adaptive layout.
#[allow(dead_code)]
pub const SPLASH_HEIGHT: usize =
    FRAME_ROWS + WORDMARK_RESERVE_ROWS + WORDMARK_TO_MENU_GAP_ROWS + MENU_RESERVE_ROWS;

/// Per-frame splash layout sizes, scaled to the live pane
/// height so a small terminal still renders a (smaller) splash
/// instead of nothing.
#[derive(Clone, Copy, Debug)]
pub struct SplashLayout {
    pub top_pad_rows: usize,
    pub wordmark_rows: usize,
    pub gap_rows: usize,
    pub menu_rows: usize,
    pub total_rows: usize,
}

impl SplashLayout {
    /// Cell row offset (from top of pane) where the wordmark
    /// band starts: `top_pad + 1` (the leading blank).
    pub fn wordmark_row(&self) -> usize {
        self.top_pad_rows + 1
    }
}

/// Compute the splash layout that fits in `available_rows` cell
/// rows. Returns `None` only when the pane is so small that
/// even the floor (~12 rows) won't fit. On larger panes returns
/// the desired layout; on mid-sized panes proportionally
/// shrinks the bands.
pub fn adapt_layout(available_rows: usize) -> Option<SplashLayout> {
    if available_rows < MIN_TOTAL_ROWS {
        return None;
    }

    // Frame (leading + trailing blank) is optional on tiny
    // panes — drop it when the body floors only just fit.
    let frame_rows = if available_rows >= MIN_TOTAL_ROWS + FRAME_ROWS {
        FRAME_ROWS
    } else {
        0
    };

    // Desired total without frame.
    let desired_body =
        WORDMARK_RESERVE_ROWS + WORDMARK_TO_MENU_GAP_ROWS + MENU_RESERVE_ROWS;
    let body_budget = available_rows.saturating_sub(frame_rows);

    let (wordmark_rows, gap_rows, menu_rows) = if body_budget >= desired_body {
        (
            WORDMARK_RESERVE_ROWS,
            WORDMARK_TO_MENU_GAP_ROWS,
            MENU_RESERVE_ROWS,
        )
    } else {
        // Shrink each band proportionally to its desired share,
        // then bump back up to its minimum if we went under.
        let scale = body_budget as f32 / desired_body as f32;
        let mut w = ((WORDMARK_RESERVE_ROWS as f32 * scale).round() as usize)
            .max(MIN_WORDMARK_ROWS);
        let mut g = ((WORDMARK_TO_MENU_GAP_ROWS as f32 * scale).round() as usize)
            .max(MIN_GAP_ROWS);
        let mut m =
            ((MENU_RESERVE_ROWS as f32 * scale).round() as usize).max(MIN_MENU_ROWS);
        // After flooring we may have over-allocated. Steal back
        // from the wordmark first (it's the most flexible — it
        // just renders smaller), then the gap, never below the
        // floors.
        while w + g + m > body_budget {
            if w > MIN_WORDMARK_ROWS {
                w -= 1;
            } else if g > MIN_GAP_ROWS {
                g -= 1;
            } else if m > MIN_MENU_ROWS {
                m -= 1;
            } else {
                break;
            }
        }
        (w, g, m)
    };

    let total_rows = frame_rows + wordmark_rows + gap_rows + menu_rows;
    let top_pad_rows = available_rows.saturating_sub(total_rows).saturating_sub(1) / 2;
    Some(SplashLayout {
        top_pad_rows,
        wordmark_rows,
        gap_rows,
        menu_rows,
        total_rows,
    })
}

/// Aspect ratio of the rasterised wordmark PNG (width / height).
/// PNG is auto-trimmed to its non-transparent bounding box (via
/// `magick -trim`), so this is the actual letter-band aspect —
/// no transparent gutter top/bottom.
///
/// File: 1196 × 193 → ~6.197.
pub const WORDMARK_ASPECT: f32 = 6.197;

/// Build the splash byte stream + the layout used. The whole
/// splash is blank cells — only newlines for vertical centering
/// and the reserved-row bands. The GPU overlay paints the
/// wordmark + menu on top, sized against the same layout.
pub fn splash_bytes(cols: usize, rows: usize) -> Option<(String, SplashLayout)> {
    if cols < 24 {
        return None;
    }
    let layout = adapt_layout(rows)?;

    let mut out = String::with_capacity(512);
    for _ in 0..layout.top_pad_rows {
        out.push_str(NL);
    }
    // Leading blank + wordmark band + gap + menu band + trailing
    // blank — total `layout.total_rows` newlines.
    for _ in 0..layout.total_rows {
        out.push_str(NL);
    }
    Some((out, layout))
}
