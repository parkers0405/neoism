//! Words flowed as separate selectable texts draw one continuous highlight per line.

use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, TextStyle, row, text};

fn view(_: &(), _: &mut Cx<()>) -> El {
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    let hl = rune::color::hex("#3080ff").unwrap().alpha(0.3);
    let words = ["one ", "two ", "three ", "four"].map(|w| text(w).font(style).selectable(true).selection_color(hl));
    row().w_full().padding(10.0).selection_group(true).children(words)
}

#[test]
fn a_run_of_words_is_one_rect() {
    let mut rt = Runtime::new((), view, (400.0, 100.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Command('a'), mods: Mods { cmd: true, ..Mods::default() } });
    rt.frame(1.0 / 60.0);
    let want = rune::color::hex("#3080ff").unwrap().alpha(0.3).premul(1.0);
    let n = rt.instances.iter().filter(|i| i.fill.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-3)).count();
    assert_eq!(n, 1, "four words, one highlight");
}

fn chipped(_: &(), _: &mut Cx<()>) -> El {
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    let big = TextStyle::new(11.0, 400.0).lh(14.0);
    let hl = rune::color::hex("#3080ff").unwrap().alpha(0.3);
    let chip = row().padding(6.0).child(text("abc").font(big).selectable(true).selection_color(hl));
    row().w_full().padding(10.0).selection_group(true)
        .child(text("one ").font(style).selectable(true).selection_color(hl))
        .child(chip)
        .child(text(" two").font(style).selectable(true).selection_color(hl))
}

#[test]
fn a_chip_does_not_break_the_run() {
    let mut rt = Runtime::new((), chipped, (400.0, 100.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Command('a'), mods: Mods { cmd: true, ..Mods::default() } });
    rt.frame(1.0 / 60.0);
    let want = rune::color::hex("#3080ff").unwrap().alpha(0.3).premul(1.0);
    let n = rt.instances.iter().filter(|i| i.fill.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-3)).count();
    assert_eq!(n, 1, "words and the chip between them, one highlight");
}
