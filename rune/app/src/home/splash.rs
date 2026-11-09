//! Splash geometry, ported from `shared/src/panels/terminal_splash.rs` (row bands) and the menu
//! sizing in `splash_overlay.rs`: the pane is measured in terminal cells, split into wordmark,
//! gap and menu bands that shrink on small panes and grow up to 1.5x on roomy ones; the menu is
//! two columns (one when two would squeeze below 75%) filling the width, scaled down to fit.

pub const WORDMARK_ASPECT: f32 = 6.197;

const WORDMARK_ROWS: usize = 5;
const GAP_ROWS: usize = 2;
const MENU_ROWS: usize = 16;
const BODY_ROWS: usize = WORDMARK_ROWS + GAP_ROWS + MENU_ROWS;
const MAX_SCALE: f32 = 1.5;
const LARGE_PANE_FILL: f32 = 0.82;
const MIN_BANDS: [usize; 3] = [2, 0, 4];
const FRAME_ROWS: usize = 2;
const MIN_TOTAL_ROWS: usize = 6;
const SPLASH_HEIGHT: usize = FRAME_ROWS + BODY_ROWS;

const COLUMNS: usize = 2;
const COLUMN_GAP: f32 = 32.0;
const BUTTON_H: f32 = 42.0;
const BUTTON_GAP: f32 = 8.0;
const RADIUS: f32 = 10.0;
const PAD: f32 = 22.0;
const ICON_SLOT: f32 = 22.0;
const LABEL_GAP: f32 = 14.0;
const KEY_GAP: f32 = 32.0;
const LABEL_FONT: f32 = 15.0;
const KEY_FONT: f32 = 14.0;
const ICON_FONT: f32 = 15.0;
/// Geist Mono's advance is 0.6 em, so text width is exact from the character count.
const MONO_ADVANCE: f32 = 0.6;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MenuAction {
    ChangeDirectory,
    Notes,
    FileTree,
    Agent,
    Search,
    AgentPanel,
    CommandPalette,
    NewTerminal,
}

pub struct MenuItem {
    pub action: MenuAction,
    pub icon: &'static str,
    pub label: &'static str,
    pub keys: &'static str,
}

pub const MENU: [MenuItem; 8] = [
    MenuItem { action: MenuAction::ChangeDirectory, icon: "\u{f07b}", label: "Change Directory", keys: "Alt + D" },
    MenuItem { action: MenuAction::Notes, icon: "\u{f15c}", label: "Notes", keys: "Alt + N" },
    MenuItem { action: MenuAction::FileTree, icon: "\u{f07b}", label: "Open file tree", keys: "Alt + E" },
    MenuItem { action: MenuAction::Agent, icon: "\u{f135}", label: "Neoism Agent", keys: "Alt + A" },
    MenuItem { action: MenuAction::Search, icon: "\u{f002}", label: "Search", keys: "Alt + S" },
    MenuItem { action: MenuAction::AgentPanel, icon: "\u{f0674}", label: "Agent panel", keys: "Alt + C" },
    MenuItem { action: MenuAction::CommandPalette, icon: "\u{f0c9}", label: "Command palette", keys: "Alt + P" },
    MenuItem { action: MenuAction::NewTerminal, icon: "\u{f120}", label: "New Terminal", keys: "Alt + T" },
];

/// Everything the splash view places, in points relative to the splash area.
#[derive(Clone, Debug, PartialEq)]
pub struct Geometry {
    pub wordmark: [f32; 4],
    pub buttons: Vec<[f32; 4]>,
    pub label_font: f32,
    pub key_font: f32,
    pub icon_font: f32,
    pub pad: f32,
    pub icon_slot: f32,
    pub label_gap: f32,
    pub radius: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Bands {
    top_pad: usize,
    wordmark: usize,
    gap: usize,
    menu: usize,
    total: usize,
}

impl Bands {
    fn wordmark_row(&self) -> usize {
        self.top_pad + (self.total - self.wordmark - self.gap - self.menu) / 2
    }
}

/// Split `budget` rows in the baseline 5:2:16 proportions, honouring the floors, giving leftover
/// rows to the band furthest below its target (largest remainder).
fn proportional_bands(budget: usize) -> [usize; 3] {
    let scale = budget as f32 / BODY_ROWS as f32;
    let target = [WORDMARK_ROWS, GAP_ROWS, MENU_ROWS].map(|rows| rows as f32 * scale);
    let mut bands = [0, 1, 2].map(|i| (target[i].floor() as usize).max(MIN_BANDS[i]));
    while bands.iter().sum::<usize>() < budget {
        let best = (0..3).max_by(|&a, &b| (target[a] - bands[a] as f32).total_cmp(&(target[b] - bands[b] as f32))).unwrap_or(0);
        bands[best] += 1;
    }
    while bands.iter().sum::<usize>() > budget {
        let Some(best) = (0..3).filter(|&i| bands[i] > MIN_BANDS[i])
            .max_by(|&a, &b| (bands[a] as f32 - target[a]).total_cmp(&(bands[b] as f32 - target[b]))) else { break };
        bands[best] -= 1;
    }
    bands
}

fn adapt(rows: usize) -> Option<Bands> {
    if rows < MIN_TOTAL_ROWS { return None; }
    let frame = if rows >= MIN_TOTAL_ROWS + FRAME_ROWS { FRAME_ROWS } else { 0 };
    let target = if rows <= SPLASH_HEIGHT {
        rows
    } else {
        let max = FRAME_ROWS + (BODY_ROWS as f32 * MAX_SCALE).round() as usize;
        ((rows as f32 * LARGE_PANE_FILL).round() as usize).clamp(SPLASH_HEIGHT, max).min(rows)
    };
    let [wordmark, gap, menu] = proportional_bands(target.saturating_sub(frame));
    let total = frame + wordmark + gap + menu;
    Some(Bands { top_pad: rows.saturating_sub(total) / 2, wordmark, gap, menu, total })
}

fn text_width(text: &str, font: f32) -> f32 { text.chars().count() as f32 * font * MONO_ADVANCE }

fn widest_button(s: f32) -> f32 {
    MENU.iter().map(|item| {
        PAD * s + ICON_SLOT * s + LABEL_GAP * s + text_width(item.label, LABEL_FONT * s)
            + KEY_GAP * s + text_width(&format!("[{}]", item.keys), KEY_FONT * s) + PAD * s
    }).fold(0.0, f32::max)
}

/// Lay the splash out in a `width` x `height` area for the configured font size and UI scale.
/// `None` when the area is too small to show it (the original's 24 column / 6 row floors).
pub fn layout(width: f32, height: f32, font_size: f32, ui_scale: f32) -> Option<Geometry> {
    let cell_h = font_size * 1.3;
    let cols = (width / (font_size * 0.6)).floor() as usize;
    if cols < 24 { return None; }
    let bands = adapt((height / cell_h).floor() as usize)?;

    let band_top = bands.wordmark_row() as f32 * cell_h;
    let band_h = bands.wordmark as f32 * cell_h;
    let logo_w = (band_h * 0.96 * WORDMARK_ASPECT).min((width - 32.0 * ui_scale).max(0.0));
    let logo_h = logo_w / WORDMARK_ASPECT;
    let mut wordmark = [(width - logo_w) * 0.5, band_top + (band_h - logo_h) * 0.5, logo_w, logo_h];

    let menu_top = band_top + band_h + bands.gap as f32 * cell_h;
    let menu_band_h = bands.menu as f32 * cell_h;
    let layout_scale = ((bands.wordmark + bands.gap + bands.menu) as f32 / BODY_ROWS as f32).clamp(0.2, MAX_SCALE);
    let side_pad = (16.0 * ui_scale.max(0.1)).min((width * 0.08).max(0.0));
    let max_menu_w = (width - side_pad * 2.0).max(1.0);

    let mut s = ui_scale.max(0.1) * layout_scale;
    let base_widest = widest_button(s);
    let grid_w = COLUMNS as f32 * base_widest + (COLUMNS - 1) as f32 * COLUMN_GAP * s;
    let columns = if max_menu_w < grid_w * 0.75 { 1 } else { COLUMNS };
    let rows = MENU.len().div_ceil(columns);
    let base_button_w = (logo_w / columns as f32).max(base_widest);
    let base_total_w = columns as f32 * base_button_w + (columns - 1) as f32 * COLUMN_GAP * s;
    let base_total_h = rows as f32 * BUTTON_H * s + (rows - 1) as f32 * BUTTON_GAP * s;
    let width_fit = if base_total_w > max_menu_w { max_menu_w / base_total_w } else { 1.0 };
    let height_fit = if base_total_h > menu_band_h && base_total_h > 0.0 { menu_band_h / base_total_h } else { 1.0 };
    s *= width_fit.min(height_fit).clamp(0.2, 1.0);

    let column_gap = (COLUMN_GAP * s).min(max_menu_w / columns as f32);
    let gaps = (columns - 1) as f32 * column_gap;
    let button_w = (max_menu_w - gaps) / columns as f32;
    let button_h = BUTTON_H * s;
    let row_gap = BUTTON_GAP * s;
    let total_w = columns as f32 * button_w + gaps;
    let total_h = rows as f32 * button_h + (rows - 1) as f32 * row_gap;
    let menu_y = (menu_top + (menu_band_h - total_h) / 2.0).clamp(menu_top, menu_top + (menu_band_h - total_h).max(0.0));
    let menu_x = ((width - total_w) / 2.0).clamp(0.0, (width - total_w).max(0.0));
    let mut buttons: Vec<[f32; 4]> = (0..MENU.len()).map(|i| [
        menu_x + (i % columns) as f32 * (button_w + column_gap),
        menu_y + (i / columns) as f32 * (button_h + row_gap),
        button_w,
        button_h,
    ]).collect();

    // GPUiX centred the union of the wordmark and buttons vertically in the area.
    let top = buttons.iter().chain([&wordmark]).map(|r| r[1]).fold(f32::INFINITY, f32::min);
    let bottom = buttons.iter().chain([&wordmark]).map(|r| r[1] + r[3]).fold(f32::NEG_INFINITY, f32::max);
    let shift = ((height - (bottom - top)) / 2.0).max(0.0) - top;
    wordmark[1] += shift;
    for button in &mut buttons { button[1] += shift; }

    Some(Geometry {
        wordmark, buttons, label_font: LABEL_FONT * s, key_font: KEY_FONT * s, icon_font: ICON_FONT * s,
        pad: PAD * s, icon_slot: ICON_SLOT * s, label_gap: LABEL_GAP * s, radius: RADIUS * s,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_keeps_authored_proportions() {
        let bands = adapt(SPLASH_HEIGHT).unwrap();
        assert_eq!((bands.wordmark, bands.gap, bands.menu), (WORDMARK_ROWS, GAP_ROWS, MENU_ROWS));
    }

    #[test]
    fn large_panes_stop_growing() {
        let bands = adapt(400).unwrap();
        assert!(bands.wordmark + bands.gap + bands.menu <= (BODY_ROWS as f32 * MAX_SCALE).round() as usize);
    }

    #[test]
    fn narrow_panes_stack_one_column_inside_the_width() {
        let g = layout(560.0, 560.0, 15.0, 15.0 / 14.0).unwrap();
        assert_eq!(g.buttons[1][0], g.buttons[0][0], "second item stacks under the first");
        assert!(g.wordmark[2] <= 560.0 - 32.0);
        for b in &g.buttons { assert!(b[0] >= 0.0 && b[0] + b[2] <= 560.0 + 0.01); }
    }

    #[test]
    fn wide_panes_use_two_columns() {
        let g = layout(1196.0, 560.0, 15.0, 15.0 / 14.0).unwrap();
        assert!(g.buttons[1][0] > g.buttons[0][0]);
    }

    #[test]
    fn tiny_panes_hide_the_splash() {
        assert!(layout(100.0, 400.0, 15.0, 1.0).is_none());
        assert!(layout(800.0, 60.0, 15.0, 1.0).is_none());
    }
}
