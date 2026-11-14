//! Debug-only layout warnings for mistakes that lay out without error but look broken.
use std::collections::BTreeSet;
use std::sync::Mutex;

use rune_taffy::prelude::*;

use super::Node;

/// A scroll viewport that resolved to 0 pt inside a `.grow()` container while its content has
/// height: `.grow()` sets flex-basis 0, so with no bounded ancestor there is nothing to grow
/// into. Warns once per `.scroll(..)` call site.
static WARNED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

pub(super) fn collapsed_scrolls(taffy: &LayoutEngine, nodes: &[Node]) {
    let height = |i: usize| taffy.box_layout(nodes[i].taffy).h;
    let bottom = |i: usize| {
        let l = taffy.box_layout(nodes[i].taffy);
        l.y + l.h
    };
    let grown = |i: usize| {
        let s = taffy.style(nodes[i].taffy);
        s.flex_grow() > 0.0 && s.flex_basis() == length(0.0)
    };
    for (i, n) in nodes.iter().enumerate() {
        if n.scroll.is_none() { continue; }
        let direct = || (i + 1..nodes.len()).take_while(|&j| nodes[j].parent >= Some(i)).filter(|&j| nodes[j].parent == Some(i));
        if n.growth.is_some() {
            let site = n.scroll_source.map_or_else(|| n.path.clone(), |s| s.to_string());
            if WARNED.lock().unwrap().insert(format!("growth:{site}")) {
                eprintln!(
                    "rune: scroll viewport {:?} (.scroll at {site}) also sets .growth(..); \
                     put .growth(..) on a child column inside .scroll(..) so the viewport box \
                     does not take the stack's presented height. (debug builds only)",
                    n.path,
                );
            }
        }
        let mut kids = direct();
        if let Some(g) = kids.find(|&j| nodes[j].growth.is_some())
            && kids.next().is_some()
        {
            let site = n.scroll_source.map_or_else(|| n.path.clone(), |s| s.to_string());
            if WARNED.lock().unwrap().insert(format!("growth-sibling:{site}")) {
                eprintln!(
                    "rune: growth stack {:?} inside scroll viewport {:?} (.scroll at {site}) \
                     has sibling elements after it; a growth stack must be the last (or only) \
                     child of its scroll viewport so nothing below it overlaps its growing rows. (debug builds only)",
                    nodes[g].path, n.path,
                );
            }
        }
        if !n.scroll_axes.y() || height(i) >= 0.5 { continue; }
        let content = direct().map(bottom).fold(0.0_f32, f32::max);
        if content < 0.5 { continue; }
        let mut up = Some(i);
        let mut collapsed = false;
        while let Some(j) = up {
            if grown(j) && height(j) < 0.5 { collapsed = true; break; }
            up = nodes[j].parent;
        }
        if !collapsed { continue; }
        let site = n.scroll_source.map_or_else(|| n.path.clone(), |s| s.to_string());
        if !WARNED.lock().unwrap().insert(site.clone()) { continue; }
        eprintln!(
            "rune: scroll viewport {:?} (.scroll at {site}) is 0 pt tall inside a .grow() container \
             while its content is {content} pt. .grow() sets flex-basis 0, so it only fills space a \
             bounded parent leaves: bound an ancestor's height (.h(..), .h_full()) or set .min_h(..), \
             and put .min_h(0.0) on nested flex columns. (debug builds only)",
            n.path,
        );
    }
}

#[cfg(test)]
pub(super) fn warned_at(needle: &str) -> bool {
    WARNED.lock().unwrap().iter().any(|site| site.contains(needle))
}

#[cfg(test)]
mod tests {
    use crate::{Env, Runtime, ScrollSpec, col, el};

    fn view(bounded: &bool, _: &mut crate::Cx<bool>) -> crate::El {
        let list = col().key("list").scroll(ScrollSpec { stick_bottom: false }).grow(1.0)
            .child(col().key("rows").child(el().h(400.0)));
        let page = col().key("page").child(list);
        col().child(if *bounded { page.h_full() } else { page })
    }

    fn warned() -> bool {
        super::WARNED.lock().unwrap().iter().any(|site| site.contains("layout_diagnostics.rs"))
    }

    #[test]
    fn a_grown_scroll_in_an_auto_height_column_warns_and_keeps_its_layout() {
        for bounded in [true, false] {
            let mut rt = Runtime::new(bounded, view, (200.0, 200.0), Env::new(true, 1.0));
            rt.frame(0.0);
            let list = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("list")).unwrap();
            assert_eq!(list.rect[3], if bounded { 200.0 } else { 0.0 }, "layout is unchanged");
            assert_eq!(warned(), !bounded);
        }
    }
}
