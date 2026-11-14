use super::*;
use crate::edit::Block;

/// Every line's range and metrics, and every glyph and inline box with its position, as bits.
fn geometry(l: &Layout<Ink>) -> Vec<Vec<u32>> {
    let mut out = vec![vec![l.full_width().to_bits(), l.width().to_bits(), l.height().to_bits(), l.len() as u32]];
    for line in l.lines() {
        let m = line.metrics();
        let r = line.text_range();
        out.push(vec![r.start as u32, r.end as u32, m.offset.to_bits(), m.baseline.to_bits(), m.advance.to_bits(), m.inline_min_coord.to_bits()]);
        for item in line.items() {
            match item {
                PositionedLayoutItem::GlyphRun(run) => {
                    let mut g = vec![run.run().font().index, run.run().font_size().to_bits(), run.baseline().to_bits(), run.offset().to_bits()];
                    g.extend(run.positioned_glyphs().flat_map(|p| [p.id, p.x.to_bits(), p.y.to_bits()]));
                    out.push(g);
                }
                PositionedLayoutItem::InlineBox(b) => out.push(vec![b.id as u32, b.x.to_bits(), b.y.to_bits()]),
            }
        }
    }
    out
}

struct Case {
    name: &'static str,
    text: String,
    spans: Vec<Span>,
    blocks: Vec<Block>,
    pills: Vec<PillChip>,
    style: TextStyle,
    align: Alignment,
    wrap_anywhere: bool,
}

fn case(name: &'static str, text: &str, style: TextStyle) -> Case {
    Case { name, text: text.into(), spans: Vec::new(), blocks: Vec::new(), pills: Vec::new(), style, align: Alignment::Start, wrap_anywhere: false }
}

fn cases() -> Vec<Case> {
    let body = TextStyle::new(15.0, 400.0).lh(22.0);
    let rich = "The quick brown fox jumps over the_lazy_dog and keeps running past \
        https://example.com/a/very/long/unbreakable/path/that/overflows/any/line while larger words sit beside it.";
    let at = |s: &str| rich.find(s).unwrap();
    let mut spans = vec![
        Span::new(at("jumps"), at("jumps") + 5).bold(),
        Span::new(at("the_lazy_dog"), at("the_lazy_dog") + 12).mono(),
        Span::new(at("keeps"), at("keeps") + 5).italic(),
        Span::new(at("larger"), at("larger") + 12).size(22.0),
    ];
    spans.sort_by_key(|s| s.start);
    let paragraphs = "First a plain paragraph that wraps over a few lines of text.\nA bullet item long enough to wrap onto a second line at most widths.\nA quoted line that also wraps when the width narrows enough.\nlet code = run(\"a long line of code\");";
    let pill_text = "ask @code-reviewer about @src/main.rs and @notes today please, then wrap";
    let pills = ["@code-reviewer", "@src/main.rs", "@notes"]
        .iter()
        .map(|label| {
            let start = pill_text.find(label).unwrap() as u32;
            PillChip { start, end: start + label.len() as u32, icon: None, bg: None, fg: None, bare: None }
        })
        .collect();
    vec![
        Case { spans: spans.clone(), ..case("spans", rich, body) },
        Case { spans: spans.clone(), wrap_anywhere: true, ..case("wrap anywhere", rich, body) },
        Case { spans: spans.clone(), align: Alignment::Center, ..case("centred", rich, body) },
        Case { spans: spans.clone(), align: Alignment::End, ..case("end", rich, body) },
        Case { spans, align: Alignment::Justify, ..case("justified", rich, body) },
        case("right to left", "مرحبا بالعالم هذا نص طويل يلتف عبر عدة أسطر في التخطيط، ثم שלום עולם וטקסט נוסף", body),
        Case { align: Alignment::Justify, ..case("right to left justified", "مرحبا بالعالم هذا نص طويل يلتف عبر عدة أسطر في التخطيط والمزيد من الكلمات هنا", body) },
        case("mixed fonts", "Mixed 中文字符和日本語のテキスト with emoji 🎉🚀 and math 𝑥² ∑ beside Latin words that wrap", body),
        case("mono tabs", "fn main() {\n\tlet x = 1;\t// a comment that runs long enough to wrap\n}", TextStyle::new(13.0, 400.0).family(Family::Mono)),
        Case { blocks: vec![Block::Paragraph, Block::Bullet(0), Block::Quote, Block::Code], ..case("blocks", paragraphs, body) },
        Case { pills, ..case("pills", pill_text, TextStyle::new(14.0, 400.0)) },
    ]
}

fn lay(ts: &mut TextSystem, c: &Case, width: Option<f32>) -> Rc<Layout<Ink>> {
    ts.layout_blocks_inner(&c.text, &c.spans, &c.blocks, &c.pills, c.style, 2.0, width, c.align, c.wrap_anywhere, true)
}

/// A text shaped once is re-broken, not reshaped, at every other width, and lands every
/// line, glyph and box where laying it out from scratch at that width does.
#[test]
fn a_new_width_rebreaks_without_shaping_and_matches_a_fresh_layout() {
    let (mut ts, mut fresh) = (TextSystem::default(), TextSystem::default());
    let widths = [Some(1200.0), Some(601.0), Some(433.5), Some(210.0), Some(80.0), Some(0.0), None, Some(900.0)];
    for c in cases() {
        let wide = lay(&mut ts, &c, Some(1800.0));
        let wide_geom = geometry(&wide);
        for width in widths {
            let (shaped, rebroken) = (ts.shaped, ts.rebroken);
            let l = lay(&mut ts, &c, width);
            assert_eq!((ts.shaped, ts.rebroken), (shaped, rebroken + 1), "{} at {width:?}: re-broken, not shaped", c.name);
            assert_eq!(geometry(&wide), wide_geom, "{}: re-breaking a new width must not mutate an earlier layout in place", c.name);
            // The same text shaped from scratch at this width, both in `ts` and in a separate TextSystem.
            let cached = (std::mem::take(&mut ts.layouts), std::mem::take(&mut ts.shapes));
            let f = lay(&mut ts, &c, width);
            assert_eq!(ts.shaped, shaped + 1);
            (ts.layouts, ts.shapes) = cached;
            fresh.layouts.clear();
            fresh.shapes.clear();
            let f_other = lay(&mut fresh, &c, width);
            let (a, b, b_other) = (geometry(&l), geometry(&f), geometry(&f_other));
            let first = a.iter().zip(&b).position(|(x, y)| x != y);
            assert!(a == b, "{} at {width:?}: row {first:?} of {}: {:?} vs {:?}", c.name, a.len(),
                first.map(|i| &a[i]), first.map(|i| &b[i]));
            assert!(a == b_other, "{} at {width:?}: fresh TextSystem chose different fallback fonts", c.name);
        }
        // An earlier width from the same commit is still in `layouts` and hits without re-breaking.
        let (shaped, rebroken) = (ts.shaped, ts.rebroken);
        assert!(lay(&mut ts, &c, Some(210.0)).len() > 1, "{} wraps", c.name);
        assert_eq!((ts.shaped, ts.rebroken), (shaped, rebroken), "{}: cached width hits without re-breaking", c.name);
    }
}

#[test]
fn a_text_style_span_or_scale_change_still_shapes() {
    let mut ts = TextSystem::default();
    let style = TextStyle::new(15.0, 400.0);
    let text = "a paragraph long enough to wrap at a narrow width";
    ts.layout_rich(text, &[], style, 2.0, Some(400.0));
    let shapes = |ts: &mut TextSystem, f: &dyn Fn(&mut TextSystem)| {
        let n = ts.shaped;
        f(ts);
        ts.shaped - n
    };
    assert_eq!(shapes(&mut ts, &|ts| { ts.layout_rich(text, &[], style, 2.0, Some(300.0)); }), 0);
    assert_eq!(shapes(&mut ts, &|ts| { ts.layout_rich("another paragraph entirely", &[], style, 2.0, Some(300.0)); }), 1);
    assert_eq!(shapes(&mut ts, &|ts| { ts.layout_rich(text, &[], TextStyle::new(15.0, 600.0), 2.0, Some(300.0)); }), 1);
    assert_eq!(shapes(&mut ts, &|ts| { ts.layout_rich(text, &[Span::new(0, 1).bold()], style, 2.0, Some(300.0)); }), 1);
    assert_eq!(shapes(&mut ts, &|ts| { ts.layout_rich(text, &[], style, 3.0, Some(300.0)); }), 1);
    assert_eq!(shapes(&mut ts, &|ts| { ts.layout_blocks_inner(text, &[], &[], &[], style, 2.0, Some(300.0), Alignment::Start, true, true); }), 1);
    assert_eq!(shapes(&mut ts, &|ts| { ts.layout_blocks_inner(text, &[], &[], &[], style, 2.0, Some(200.0), Alignment::Start, true, true); }), 0);
}

/// The re-break index follows the layouts cache: it survives commits while a width of the
/// text is in use, and goes with the last of its layouts.
#[test]
fn the_rebreak_index_keeps_to_the_layout_cache() {
    let mut ts = TextSystem::default();
    let style = TextStyle::new(15.0, 400.0);
    let text = "a paragraph long enough to wrap at a narrow width";
    ts.layout_rich(text, &[], style, 2.0, Some(400.0));
    ts.layout_rich(text, &[], style, 2.0, Some(300.0));
    for _ in 0..6 {
        ts.layout_rich(text, &[], style, 2.0, Some(400.0));
        ts.end_commit();
    }
    let shaped = ts.shaped;
    ts.layout_rich(text, &[], style, 2.0, Some(250.0));
    assert_eq!(ts.shaped, shaped, "the width in use is the one re-broken");
    for i in 0..2000 {
        ts.layout_rich(&format!("filler label {i} with a few words"), &[], style, 2.0, Some(300.0));
        ts.end_commit();
    }
    assert!(ts.shapes.len() <= ts.layouts.len());
    assert!(ts.shapes.values().all(|k| ts.layouts.contains_key(k)));
}

/// A window narrowing re-breaks its wrapped texts without shaping them, and draws what a
/// window opened at that width draws.
#[test]
fn resizing_a_window_rebreaks_wrapped_text_and_draws_as_a_fresh_one() {
    use crate::{Cx, El, Env, Runtime, col};
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let para = "Paragraph text long enough to wrap over several lines as the window narrows, with words of varied length.";
        col().w_full().padding(12.0).gap(8.0).children((0..6).map(|i| {
            let t = crate::el::text(format!("{i}. {para}")).w_full().wrap();
            if i % 2 == 0 { t.align(crate::Align::Center) } else { t }
        }))
    }
    let glyphs = |rt: &Runtime<()>| {
        rt.instances.iter().filter(|i| i.params[1] == crate::render::KIND_GLYPH).map(|i| i.rect.map(f32::to_bits)).collect::<Vec<_>>()
    };
    let mut rt = Runtime::new((), view, (700.0, 900.0), Env::new(true, 2.0));
    rt.frame(0.0);
    for w in [640.0, 517.5, 380.0, 700.0, 233.0] {
        rt.set_viewport((w, 900.0), 2.0);
        rt.frame(1.0 / 120.0);
        assert_eq!(rt.stats.shaped, 0, "at {w}");
        let mut fresh = Runtime::new((), view, (w, 900.0), Env::new(true, 2.0));
        fresh.frame(0.0);
        assert_eq!(glyphs(&rt), glyphs(&fresh), "at {w}");
    }
}

/// A width change re-breaks a layout no one holds, from an earlier commit, in place: it moves
/// to the new width rather than being copied. One still held, or handed out this commit, is
/// copied and left as it was.
#[test]
fn a_layout_nothing_holds_rebreaks_in_place_at_a_new_width() {
    let mut ts = TextSystem::default();
    let style = TextStyle::new(15.0, 400.0);
    let text = "a paragraph long enough to wrap over several lines at a narrow width";
    let wide = ts.layout_rich(text, &[], style, 2.0, Some(400.0));
    let at = Rc::as_ptr(&wide);
    drop(wide);
    ts.end_commit();
    let narrow = ts.layout_rich(text, &[], style, 2.0, Some(300.0));
    assert_eq!(Rc::as_ptr(&narrow), at, "moved, not copied");
    assert_eq!(ts.cached_layout_count(), 1, "the old width's entry moved with it");
    assert_eq!(geometry(&narrow), geometry(&TextSystem::default().layout_rich(text, &[], style, 2.0, Some(300.0))));

    // Handed out this commit: copied, and the width it was at still hits.
    let (shaped, rebroken) = (ts.shaped, ts.rebroken);
    let narrower = ts.layout_rich(text, &[], style, 2.0, Some(200.0));
    assert_ne!(Rc::as_ptr(&narrower), at);
    assert!(Rc::ptr_eq(&ts.layout_rich(text, &[], style, 2.0, Some(300.0)), &narrow));
    assert_eq!((ts.shaped, ts.rebroken), (shaped, rebroken + 1));

    // Held from an earlier commit: copied, and the holder's layout is untouched.
    ts.end_commit();
    let before = geometry(&narrower);
    let other = ts.layout_rich(text, &[], style, 2.0, Some(150.0));
    assert!(!Rc::ptr_eq(&other, &narrower));
    assert_eq!(geometry(&narrower), before);
}
