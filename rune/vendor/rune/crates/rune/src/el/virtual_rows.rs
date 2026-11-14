//! Index-built virtual rows: [`El::virtual_rows`](crate::El::virtual_rows) gives a virtual
//! list a row count, a key per index and a builder per index. Layout builds only the rows in
//! the mounted window, so a commit costs a key check per row plus the rows it mounts, however
//! long the list is.

use std::borrow::Cow;
use std::rc::Rc;

use crate::El;
use crate::el::ElFlags;
use crate::states::downcast;

impl El {
    /// Window keyed rows of a growth stack directly inside a vertical scroll viewport.
    /// Unvisited rows use an estimated height; measurement preserves the visible anchor.
    /// Offscreen rows have no layout, text or scene nodes.
    ///
    /// Use [`El::virtual_rows`] unless the list is short: the view still builds every row
    /// handed to `virtualize` on every commit, mounted or not. Debug builds warn once per list
    /// that builds more than 200 rows and four times what it mounts.
    ///
    /// The same element must also be a growth stack: call `.growth(GrowthSpec)` on it as well
    /// as `.virtualize(..)`, and make it the first child of the `.scroll(..)` viewport. Layout
    /// panics with the element's path otherwise.
    ///
    /// ```ignore
    /// col().scroll(ScrollSpec { stick_bottom: false }).grow(1.0).child(
    ///     col().key("rows").growth(GrowthSpec::default()).virtualize(32.0).children(rows),
    /// )
    /// ```
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn virtualize(mut self, estimated_row_height: f32) -> Self {
        self.cold_mut().virtual_estimate = Some(estimated_row_height.max(1.0));
        self.flags.set(ElFlags::HAS_VIRTUAL, true);
        #[cfg(debug_assertions)]
        { self.cold_mut().virtual_source = Some(std::panic::Location::caller()); }
        self
    }

    /// [`El::virtualize`] with rows built by index: `count` rows, row `i` keyed `key(s, i)` and
    /// built by `build(s, i)`. Layout calls `build` only for the rows in the mounted window (the
    /// viewport and up to two viewports either side), after it knows where that window is, so a
    /// commit costs one `key` call per row plus the rows it mounts, however long the list is.
    /// Rows new to the window beyond the viewport mount half a viewport per side per frame, so a
    /// jump or a prepended page doesn't lay out three viewports of text at once.
    /// Both closures get the window's state, so they capture nothing borrowed.
    ///
    /// Everything `virtualize` does holds: rows keep their measured heights by key while the key
    /// stays, unvisited rows use the estimate, inserting or prepending rows keeps the reader's
    /// row in place, and `Tx::scroll_into_view` reaches any row by key. A row's key is
    /// `key(s, i)`, whatever key its element had. The same element needs `.growth(GrowthSpec)`
    /// and must be the direct child of a `.scroll(..)` viewport, and holds no other children.
    ///
    /// A row outside the window isn't built, so it has no elements at all: the inspector,
    /// `Runtime::elements`, accessibility, Tab order, text selection and matched elements see the
    /// built rows only. A focused field in a row that scrolls out keeps its focus and typing (its
    /// row is built for that, not mounted); a field focused from state in a row that isn't built
    /// takes focus once the row is. A [`memo`](crate::memo) inside `build` keeps its subtree while
    /// the row stays built; a row built again after scrolling out builds afresh.
    ///
    /// Handlers in a row must find their item by its key or id, never by `i`. An index is only
    /// where the item sat when the row was built: a row kept by `memo` keeps its handlers while
    /// rows are inserted or removed above it, so a handler holding `i` would act on whatever
    /// item moved into that slot.
    ///
    /// ```ignore
    /// col().scroll(ScrollSpec { stick_bottom: true }).grow(1.0).child(
    ///     col().key("rows").growth(GrowthSpec::default()).virtual_rows(s.messages.len(), 72.0,
    ///         |s: &App, i| s.messages[i].id.as_str().into(),
    ///         |s: &App, i| {
    ///             let m = &s.messages[i];
    ///             let id = m.id.clone();
    ///             memo(&m.id, m.revision, || message(m).on_press(move |s: &mut App| s.open(&id)))
    ///         }),
    /// )
    /// ```
    pub fn virtual_rows<S: 'static>(
        mut self,
        count: usize,
        estimated_row_height: f32,
        key: impl for<'a> Fn(&'a S, usize) -> Cow<'a, str> + 'static,
        build: impl Fn(&S, usize) -> El + 'static,
    ) -> Self {
        self.cold_mut().virtual_rows = Some(Rc::new(RowSource::new(count, key, build)));
        self.virtualize(estimated_row_height)
    }

    /// Tells [`El::virtual_rows`] its keys are those of the last commit that gave the same
    /// `version`, so layout skips calling `key` for every row to find out. A list of tens of
    /// thousands of rows then commits in time independent of its length. Change `version`
    /// whenever rows are reordered, replaced or any row's key changes while the row count stays
    /// the same (a change in `count` is always detected without a bump); with a stale version on
    /// an in-place change the rows keep their old keys.
    pub fn virtual_rows_version(mut self, version: impl std::hash::Hash) -> Self {
        self.cold_mut().virtual_rows_version = Some(crate::memo::hash_of(&version));
        self
    }

    /// On a virtual list ([`El::virtualize`] or [`El::virtual_rows`]), called with the index
    /// range `start..end` of rows currently visible in its scroll viewport: once after its
    /// first layout, then whenever that range or the rows change. Return `true` when `f`
    /// changed state the view reads (so the view runs again), or `false` when it only updated
    /// bookkeeping (so scrolling through rows that don't change the view's output doesn't
    /// re-run the view).
    #[track_caller]
    pub fn on_visible_rows<S: 'static>(
        mut self,
        f: impl Fn(&mut S, std::ops::Range<usize>) -> bool + 'static,
    ) -> Self {
        self.handlers.on_visible_rows = Some(Rc::new(move |s, r| f(downcast::<S>(s), r)));
        #[cfg(debug_assertions)]
        { self.handlers.sites.visible_rows = Some(std::panic::Location::caller()); }
        self
    }
}

pub(crate) type KeyFn = dyn for<'a> Fn(crate::states::StatesRef<'a>, usize) -> Cow<'a, str>;
pub(crate) type BuildFn = dyn Fn(crate::states::StatesRef<'_>, usize) -> El;

/// The rows of an index-built list, built at layout time from the window's state.
pub(crate) struct RowSource {
    pub(crate) count: usize,
    pub(crate) key: Box<KeyFn>,
    pub(crate) build: Box<BuildFn>,
}

fn key_fn<F: for<'a> Fn(crate::states::StatesRef<'a>, usize) -> Cow<'a, str>>(f: F) -> F {
    f
}

fn state<S: 'static>(s: crate::states::StatesRef<'_>) -> &S {
    s.cast::<S>().expect("virtual rows registered for a different state type than the window's")
}

impl RowSource {
    pub(crate) fn new<S: 'static>(
        count: usize,
        key: impl for<'a> Fn(&'a S, usize) -> Cow<'a, str> + 'static,
        build: impl Fn(&S, usize) -> El + 'static,
    ) -> Self {
        RowSource {
            count,
            key: Box::new(key_fn(move |s, i| key(state::<S>(s), i))),
            build: Box::new(move |s, i| build(state::<S>(s), i)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::time::Instant;

    use crate::{Cx, El, Env, GrowthSpec, Runtime, ScrollSpec, col, el, memo, text};

    thread_local! {
        static VIEWS: Cell<usize> = const { Cell::new(0) };
        static VIEW_NS: Cell<u128> = const { Cell::new(0) };
        static BUILT: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
        /// Calls of `grid_rows`' builder for its first row: one per measurement pass.
        static PASSES: Cell<usize> = const { Cell::new(0) };
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Mode {
        /// `virtualize` with every row a `memo` the view makes.
        Memo,
        /// `virtual_rows`, each built row a `memo`.
        Index,
    }

    struct Chat {
        ids: Vec<u64>,
        keys: Vec<String>,
        tick: u64,
        markdown: bool,
        mode: Mode,
        /// Room above the first row, as a timeline's top padding.
        pad: f32,
    }

    impl Chat {
        fn new(n: u64, markdown: bool, mode: Mode) -> Self {
            let mut chat = Chat { ids: Vec::new(), keys: Vec::new(), tick: 0, markdown, mode, pad: 0.0 };
            chat.splice(0, (0..n).map(|i| 100_000 + i));
            chat
        }

        fn splice(&mut self, at: usize, ids: impl Iterator<Item = u64>) {
            let ids: Vec<u64> = ids.collect();
            self.keys.splice(at..at, ids.iter().map(|id| format!("m{id}")));
            self.ids.splice(at..at, ids);
        }
    }

    fn body(id: u64, markdown: bool) -> El {
        if !markdown {
            return col().w_full().h(72.0).padding(8.0).gap(4.0).children([
                text(format!("Message {id}")).key("title"),
                text(format!("Body of message {id}, a line of text that is long enough to wrap twice")).wrap().w_full().key("body"),
                text(format!("{id} min ago")).key("time"),
            ]);
        }
        let para = |n: u64| format!("Paragraph {n} of message {id}: {}", "some prose with `code`, **bold** words and links to wrap ".repeat((id % 4 + 1) as usize));
        col().w_full().padding(8.0).gap(6.0).children([
            text(format!("## Heading for message {id}")).key("h"),
            text(para(1)).wrap().w_full().key("p1"),
            text(para(2)).wrap().w_full().key("p2"),
            text(format!("fn message_{id}() -> u64 {{ {id} }}")).key("code"),
        ])
    }

    fn view(s: &Chat, _: &mut Cx<Chat>) -> El {
        let start = Instant::now();
        let stack = col().key("rows").w_full().pt(s.pad).growth(GrowthSpec::default());
        let stack = match s.mode {
            Mode::Memo => stack.virtualize(72.0).children(s.ids.iter().map(|&id| memo(format!("m{id}"), 0, || body(id, s.markdown)))),
            Mode::Index => stack.virtual_rows(s.ids.len(), 72.0, |s: &Chat, i| s.keys[i].as_str().into(), |s: &Chat, i| {
                BUILT.with_borrow_mut(|b| b.push(i));
                memo(s.keys[i].as_str(), 0, || body(s.ids[i], s.markdown))
            }),
        };
        let root = col().w_full().h_full().children([
            text(format!("tick {}", s.tick)).key("counter"),
            col().key("scroll").w_full().grow(1.0).scroll(ScrollSpec { stick_bottom: true }).child(stack),
        ]);
        VIEWS.set(VIEWS.get() + 1);
        VIEW_NS.set(VIEW_NS.get() + start.elapsed().as_nanos());
        root
    }

    fn commit<S: 'static>(rt: &mut Runtime<S>) {
        rt.invalidate();
        rt.frame(1.0 / 120.0);
    }

    fn chat(n: u64, markdown: bool, mode: Mode) -> Runtime<Chat> {
        let mut rt = Runtime::new(Chat::new(n, markdown, mode), view, (600.0, 800.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt
    }

    fn built() -> Vec<usize> {
        let mut b = BUILT.with_borrow_mut(std::mem::take);
        b.sort_unstable();
        b.dedup();
        b
    }

    /// Visible rows' keys and rects.
    fn visible<S: 'static>(rt: &Runtime<S>) -> Vec<(String, [f32; 4])> {
        rt.elements().into_iter()
            .filter(|e| e.visible[3] > 0.0)
            .filter_map(|e| Some((e.path.strip_prefix("#0/scroll/rows/").filter(|k| !k.contains('/'))?.to_owned(), e.rect)))
            .collect()
    }

    /// Rows `r{i}` of height `s[i]`, estimated at 50 points, in a 500-point viewport that starts
    /// at the top.
    fn fixed_view(s: &Vec<u32>, _: &mut Cx<Vec<u32>>) -> El {
        col().w_full().h_full().child(
            col().key("scroll").w_full().grow(1.0).scroll(ScrollSpec { stick_bottom: false }).child(
                col().key("rows").w_full().growth(GrowthSpec::default()).virtual_rows(s.len(), 50.0,
                    |_: &Vec<u32>, i| format!("r{i}").into(),
                    |s: &Vec<u32>, i| memo(format!("r{i}"), s[i], || {
                        BUILT.with_borrow_mut(|b| b.push(i));
                        el().w_full().h(s[i] as f32)
                    }))),
        )
    }

    fn fixed(n: usize) -> Runtime<Vec<u32>> {
        let mut rt = Runtime::new(vec![50; n], fixed_view, (300.0, 500.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt
    }

    /// Draws frames while the runtime asks for them, as an idle host does.
    fn until_idle<S: 'static>(rt: &mut Runtime<S>) -> usize {
        let mut frames = 0;
        while rt.frame(1.0 / 120.0) {
            frames += 1;
            assert!(frames < 60, "never rests");
        }
        frames
    }

    #[test]
    fn only_the_window_builds_and_scrolling_builds_what_comes_in() {
        let mut rt = chat(5000, false, Mode::Index);
        let first = built();
        assert!(!first.is_empty() && first.len() < 60, "built {} rows", first.len());
        assert!(first.iter().all(|&i| i >= 4900), "pinned to the bottom, the tail builds: {first:?}");
        assert!(rt.nodes.len() < 400, "{} nodes", rt.nodes.len());
        let (count, commits) = (first.len(), rt.stats.commits);
        for _ in 0..5 {
            rt.state.tick += 1;
            commit(&mut rt);
        }
        let ticks = built();
        assert!(rt.stats.commits >= commits + 5);
        assert!(ticks.len() <= count, "a tick builds the window only: {}", ticks.len());

        for _ in 0..80 {
            rt.wheel(300.0, 400.0, 400.0);
            rt.frame(1.0 / 120.0);
        }
        assert!(built().iter().any(|&i| i < 4900), "scrolling builds rows it brings in");
        rt.state.tick += 1;
        commit(&mut rt);
        let top = rt.state.keys.iter().position(|k| visible(&rt)[0].0 == *k).unwrap();
        assert!(top < 4600, "{top}");
        // The window is the viewport and two viewports either side; rows here are 72 points.
        let now = built();
        assert!(now.iter().all(|&i| i + 25 >= top && i <= top + 36), "only rows near the viewport build: {now:?} vs top {top}");
    }

    #[test]
    fn index_rows_lay_out_as_virtualize_does_while_paging_and_prepending() {
        let mut a = chat(600, true, Mode::Memo);
        let mut b = chat(600, true, Mode::Index);
        let same = |a: &Runtime<Chat>, b: &Runtime<Chat>, step: &str| {
            let (va, vb) = (visible(a), visible(b));
            assert!(!va.is_empty(), "{step}");
            assert_eq!(va.len(), vb.len(), "{step}");
            for ((ka, ra), (kb, rb)) in va.iter().zip(&vb) {
                assert_eq!(ka, kb, "{step}");
                assert!(ra.iter().zip(rb).all(|(x, y)| (x - y).abs() < 0.05), "{step}: {ka} {ra:?} vs {rb:?}");
            }
        };
        for step in 0..120 {
            same(&a, &b, &format!("step {step}"));
            let dy = if step < 80 { 211.5 } else { -97.0 };
            for rt in [&mut a, &mut b] {
                rt.wheel(300.0, 400.0, dy);
                rt.frame(1.0 / 120.0);
            }
        }
        let before = visible(&b);
        for rt in [&mut a, &mut b] {
            let first = rt.state.ids[0];
            rt.state.splice(0, first - 60..first);
            commit(rt);
        }
        same(&a, &b, "prepended");
        let after = visible(&b);
        assert_eq!(before[0].0, after[0].0, "prepending keeps the reader's row");
        assert!((before[0].1[1] - after[0].1[1]).abs() < 0.05, "in place: {:?} vs {:?}", before[0], after[0]);
        for _ in 0..40 {
            for rt in [&mut a, &mut b] {
                rt.wheel(300.0, 400.0, 3000.0);
                rt.frame(1.0 / 120.0);
            }
        }
        same(&a, &b, "scrolled into the prepended page");
        assert_eq!(visible(&b)[0].0, "m99940", "reached the top");
    }

    #[test]
    fn a_page_prepended_above_the_reader_at_the_top_keeps_them_in_place() {
        let mut rt = chat(300, true, Mode::Index);
        for _ in 0..20 {
            rt.wheel(300.0, 400.0, 100_000.0);
            rt.frame(1.0 / 120.0);
        }
        let before = visible(&rt);
        assert_eq!(before[0].0, "m100000", "at the top");
        built();
        let first = rt.state.ids[0];
        rt.state.splice(0, first - 60..first);
        commit(&mut rt);
        let after = visible(&rt);
        assert_eq!(before.len(), after.len());
        for ((kb, rb), (ka, ra)) in before.iter().zip(&after) {
            assert_eq!(kb, ka);
            assert!(rb.iter().zip(ra).all(|(x, y)| (x - y).abs() < 0.05), "{kb} moved: {rb:?} vs {ra:?}");
        }
        let page = built();
        assert!(page.iter().all(|&i| i < 60 + 40), "{page:?}");
        // The page above mounts a little per frame, not two viewports of new rows at once.
        assert!(page.iter().filter(|&&i| i < 60).count() <= 10, "{page:?}");
        for _ in 0..10 {
            rt.frame(1.0 / 120.0);
        }
        assert_eq!(visible(&rt), after, "and nothing drifts after");
    }

    #[test]
    fn a_page_prepended_while_the_top_padding_shows_keeps_the_reader_in_place() {
        // At the very top the padding above the first row shows, so the reader's row starts
        // below the offset. Once the page lands, that gap is the end of the row above it, still
        // at its estimate: measuring it must not push the reader's row down.
        let mut rt = chat(300, true, Mode::Index);
        rt.state.pad = 8.0;
        commit(&mut rt);
        for _ in 0..20 {
            rt.wheel(300.0, 400.0, 100_000.0);
            rt.frame(1.0 / 120.0);
        }
        let before = visible(&rt);
        assert_eq!(before[0].0, "m100000", "at the top");
        let first = rt.state.ids[0];
        rt.state.splice(0, first - 60..first);
        commit(&mut rt);
        for _ in 0..10 {
            let after = visible(&rt);
            for (key, was) in &before {
                let now = after.iter().find(|(k, _)| k == key).map(|(_, r)| r);
                assert!(now.is_some_and(|now| (now[1] - was[1]).abs() < 0.05), "{key} moved: {was:?} vs {now:?}");
            }
            rt.frame(1.0 / 120.0);
        }
    }

    #[test]
    fn scroll_into_view_reaches_a_row_never_built() {
        let mut rt = chat(5000, true, Mode::Index);
        built();
        rt.update(|s, tx| tx.scroll_into_view(s.keys[10].clone()));
        for _ in 0..3 {
            rt.frame(1.0 / 120.0);
        }
        let shown = visible(&rt);
        assert!(shown.iter().any(|(k, _)| *k == rt.state.keys[10]), "{:?}", shown.iter().map(|(k, _)| k).collect::<Vec<_>>());
        // The update's own commit, before the reveal, builds the tail it was at.
        let now = built();
        assert!(now.iter().all(|&i| i < 60 || i >= 4900), "only rows near it build: {now:?}");
    }

    #[test]
    fn a_count_shrunk_below_the_offset_shows_the_rows_left_in_the_same_frame() {
        let mut rt = fixed(1000);
        rt.wheel(150.0, 250.0, -40_000.0);
        until_idle(&mut rt);
        assert_eq!(visible(&rt)[0].0, "r800");
        rt.state.truncate(10);
        commit(&mut rt);
        let shown: Vec<_> = visible(&rt).into_iter().map(|(k, _)| k).collect();
        assert_eq!(shown, (0..10).map(|i| format!("r{i}")).collect::<Vec<_>>());
    }

    #[test]
    fn scroll_into_view_lands_a_row_taller_than_its_estimate_in_one_frame() {
        let mut rt = chat(5000, true, Mode::Index);
        for _ in 0..20 {
            rt.wheel(300.0, 400.0, 100_000.0);
            rt.frame(1.0 / 120.0);
        }
        assert_eq!(visible(&rt)[0].0, "m100000", "at the top");
        let key = rt.state.keys[500].clone();
        rt.update(|_, tx| tx.scroll_into_view(key.clone()));
        rt.frame(1.0 / 120.0);
        let viewport = rt.elements().into_iter().find(|e| e.path == "#0/scroll").unwrap().rect;
        let (_, row) = visible(&rt).into_iter().find(|(k, _)| *k == key).expect("revealed");
        assert!(row[3] > 90.0, "taller than the 72-point estimate: {row:?}");
        assert!(row[1] >= viewport[1] - 0.5 && row[1] + row[3] <= viewport[1] + viewport[3] + 0.5, "{row:?} in {viewport:?}");
    }

    /// Rows shown last commit that the first measurement pass leaves out and a retry brings
    /// back keep their memos and stay mounted, and a retry mounts its full overscan.
    #[test]
    fn a_retry_pass_keeps_what_the_last_commit_showed() {
        let mut rt = fixed(1000);
        rt.wheel(150.0, 250.0, -10_000.0);
        until_idle(&mut rt);
        assert_eq!(rt.virtuals["#0/scroll/rows"].range, 180..230);
        // Too small a step to move the window.
        rt.wheel(150.0, 250.0, -100.0);
        rt.frame(1.0 / 120.0);
        assert_eq!(rt.virtuals["#0/scroll/rows"].range, 180..230);
        built();
        // The first pass, at 10,100, leaves out rows 180 and 181. The rows it mounts collapse,
        // which moves the reader's row up and leaves the viewport short, so a retry mounts from
        // row 162 down, 180 and 181 among them.
        for h in &mut rt.state[182..232] { *h = 1; }
        commit(&mut rt);
        let v = &rt.virtuals["#0/scroll/rows"];
        assert_eq!(v.range.start, 162, "a retry isn't held to the per-frame budget");
        let now = built();
        assert!(!now.contains(&180) && !now.contains(&181), "their memos stay: {now:?}");
        assert!(now.len() < 120, "built {} rows", now.len());
        assert!(!rt.virtual_culled.iter().any(|p| p.ends_with("/r180") || p.ends_with("/r181")), "{:?}", rt.virtual_culled);
    }

    #[test]
    fn an_idle_window_mounts_all_its_overscan() {
        let mut rt = fixed(1000);
        assert_eq!(rt.virtuals["#0/scroll/rows"].range, 0..15, "the first commit mounts the viewport and half a viewport");
        until_idle(&mut rt);
        assert_eq!(rt.virtuals["#0/scroll/rows"].range, 0..30, "then the full overscan");
        rt.wheel(150.0, 250.0, -10_000.0);
        let frames = until_idle(&mut rt);
        assert_eq!(rt.virtuals["#0/scroll/rows"].range, 180..230, "after {frames} frames");
        assert!(!rt.frame(1.0 / 120.0), "then it rests");
    }

    /// A launcher's emoji grid: `s.1` rows of 74.4 points under a 100-point header and above a
    /// 50-point footer in a 500-point window, so the viewport is 350 points, under five rows.
    /// `s.0` shows the grid; until then the body is a label.
    fn grid_view(s: &(bool, usize), _: &mut Cx<(bool, usize)>) -> El {
        let body = if s.0 {
            col().key("scroll").w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(grid_rows::<(bool, usize)>(s.1, 74.4))
        } else {
            text("root").key("root")
        };
        col().w_full().h_full().children([el().key("header").w_full().h(100.0), body, el().key("footer").w_full().h(50.0)])
    }

    /// `n` index-built rows `g{i}` that lay out at 74.4 points, estimated at `estimate`.
    fn grid_rows<S: 'static>(n: usize, estimate: f32) -> El {
        col().key("rows").w_full().growth(GrowthSpec::default()).virtual_rows(n, estimate,
            |_: &S, i| format!("g{i}").into(),
            |_: &S, i| {
                if i == 0 { PASSES.set(PASSES.get() + 1) }
                memo(format!("g{i}"), 0, || {
                    BUILT.with_borrow_mut(|b| b.push(i));
                    el().w_full().h(74.4)
                })
            })
    }

    /// The viewport's rows: none missing between its top and bottom.
    fn assert_covered<S: 'static>(rt: &Runtime<S>, step: &str) {
        let viewport = rt.elements().into_iter().find(|e| e.path == "#0/scroll").expect("scroll").rect;
        let mut rows: Vec<_> = visible(rt).into_iter().map(|(_, r)| (r[1], r[1] + r[3])).collect();
        rows.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut y = viewport[1];
        for (top, bottom) in rows {
            assert!(top <= y + 0.5, "{step}: blank from {y} to {top} in {viewport:?}");
            y = y.max(bottom);
        }
        assert!(y >= viewport[1] + viewport[3] - 0.5, "{step}: blank below {y} in {viewport:?}");
    }

    #[test]
    fn a_first_showing_builds_its_viewport_not_the_window_times_overscan() {
        let mut rt = Runtime::new((false, 20), grid_view, (300.0, 500.0), Env::new(true, 1.0));
        rt.frame(0.0);
        built();
        rt.update(|s, _| s.0 = true);
        rt.frame(1.0 / 120.0);
        assert_covered(&rt, "first showing");
        let first = built();
        // The viewport's rows and half a viewport of overscan, from its 350 points, not the
        // window's 500 times the full overscan.
        assert_eq!(first, (0..8).collect::<Vec<_>>());
        // The rows' entrance outlasts the fill; the overscan fills within a few frames of it.
        for frame in 1..=6 {
            rt.frame(1.0 / 120.0);
            assert_covered(&rt, &format!("frame {frame}"));
        }
        assert_eq!(rt.virtuals["#0/scroll/rows"].range, 0..14, "then rows until its edge is 1.75 viewports away");
        assert_eq!(built(), (8..14).collect::<Vec<_>>(), "building only rows new to it");
    }

    /// A header that takes less than its height (a negative margin) makes the first showing's
    /// estimate short, 90 points of a 430-point viewport; one retry, measuring the viewport,
    /// fills it before anything is presented.
    #[test]
    fn a_first_showing_estimated_short_still_fills_its_viewport() {
        fn view(s: &(bool, usize), cx: &mut Cx<(bool, usize)>) -> El {
            let mut root = grid_view(s, cx);
            root.children[0] = el().key("header").w_full().h(360.0).mb(-340.0);
            root
        }
        let mut rt = Runtime::new((false, 20), view, (300.0, 500.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.update(|s, _| s.0 = true);
        PASSES.set(0);
        rt.frame(1.0 / 120.0);
        assert_covered(&rt, "first showing");
        assert_eq!(rt.elements().into_iter().find(|e| e.path == "#0/scroll").unwrap().rect[3], 430.0);
        assert_eq!(PASSES.get(), 2, "the estimate's pass and one retry");
        assert!(rt.virtuals["#0/scroll/rows"].range.end < 20, "not every row");
    }

    /// A window that fits its content has no height to bound its root: a results list opening
    /// under a 56-point search bar estimates its viewport from its own `max_h`.
    #[test]
    fn a_first_showing_in_a_window_that_fits_its_content_estimates_from_max_h() {
        fn view(s: &bool, _: &mut Cx<bool>) -> El {
            let results = s.then(|| col().key("scroll").w_full().max_h(350.0).scroll(ScrollSpec { stick_bottom: false })
                .child(grid_rows::<bool>(20, 74.4)));
            col().w_full().child(el().key("search").w_full().h(56.0)).children(results)
        }
        let mut rt = Runtime::new(false, view, (300.0, 500.0), Env::new(true, 1.0));
        rt.fit_height = true;
        let height = rt.content_height();
        rt.set_viewport((300.0, height), 1.0);
        rt.frame(0.0);
        assert_eq!(rt.viewport.1, 56.0);
        rt.update(|s, _| *s = true);
        PASSES.set(0);
        rt.frame(1.0 / 120.0);
        assert_eq!(PASSES.get(), 1, "measured once");
        assert_eq!(rt.virtuals["#0/scroll/rows"].range, 0..8, "350 points' rows and the overscan budget");
    }

    /// An out-of-flow popover isn't bounded by the 32-point header it hangs from, only by its
    /// own `max_h`.
    #[test]
    fn a_first_showing_in_an_absolute_popover_estimates_from_its_own_max_h() {
        fn view(s: &bool, _: &mut Cx<bool>) -> El {
            let popover = s.then(|| col().key("scroll").absolute(0.0, 32.0).w_full().max_h(300.0)
                .scroll(ScrollSpec { stick_bottom: false }).child(grid_rows::<bool>(20, 74.4)));
            col().w_full().h_full().child(el().key("header").w_full().h(32.0).children(popover))
        }
        let mut rt = Runtime::new(false, view, (300.0, 500.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.update(|s, _| *s = true);
        PASSES.set(0);
        rt.frame(1.0 / 120.0);
        assert_eq!(PASSES.get(), 1, "measured once");
        assert_eq!(rt.virtuals["#0/header/scroll/rows"].range, 0..8, "300 points' rows and the overscan budget");
    }

    /// Rows estimated short of their height: revealing the last on the first showing measures
    /// the rows it mounts on the way, and lands it in view against the end they make.
    #[test]
    fn a_first_showing_reveals_its_last_row_taller_than_estimated() {
        fn view(s: &(bool, usize), cx: &mut Cx<(bool, usize)>) -> El {
            let mut root = grid_view(s, cx);
            if s.0 { root.children[1].children[0] = grid_rows::<(bool, usize)>(s.1, 50.0) }
            root
        }
        let mut rt = Runtime::new((false, 20), view, (300.0, 500.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.update(|s, tx| { s.0 = true; tx.scroll_into_view("g19") });
        rt.frame(1.0 / 120.0);
        let viewport = rt.elements().into_iter().find(|e| e.path == "#0/scroll").unwrap().rect;
        let (_, row) = visible(&rt).into_iter().find(|(k, _)| k == "g19").expect("g19 mounted");
        assert!(row[1] >= viewport[1] - 0.5 && row[1] + row[3] <= viewport[1] + viewport[3] + 0.5, "{row:?} in {viewport:?}");
        assert_covered(&rt, "revealed");
    }

    #[test]
    fn a_first_showing_reveals_a_row_below_its_first_window() {
        let mut rt = Runtime::new((false, 20), grid_view, (300.0, 500.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.update(|s, tx| { s.0 = true; tx.scroll_into_view("g16") });
        rt.frame(1.0 / 120.0);
        assert_covered(&rt, "revealed");
        let viewport = rt.elements().into_iter().find(|e| e.path == "#0/scroll").unwrap().rect;
        let (_, row) = visible(&rt).into_iter().find(|(k, _)| k == "g16").expect("revealed in the first frame");
        assert!(row[1] >= viewport[1] - 0.5 && row[1] + row[3] <= viewport[1] + viewport[3] + 0.5, "{row:?} in {viewport:?}");
    }

    #[test]
    fn tooling_records_duplicate_keys_inside_a_row() {
        fn view(s: &usize, _: &mut Cx<usize>) -> El {
            col().key("scroll").size(200.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).child(
                col().key("rows").w_full().growth(GrowthSpec::default()).virtual_rows(*s, 40.0,
                    |_: &usize, i| format!("r{i}").into(),
                    |_: &usize, i| col().child(el().key("a").h(20.0)).child(el().key(if i == 1 { "a" } else { "b" }).h(20.0))))
        }
        let mut rt = Runtime::new(4, view, (200.0, 200.0), Env::new(true, 1.0));
        rt.key_warnings = Some(Vec::new());
        rt.frame(0.0);
        let warnings = rt.key_warnings.as_ref().unwrap();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("duplicate sibling key \"a\" under \"scroll/rows/r1\""), "{warnings:?}");
        let mut rt = Runtime::new(4, view, (200.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("a#dup1")), "the row lays out with the duplicate renamed");
    }

    #[test]
    fn duplicate_row_keys_are_renamed_instead_of_panicking() {
        fn view(indexed: &bool, _: &mut Cx<bool>) -> El {
            let rows = col().key("rows").w_full().growth(GrowthSpec::default());
            let rows = if *indexed {
                rows.virtual_rows(3, 40.0, |_: &bool, _| "dup".into(), |_: &bool, _| el().h(20.0))
            } else {
                rows.children((0..3).map(|_| el().key("dup").h(20.0))).virtualize(40.0)
            };
            col().key("scroll").size(200.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).child(rows)
        }
        for indexed in [true, false] {
            let mut rt = Runtime::new(indexed, view, (200.0, 200.0), Env::new(true, 1.0));
            rt.key_warnings = Some(Vec::new());
            rt.frame(0.0);
            rt.frame(1.0);
            let keys: Vec<_> = rt.elements().into_iter().filter_map(|e| e.key).filter(|k| k.starts_with("dup")).collect();
            assert_eq!(keys, ["dup", "dup#dup1", "dup#dup2"], "indexed {indexed}");
            let warnings = rt.key_warnings.as_ref().unwrap();
            assert!(warnings.iter().any(|w| w.contains("duplicate virtual row key \"dup\" under \"scroll/rows\"")), "{warnings:?}");
        }
    }

    /// A row holding an `aspect_fit` element measures at its fitted height, so rows below it sit
    /// where the second layout pass put it.
    #[test]
    fn rows_measure_an_aspect_fit_element_at_its_fitted_size() {
        fn view(s: &usize, _: &mut Cx<usize>) -> El {
            col().w_full().h_full().child(
                col().key("scroll").w_full().grow(1.0).scroll(ScrollSpec { stick_bottom: false }).child(
                    col().key("rows").w_full().growth(GrowthSpec::default()).virtual_rows(*s, 50.0,
                        |_: &usize, i| format!("r{i}").into(),
                        |_: &usize, _| col().w_full().child(el().key("pic").max_h(80.0).aspect_fit(2.0)))))
        }
        let mut rt = Runtime::new(20, view, (300.0, 500.0), Env::new(true, 1.0));
        rt.frame(0.0);
        until_idle(&mut rt);
        let rects: Vec<_> = rt.elements().into_iter().filter(|e| e.visible[3] > 0.0).map(|e| (e.path, e.rect)).collect();
        for i in 0..6 {
            let pic = rects.iter().find(|(p, _)| *p == format!("#0/scroll/rows/r{i}/pic")).expect("built").1;
            assert_eq!(pic, [0.0, i as f32 * 80.0, 160.0, 80.0], "r{i}");
        }
        assert_eq!(rt.virtuals["#0/scroll/rows"].height("r3"), 80.0);
    }

    #[test]
    fn heights_persist_for_rows_scrolled_away() {
        let mut rt = chat(800, true, Mode::Index);
        let key = rt.state.keys[795].clone();
        let measured = visible(&rt).into_iter().find(|(k, _)| *k == key).expect("near the tail").1[3];
        assert!((measured - 72.0).abs() > 1.0, "a markdown row isn't the estimate: {measured}");
        for _ in 0..10 {
            rt.wheel(300.0, 400.0, 2000.0);
            rt.frame(1.0 / 120.0);
        }
        assert!(rt.find(&key).is_none(), "scrolled away, unbuilt");
        let v = &rt.virtuals["#0/scroll/rows"];
        assert_eq!(v.height(&key), measured, "its height stays measured");
        assert_eq!(v.height(&rt.state.keys[400]), 72.0, "a row never seen is estimated");
    }

    #[test]
    fn a_focused_field_in_a_row_scrolled_out_keeps_typing() {
        use crate::{Key, KeyEvent, TextEdit, TextStyle, text_input};
        fn view(s: &Vec<TextEdit>, _: &mut Cx<Vec<TextEdit>>) -> El {
            col().key("viewport").size(200.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).clip_soft(0.0)
                .child(col().key("rows").w_full().growth(GrowthSpec::default()).virtual_rows(s.len(), 40.0,
                    // A `/` in a key doesn't lose the row.
                    |_: &Vec<TextEdit>, i| format!("row/{i}").into(),
                    |s: &Vec<TextEdit>, i| col().size(200.0, 40.0).child(text_input(&s[i], TextStyle::new(14.0, 400.0)).key("field")
                        .size(200.0, 40.0).on_edit(move |s: &mut Vec<TextEdit>| &mut s[i]))))
        }
        let mut state: Vec<_> = (0..1000).map(|_| TextEdit::new("")).collect();
        state[0].focused = true;
        let mut rt = Runtime::new(state, view, (200.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.wheel(100.0, 100.0, -20_000.0);
        rt.frame(0.0);
        assert!(rt.find("row/0/field").is_none(), "the focused row scrolled out");
        rt.key(KeyEvent { key: Key::Text("offscreen".into()), mods: Default::default() });
        rt.frame(0.0);
        assert_eq!(rt.state[0].text, "offscreen");
        assert!(rt.state[0].focused);
        rt.wheel(100.0, 100.0, 20_000.0);
        rt.frame(0.0);
        assert!(rt.find("row/0/field").is_some(), "back in view");
    }

    #[test]
    fn a_version_skips_checking_every_key_until_it_changes() {
        thread_local! { static KEYS: Cell<usize> = const { Cell::new(0) }; }
        // (tick, version, key prefix, count)
        type S = (u32, u32, &'static str, usize);
        fn view(s: &S, _: &mut Cx<S>) -> El {
            col().key("scroll").w_full().h_full().scroll(ScrollSpec { stick_bottom: false }).child(
                col().key("rows").w_full().growth(GrowthSpec::default())
                    .virtual_rows(s.3, 20.0,
                        |s: &S, i| {
                            KEYS.set(KEYS.get() + 1);
                            format!("{}{i}", s.2).into()
                        },
                        |s: &S, i| text(format!("{} {i}", s.0)).h(20.0))
                    .virtual_rows_version(s.1))
        }
        let mut rt = Runtime::new((0, 0, "a", 10_000), view, (300.0, 400.0), Env::default());
        rt.frame(0.0);
        KEYS.set(0);
        rt.state.0 += 1;
        rt.invalidate();
        rt.frame(1.0 / 60.0);
        assert!(KEYS.get() < 1000, "the same version checks no keys: {}", KEYS.get());
        assert!(rt.find("a0").is_some());
        rt.state.2 = "b";
        rt.state.1 += 1;
        rt.invalidate();
        rt.frame(1.0 / 60.0);
        assert!(rt.find("b0").is_some() && rt.find("a0").is_none(), "a new version takes the new keys");
        // Changing the row count refreshes keys even if the caller forgot to bump `version`.
        rt.state.2 = "c";
        rt.state.3 = 10_001;
        rt.invalidate();
        rt.frame(1.0 / 60.0);
        assert!(rt.find("c0").is_some() && rt.find("b0").is_none(), "a count change refreshes keys without a version bump");
    }

    #[test]
    fn visible_rows_reports_range_without_committing_when_unchanged() {
        // (visible range, commit on change)
        type S = (std::ops::Range<usize>, bool);
        fn view(_: &S, _: &mut Cx<S>) -> El {
            col().key("scroll").w_full().h_full().scroll(ScrollSpec { stick_bottom: false }).child(
                col().key("rows").w_full().growth(GrowthSpec::default())
                    .virtual_rows(1_000, 20.0,
                        |_: &S, i| format!("r{i}").into(),
                        |_: &S, i| text(format!("row {i}")).h(20.0))
                    .virtual_rows_version(0u32)
                    .on_visible_rows(|s: &mut S, r| {
                        s.0 = r;
                        s.1
                    }))
        }
        let mut rt = Runtime::new((0..0, false), view, (300.0, 200.0), Env::default());
        for _ in 0..6 {
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(rt.state.0, 0..10);
        let commits = rt.stats.commits;
        // Scrolling within the mounted overscan updates the visible range without re-running the view.
        rt.wheel(150.0, 100.0, -40.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.0, 2..12);
        assert_eq!(rt.stats.commits, commits, "returning false from on_visible_rows skips a commit");
        // Returning true triggers a commit when the visible range changes.
        rt.state.1 = true;
        rt.wheel(150.0, 100.0, -40.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.0, 4..14);
        assert_eq!(rt.stats.commits, commits + 1, "returning true commits");
    }

    #[test]
    fn culled_rows_unmount_their_whole_subtree_immediately() {
        fn view(_: &(), _: &mut Cx<()>) -> El {
            col().key("scroll").w_full().h_full().scroll(ScrollSpec { stick_bottom: false }).child(
                col().key("rows").w_full().growth(GrowthSpec::default())
                    .virtual_rows(500, 20.0,
                        |_: &(), i| format!("r{i}").into(),
                        |_: &(), i| col().h(20.0).child(col().key("cell").child(text(format!("row {i}")).key("txt")))))
        }
        let mut rt = Runtime::new((), view, (300.0, 200.0), Env::default());
        for _ in 0..6 {
            rt.frame(1.0 / 60.0);
        }
        assert!(rt.nodes.iter().any(|n| n.path.ends_with("/r0/cell/txt")));
        // Scroll far enough down that row 0 leaves the mounted overscan window.
        rt.wheel(150.0, 100.0, -2_000.0);
        rt.frame(1.0 / 60.0);
        assert!(
            !rt.nodes.iter().any(|n| n.path.contains("/r0/")),
            "culled row descendants are unmounted immediately, not left orphaned in nodes"
        );
    }

    #[test]
    fn memo_rows_build_once_while_they_stay_in_the_window() {
        thread_local! { static BUILDS: Cell<usize> = const { Cell::new(0) }; }
        fn view(s: &(u32, u32), _: &mut Cx<(u32, u32)>) -> El {
            col().w_full().h_full().children([
                text(format!("tick {}", s.0)).key("counter"),
                col().key("scroll").w_full().grow(1.0).scroll(ScrollSpec { stick_bottom: true }).child(
                    col().key("rows").w_full().growth(GrowthSpec::default()).virtual_rows(s.1 as usize, 40.0,
                        |_: &(u32, u32), i| format!("r{i}").into(),
                        |_: &(u32, u32), i| memo(format!("r{i}"), 0, || {
                            BUILDS.set(BUILDS.get() + 1);
                            text(format!("row {i}")).h(40.0)
                        }))),
            ])
        }
        let mut rt = Runtime::new((0, 2000), view, (300.0, 400.0), Env::default());
        rt.frame(0.0);
        let first = BUILDS.get();
        assert!(first > 0 && first < 60, "{first}");
        for _ in 0..10 {
            rt.state.0 += 1;
            rt.invalidate();
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(BUILDS.get(), first, "ticks reuse every mounted row");
        rt.state.1 += 1;
        rt.invalidate();
        rt.frame(1.0 / 60.0);
        assert!(rt.find("r2000").is_some(), "an appended row mounts at the pinned tail");
        assert_eq!(BUILDS.get(), first + 1, "and only it builds");
    }

    /// `cargo test -p rune --profile perf --lib virtual_rows_time -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn virtual_rows_time() {
        for mode in [Mode::Memo, Mode::Index] {
            let name = if mode == Mode::Memo { "memo " } else { "index" };
            for n in [500, 2000, 5000] {
                let mut rt = chat(n, false, mode);
                for _ in 0..30 {
                    rt.state.tick += 1;
                    commit(&mut rt);
                }
                let frames = 300;
                VIEW_NS.set(0);
                let start = Instant::now();
                for _ in 0..frames {
                    rt.state.tick += 1;
                    commit(&mut rt);
                }
                let per = start.elapsed().as_secs_f64() * 1e6 / f64::from(frames);
                let view = VIEW_NS.get() as f64 / 1e3 / f64::from(frames);
                println!("{name} tick rows {n:5}: {per:8.1} µs per frame ({view:7.1} µs in the view)");
            }

            // Older messages load at the top while the reader is there.
            for _ in 0..3 {
                let mut rt = chat(3700, true, mode);
                for _ in 0..10 {
                    rt.wheel(300.0, 400.0, 100_000.0);
                    rt.frame(1.0 / 120.0);
                }
                let (views, shaped) = (VIEWS.get(), rt.text.shaped);
                VIEW_NS.set(0);
                let first = rt.state.ids[0];
                rt.state.splice(0, first - 60..first);
                let start = Instant::now();
                commit(&mut rt);
                let ms = start.elapsed().as_secs_f64() * 1e3;
                // An index-built list mounts the rest of the page above over the next frames.
                let mut after = 0.0f64;
                for _ in 0..30 {
                    let start = Instant::now();
                    rt.frame(1.0 / 120.0);
                    after = after.max(start.elapsed().as_secs_f64() * 1e3);
                }
                println!("{name} prepend 60 to 3700: {ms:6.2} ms, {} view runs ({:.2} ms), {} shaped, {} nodes; slowest frame after {after:.2} ms",
                    VIEWS.get() - views, VIEW_NS.get() as f64 / 1e6, rt.text.shaped - shaped, rt.nodes.len());
            }

            // Reading up through history: which frames are slow, and do they shape?
            let mut rt = chat(3700, true, mode);
            let mut slow = Vec::new();
            let mut times = Vec::new();
            for f in 0..400 {
                rt.wheel(300.0, 400.0, 30.0);
                let (shaped, commits, views) = (rt.text.shaped, rt.stats.commits, VIEWS.get());
                VIEW_NS.set(0);
                let start = Instant::now();
                rt.frame(1.0 / 120.0);
                let us = start.elapsed().as_secs_f64() * 1e6;
                times.push(us);
                if us > 1000.0 {
                    slow.push(format!("f{f}: {us:.0} µs, {} commits, {} views ({:.0} µs), {} shaped",
                        rt.stats.commits - commits, VIEWS.get() - views, VIEW_NS.get() as f64 / 1e3, rt.text.shaped - shaped));
                }
            }
            times.sort_by(f64::total_cmp);
            println!("{name} scroll 400 frames: p50 {:.0} µs, p90 {:.0} µs, max {:.0} µs; {} over 1 ms:", times[200], times[360], times[399], slow.len());
            for s in slow.iter().take(6) { println!("  {s}"); }
        }
    }
}
