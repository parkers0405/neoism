//! Layout reads find their element whether a few reads scan the tree or many index it.

use super::*;
use crate::el::row;

struct Cells { width: f32, reads: usize, seen: std::rc::Rc<std::cell::RefCell<Vec<Option<[f32; 2]>>>> }

fn view(s: &Cells, cx: &mut Cx<Cells>) -> El {
    let mut seen: Vec<Option<[f32; 2]>> = (0..s.reads).map(|i| cx.layout_size(&format!("cell{i}"))).collect();
    seen.push(cx.layout_size("panel/cell3"));
    seen.push(cx.layout_size("missing"));
    *s.seen.borrow_mut() = seen;
    let cells = |w: f32| (0..12).map(move |i| el().key(format!("cell{i}")).w(w + i as f32).h(10.0));
    col()
        .child(row().key("other").children(cells(1.0)))
        .child(row().key("panel").children(cells(s.width)))
}

#[test]
fn few_and_many_reads_see_the_same_rects() {
    for reads in [2, 12] {
        let seen = std::rc::Rc::default();
        let mut rt = Runtime::new(Cells { width: 20.0, reads, seen: std::rc::Rc::clone(&seen) }, view, (400.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        rt.update(|s, _| s.width = 30.0);
        for _ in 0..3 { rt.frame(1.0 / 60.0); }
        let seen = seen.borrow();
        assert_eq!(seen.len(), reads + 2);
        for (i, size) in seen[..reads].iter().enumerate() {
            assert_eq!(*size, Some([1.0 + i as f32, 10.0]), "{reads} reads: cell{i} is the first one keyed so");
        }
        assert_eq!(seen[reads], Some([33.0, 10.0]), "{reads} reads: a scoped selector finds the panel's cell");
        assert_eq!(seen[reads + 1], None, "{reads} reads");
    }
}

type Seen = std::rc::Rc<std::cell::RefCell<[Option<[f32; 4]>; 2]>>;

/// A 200-point viewport over a hundred 40-point virtual rows, each with a keyed label 5 points
/// down, reading where row 27 and its label lie.
fn virtual_view(seen: &Seen, cx: &mut Cx<Seen>) -> El {
    *seen.borrow_mut() = [cx.window_layout("r27"), cx.window_layout("r27/label")];
    let rows = col().key("rows").w_full().growth(crate::GrowthSpec::default())
        .virtual_rows(100, 40.0, |_: &Seen, i| format!("r{i}").into(), |_: &Seen, _| {
            col().w_full().h(40.0).pt(5.0).child(el().key("label").w(10.0).h(10.0))
        });
    col().w_full().h_full().child(col().key("list").w_full().h(200.0).scroll(crate::ScrollSpec { stick_bottom: false }).child(rows))
}

#[test]
fn a_virtual_row_reads_where_it_draws_with_rows_culled_above() {
    let seen = Seen::default();
    let mut rt = Runtime::new(std::rc::Rc::clone(&seen), virtual_view, (300.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|_, tx| tx.scroll_to_instant("list", 0.0, 1000.0));
    for _ in 0..4 { rt.frame(1.0 / 60.0); }
    assert!(rt.virtuals.values().next().unwrap().before > 0.0, "rows above the window are culled");
    let drawn = |key: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).map(|e| e.rect);
    let [row, label] = *seen.borrow();
    assert_eq!(drawn("r27").map(|r| r[1]), Some(80.0), "row 27 draws 1080 points down, scrolled 1000");
    assert_eq!(row, drawn("r27"), "the row reads where it draws");
    let label_drawn = rt.elements().into_iter().find(|e| e.path.contains("/r27/") && e.key.as_deref() == Some("label")).map(|e| e.rect);
    assert_eq!(label, label_drawn, "so does an element inside it");
}
