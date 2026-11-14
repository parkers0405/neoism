use super::*;

const TOKEN: &str = "sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
const WORDS: &str = "a plain sentence that wraps at spaces across a few short lines";

fn lines(l: &Layout<Ink>) -> Vec<std::ops::Range<usize>> {
    l.lines().map(|line| line.text_range()).collect()
}

#[test]
fn wrap_anywhere_breaks_an_unbroken_word_inside_the_width() {
    let mut ts = TextSystem::default();
    let style = TextStyle::new(14.0, 400.0);
    let (scale, max_w) = (2.0, 160.0 * 2.0);
    let clipped = ts.layout_rich(TOKEN, &[], style, scale, Some(max_w));
    assert_eq!(clipped.len(), 1, "without wrap-anywhere the token stays on one line");
    assert!(clipped.full_width() > max_w);

    let spans = [Span::new(0, 6).bold()];
    let opts = RichLayoutOptions::new().wrap_anywhere();
    let wrapped = ts.layout_rich_with(TOKEN, &spans, style, scale, Some(max_w), opts);
    assert!(wrapped.len() >= 3, "the token wraps over several lines (got {})", wrapped.len());
    for line in wrapped.lines() {
        assert!(line.metrics().advance <= max_w + 0.5, "line {:?} fits {max_w}", line.text_range());
    }
    let r = lines(&wrapped);
    assert_eq!((r[0].start, r.last().unwrap().end), (0, TOKEN.len()));
    assert!(r.windows(2).all(|w| w[0].end == w[1].start), "lines cover the token without gaps");
}

#[test]
fn wrap_anywhere_keeps_word_breaks_and_defaults_match_layout_rich() {
    let mut ts = TextSystem::default();
    let style = TextStyle::new(14.0, 400.0);
    let plain = ts.layout_rich(WORDS, &[], style, 2.0, Some(240.0));
    let anywhere = ts.layout_rich_with(WORDS, &[], style, 2.0, Some(240.0), RichLayoutOptions::new().wrap_anywhere());
    assert!(plain.len() > 1);
    assert_eq!(lines(&plain), lines(&anywhere), "words that fit break at spaces either way");

    let default = ts.layout_rich_with(WORDS, &[], style, 2.0, Some(240.0), RichLayoutOptions::default());
    assert!(Rc::ptr_eq(&plain, &default), "default options share the layout_rich cache entry");
    let aligned = ts.layout_aligned(WORDS, &[], style, 2.0, Some(240.0), Alignment::Center);
    let with = ts.layout_rich_with(WORDS, &[], style, 2.0, Some(240.0), RichLayoutOptions::new().align(Alignment::Center));
    assert!(Rc::ptr_eq(&aligned, &with), "align matches layout_aligned");
}
