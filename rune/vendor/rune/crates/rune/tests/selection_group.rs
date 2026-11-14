//! `El::selection_group`: the selectable texts inside one element select as one flow in tree
//! order. A drag crosses from one into the next, Shift-click extends into any of them, Cmd+A
//! takes them all and Cmd+C joins them with newlines (tabs between texts side by side).
//! Double- and triple-clicks stay inside one text, links still open, a draggable row still
//! drags, and text appended after the selection keeps it. A drag into a gap reaches no
//! further than the pointer, table rows select by band, inner groups stay their own, and a
//! change to a text between the ends drops the selection. Elided texts select through their
//! ellipsis, and a virtual row scrolling out drops the selection it holds.
mod support;

use rune::{Cursor, Cx, Drag, El, Env, GrowthSpec, Link, Mods, PhysicalInput, Runtime, ScrollSpec, TextStyle, col, rich, row, text};
use support::{click, command, drag, find, glyphs, glyphs_at, tick, wait};

const A: &str = "first paragraph here";
const ITEM: &str = "list item";
const CODE: &str = "let x = 1;\nlet y = 2;";
const LINKED: &str = "see docs now";

#[derive(Default)]
struct S { tail: String, opened: Vec<String>, drags: u32, hide_item: bool, edited: bool, code_changed: bool }

fn view(s: &S, _: &mut Cx<S>) -> El {
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    let docs = LINKED.find("docs").unwrap();
    let t = |content: &str, key: &str| text(content.to_owned()).key(key).font(style).w_full().selectable(true);
    let item = if s.edited { "edited item" } else { ITEM };
    let code = if s.code_changed { "fn other() {}" } else { CODE };
    col().w_full().h_full().padding(10.0)
        .child(col().key("group").w(400.0).gap(12.0).selection_group(true)
            .child(t(A, "a"))
            .children((!s.hide_item).then(|| row().key("li").w_full().child(text("•").key("marker").font(style).w(20.0)).child(t(item, "item").grow(1.0).shrink())))
            .child(col().key("block").w_full().padding(8.0).child(t(code, "code")))
            .child(row().key("cells").w_full().child(t("left", "c0").w(120.0)).child(t("right", "c1").w(120.0)))
            .child(rich(format!("{LINKED}{}", s.tail), Vec::new()).key("linked").font(style).w_full().selectable(true)
                .link(Link::new(docs..docs + 4, "docs"))
                .on_link(|s: &mut S, id, _| s.opened.push(id.to_owned()))))
        .child(t("outside text", "out").w(200.0).mt(12.0))
        .child(row().key("dragrow").w(400.0).h(30.0).mt(12.0).selection_group(true)
            .on_drag(|s: &mut S, _: &mut Drag, _| s.drags += 1)
            .child(t("drag one", "d0").w(150.0)).child(t("drag two", "d1").w(150.0)))
}

fn runtime() -> Runtime<S> {
    let mut rt = Runtime::new(S::default(), view, (500.0, 500.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt
}

/// A double-click at `from`, held and dragged to `to`: selects by words.
fn word_drag(rt: &mut Runtime<S>, (x0, y0): (f32, f32), (x1, y1): (f32, f32)) {
    click(rt, (x0, y0));
    drag(rt, (x0, y0), (x1, y1));
}

/// Just left of glyph `g`: a drag ending there stops before it.
fn before(g: (f32, f32)) -> (f32, f32) { (g.0 - 3.0, g.1) }

#[test]
fn a_drag_crosses_texts_and_copy_joins_them() {
    let mut rt = runtime();
    let clipboard = rune::clipboard::memory();
    rt.use_clipboard(&clipboard);
    let a = glyphs(&rt, "a");
    let code = glyphs(&rt, "code");
    // From "paragraph" down into the code block, stopping before "x".
    drag(&mut rt, before(a[5]), before(code[3]));
    assert_eq!(rt.selected_text().as_deref(), Some("paragraph here\nlist item\nlet "), "the marker isn't selectable");
    assert_eq!(find(&rt, "a").selected.as_deref(), Some("paragraph here"));
    assert_eq!(find(&rt, "item").selected.as_deref(), Some(ITEM), "texts between are selected whole");
    assert_eq!(find(&rt, "code").selected.as_deref(), Some("let "));
    assert_eq!(find(&rt, "c0").selected, None);
    assert_eq!(rt.text_selection().map(|(p, r)| (p.ends_with("/a"), r)), Some((true, 6..A.len())), "the anchor's text");
    rt.events.clear();
    command(&mut rt, 'c');
    assert_eq!(clipboard.text().as_deref().map(String::as_str), Some("paragraph here\nlist item\nlet "));
    // The Edit menu's Copy hands Rune this one key: one copy, not one per text.
    assert_eq!(rt.events.iter().filter(|e| e.starts_with("copy ")).count(), 1, "{:?}", rt.events);

    // Upward from the cells into the item: the anchor stays put.
    wait(&mut rt);
    let c1 = glyphs(&rt, "c1");
    let item = glyphs(&rt, "item");
    drag(&mut rt, before(c1[2]), before(item[4]));
    assert_eq!(rt.selected_text().as_deref(), Some("item\nlet x = 1;\nlet y = 2;\nleft\tri"), "cells side by side are tab-separated");

    // Cmd+A takes the whole group, and stops at it.
    rt.events.clear();
    command(&mut rt, 'a');
    assert_eq!(rt.events.iter().filter(|e| e.starts_with("select all ")).count(), 1, "{:?}", rt.events);
    assert_eq!(rt.selected_text().as_deref(), Some(format!("{A}\n{ITEM}\n{CODE}\nleft\tright\n{LINKED}").as_str()));
    assert_eq!(find(&rt, "out").selected, None);
}

#[test]
fn gaps_and_the_outside_clamp_to_the_group() {
    let mut rt = runtime();
    let a = glyphs(&rt, "a");
    let item = find(&rt, "li");
    // Released in the gap above the list item: up to the item's start.
    drag(&mut rt, before(a[5]), (a[5].0, item.rect[1] - 4.0));
    assert_eq!(rt.selected_text().as_deref(), Some("paragraph here"));
    wait(&mut rt);
    // Past the group into a text outside it: the group's end, not the outside text.
    let out = glyphs(&rt, "out");
    drag(&mut rt, before(a[5]), out[3]);
    let got = rt.selected_text().unwrap();
    assert!(got.starts_with("paragraph here") && got.ends_with(LINKED), "{got:?}");
    assert_eq!(find(&rt, "out").selected, None);
}

#[test]
fn shift_click_extends_into_another_text() {
    let mut rt = runtime();
    let a = glyphs(&rt, "a");
    let c0 = glyphs(&rt, "c0");
    click(&mut rt, before(a[6]));
    rt.physical_input(PhysicalInput::Modifiers(Mods { shift: true, ..Mods::default() }));
    click(&mut rt, before(c0[2]));
    assert_eq!(rt.selected_text().as_deref(), Some(format!("aragraph here\n{ITEM}\n{CODE}\nle").as_str()));
    // And back into the anchor's own text.
    click(&mut rt, before(a[9]));
    rt.physical_input(PhysicalInput::Modifiers(Mods::default()));
    assert_eq!(rt.selected_text().as_deref(), Some("ara"));
}

#[test]
fn double_and_triple_clicks_stay_in_one_text() {
    let mut rt = runtime();
    let code = glyphs(&rt, "code");
    let x = (code[3].0 - 1.0, code[3].1);
    click(&mut rt, x);
    click(&mut rt, x);
    assert_eq!(rt.selected_text().as_deref(), Some("x"));
    click(&mut rt, x);
    assert_eq!(rt.selected_text().as_deref(), Some("let x = 1;\n"));
    wait(&mut rt);
    // A double-click drag across texts extends by whole words.
    let a = glyphs(&rt, "a");
    let item = glyphs(&rt, "item");
    rt.pointer_move(a[7].0, a[7].1);
    rt.pointer_down(a[7].0, a[7].1);
    rt.pointer_up(a[7].0, a[7].1);
    tick(&mut rt);
    rt.pointer_down(a[7].0, a[7].1);
    tick(&mut rt);
    rt.pointer_move(item[1].0, item[1].1);
    tick(&mut rt);
    rt.pointer_up(item[1].0, item[1].1);
    tick(&mut rt);
    assert_eq!(rt.selected_text().as_deref(), Some("paragraph here\nlist"));
}

#[test]
fn links_open_and_show_the_hand_over_selectable_text() {
    let mut rt = runtime();
    let a = glyphs(&rt, "a");
    rt.pointer_move(a[2].0, a[2].1);
    tick(&mut rt);
    assert_eq!(rt.cursor(), Cursor::Text);
    let docs = find(&rt, "docs");
    let at = (docs.rect[0] + docs.rect[2] / 2.0, docs.rect[1] + docs.rect[3] / 2.0);
    rt.pointer_move(at.0, at.1);
    tick(&mut rt);
    assert_eq!(rt.cursor(), Cursor::Pointer);
    click(&mut rt, at);
    assert_eq!(rt.state.opened, ["docs"]);
    assert_eq!(rt.selected_text(), None);
    // A drag through the link still selects.
    wait(&mut rt);
    let l = glyphs(&rt, "linked");
    drag(&mut rt, before(a[15]), before(l[5]));
    assert_eq!(rt.selected_text().as_deref(), Some(format!("ere\n{ITEM}\n{CODE}\nleft\tright\nsee do").as_str()));
    assert_eq!(rt.state.opened, ["docs"]);
}

#[test]
fn appended_text_keeps_the_selection_and_a_changed_one_drops_it() {
    let mut rt = runtime();
    let a = glyphs(&rt, "a");
    let l = glyphs(&rt, "linked");
    drag(&mut rt, before(a[15]), before(l[5]));
    let selected = rt.selected_text().unwrap();
    for word in [" and", " more", " words"] {
        rt.state.tail.push_str(word);
        rt.invalidate();
        tick(&mut rt);
        assert_eq!(rt.selected_text().as_deref(), Some(selected.as_str()), "streaming after the selection keeps it");
    }
    assert_eq!(find(&rt, "linked").selected.as_deref(), Some("see do"));
    // Shift-click past the old end reaches the new text.
    let l = glyphs(&rt, "linked");
    rt.physical_input(PhysicalInput::Modifiers(Mods { shift: true, ..Mods::default() }));
    click(&mut rt, (l.last().unwrap().0 + 6.0, l.last().unwrap().1));
    rt.physical_input(PhysicalInput::Modifiers(Mods::default()));
    assert!(rt.selected_text().unwrap().ends_with("see docs now and more words"));
    // A change inside what's selected drops it.
    rt.state.tail = String::new();
    rt.invalidate();
    tick(&mut rt);
    assert_eq!(rt.selected_text(), None);
}

#[test]
fn a_draggable_row_drags_and_clicks_still_select() {
    let mut rt = runtime();
    let d0 = glyphs(&rt, "d0");
    let d1 = glyphs(&rt, "d1");
    drag(&mut rt, d0[0], d1[3]);
    assert!(rt.state.drags >= 3, "the row drags: {}", rt.state.drags);
    assert_eq!(rt.selected_text(), None);
    wait(&mut rt);
    click(&mut rt, d1[5]);
    click(&mut rt, d1[5]);
    assert_eq!(rt.selected_text().as_deref(), Some("two"));
    command(&mut rt, 'a');
    assert_eq!(rt.selected_text().as_deref(), Some("drag one\tdrag two"));
}

#[test]
fn a_drag_into_a_gap_reaches_no_further_than_the_pointer() {
    let mut rt = runtime();
    let a = glyphs(&rt, "a");
    let li = find(&rt, "li");
    // Down into the gap above the item, by words: the paragraph's end, not the item's first word.
    word_drag(&mut rt, a[7], (a[7].0, li.rect[1] - 4.0));
    assert_eq!(rt.selected_text().as_deref(), Some("paragraph here"));
    // Released there, it doesn't hang on the block below: that block going keeps it.
    rt.state.hide_item = true;
    rt.invalidate();
    wait(&mut rt);
    assert_eq!(rt.selected_text().as_deref(), Some("paragraph here"));
    // Up into a gap: the start of the block below.
    let code = glyphs(&rt, "code");
    let block = find(&rt, "block");
    word_drag(&mut rt, code[5], (code[5].0, block.rect[1] - 4.0));
    assert_eq!(rt.selected_text().as_deref(), Some("let x = 1"));
}

#[test]
fn a_change_between_the_ends_drops_the_selection() {
    let changes: [fn(&mut S); 2] = [|s| s.edited = true, |s| s.hide_item = true];
    for change in changes {
        let mut rt = runtime();
        let a = glyphs(&rt, "a");
        let code = glyphs(&rt, "code");
        drag(&mut rt, before(a[5]), before(code[3]));
        assert_eq!(rt.selected_text().as_deref(), Some("paragraph here\nlist item\nlet "));
        change(&mut rt.state);
        rt.invalidate();
        tick(&mut rt);
        assert_eq!(rt.selected_text(), None, "a text between rewritten or removed");
    }
    // An end with nothing selected in it keeps the selection only while text is appended to it.
    let mut rt = runtime();
    let a = glyphs(&rt, "a");
    let code = glyphs(&rt, "code");
    drag(&mut rt, before(a[5]), before(code[0]));
    assert_eq!(rt.selected_text().as_deref(), Some("paragraph here\nlist item"));
    rt.state.code_changed = true;
    rt.invalidate();
    tick(&mut rt);
    assert_eq!(rt.selected_text(), None);
}

/// A table whose first row has a short cell beside a tall one, and an inner group.
fn table_view(_: &S, _: &mut Cx<S>) -> El {
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    let t = |content: &str, key: &str| text(content.to_owned()).key(key).font(style).w_full().selectable(true);
    let cell = |content: &str, key: &str| col().w(100.0).padding(6.0).child(t(content, key).wrap());
    col().w_full().h_full().padding(10.0)
        .child(col().key("doc").w(400.0).selection_group(true)
            .child(t("intro words", "intro").mb(12.0))
            .child(row().key("r0").child(cell("short", "s")).child(cell("a tall cell that wraps onto lines", "tall")))
            .child(row().key("r1").child(cell("next row", "n0")).child(cell("more", "n1")))
            .child(col().key("inner").mt(12.0).selection_group(true).child(t("inner one", "i1")).child(t("inner two", "i2")))
            .child(t("outro words", "outro").mt(12.0)))
}

fn table_runtime() -> Runtime<S> {
    let mut rt = Runtime::new(S::default(), table_view, (500.0, 500.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt
}

const TABLE: &str = "short\ta tall cell that wraps onto lines\nnext row\tmore";

#[test]
fn table_rows_select_by_band() {
    let mut rt = table_runtime();
    let intro = glyphs(&rt, "intro");
    let (s, tall) = (find(&rt, "s"), find(&rt, "tall"));
    assert!(tall.rect[3] > s.rect[3] * 2.0, "the tall cell wraps: {tall:?}");
    // Below the short cell's text but beside the tall one's: still the short cell.
    let low = tall.rect[1] + tall.rect[3] - 4.0;
    drag(&mut rt, (intro[0].0 - 1.5, intro[0].1), (s.rect[0] + s.rect[2] - 1.0, low));
    assert_eq!(rt.selected_text().as_deref(), Some("intro words\nshort"));
    // Into the padding below the row, by words: the row's end, not the next row's first word.
    wait(&mut rt);
    word_drag(&mut rt, intro[1], (s.rect[0] + 4.0, tall.rect[1] + tall.rect[3] + 3.0));
    assert_eq!(rt.selected_text().as_deref(), Some("intro words\nshort\ta tall cell that wraps onto lines"));
}

#[test]
fn an_inner_group_is_its_own() {
    let mut rt = table_runtime();
    let intro = glyphs(&rt, "intro");
    click(&mut rt, intro[1]);
    command(&mut rt, 'a');
    assert_eq!(rt.selected_text().as_deref(), Some(format!("intro words\n{TABLE}\noutro words").as_str()));
    assert_eq!(find(&rt, "i1").selected, None);
    // A drag across it skips it too.
    wait(&mut rt);
    let outro = glyphs(&rt, "outro");
    drag(&mut rt, (intro[0].0 - 1.5, intro[0].1), before(outro[5]));
    assert_eq!(rt.selected_text().as_deref(), Some(format!("intro words\n{TABLE}\noutro ").as_str()));
    // And inside, Cmd+A takes only its texts.
    wait(&mut rt);
    let i1 = glyphs(&rt, "i1");
    click(&mut rt, i1[1]);
    command(&mut rt, 'a');
    assert_eq!(rt.selected_text().as_deref(), Some("inner one\ninner two"));
}

const CUT: &str = "a line far too long to fit in its box";
const CLAMPED: &str = "a paragraph clamped to one line though it wraps onto three lines here";

fn elided_view(_: &S, _: &mut Cx<S>) -> El {
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    let t = |content: &str, key: &str| text(content.to_owned()).key(key).font(style).selectable(true);
    col().w_full().h_full().padding(10.0)
        .child(col().key("doc").w(160.0).gap(12.0).selection_group(true)
            .child(t("top", "top").w_full())
            .child(t(CUT, "cut").w_full().truncate())
            .child(t(CLAMPED, "clamped").w_full().max_lines(1))
            .child(t("end", "end").w_full()))
}

#[test]
fn elided_texts_select_through_their_ellipsis_and_copy_whole() {
    let mut rt = Runtime::new(S::default(), elided_view, (500.0, 500.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let (cut, clamped) = (find(&rt, "cut"), find(&rt, "clamped"));
    assert!(clamped.rect[3] < 30.0, "clamped to a line: {clamped:?}");
    let top = glyphs(&rt, "top");
    // Into the ellipsis of the cut text: through what it hides.
    drag(&mut rt, (top[0].0 - 1.5, top[0].1), (cut.rect[0] + cut.rect[2] - 1.0, cut.rect[1] + cut.rect[3] / 2.0));
    assert_eq!(rt.selected_text().as_deref(), Some(format!("top\n{CUT}").as_str()));
    // Across both: each whole, as copying an elided text gives.
    wait(&mut rt);
    let end = glyphs(&rt, "end");
    drag(&mut rt, (top[0].0 - 1.5, top[0].1), (end[2].0 + 3.0, end[2].1));
    assert_eq!(rt.selected_text().as_deref(), Some(format!("top\n{CUT}\n{CLAMPED}\nend").as_str()));
    assert_eq!(find(&rt, "clamped").selected.as_deref(), Some(CLAMPED));
    // Inside the drawn part of the clamped text, by words: only what it covers.
    wait(&mut rt);
    let y = clamped.rect[1] + clamped.rect[3] / 2.0;
    word_drag(&mut rt, (clamped.rect[0] + 3.0, y), (clamped.rect[0] + 40.0, y));
    assert_eq!(rt.selected_text().as_deref(), Some("a paragraph"));
}

fn rows_view(_: &S, _: &mut Cx<S>) -> El {
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    col().w_full().h_full().child(
        col().key("scroll").w_full().grow(1.0).scroll(ScrollSpec { stick_bottom: false }).child(
            col().key("rows").w_full().growth(GrowthSpec::default()).virtual_rows(400, 60.0,
                |_: &S, i| format!("r{i}").into(),
                move |_: &S, i| col().w_full().h(60.0).padding(6.0).selection_group(true).children([
                    text(format!("row {i} title")).key("a").font(style).w_full().selectable(true),
                    text(format!("row {i} body")).key("b").font(style).w_full().selectable(true),
                ]))))
}

#[test]
fn a_selection_in_a_virtual_row_scrolled_out_drops_without_panicking() {
    let mut rt = Runtime::new(S::default(), rows_view, (400.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let a = glyphs_at(&rt, "#0/scroll/rows/r0/a");
    let b = glyphs_at(&rt, "#0/scroll/rows/r0/b");
    drag(&mut rt, (a[0].0 - 1.5, a[0].1), (b[b.len() - 1].0 + 3.0, b[0].1));
    assert_eq!(rt.selected_text().as_deref(), Some("row 0 title\nrow 0 body"));
    // Mid-drag, the row scrolls out under the held press.
    wait(&mut rt);
    rt.pointer_move(a[0].0 - 1.5, a[0].1);
    rt.pointer_down(a[0].0 - 1.5, a[0].1);
    tick(&mut rt);
    for _ in 0..40 {
        rt.wheel(200.0, 200.0, -400.0);
        rt.pointer_move(200.0, 300.0);
        tick(&mut rt);
    }
    rt.pointer_up(200.0, 300.0);
    tick(&mut rt);
    assert!(!rt.elements().iter().any(|e| e.path == "#0/scroll/rows/r0/a"), "row 0 scrolled out");
    assert_eq!(rt.selected_text().as_deref(), Some("row 0 title\nrow 0 body"));
    command(&mut rt, 'c');
    command(&mut rt, 'a');
    // Back up: the pinned row's selection is preserved when it mounts again.
    for _ in 0..40 {
        rt.wheel(200.0, 200.0, 400.0);
        tick(&mut rt);
    }
    wait(&mut rt);
    assert!(rt.elements().iter().any(|e| e.path == "#0/scroll/rows/r0/a"));
    assert_eq!(rt.selected_text().as_deref(), Some("row 0 title\nrow 0 body"));
}

