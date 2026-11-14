//! The focused text field's caret in window points, for views (`Cx::caret`) and tools
//! (`ElementInfo::caret`). It comes from layout, not the drawn frame, so a view that reads it
//! is re-measured in the same commit when an edit moves it.
use super::*;

impl<S: 'static> Runtime<S> {
    /// Any vertical shift from ancestor growth rows whose presented height has not settled to layout.
    pub(super) fn stack_row_dy(&self, nodes: &[Node], i: usize) -> f32 {
        let mut dy = 0.0;
        let applied = std::ptr::eq(nodes, self.nodes.as_slice());
        for r in std::iter::successors(Some(i), |&p| nodes[p].parent.filter(|_| nodes[p].above.is_none())) {
            let Some(c) = nodes[r].parent.filter(|&c| nodes[c].growth.is_some()) else { continue };
            let Some(stack) = self.stacks.get(&nodes[c].id) else { continue };
            if applied && let Some(&top) = stack.tops.get(&nodes[r].id) {
                let mut count = 0usize;
                let before_r: f32 = stack.order.iter()
                    .take_while(|&&id| id != nodes[r].id)
                    .filter_map(|id| self.grows.get(id))
                    .map(|g| { count += 1; g.h })
                    .sum::<f32>() + stack.gap * count as f32;
                dy += stack.pad_top + before_r - top;
            } else if stack.moving {
                for j in (c + 1..r).filter(|&j| nodes[j].parent == Some(c)) {
                    if let Some(g) = self.grows.get(&nodes[j].id) {
                        dy += g.h - nodes[j].local.h;
                    }
                }
            }
        }
        dy
    }

    /// Node `i`'s caret line `[x, y, w, h]` in window points at its laid-out place, after
    /// scrolling and sticky shifts, if it is a focused text field.
    pub(super) fn caret_rect(&self, nodes: &[Node], i: usize) -> Option<[f32; 4]> {
        let n = &nodes[i];
        let (inp, spec, layout) = (n.input.as_ref()?, n.text.as_ref()?, n.text_layout.as_ref()?);
        if !inp.edit.focused { return None; }
        let s = self.env.scale;
        let t = sticky::anchored_target(nodes, &self.scrolls, i, self.viewport);
        let y = t[1] + self.stack_row_dy(nodes, i);
        let tw = layout.full_width().ceil() / s;
        let (x0, x1) = (t[0] + n.inset[0], t[2] - n.inset[2]);
        let mut x = text_x(spec, x0, x1, tw, crate::el::Flow::of_layout(layout));
        if inp.single_line {
            let prev = self.input_scroll.get(&n.path).copied().unwrap_or(0.0);
            x -= single_line_offset(prev, inp, &spec.content, layout, s, (x1 - x0) * s) / s;
        }
        let cur = Cursor::from_byte_index(layout, caret_index(&inp.edit, &spec.content), Affinity::Downstream);
        let bb = crate::text::cursor_geometry(&cur, layout, 1.5 * s);
        let h = if bb.y1 > bb.y0 { (bb.y1 - bb.y0) as f32 / s } else { spec.style.line_px() };
        Some([x + bb.x0 as f32 / s, y + n.inset[1] + bb.y0 as f32 / s, 1.5, h])
    }

    /// After an edit, scrolls the focused field's nearest scroll viewport on each axis just far
    /// enough to show the caret line, as a browser does for a textarea, leaving the viewport's
    /// `El::scroll_margin` like CSS `scroll-padding`.
    pub(super) fn follow_caret(&mut self) {
        let Some(i) = self.focused_index() else { return };
        let Some(r) = self.caret_rect(&self.nodes, i) else { return };
        let mut scrolled = false;
        for horizontal in [false, true] {
            let Some(v) = std::iter::successors(Some(i), |&p| self.nodes[p].parent.filter(|_| self.nodes[p].above.is_none()))
                .skip(1)
                .find(|&p| self.nodes[p].scroll.is_some() && self.nodes[p].scroll_axes.has(horizontal)) else { continue };
            let a = usize::from(!horizontal);
            let start = scroll_offsets(&self.scrolls, &self.nodes, v).fold(self.nodes[v].target[a], |p, o| p - o[a]);
            let view = f64::from(self.nodes[v].target[a + 2] - self.nodes[v].target[a]);
            let (at, len) = (f64::from(r[a] - start), f64::from(r[a + 2]));
            let margin = f64::from(self.nodes[v].scroll_edges.margin);
            let delta = scroll_edge::reveal(0.0, at, len, view, margin);
            if delta == 0.0 { continue; }
            let stick = self.nodes[v].scroll.is_some_and(|s| s.stick_bottom);
            let Some(st) = self.scrolls.get_mut(&self.nodes[v].id) else { continue };
            if horizontal {
                st.x.offset = (st.x.offset + st.x.toward(-delta)).clamp(0.0, st.x.max);
                st.x.v = 0.0;
                st.x.glide = None;
            } else {
                st.offset = (st.offset + delta).clamp(0.0, st.max);
                st.land(stick);
                scrolled = true;
            }
            st.direct = true;
        }
        if scrolled && self.virtual_window_changed() {
            self.commit(&Tx::default());
            self.step_scroll(0.0);
        }
    }

    /// Refreshes the caret the next view sees from `nodes` (the current frame's when `None`).
    /// Returns whether it moved.
    pub(super) fn caret_moved(&mut self, nodes: Option<&[Node]>) -> bool {
        let nodes = nodes.unwrap_or(&self.nodes);
        let next = nodes.iter().position(|n| n.input.as_ref().is_some_and(|inp| inp.edit.focused))
            .and_then(|i| Some((i, self.caret_rect(nodes, i)?)));
        let same = match (&self.caret, next) {
            (Some((path, rect)), Some((i, r))) => *path == nodes[i].path && *rect == r,
            (None, None) => true,
            _ => false,
        };
        if same { return false; }
        self.caret = next.map(|(i, r)| (nodes[i].path.clone(), r));
        true
    }
}

#[cfg(test)]
mod tests {
    use crate::{Cx, El, Env, GrowthSpec, Ime, Key, KeyEvent, Mods, Runtime, ScrollSpec, TextEdit, TextStyle, col, text_input};

    fn make_blocks(count: usize, focus_at: Option<usize>) -> Vec<TextEdit> {
        (0..count)
            .map(|i| {
                let lines = 1 + (i * 5 + 2) % 7;
                let text = (0..lines)
                    .map(|l| format!("block {i} line {l} with prose words to vary height"))
                    .collect::<Vec<_>>()
                    .join("\n");
                let mut edit = TextEdit::new(text);
                if focus_at == Some(i) {
                    edit.focused = true;
                }
                edit
            })
            .collect()
    }

    fn blocks_view(s: &Vec<TextEdit>, _: &mut Cx<Vec<TextEdit>>) -> El {
        let style = TextStyle::new(15.0, 400.0);
        let children = s.iter().enumerate().map(|(i, edit)| {
            text_input(edit, style)
                .key(format!("b{i}"))
                .w_full()
                .on_edit(move |s: &mut Vec<TextEdit>| &mut s[i])
                .on_submit(move |s: &mut Vec<TextEdit>, _| {
                    let _ = s[i].insert("\n");
                })
        });
        col().w_full().h_full().padding(20.0).child(
            col()
                .key("viewport")
                .w_full()
                .grow(1.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(
                    col()
                        .key("rows")
                        .w_full()
                        .gap(10.0)
                        .children(children)
                        .growth(GrowthSpec::default())
                        .virtualize(36.0),
                ),
        )
    }

    fn assert_caret_aligned(rt: &Runtime<Vec<TextEdit>>, key: &str, step: &str) {
        let els = rt.elements();
        let viewport = els.iter().find(|e| e.key.as_deref() == Some("viewport")).expect("viewport").rect;
        let field = els.iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("{step}: {key} not mounted"));
        assert!(field.focused, "{step}: {key} should be focused");
        let caret = field.caret.unwrap_or_else(|| panic!("{step}: {key} missing ElementInfo::caret"));
        assert!(
            caret[0] >= field.rect[0] - 1.0
                && caret[0] + caret[2] <= field.rect[0] + field.rect[2] + 2.0
                && caret[1] >= field.rect[1] - 1.0
                && caret[1] + caret[3] <= field.rect[1] + field.rect[3] + 1.0,
            "{step}: caret {caret:?} outside field rect {:?}",
            field.rect
        );
        assert!(
            caret[1] >= viewport[1] - 1.0 && caret[1] + caret[3] <= viewport[1] + viewport[3] + 1.0,
            "{step}: caret {caret:?} outside viewport {viewport:?}"
        );
        let ime = rt.ime_area.unwrap_or_else(|| panic!("{step}: missing ime_area"));
        assert!(
            (ime[0] - caret[0]).abs() <= 1.0 && (ime[1] - caret[1]).abs() <= 1.0,
            "{step}: ime_area {ime:?} disagrees with caret {caret:?}"
        );
    }

    #[test]
    fn growth_gap_is_painted_and_included_in_scroll_extent() {
        // Cover both a plain growth column and a virtual window. Fixed row heights make
        // the expected painted spacing independent of text shaping and estimates.
        for virtualized in [false, true] {
            let mut rt = Runtime::new(virtualized, |virtualized: &bool, _: &mut Cx<bool>| {
                let mut rows = col().key("rows").w_full().gaps(7.0, 19.0)
                    .growth(GrowthSpec::default())
                    .children((0..100).map(|i| crate::el().key(format!("r{i}")).h(20.0).w_full()));
                if *virtualized { rows = rows.virtualize(20.0); }
                col().w_full().h_full().scroll(ScrollSpec { stick_bottom: false }).child(rows)
            }, (300.0, 120.0), Env::new(true, 1.0));
            rt.frame(0.0);
            let elements = rt.elements();
            let a = elements.iter().find(|e| e.key.as_deref() == Some("r0")).unwrap();
            let b = elements.iter().find(|e| e.key.as_deref() == Some("r1")).unwrap();
            assert_eq!(b.rect[1] - a.rect[1] - a.rect[3], 7.0, "virtualized={virtualized}");
            assert!((rt.scrolls.values().next().unwrap().max - 2573.0).abs() < 0.01);
            rt.update(|_, tx| tx.scroll_into_view("r99"));
            rt.frame(0.0);
            let end = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r99")).unwrap();
            assert!((end.rect[1] + end.rect[3] - 120.0).abs() < 0.01, "last row lands at bottom: {:?}", end.rect);
            let commits = rt.stats.commits;
            rt.frame(0.0);
            assert_eq!(rt.stats.commits, commits, "an idle frame does not recommit");
        }
    }

    #[test]
    fn offscreen_focus_is_deterministic_and_does_not_resurrect_after_blur_or_removal() {
        let mut blocks = make_blocks(240, Some(180));
        blocks[190].focused = true;
        let mut rt = Runtime::new(blocks, blocks_view, (640.0, 500.0), Env::new(true, 2.0));
        rt.frame(0.0);
        assert!(rt.state[180].focused, "the first offscreen path wins simultaneous requests");
        assert!(!rt.state[190].focused);
        rt.update(|s, _| s[180].focused = false);
        rt.frame(0.0);
        assert!(rt.focus.path().is_none(), "explicitly blurred offscreen input stays blurred");
        rt.update(|s, _| s[190].focused = true);
        rt.frame(0.0);
        assert!(rt.state[190].focused);
        rt.update(|s, _| { s.truncate(100); });
        rt.frame(0.0);
        assert!(rt.focus.path().is_none(), "deleted offscreen input loses focus");
        rt.update(|s, tx| { s.extend(make_blocks(140, None)); tx.scroll_into_view("b190"); });
        rt.frame(0.0);
        assert!(!rt.state[190].focused, "reusing a deleted path must not restore focus");
    }

    #[test]
    fn virtual_growth_stack_scroll_into_view_keeps_focus_and_aligns_caret() {
        // Focus an initially offscreen far block before the first frame, then reveal and type.
        let mut rt = Runtime::new(make_blocks(240, Some(150)), blocks_view, (640.0, 500.0), Env::new(true, 2.0));
        rt.frame(0.0);
        assert!(rt.state[150].focused, "offscreen focused field keeps focus across initial commit");
        rt.update(|_, tx| tx.scroll_into_view("b150"));
        rt.frame(1.0 / 120.0);
        assert_caret_aligned(&rt, "b150", "revealed b150");

        let before = rt.state[150].text.clone();
        for (step, ch) in ["!", "?", "#"].into_iter().enumerate() {
            rt.ime(Ime::Commit(ch.into()));
            rt.frame(1.0 / 120.0);
            assert_caret_aligned(&rt, "b150", &format!("b150 type {step}"));
        }
        assert_eq!(rt.state[150].text, format!("{before}!?#"));

        // Focus the last block (239) via update + scroll_into_view and type in the same frame and next frames.
        let end_before = rt.state[239].text.clone();
        rt.update(|s, tx| {
            s[150].focused = false;
            s[239].focused = true;
            tx.scroll_into_view("b239");
        });
        for (step, ch) in ["a", "b", "c"].into_iter().enumerate() {
            rt.ime(Ime::Commit(ch.into()));
            rt.frame(1.0 / 120.0);
            assert_caret_aligned(&rt, "b239", &format!("b239 type {step}"));
        }
        assert_eq!(rt.state[239].text, format!("{end_before}abc"));
    }

    #[test]
    fn virtual_growth_stack_scrolling_away_and_back_keeps_focus_and_caret_in_rect() {
        let mut rt = Runtime::new(make_blocks(240, Some(180)), blocks_view, (640.0, 500.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.update(|_, tx| tx.scroll_into_view("b180"));
        rt.frame(1.0 / 120.0);
        assert_caret_aligned(&rt, "b180", "initial reveal b180");

        let base = rt.state[180].text.clone();
        rt.ime(Ime::Commit(" alpha".into()));
        rt.frame(1.0 / 120.0);
        assert_eq!(rt.state[180].text, format!("{base} alpha"));
        assert_caret_aligned(&rt, "b180", "after alpha");

        // Scroll all the way up to the top so b180 leaves the mounted window, measuring rows above.
        for step in 0..45 {
            rt.wheel(320.0, 250.0, 600.0);
            rt.frame(1.0 / 120.0);
            assert!(rt.state[180].focused, "step {step}: b180 stays focused while scrolling away");
            if let Some(el) = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("b180")) {
                let caret = el.caret.expect("mounted b180 has caret");
                assert!(
                    caret[1] >= el.rect[1] - 1.0 && caret[1] + caret[3] <= el.rect[1] + el.rect[3] + 1.0,
                    "step {step} up: caret {caret:?} outside rect {:?}",
                    el.rect
                );
            }
        }
        assert!(rt.find("b180").is_none(), "b180 is culled when scrolled to the top");

        // Scroll back down in wheel steps until b180 is in the viewport again, checking caret alignment on every mounted frame.
        for step in 0..45 {
            rt.wheel(320.0, 250.0, -600.0);
            rt.frame(1.0 / 120.0);
            assert!(rt.state[180].focused, "step {step} down: b180 stays focused while scrolling back");
            if let Some(el) = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("b180")) {
                let caret = el.caret.expect("remounted b180 has caret");
                assert!(
                    caret[1] >= el.rect[1] - 1.0 && caret[1] + caret[3] <= el.rect[1] + el.rect[3] + 1.0,
                    "step {step} down: caret {caret:?} outside rect {:?}",
                    el.rect
                );
            }
        }
        rt.update(|_, tx| tx.scroll_into_view("b180"));
        rt.frame(1.0 / 120.0);
        assert_caret_aligned(&rt, "b180", "revealed back at b180");

        // Grow a mounted row above b180 and verify caret_rect tracks the active growth shift each frame.
        rt.update(|s, _| {
            let grown = format!("{}\nextra line 1\nextra line 2\nextra line 3", s[179].text);
            s[179] = TextEdit::new(grown);
        });
        for frame in 0..10 {
            rt.frame(1.0 / 120.0);
            assert_caret_aligned(&rt, "b180", &format!("above row growing frame {frame}"));
        }

        rt.ime(Ime::Commit(" beta".into()));
        rt.frame(1.0 / 120.0);
        assert_eq!(rt.state[180].text, format!("{base} alpha beta"));
        assert_caret_aligned(&rt, "b180", "after beta");
    }

    #[test]
    fn virtual_growth_stack_arrow_key_follow_caret_tracks_drawn_caret() {
        let mut blocks = make_blocks(220, None);
        blocks[160] = TextEdit::new(
            &(0..16).map(|l| format!("tall block line {l}")).collect::<Vec<_>>().join("\n"),
        );
        blocks[160].focused = true;
        blocks[160].set_caret(0);
        let mut rt = Runtime::new(blocks, blocks_view, (640.0, 500.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.update(|_, tx| tx.scroll_into_view("b160"));
        rt.frame(1.0 / 120.0);
        // Position the scroll viewport so the top of b160 is near the bottom of the viewport,
        // forcing Down arrow steps within b160 to scroll via follow_caret.
        rt.wheel(320.0, 250.0, 260.0);
        rt.frame(1.0 / 120.0);
        assert_caret_aligned(&rt, "b160", "before arrow down");

        for step in 0..15 {
            rt.key(KeyEvent { key: Key::Down, mods: Mods::default() });
            rt.frame(1.0 / 120.0);
            assert_caret_aligned(&rt, "b160", &format!("down step {step}"));
        }
    }

    #[test]
    fn follow_caret_leaves_the_scroll_margin_clear_until_the_content_ends() {
        let mut edit = TextEdit::new((0..40).map(|l| format!("line {l}")).collect::<Vec<_>>().join("\n"));
        edit.focused = true;
        let mut rt = Runtime::new(edit, |e: &TextEdit, _: &mut Cx<TextEdit>| {
            col().w_full().h_full().child(
                col().key("viewport").w_full().h(160.0).scroll(ScrollSpec { stick_bottom: false }).scroll_margin(16.0)
                    .child(text_input(e, TextStyle::new(13.0, 400.0).lh(20.0)).key("field").w_full().on_edit(|s: &mut TextEdit| s)),
            )
        }, (300.0, 400.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.key(KeyEvent { key: Key::Text("!".into()), mods: Mods::default() });
        rt.frame(1.0 / 120.0);
        let rects = |rt: &Runtime<TextEdit>| {
            let els = rt.elements();
            let viewport = els.iter().find(|e| e.key.as_deref() == Some("viewport")).unwrap().rect;
            let caret = els.iter().find(|e| e.key.as_deref() == Some("field")).unwrap().caret.unwrap();
            (viewport[1], viewport[1] + viewport[3], caret[1], caret[1] + caret[3])
        };
        let (_, bottom, _, low) = rects(&rt);
        assert!((low - bottom).abs() < 1.0, "the last line stops flush with the content's end: {low} vs {bottom}");
        for step in 0..20 {
            rt.key(KeyEvent { key: Key::Up, mods: Mods::default() });
            rt.frame(1.0 / 120.0);
            let (top, bottom, high, low) = rects(&rt);
            assert!(high >= top + 16.0 - 0.5, "up {step}: caret top {high} leaves 16 pt under {top}");
            assert!(low <= bottom + 0.5, "up {step}: caret bottom {low} inside {bottom}");
        }
    }
}
