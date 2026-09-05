//! "Share with phone" sheet — a QR code for the URL a phone can open to
//! join the workspace that is currently on screen.
//!
//! The panel is deliberately dumb: the HOST decides what URL is
//! reachable (only it knows whether the daemon is listening on a LAN
//! address, a tunnel, or a tailnet) and pushes it in via
//! [`ShareSheet::show`]. This panel just encodes and paints it.

use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::primitives::IdeTheme;

/// Painted above everything else in the chrome.
const DEPTH: f32 = 0.0;
const ORDER_SCRIM: u8 = 240;
const ORDER_CARD: u8 = 242;
const ORDER_MODULE: u8 = 244;

const CARD_PAD: f32 = 22.0;
const HEADER_H: f32 = 28.0;
const MESSAGE_H: f32 = 18.0;

#[derive(Debug, Clone, Default)]
pub struct ShareSheet {
    visible: bool,
    /// The URL encoded in the QR. It is never painted as text.
    url: Option<String>,
    /// Error shown only when there is no QR to scan.
    hint: Option<String>,
    /// Cached `(url, width, dark-cells)` so a repaint doesn't re-encode.
    encoded: Option<(String, usize, Vec<bool>)>,
    card_rect: Option<[f32; 4]>,
    /// Hit rect for the explicit close affordance. Dismissing on any
    /// click is not discoverable on its own — a phone-sharing sheet is
    /// something you sit and look at, so it needs a visible way out.
    close_rect: Option<[f32; 4]>,
}

impl ShareSheet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    /// Show the sheet for `url`. Encoding happens here, once, rather
    /// than per frame. A successful QR deliberately discards the host hint:
    /// the visible card is only its title, close button, and scan target.
    pub fn show(&mut self, url: impl Into<String>, hint: Option<String>) {
        let url = url.into();
        if self
            .encoded
            .as_ref()
            .is_none_or(|(cached, _, _)| cached != &url)
        {
            self.encoded = encode_qr(&url).map(|(w, cells)| (url.clone(), w, cells));
        }
        self.url = Some(url);
        self.hint = if self.encoded.is_some() { None } else { hint };
        self.visible = true;
    }

    /// Show a message with no QR — e.g. the daemon reported no
    /// phone-reachable address.
    pub fn show_message(&mut self, hint: impl Into<String>) {
        self.url = None;
        self.encoded = None;
        self.hint = Some(hint.into());
        self.visible = true;
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.card_rect = None;
        self.close_rect = None;
    }

    /// The close button's hit rect while the sheet is open.
    pub fn close_button_rect(&self) -> Option<[f32; 4]> {
        self.close_rect
    }

    /// Dismiss only from the explicit close target or the scrim. Card taps
    /// are consumed but retained, avoiding accidental dismissal while a user
    /// is positioning another device over the QR code.
    pub fn handle_click(&mut self, x: f32, y: f32) -> bool {
        if !self.visible {
            return false;
        }
        let in_rect = |rect: [f32; 4]| {
            x >= rect[0]
                && x <= rect[0] + rect[2]
                && y >= rect[1]
                && y <= rect[1] + rect[3]
        };
        if self.close_rect.is_some_and(in_rect)
            || self.card_rect.is_none_or(|r| !in_rect(r))
        {
            self.hide();
        }
        true
    }

    pub fn render(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        viewport: [f32; 4],
        theme: &IdeTheme,
        scale: f32,
    ) {
        if !self.visible {
            return;
        }
        let [vx, vy, vw, vh] = viewport;
        if vw <= 0.0 || vh <= 0.0 {
            return;
        }
        // Scrim.
        sugarloaf.rect(
            None,
            vx,
            vy,
            vw,
            vh,
            // Near-opaque: this is a modal the user points a camera at,
            // so nothing behind it should compete with the code.
            theme.f32_alpha(theme.bg, 0.97),
            DEPTH,
            ORDER_SCRIM,
        );

        let modules = self.encoded.as_ref().map(|(_, w, _)| *w).unwrap_or(0);
        // Quiet zone is part of the spec — a QR without it scans badly.
        let quiet = 4usize;
        let total_modules = modules + quiet * 2;
        let module_px = if total_modules > 0 {
            ((vw.min(vh) * 0.42) / total_modules as f32)
                .max(1.0)
                .floor()
        } else {
            0.0
        };
        let qr_px = module_px * total_modules as f32;

        let pad = CARD_PAD * scale;
        let header_h = HEADER_H * scale;
        let has_qr = self.encoded.is_some();
        let (wanted_w, wanted_h) = share_card_dimensions(qr_px, scale, has_qr);
        let safe_margin = 12.0 * scale;
        let card_w = wanted_w.min((vw - safe_margin * 2.0).max(1.0));
        let card_h = wanted_h.min((vh - safe_margin * 2.0).max(1.0));
        let card_x = vx + (vw - card_w) * 0.5;
        let card_y = vy + (vh - card_h) * 0.5;
        self.card_rect = Some([card_x, card_y, card_w, card_h]);

        sugarloaf.rounded_rect(
            None,
            card_x,
            card_y,
            card_w,
            card_h,
            // Fully opaque card — `theme.f32` already carries alpha 1.0.
            theme.f32(theme.surface),
            DEPTH,
            10.0 * scale,
            ORDER_CARD,
        );

        // The QR itself. Modules are painted as plain rects — always on
        // a WHITE field regardless of theme, because scanners expect
        // dark-on-light and a dark-theme inversion fails to read.
        if let Some((_, width, cells)) = self.encoded.as_ref() {
            let qr_x = card_x + (card_w - qr_px) * 0.5;
            let qr_y = card_y + pad + header_h;
            sugarloaf.rect(
                None,
                qr_x,
                qr_y,
                qr_px,
                qr_px,
                [1.0, 1.0, 1.0, 1.0],
                DEPTH,
                ORDER_CARD,
            );
            let origin_x = qr_x + quiet as f32 * module_px;
            let origin_y = qr_y + quiet as f32 * module_px;
            for row in 0..*width {
                for col in 0..*width {
                    if !cells[row * width + col] {
                        continue;
                    }
                    sugarloaf.rect(
                        None,
                        origin_x + col as f32 * module_px,
                        origin_y + row as f32 * module_px,
                        module_px,
                        module_px,
                        [0.0, 0.0, 0.0, 1.0],
                        DEPTH,
                        ORDER_MODULE,
                    );
                }
            }
        }

        // Header and close affordance live above the QR quiet zone.
        let close_size = 44.0 * scale;
        let close_rect = [
            card_x + card_w - close_size - 8.0 * scale,
            card_y + 8.0 * scale,
            close_size,
            close_size,
        ];
        self.close_rect = Some(close_rect);
        {
            let opts = DrawOpts {
                font_size: 13.0 * scale,
                color: theme.u8(theme.fg),
                bold: true,
                ..DrawOpts::default()
            };
            sugarloaf.text_mut().draw(
                card_x + pad,
                card_y + 15.0 * scale,
                "Share with phone",
                &opts,
            );
        }
        sugarloaf.rounded_rect(
            None,
            close_rect[0],
            close_rect[1],
            close_rect[2],
            close_rect[3],
            theme.f32_alpha(theme.fg, 0.10),
            DEPTH,
            close_size * 0.5,
            ORDER_MODULE,
        );
        {
            let opts = DrawOpts {
                font_size: 18.0 * scale,
                color: theme.u8(theme.fg),
                bold: true,
                ..DrawOpts::default()
            };
            // Plain ASCII 'x': a multiplication-sign glyph would depend on
            // font coverage, which is exactly what bit the scanner.
            let w = sugarloaf.text_mut().measure("x", &opts);
            sugarloaf.text_mut().draw(
                close_rect[0] + (close_size - w) * 0.5,
                close_rect[1] + (close_size - opts.font_size) * 0.5,
                "x",
                &opts,
            );
        }

        // A hint is an error fallback, never a footer below a valid QR.
        if !has_qr {
            let hint = self.hint.as_deref().unwrap_or("Share link unavailable");
            let opts = DrawOpts {
                font_size: 11.0 * scale,
                color: theme.u8(theme.muted),
                clip_rect: Some([
                    card_x + pad,
                    card_y + header_h + pad,
                    card_w - pad * 2.0,
                    MESSAGE_H * scale,
                ]),
                ..DrawOpts::default()
            };
            let w = sugarloaf.text_mut().measure(hint, &opts);
            let text_x = card_x + ((card_w - w) * 0.5).max(pad);
            sugarloaf
                .text_mut()
                .draw(text_x, card_y + header_h + pad, hint, &opts);
        }
    }
}

/// Card geometry is intentionally independent of URL/hint text. A valid QR
/// has no footer, so its bottom edge is exactly one padding unit below the
/// white quiet-zone field.
fn share_card_dimensions(qr_px: f32, scale: f32, has_qr: bool) -> (f32, f32) {
    let pad = CARD_PAD * scale;
    let header_h = HEADER_H * scale;
    let body_h = if has_qr { qr_px } else { MESSAGE_H * scale };
    (
        (qr_px + pad * 2.0).max(280.0 * scale),
        header_h + body_h + pad * 2.0,
    )
}

/// `(width, dark-cell grid)` for `text`, or `None` when it can't be
/// encoded (over capacity).
fn encode_qr(text: &str) -> Option<(usize, Vec<bool>)> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let width = code.width();
    let cells = code
        .to_colors()
        .into_iter()
        .map(|c| c == qrcode::Color::Dark)
        .collect();
    Some((width, cells))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_a_url_into_a_square_grid() {
        let (width, cells) = encode_qr("http://192.168.1.20:7878/?token=abc").unwrap();
        assert!(width >= 21, "QR is at least version 1");
        assert_eq!(cells.len(), width * width);
        // Finder pattern: top-left module of a QR is always dark.
        assert!(cells[0]);
    }

    #[test]
    fn show_caches_the_encode_and_hide_clears_visibility() {
        let mut sheet = ShareSheet::new();
        sheet.show("http://10.0.0.5:7878/", None);
        assert!(sheet.is_visible());
        let first = sheet.encoded.clone().unwrap();
        sheet.show("http://10.0.0.5:7878/", None);
        assert_eq!(
            sheet.encoded.clone().unwrap().1,
            first.1,
            "re-encode avoided"
        );
        sheet.hide();
        assert!(!sheet.is_visible());
    }

    #[test]
    fn message_only_mode_has_no_qr() {
        let mut sheet = ShareSheet::new();
        sheet.show_message("No phone-reachable address");
        assert!(sheet.is_visible());
        assert!(sheet.url().is_none());
        assert!(sheet.encoded.is_none());
    }

    #[test]
    fn close_rect_is_cleared_on_hide() {
        let mut sheet = ShareSheet::new();
        sheet.show("http://x/", None);
        // Rects are assigned during render; hiding must drop them so a
        // stale rect can't answer hit tests for an invisible sheet.
        sheet.close_rect = Some([0.0, 0.0, 10.0, 10.0]);
        sheet.hide();
        assert!(sheet.close_button_rect().is_none());
    }

    #[test]
    fn a_click_dismisses_and_consumes() {
        let mut sheet = ShareSheet::new();
        sheet.show("http://x/", None);
        assert!(sheet.handle_click(1.0, 1.0));
        assert!(!sheet.is_visible());
        assert!(
            !sheet.handle_click(1.0, 1.0),
            "hidden sheet consumes nothing"
        );
    }

    #[test]
    fn valid_qr_discards_display_text_and_has_no_footer() {
        let mut sheet = ShareSheet::new();
        sheet.show(
            "http://192.168.1.20:43111/?token=secret&workspace=workspace-1",
            Some("connection guidance".to_string()),
        );
        assert!(sheet.encoded.is_some());
        assert!(sheet.hint.is_none(), "a QR card must not paint hint text");

        let qr_px = 216.0;
        let scale = 1.0;
        let (_, height) = share_card_dimensions(qr_px, scale, true);
        assert_eq!(height, HEADER_H + qr_px + CARD_PAD * 2.0);
    }

    #[test]
    fn message_only_card_reserves_error_body_instead_of_qr_footer() {
        let (_, height) = share_card_dimensions(0.0, 1.0, false);
        assert_eq!(height, HEADER_H + MESSAGE_H + CARD_PAD * 2.0);
    }
}
