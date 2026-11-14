//! Text coverage: colour emoji, math alphanumerics and aligned wrapped text.
//! `cargo run -p rune --example text_coverage -- --snap emoji.png --scene emoji --theme dark`

use rune::tok::{self, TextStyle};
use rune::{Align, AppSpec, Cx, El, col, row, text};

#[derive(Default)]
struct State {
    scene: Scene,
}

#[derive(Default, Clone, Copy, PartialEq)]
enum Scene {
    #[default]
    Emoji,
    Math,
    Align,
}

fn view(s: &State, cx: &mut Cx<State>) -> El {
    let t = tok::theme(cx.system_dark());
    let line = |content: &str, size: f32| text(content).font(TextStyle::new(size, 400.0)).color(t.text);
    let note = |content: &str| text(content).font(TextStyle::new(12.0, 400.0)).color(t.text_secondary);
    // A card shows the element's width, so each line's alignment inside it is visible.
    let para = |content: &str, align: Align| {
        text(content).font(TextStyle::new(13.0, 400.0)).color(t.text).wrap().w_full().align(align)
            .fill(t.card).padding(8.0).radius(6.0)
    };
    let pair = |a: El, b: El| row().w_full().gap(12.0).children(vec![a.w(250.0), b.w(250.0)]);
    let rows: Vec<El> = match s.scene {
        Scene::Emoji => vec![
            note("ZWJ family, flag, skin tone, ❤️, keycap"),
            line("Family 👨‍👩‍👧‍👦 flag 🇯🇵 wave 👋🏽 love ❤️ key 1️⃣", 17.0),
            line("Small 👨‍👩‍👧‍👦🇯🇵👋🏽❤️ at 12pt", 12.0),
            line("Large 👨‍👩‍👧‍👦🇯🇵👋🏽❤️", 40.0),
            line("Faded 👨‍👩‍👧‍👦 🇯🇵 👋🏽 ❤️ (opacity 0.4)", 17.0).opacity(0.4),
            note("Plain ❤ and ☺︎ stay text symbols"),
            line("Text ❤ ☺︎ ⌘ ⇧", 17.0),
        ],
        Scene::Math => vec![
            note("Mathematical Alphanumeric Symbols"),
            line("𝐁𝐨𝐥𝐝 𝐴𝑙𝑔𝑒𝑏𝑟𝑎 𝓢𝓬𝓻𝓲𝓹𝓽 𝔉𝔯𝔞𝔨𝔱𝔲𝔯 𝕯𝖔𝖚𝖇𝖑𝖊", 17.0),
            line("𝔸 𝔹 ℂ 𝔻 𝕖 𝕗 𝟘𝟙𝟚 𝚖𝚘𝚗𝚘 𝖲𝖺𝗇𝗌 𝛼𝛽𝛾 𝜕𝑓/𝜕𝑥", 17.0),
            line("Let 𝑓(𝑥) = 𝑎𝑥² + 𝑏𝑥 + 𝑐 for 𝑥 ∈ ℝ", 17.0),
            note("Enclosed and other symbols"),
            line("Ⓐ Ⓑ ① ② ⑴ ⒜ 🄰 🅐 ⠁⠃⠉ 🁣 🜁 ∮ ∯ ⨀ ⨁", 17.0),
        ],
        Scene::Align => {
            let latin = "Wrapped lines align one by one inside the element's width, so a short last line sits where the alignment puts it.";
            let hebrew = "שורות עוטפות מתיישרות אחת אחת בתוך רוחב האלמנט, כך שהשורה האחרונה הקצרה יושבת במקום הנכון.";
            vec![
                pair(note("Center"), note("Justify")),
                pair(para(latin, Align::Center), para(latin, Align::Start).justify()),
                pair(note("Right-to-left Start"), note("Right-to-left Justify")),
                pair(para(hebrew, Align::Start), para(hebrew, Align::Start).justify()),
            ]
        }
    };
    col().w_full().h_full().padding(24.0).gap(12.0).fill(t.bg).children(rows)
}

fn main() {
    rune::run(AppSpec::<State> {
        scenes: &[("emoji", |s| s.scene = Scene::Emoji), ("math", |s| s.scene = Scene::Math),
            ("align", |s| s.scene = Scene::Align)],
        ..AppSpec::new("Text coverage", (560.0, 360.0), State::default, view)
    });
}
