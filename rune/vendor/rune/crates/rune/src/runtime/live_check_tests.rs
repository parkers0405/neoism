//! Debug builds' check of `Tx::live_only` handlers: what its warning names, and hover rows,
//! whose paint changes without a commit, that it must not flag.
use super::*;
use crate::{Cx, El, Env, Key, Mods, col, el, row, text};

/// Which side copies, as a diff's code cells track it: rows on the old side and the new.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Side { Old, New }

struct Rows {
    copy: Side,
    hovered: Vec<usize>,
    changes: usize,
    /// `on_selection_change` calls, which the view shows.
    selections: usize,
}

const ROW_H: f32 = 20.0;

fn side(i: usize) -> Side { if i % 4 == 3 { Side::Old } else { Side::New } }

/// Rows with hover paint whose code cell tracks the hovered side: it calls `Tx::live_only` on
/// the branch that changes nothing, and the copy side picks which rows' text is selectable.
fn rows_view(s: &Rows, _: &mut Cx<Rows>) -> El {
    let rows = (0..24).map(|i| {
        let side = side(i);
        let copy = s.copy == side;
        let mut t = text(format!("let value_{i} = compute({i});")).key("text");
        if copy { t = t.selectable(true).selection_color(Color::WHITE); }
        let code = col().key("code").grow(1.0).min_h(ROW_H).child(t).on_hover_change(move |s: &mut Rows, inside, tx: &mut Tx| {
            if inside { s.hovered.push(i) } else { s.hovered.retain(|&h| h != i) }
            if inside && s.copy != side { s.copy = side; s.changes += 1; } else { tx.live_only(); }
        });
        row().key(format!("r{i}")).w_full().min_h(ROW_H)
            .fill(if copy { Color::BLACK } else { Color::TRANSPARENT })
            .hover(Color::WHITE.alpha(0.06)).hover_group()
            .child(el().key("gutter").w(40.0).h(ROW_H).child(text(i.to_string()).key("n").group_color(Color::WHITE)))
            .child(code)
    });
    col().w_full().h_full().scroll(ScrollSpec { stick_bottom: false })
        .child(text(format!("{} selections", s.selections)).key("selections"))
        .child(col().key("rows").w_full().selection_group(true).children(rows)
            .on_selection_change(|s: &mut Rows, _, _| s.selections += 1))
}

fn rows_app() -> Runtime<Rows> {
    let mut rt = Runtime::new(Rows { copy: Side::New, hovered: Vec::new(), changes: 0, selections: 0 }, rows_view, (400.0, 600.0), Env::new(true, 2.0));
    for _ in 0..60 { rt.frame(1.0 / 60.0); }
    rt
}

/// Below the "selections" line.
fn row_y(i: usize) -> f32 { (i + 1) as f32 * ROW_H + ROW_H / 2.0 }

#[test]
fn hover_rows_at_rest_are_not_a_view_change() {
    let mut rt = rows_app();
    // The new-side rows one after another, each move a quarter second after the last commit:
    // checked, with hover tracks fading in and out. Then the same a frame apart.
    for (n, i) in [0, 1, 2, 4, 5, 6, 8, 9, 10, 12, 13, 14].into_iter().enumerate() {
        rt.frame(if n < 6 { 0.3 } else { 1.0 / 60.0 });
        rt.pointer_move(200.0, row_y(i));
        for _ in 0..3 { rt.frame(1.0 / 60.0); }
    }
    assert!(rt.live_checks >= 6, "the moves were checked: {}", rt.live_checks);
    assert_eq!(rt.state.changes, 0);
    assert!(rt.live_only_warnings.is_empty(), "{:#?}", rt.live_only_warnings);
}

#[test]
fn a_leave_and_an_enter_in_one_frame_blame_neither_when_the_enter_changes_state() {
    let mut rt = rows_app();
    rt.frame(0.3);
    rt.pointer_move(200.0, row_y(2));
    for _ in 0..30 { rt.frame(1.0 / 60.0); }
    // To an old-side row a quarter second on: the leave is live-only and checked, the enter
    // in the same frame changes the copy side, and the commit is the enter's.
    rt.frame(0.3);
    let checks = rt.live_checks;
    rt.pointer_move(200.0, row_y(3));
    rt.frame(1.0 / 60.0);
    assert_eq!((rt.state.copy, rt.state.changes, rt.live_checks - checks), (Side::Old, 1, 1));
    assert!(rt.live_only_warnings.is_empty(), "{:#?}", rt.live_only_warnings);
    let fill = |rt: &Runtime<Rows>, key: &str| rt.nodes.iter().find(|n| n.key.as_deref() == Some(key)).unwrap().vis.fill;
    assert_eq!(fill(&rt, "r3"), Color::BLACK, "the commit built the new copy side");

    // The other order: the changing enter first, then a checked live-only leave.
    rt.frame(0.3);
    let code = |rt: &Runtime<Rows>, row: &str| rt.nodes.iter()
        .find(|n| n.key.as_deref() == Some("code") && n.parent.is_some_and(|p| rt.nodes[p].key.as_deref() == Some(row)))
        .unwrap().path.clone();
    let (enter, leave) = (code(&rt, "r4"), code(&rt, "r3"));
    rt.queue.push(Action::HoverChange(enter, true));
    rt.queue.push(Action::HoverChange(leave, false));
    rt.frame(1.0 / 60.0);
    assert_eq!((rt.state.copy, rt.state.changes), (Side::New, 2));
    assert!(rt.live_only_warnings.is_empty(), "{:#?}", rt.live_only_warnings);
    assert_eq!(fill(&rt, "r4"), Color::BLACK);
}

#[test]
fn dragging_a_text_selection_across_hover_rows_blames_no_hover_handler() {
    let mut rt = rows_app();
    rt.pointer_move(60.0, row_y(0));
    rt.pointer_down(60.0, row_y(0));
    for _ in 0..30 { rt.frame(1.0 / 60.0); }
    // Each move enters a new-side row (a live-only hover change, checked a quarter second on)
    // and grows the selection, whose handler changes what the view shows.
    for i in [1, 2, 4, 5] {
        rt.frame(0.3);
        rt.pointer_move(200.0, row_y(i));
        rt.frame(1.0 / 60.0);
    }
    rt.pointer_up(200.0, row_y(5));
    rt.frame(1.0 / 60.0);
    assert!(rt.state.selections > 1 && rt.live_checks >= 4, "{} selections, {} checks", rt.state.selections, rt.live_checks);
    assert!(rt.live_only_warnings.is_empty(), "{:#?}", rt.live_only_warnings);
}

/// A view that builds one thing differently once a live-only key handler (wrongly) flips
/// `changed`.
struct Case {
    case: u8,
    changed: bool,
}

fn case_view(s: &Case, _: &mut Cx<Case>) -> El {
    let on = s.changed;
    let mut c = col().key("list").w(200.0)
        .child(el().key("box").w(if on && s.case == 0 { 60.0 } else { 40.0 }).h(20.0)
            .fill(if on && s.case == 1 { Color::WHITE } else { Color::BLACK }))
        .child(text(if on && s.case == 2 { "a much longer label than the one before it, cut short" } else { "label" }).key("label").w(200.0).h(20.0));
    if on && s.case == 3 { c = c.child(el().key("extra").size(10.0, 10.0)); }
    if !(on && s.case == 4) { c = c.child(el().key("tail").size(10.0, 10.0)); }
    c
}

fn case_app(case: u8) -> Runtime<Case> {
    let mut rt = Runtime::new(Case { case, changed: false }, case_view, (400.0, 400.0), Env::new(true, 1.0));
    rt.on_key = Some(|s, _, tx| { s.changed = !s.changed; tx.live_only(); true });
    rt.frame(0.0);
    rt
}

fn list_path(rt: &Runtime<Case>) -> String {
    rt.nodes.iter().find(|n| n.key.as_deref() == Some("list")).unwrap().path.clone()
}

#[test]
fn the_warning_names_the_element_and_property_that_differed() {
    let want = [
        r#"layout rect of "{list}/box" went from [0.0, 0.0, 40.0, 20.0] to [0.0, 0.0, 60.0, 20.0]"#,
        r##"fill of "{list}/box" went from #000000ff to #ffffffff"##,
        r#"text of "{list}/label" went from "label" to "a much longer label than the one before …""#,
        r#"added element of "{list}/extra""#,
        r#"removed element of "{list}/tail""#,
    ];
    for (case, want) in want.into_iter().enumerate() {
        let mut rt = case_app(case as u8);
        let want = want.replace("{list}", &list_path(&rt));
        rt.frame(0.5);
        rt.key(KeyEvent { key: Key::Right, mods: Mods::default() });
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.live_only_warnings.len(), 1, "case {case}: {:?}", rt.live_only_warnings);
        let w = &rt.live_only_warnings[0];
        assert!(w.starts_with("rune: on_key called Tx::live_only, but the view built something else after it: "), "{w}");
        assert!(w.contains(&want), "case {case}: {w}\nwanted {want}");
    }
}

#[test]
fn the_warning_repeats_only_for_another_element_or_property() {
    let mut rt = case_app(1);
    for _ in 0..4 {
        rt.frame(0.5);
        rt.key(KeyEvent { key: Key::Right, mods: Mods::default() });
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(rt.live_checks, 4);
    assert_eq!(rt.live_only_warnings.len(), 1, "the same handler, element and property: {:#?}", rt.live_only_warnings);
    rt.state.case = 0;
    rt.frame(0.5);
    rt.key(KeyEvent { key: Key::Right, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.live_only_warnings.len(), 2, "{:#?}", rt.live_only_warnings);
    let head = format!("layout rect of \"{}/box\"", list_path(&rt));
    assert!(rt.live_only_warnings[1].contains(&head), "{}", rt.live_only_warnings[1]);
}

#[test]
fn a_selection_handler_after_a_checked_event_is_not_blamed_on_it() {
    struct Sel { heard: u32 }
    fn view(s: &Sel, _: &mut Cx<Sel>) -> El {
        col().w(400.0)
            .child(text("some selectable words to drag across").key("t").w(400.0).selectable(true)
                .on_selection_change(|s: &mut Sel, _, _| s.heard += 1))
            .child(text(s.heard.to_string()).key("heard"))
    }
    let mut rt = Runtime::new(Sel { heard: 0 }, view, (400.0, 200.0), Env::new(true, 1.0));
    rt.on_key = Some(|_, _, tx| { tx.live_only(); true });
    rt.frame(0.0);
    let (x, y) = rt.find("t").unwrap();
    rt.pointer_down(x - 150.0, y);
    rt.pointer_move(x - 100.0, y);
    for _ in 0..30 { rt.frame(1.0 / 60.0); }
    // Still dragging: a checked live-only key, and the drag grows the selection in one frame.
    rt.frame(0.5);
    let (heard, checks) = (rt.state.heard, rt.live_checks);
    rt.key(KeyEvent { key: Key::Function(5), mods: Mods::default() });
    rt.pointer_move(x + 100.0, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.live_checks - checks, 1);
    assert!(rt.state.heard > heard, "the selection handler ran");
    assert!(rt.live_only_warnings.is_empty(), "{:#?}", rt.live_only_warnings);
}

/// 2,000 index-built rows whose cells call `Tx::live_only` on hover. Rows from `stale_from` on
/// show `s.hovers`, which the handler counts: state a live-only handler mustn't change.
struct Long { hovers: u32, stale_from: usize }

fn long_app(stale_from: usize) -> Runtime<Long> {
    fn view(_: &Long, _: &mut Cx<Long>) -> El {
        let list = col().key("rows").w_full().growth(crate::GrowthSpec::default())
            .virtual_rows(2000, ROW_H, |_: &Long, i| format!("r{i}").into(), |s: &Long, i| {
                let label = if i >= s.stale_from { format!("{i}: {}", s.hovers) } else { i.to_string() };
                col().w_full().h(ROW_H).child(text(label).key("cell").w_full().h(ROW_H)
                    .on_hover_change(|s: &mut Long, _, tx| { if s.stale_from < 2000 { s.hovers += 1; } tx.live_only(); }))
            });
        col().w_full().h_full()
            .child(col().key("scroll").w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(list))
    }
    let mut rt = Runtime::new(Long { hovers: 0, stale_from }, view, (400.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    for _ in 0..60 { rt.frame(1.0 / 60.0); }
    rt
}

/// The viewport grows without the virtual window moving (its rows still cover it), then a
/// checked live-only hover: the check's commit must not re-pick the overscan and report the
/// rows it would add below the viewport.
#[test]
fn a_grown_viewport_does_not_make_the_overscan_a_view_change() {
    let mut rt = long_app(usize::MAX);
    // Each hover lands on another row.
    for (row, h) in [310.0, 330.0, 360.0, 400.0].into_iter().enumerate() {
        let checks = rt.live_checks;
        rt.set_viewport((400.0, h), 2.0);
        for _ in 0..10 { rt.frame(1.0 / 60.0); }
        rt.frame(0.3);
        rt.pointer_move(200.0, (row as f32 + 0.5) * ROW_H);
        for _ in 0..3 { rt.frame(1.0 / 60.0); }
        assert!(rt.live_checks > checks, "viewport {h}: the hover was checked");
    }
    assert!(rt.live_only_warnings.is_empty(), "{:#?}", rt.live_only_warnings);
}

/// A row mounted below the viewport shows on screen when scrolled to without a commit, so a
/// live-only handler that changes it is still named after the viewport grows.
#[test]
fn a_stale_row_mounted_below_the_viewport_still_warns_after_the_viewport_grows() {
    let mut rt = long_app(30);
    rt.set_viewport((400.0, 360.0), 2.0);
    for _ in 0..10 { rt.frame(1.0 / 60.0); }
    rt.frame(0.3);
    rt.pointer_move(200.0, 45.0);
    for _ in 0..3 { rt.frame(1.0 / 60.0); }
    assert_eq!(rt.live_only_warnings.len(), 1, "{:#?}", rt.live_only_warnings);
    let w = &rt.live_only_warnings[0];
    assert!(w.contains("text of") && w.contains("/rows/r30/cell"), "{w}");
}
