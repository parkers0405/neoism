use crate::color::Color;
use crate::el::{PillStyle, rich, text, text_input};
use crate::text::{Span, TextSystem};
use parley::Alignment;
use crate::{TextEdit, TextStyle};

const INK: Color = Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };
const CHIP: Color = Color { r: 0.0, g: 0.0, b: 1.0, a: 0.2 };

#[test]
fn pills_record_sorted_chips_and_skip_bad_ranges() {
    let body = "ask @code-reviewer about @src/main.rs — é";
    let at = |s: &str| body.find(s).unwrap();
    let agent = at("@code-reviewer")..at("@code-reviewer") + 14;
    let file = at("@src/main.rs")..at("@src/main.rs") + 12;
    let e = text(body).pills([
        (file.clone(), PillStyle::new().bg(CHIP)),
        (agent.clone(), PillStyle::new().fg(INK)),
        (agent.start + 1..agent.end + 2, PillStyle::new()),
        (5..5, PillStyle::new()),
        (body.len() - 1..body.len(), PillStyle::new()),
        (0..body.len() + 1, PillStyle::new()),
    ]);
    let t = e.text.as_ref().unwrap();
    let got: Vec<(u32, u32, Option<Color>, Option<Color>)> = t.pills.iter().map(|p| (p.start, p.end, p.bg, p.fg)).collect();
    assert_eq!(got, [(agent.start as u32, agent.end as u32, None, Some(INK)), (file.start as u32, file.end as u32, Some(CHIP), None)]);
}

#[test]
fn a_pill_colours_its_label_and_keeps_it_through_later_spans() {
    let body = "ask @reviewer to look";
    let e = rich(body, vec![Span::new(0, 3).bold()]).pill(4..13, PillStyle::new().fg(INK)).spans(vec![Span::new(14, 16).italic()]);
    let t = e.text.as_ref().unwrap();
    let ink: Vec<(u32, u32)> = t.spans.iter().filter(|s| s.color == Some(INK)).map(|s| (s.start, s.end)).collect();
    assert_eq!(ink, [(4, 13)]);
    assert!(t.spans.iter().any(|s| s.italic && (s.start, s.end) == (14, 16)));
}

#[test]
fn a_field_ignores_static_pills() {
    let e = text_input(&TextEdit::new("ask @reviewer"), TextStyle::new(14.0, 400.0)).pill(4..13, PillStyle::new().fg(INK));
    assert!(e.text.as_ref().unwrap().pills.is_empty());
}

#[test]
fn a_pill_label_stays_on_one_line_and_takes_room() {
    let body = "please ask @recursive-self-improvement about it";
    let at = body.find('@').unwrap();
    let end = at + "@recursive-self-improvement".len();
    let mut ts = TextSystem::default();
    let plain = text(body).wrap();
    let pilled = text(body).wrap().pill(at..end, PillStyle::new());
    let mut width = |e: &crate::El, w| ts.layout_for_spec(e.text.as_ref().unwrap(), 1.0, w, Alignment::Start).full_width();
    let (wide_plain, wide_pill) = (width(&plain, None), width(&pilled, None));
    assert!(wide_pill > wide_plain + 4.0, "the chip's padding takes room: {wide_plain} vs {wide_pill}");
    // Narrow enough that plain text breaks at the label's hyphens, wide enough for the pill.
    let layout = ts.layout_for_spec(pilled.text.as_ref().unwrap(), 1.0, Some(210.0), Alignment::Start);
    let split = layout.lines().any(|l| {
        let r = l.text_range();
        r.start > at && r.start < end
    });
    assert!(!split, "a label that fits a line never splits across lines");
    let narrow = ts.layout_for_spec(pilled.text.as_ref().unwrap(), 1.0, Some(110.0), Alignment::Start);
    assert!(narrow.width() <= 110.5, "one wider than the line wraps inside it: {}", narrow.width());
}

// In a running app: selection, copy, wrapping and streaming over pilled text.

mod live {
    use std::ops::Range;

    use crate::el::{PillStyle, col, text};
    use crate::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, TextStyle};

    struct S {
        body: String,
        pills: Vec<Range<usize>>,
        width: f32,
    }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let t = text(s.body.clone()).key("t").font(TextStyle::new(14.0, 400.0).lh(20.0)).w(s.width).wrap().selectable(true);
        col().key("root").padding(10.0).child(t.pills(s.pills.iter().map(|r| (r.clone(), PillStyle::new()))))
    }

    fn runtime(body: &str, pills: &[Range<usize>], width: f32) -> Runtime<S> {
        let s = S { body: body.into(), pills: pills.to_vec(), width };
        let mut rt = Runtime::new(s, view, (600.0, 400.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt
    }

    fn rect(rt: &Runtime<S>) -> [f32; 4] {
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some("t")).unwrap().rect
    }

    /// Each line's byte range in the text's layout.
    fn lines(rt: &Runtime<S>) -> Vec<Range<usize>> {
        let n = rt.nodes.iter().find(|n| n.key.as_deref() == Some("t")).unwrap();
        n.text_layout.as_ref().unwrap().lines().map(|l| l.text_range()).collect()
    }

    fn drag(rt: &mut Runtime<S>, from: (f32, f32), to: (f32, f32)) {
        rt.pointer_down(from.0, from.1);
        rt.pointer_move(to.0, to.1);
        rt.pointer_up(to.0, to.1);
        rt.frame(0.0);
    }

    #[test]
    fn a_selection_and_copy_across_a_pill_give_its_label_text() {
        let body = "ask @reviewer to look";
        let mut rt = runtime(body, &[4..13], 400.0);
        let r = rect(&rt);
        let y = r[1] + r[3] * 0.5;
        drag(&mut rt, (r[0] + 1.0, y), (r[0] + r[2] - 1.0, y));
        assert_eq!(rt.selected_text().as_deref(), Some(body));
        // The highlight follows the selected range, which covers the whole label.
        let (_, range) = rt.text_selection().unwrap();
        assert!(range.start <= 4 && range.end >= 13, "{range:?}");
        rt.key(KeyEvent { key: Key::Command('c'), mods: Mods { cmd: true, ..Mods::default() } });
        rt.frame(0.0);
        let copied = rt.copy_request.as_ref().map(|w| w.content().text().to_owned());
        assert_eq!(copied.as_deref(), Some(body));
    }

    #[test]
    fn every_pill_label_stays_on_one_line() {
        let body = "ping @recursive-self-improvement and @code-review-standards-and-spec today";
        let at = |s: &str| body.find(s).unwrap()..body.find(s).unwrap() + s.len();
        let pills = [at("@recursive-self-improvement"), at("@code-review-standards-and-spec")];
        let rt = runtime(body, &pills, 260.0);
        let ls = lines(&rt);
        assert!(ls.len() >= 2, "narrow enough to wrap: {ls:?}");
        for p in &pills {
            assert!(!ls.iter().any(|l| l.start > p.start && l.start < p.end), "{p:?} splits across {ls:?}");
        }
    }

    #[test]
    fn pills_survive_streaming_appends_and_bad_ranges_are_ignored() {
        let mut rt = runtime("ask @reviewer", &[4..13], 400.0);
        let pilled = |rt: &Runtime<S>| {
            let n = rt.nodes.iter().find(|n| n.key.as_deref() == Some("t")).unwrap();
            n.text.as_ref().unwrap().pills.iter().map(|p| p.start..p.end).collect::<Vec<_>>()
        };
        let mut w = rect(&rt)[2];
        for more in [" to", " look at", " the é diff"] {
            rt.update(|s, _| s.body.push_str(more));
            rt.frame(1.0 / 60.0);
            assert_eq!(pilled(&rt), [4..13 as u32], "after {more:?}");
            assert!(rect(&rt)[2] >= w);
            w = rect(&rt)[2];
        }
        // Past the end, inside "é", or empty: skipped, and the frame still draws.
        let e = rt.state.body.find('é').unwrap();
        rt.update(|s, _| s.pills = vec![4..13, e + 1..e + 2, 30..400, 20..20]);
        rt.frame(2.0 / 60.0);
        assert_eq!(pilled(&rt), [4..13 as u32]);
        rt.update(|s, _| s.body.truncate(8));
        rt.frame(3.0 / 60.0);
        assert!(pilled(&rt).is_empty(), "a range the text no longer reaches is dropped");
    }
}
