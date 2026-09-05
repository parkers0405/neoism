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

/// Baseline wordmark rows on a comfortably-tall pane. Actual
/// rows used are computed by `adapt_layout` against the live
/// pane height so the splash shrinks on smaller terminals and
/// grows proportionally on roomy ones.
pub const WORDMARK_RESERVE_ROWS: usize = 5;
/// DESIRED gap rows between wordmark and menu.
pub const WORDMARK_TO_MENU_GAP_ROWS: usize = 2;
/// DESIRED menu rows.
pub const MENU_RESERVE_ROWS: usize = 16;

/// Total rows in the visual body at the baseline size. The overlay uses the
/// ratio between its live body rows and this value to scale the wordmark and
/// menu as one unit.
pub const SPLASH_BODY_ROWS: usize =
    WORDMARK_RESERVE_ROWS + WORDMARK_TO_MENU_GAP_ROWS + MENU_RESERVE_ROWS;

/// On a roomy pane the splash may grow to one-and-a-half times its baseline
/// size. Keeping a ceiling leaves enough quiet space around the splash and
/// prevents an ultrawide/full-screen terminal from turning it into a poster.
pub const MAX_SPLASH_SCALE: f32 = 1.5;
/// Fraction of the available terminal rows the centered splash may occupy
/// while it is growing toward `MAX_SPLASH_SCALE`.
const LARGE_PANE_FILL: f32 = 0.82;

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

/// Baseline total height. The adaptive layout may grow beyond this up to
/// `MAX_SPLASH_SCALE` on a roomy pane.
#[allow(dead_code)]
pub const SPLASH_HEIGHT: usize = FRAME_ROWS + SPLASH_BODY_ROWS;

/// Per-frame splash layout sizes, scaled to the live pane height so a small
/// terminal still renders and a roomy terminal uses its space.
#[derive(Clone, Copy, Debug)]
pub struct SplashLayout {
    pub top_pad_rows: usize,
    pub wordmark_rows: usize,
    pub gap_rows: usize,
    pub menu_rows: usize,
    pub total_rows: usize,
}

impl SplashLayout {
    pub fn body_rows(&self) -> usize {
        self.wordmark_rows + self.gap_rows + self.menu_rows
    }

    /// Cell row offset (from top of pane) where the wordmark
    /// band starts. Normal panes have one leading frame row; the
    /// frame disappears entirely on the very smallest supported panes.
    pub fn wordmark_row(&self) -> usize {
        let frame_rows = self.total_rows.saturating_sub(self.body_rows());
        self.top_pad_rows + frame_rows / 2
    }
}

/// Divide `body_budget` between the wordmark, inter-band gap, and menu while
/// preserving their baseline proportions. Floors keep all useful content
/// present on tiny panes; largest-remainder distribution avoids giving every
/// extra row to the menu as the viewport grows.
fn proportional_bands(body_budget: usize) -> (usize, usize, usize) {
    let desired = [
        WORDMARK_RESERVE_ROWS as f32,
        WORDMARK_TO_MENU_GAP_ROWS as f32,
        MENU_RESERVE_ROWS as f32,
    ];
    let minimum = [MIN_WORDMARK_ROWS, MIN_GAP_ROWS, MIN_MENU_ROWS];
    let scale = body_budget as f32 / SPLASH_BODY_ROWS as f32;
    let target = desired.map(|rows| rows * scale);
    let mut bands = [
        (target[0].floor() as usize).max(minimum[0]),
        (target[1].floor() as usize).max(minimum[1]),
        (target[2].floor() as usize).max(minimum[2]),
    ];

    while bands.iter().sum::<usize>() < body_budget {
        let mut best = 0;
        let mut best_deficit = f32::NEG_INFINITY;
        for i in 0..bands.len() {
            let deficit = target[i] - bands[i] as f32;
            if deficit > best_deficit {
                best = i;
                best_deficit = deficit;
            }
        }
        bands[best] += 1;
    }

    while bands.iter().sum::<usize>() > body_budget {
        let mut best = None;
        let mut best_excess = f32::NEG_INFINITY;
        for i in 0..bands.len() {
            if bands[i] <= minimum[i] {
                continue;
            }
            let excess = bands[i] as f32 - target[i];
            if excess > best_excess {
                best = Some(i);
                best_excess = excess;
            }
        }
        let Some(best) = best else {
            break;
        };
        bands[best] -= 1;
    }

    (bands[0], bands[1], bands[2])
}

/// Compute the splash layout that fits in `available_rows` cell rows. Returns
/// `None` only when the pane is too small for the content floors. Mid-sized
/// panes shrink the bands; roomy panes grow every band proportionally up to a
/// bounded maximum.
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

    // Small panes continue to use every available row. Once the baseline
    // fits, grow the whole composition toward a bounded share of the pane
    // instead of freezing its size and adding nothing but empty padding.
    let target_total_rows = if available_rows <= SPLASH_HEIGHT {
        available_rows
    } else {
        let max_total_rows =
            FRAME_ROWS + (SPLASH_BODY_ROWS as f32 * MAX_SPLASH_SCALE).round() as usize;
        ((available_rows as f32 * LARGE_PANE_FILL).round() as usize)
            .clamp(SPLASH_HEIGHT, max_total_rows)
            .min(available_rows)
    };
    let body_budget = target_total_rows.saturating_sub(frame_rows);
    let (wordmark_rows, gap_rows, menu_rows) = proportional_bands(body_budget);

    let total_rows = frame_rows + wordmark_rows + gap_rows + menu_rows;
    let top_pad_rows = available_rows.saturating_sub(total_rows) / 2;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_layout_keeps_the_authored_proportions() {
        let layout = adapt_layout(SPLASH_HEIGHT).unwrap();
        assert_eq!(layout.wordmark_rows, WORDMARK_RESERVE_ROWS);
        assert_eq!(layout.gap_rows, WORDMARK_TO_MENU_GAP_ROWS);
        assert_eq!(layout.menu_rows, MENU_RESERVE_ROWS);
        assert_eq!(layout.total_rows, SPLASH_HEIGHT);
    }

    #[test]
    fn roomy_panes_grow_every_visual_band_together() {
        let baseline = adapt_layout(SPLASH_HEIGHT).unwrap();
        let roomy = adapt_layout(45).unwrap();
        assert!(roomy.wordmark_rows > baseline.wordmark_rows);
        assert!(roomy.gap_rows > baseline.gap_rows);
        assert!(roomy.menu_rows > baseline.menu_rows);
        assert!(roomy.total_rows < 45);
    }

    #[test]
    fn very_large_panes_respect_the_growth_ceiling() {
        let large = adapt_layout(80).unwrap();
        let huge = adapt_layout(800).unwrap();
        assert_eq!(large.body_rows(), huge.body_rows());
        assert_eq!(large.total_rows, huge.total_rows);
    }

    #[test]
    fn visual_body_is_vertically_centered() {
        for rows in MIN_TOTAL_ROWS..80 {
            let layout = adapt_layout(rows).unwrap();
            let top = layout.wordmark_row();
            let bottom = rows - (top + layout.body_rows());
            assert!(top.abs_diff(bottom) <= 1, "rows={rows}, layout={layout:?}");
        }
    }

    #[test]
    fn frameless_minimum_layout_stays_inside_the_pane() {
        let layout = adapt_layout(MIN_TOTAL_ROWS).unwrap();
        assert_eq!(layout.wordmark_row(), 0);
        assert_eq!(layout.body_rows(), MIN_TOTAL_ROWS);
    }
}
