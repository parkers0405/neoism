//! A commit that inserts rows above the first one of a reader's virtual growth stack and
//! reveals the new first row in the same `Tx` shows that row at full height from the first
//! frame, as rows entering under a reveal from an earlier commit do.
use rune::runtime::ElementInfo;
use rune::{Cx, El, Env, GrowthSpec, Runtime, ScrollSpec, col, el, memo, text};

struct S { items: Vec<u32> }

fn key(id: u32) -> String { format!("item{id}") }

fn view(s: &S, _: &mut Cx<S>) -> El {
    let rows = col().key("rows").w_full().px(8.0).py(6.0).growth(GrowthSpec::default()).virtual_rows(
        s.items.len(),
        44.0,
        |s: &S, i| key(s.items[i]).into(),
        |s: &S, i| {
            let id = s.items[i];
            // The first row carries its section's header, as a search list's first result does.
            memo(&key(id), (id, i == 0), || {
                let mut row = col().w_full();
                if i == 0 { row = row.child(el().key("section").w_full().h(30.0)); }
                row.child(el().key(key(id)).w_full().h(44.0).child(text(format!("Row {id}"))))
            })
        },
    );
    col().w_full().h_full().child(
        col().key("list").w_full().h(300.0).scroll(ScrollSpec { stick_bottom: false }).scroll_margin(6.0).child(rows),
    )
}

fn find(rt: &Runtime<S>, key: &str) -> Option<ElementInfo> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key))
}

fn shown(rt: &Runtime<S>, key: &str) -> f32 { find(rt, key).map_or(0.0, |e| e.visible[3]) }

fn scrolled_away_and_back() -> Runtime<S> {
    let mut rt = Runtime::new(S { items: (0..1000).collect() }, view, (400.0, 300.0), Env::new(true, 1.0));
    for _ in 0..10 { rt.frame(1.0 / 120.0); }
    rt.settle_all();
    for row in [40, 0] {
        rt.update(|_, tx| tx.scroll_into_view(key(row)));
        for _ in 0..3 { rt.frame(1.0 / 120.0); }
    }
    rt
}

#[test]
fn rows_inserted_above_the_top_and_revealed_in_the_same_commit_land_in_view() {
    for inserted in [1, 2] {
        let mut rt = scrolled_away_and_back();
        rt.update(|s, tx| {
            for n in 0..inserted { s.items.insert(0, 1001 + n); }
            tx.scroll_into_view(key(s.items[0]));
        });
        let first = key(1000 + inserted);
        rt.frame(1.0 / 120.0);
        let h = shown(&rt, &first);
        assert!(h > 43.5, "{inserted} inserted: the first frame shows the new first row whole, not {h}");
        for _ in 0..3 { rt.frame(1.0 / 120.0); }
        assert!(shown(&rt, &first) > 43.5, "{inserted} inserted: and keeps showing it");
        rt.settle_all();
        assert!(shown(&rt, &first) > 43.5, "{inserted} inserted: settled");
        let section = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("section") && !e.exiting).unwrap();
        assert!(section.visible[3] > 29.5 && section.rect[1] >= -0.05, "{inserted} inserted: at the top: {section:?}");
    }
}
