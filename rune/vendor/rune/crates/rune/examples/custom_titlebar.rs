//! A window that draws its own titlebar (`App::titlebar`): a 44 point band that drags the
//! window, with buttons in it that keep their presses. macOS keeps its traffic lights at the
//! leading edge; Windows draws the caption buttons at the trailing edge, with snap layouts
//! over maximize. `Cx::chrome_insets` says what each takes. Press "Theme" to switch the app's
//! colours away from the system's: the caption buttons follow through `Cx::caption_color`.
//! "Zoom" scales the band with `Cx::titlebar_height`; "Dialog" opens a modal, which leaves the
//! window's controls usable. Alt+Space in the field opens the window menu on Windows and
//! types nothing.
use rune::*;

const HEIGHT: f32 = 44.0;
const ZOOMS: [f32; 3] = [1.0, 1.25, 0.8];

struct State { dark: Option<bool>, presses: u32, zoom: usize, dialog: bool, note: TextEdit }

fn view(s: &State, cx: &mut Cx<State>) -> El {
    let dark = s.dark.unwrap_or(cx.system_dark());
    let (bg, band, ink, faint) = if dark {
        (oklch(0.18, 0.0, 0.0), oklch(0.22, 0.0, 0.0), oklch(0.95, 0.0, 0.0), oklch(0.65, 0.0, 0.0))
    } else {
        (oklch(0.99, 0.0, 0.0), oklch(0.95, 0.0, 0.0), oklch(0.2, 0.0, 0.0), oklch(0.5, 0.0, 0.0))
    };
    cx.caption_color(if dark { Color::WHITE } else { Color::BLACK });
    // The band scales with the zoom, and the caption buttons with it.
    let band_height = HEIGHT * ZOOMS[s.zoom];
    cx.titlebar_height(band_height);
    let insets = cx.chrome_insets();
    let button = |key: &'static str, label: &'static str| row().key(key).px(10.0).h(28.0).radius(6.0).center_y()
        .fill(faint.alpha(0.2)).hover(faint.alpha(0.3)).press(faint.alpha(0.45)).role("button").label(label)
        .child(text(label).color(ink));
    let titlebar = row().key("titlebar").w_full().h(band_height).fill(band).center_y().window_drag()
        .pl(insets.leading + 12.0).pr(insets.trailing + 12.0).gap(8.0)
        .child(text("Custom titlebar").color(ink))
        .child(el().grow(1.0))
        .child(button("zoom", "Zoom").on_press(|s: &mut State| s.zoom = (s.zoom + 1) % ZOOMS.len()))
        .child(button("dialog", "Dialog").on_press(|s: &mut State| s.dialog = true))
        .child(button("theme", "Theme").on_press(move |s: &mut State| { s.dark = Some(!dark); s.presses += 1; }));
    let line = |t: String| text(t).color(faint);
    let note = text_input(&s.note, TextStyle::new(14.0, 400.0)).key("note").single_line().w(320.0).padding(8.0).radius(6.0)
        .fill(band).color(ink).placeholder("Type here, then Alt+Space", faint).label("Note").on_edit(|s: &mut State| &mut s.note);
    let dialog = s.dialog.then(|| {
        el().key("scrim").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).fill(Color::BLACK.alpha(0.35)).center()
            .child(col().key("sheet").padding(20.0).gap(12.0).radius(10.0).fill(bg).role("dialog").label("Dialog")
                .modal(true, |s: &mut State| s.dialog = false)
                .child(text("A modal. The caption buttons still work.").color(ink))
                .child(button("close-dialog", "Close").on_press(|s: &mut State| s.dialog = false)))
    });
    col().key("root").w_full().h_full().fill(bg)
        .child(titlebar)
        .child(col().padding(24.0).gap(8.0)
            .child(text("Drag the band to move the window; double-click it to maximize; right-click it for the window menu.").color(ink).wrap())
            .child(note)
            .child(line(format!("chrome insets: leading {:.0}, trailing {:.0}, height {:.0}", insets.leading, insets.trailing, insets.height)))
            .child(line(format!("scale {:.2}, theme presses {}", cx.env().scale, s.presses))))
        .children(dialog)
}

fn main() {
    let init = || State { dark: None, presses: 0, zoom: 0, dialog: false, note: TextEdit::new("") };
    App::new(AppSpec::new("Custom titlebar", (720.0, 420.0), init, view))
        .titlebar(Titlebar::new(HEIGHT))
        .run();
}
