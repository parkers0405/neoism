//! A drag in a selection group reaches only text that shows. Text clipped out of sight but
//! laid out under the words (a folded part above them) took a drag in a paragraph to its hidden
//! text and selected everything between, in a zoomed page (t973).
use rune::{Cx, El, Env, Runtime, TextStyle, col, text};

const SEEN: &str = "the words you can see and drag across";

struct S {
    zoom: f32,
}

fn view(s: &S, _: &mut Cx<S>) -> El {
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    let t = |content: &str, key: &str| text(content.to_owned()).key(key).font(style).w_full().selectable(true);
    let folded = col().key("folded").w_full().h(0.0).clip_rounded(0.0)
        .child(t("hidden earlier note one", "h0"))
        .child(t("hidden earlier note two", "h1"))
        .child(t("hidden earlier note three", "h2"));
    col().w_full().h_full().padding(10.0).child(
        col().key("page").w(400.0).scale(s.zoom).selection_group(true)
            .child(folded)
            .child(t("first line above", "a"))
            .child(t(SEEN, "seen"))
            .child(t("last line below", "b")),
    )
}

fn drag_in_seen(zoom: f32) -> Option<String> {
    let mut rt = Runtime::new(S { zoom }, view, (600.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let e = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("seen")).unwrap();
    let y = e.rect[1] + e.rect[3] / 2.0;
    let (x0, x1) = (e.rect[0] + e.rect[2] * 0.2, e.rect[0] + e.rect[2] * 0.5);
    rt.pointer_move(x0, y);
    rt.pointer_down(x0, y);
    rt.frame(1.0 / 60.0);
    for k in 1..=4 {
        rt.pointer_move(x0 + (x1 - x0) * k as f32 / 4.0, y + 2.0);
        rt.frame(1.0 / 60.0);
    }
    rt.pointer_up(x1, y + 2.0);
    rt.frame(1.0 / 60.0);
    rt.selected_text()
}

#[test]
fn a_drag_skips_text_clipped_out_of_sight() {
    for zoom in [1.0, 1.3] {
        let sel = drag_in_seen(zoom).expect("the drag selects");
        assert!(!sel.contains("hidden") && !sel.contains('\n') && SEEN.contains(sel.as_str()), "at {zoom}: only the words dragged over: {sel:?}");
    }
}

const LONG: &str = "a long note folded to its first lines, whose hidden lines run on far below the fold and under the words that come after it, as a long prompt folded in a chat does, line after line after line after line after line after line after line after line after line after line after line after line after line after line after line after line after line";

fn partly_folded(_: &S, _: &mut Cx<S>) -> El {
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    let t = |content: &str, key: &str| text(content.to_owned()).key(key).font(style).w_full().selectable(true);
    col().w_full().h_full().padding(10.0).child(
        col().key("page").w(400.0).selection_group(true)
            .child(col().key("fold").w_full().h(40.0).clip_rounded(0.0).child(t(LONG, "long").wrap()))
            .child(t("first line above", "a"))
            .child(t(SEEN, "seen"))
            .child(t("last line below", "b")),
    )
}

#[test]
fn a_drag_skips_the_hidden_lines_of_partly_clipped_text() {
    let mut rt = Runtime::new(S { zoom: 1.0 }, partly_folded, (600.0, 600.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let e = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("seen")).unwrap();
    let long = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("long")).unwrap();
    assert!(long.rect[1] + long.rect[3] > e.rect[1] + e.rect[3], "the folded text runs on under the words: {:?} {:?}", long.rect, e.rect);
    let y = e.rect[1] + e.rect[3] / 2.0;
    let (x0, x1) = (e.rect[0] + e.rect[2] * 0.2, e.rect[0] + e.rect[2] * 0.5);
    rt.pointer_move(x0, y);
    rt.pointer_down(x0, y);
    rt.frame(1.0 / 60.0);
    for k in 1..=4 {
        rt.pointer_move(x0 + (x1 - x0) * k as f32 / 4.0, y + 2.0);
        rt.frame(1.0 / 60.0);
    }
    rt.pointer_up(x1, y + 2.0);
    rt.frame(1.0 / 60.0);
    let sel = rt.selected_text().expect("the drag selects");
    assert!(!sel.contains('\n') && SEEN.contains(sel.as_str()), "only the words dragged over: {sel:?}");
}
