//! [`memo`]: reuse a subtree whose inputs haven't changed instead of building it again.
//!
//! A view runs top to bottom every commit, so a transcript of 500 messages builds 500 messages
//! every frame its counter ticks. Wrapping each in `memo(id, version, || ...)` builds a message
//! once per version: later commits get the element the first build returned, without calling
//! the closure. Children reach the element tree only when the element mounts, so the rows a
//! `virtualize` list leaves offscreen cost a lookup each and nothing else.
//!
//! What a reused subtree keeps, and what invalidates it:
//!
//! - **Identity.** A reused subtree is the same elements under the same key paths, so node ids,
//!   hover and press, focus, scroll offsets and running springs carry on exactly as if it had
//!   been built again. The inspector, `--tree`, `Runtime::elements` and accessibility read the
//!   element tree after the subtree is in it, and see it like any other.
//! - **Handlers** in the subtree are the ones its build made. They see what they captured then,
//!   so anything a handler captures that can change must be part of `version`. Handlers that
//!   only send a message or an id never go stale. A row's position is one of those things: a
//!   handler in a list row should capture the row's id, never its index, which shifts when
//!   rows are inserted or removed above it while the memo keeps the old handler.
//! - **Live values and pixels** stay bound: a `Live`, `LiveText` or `Pixels` is a shared handle,
//!   so a reused subtree shows its newest value or image without a rebuild.
//! - **Environment.** The window's dark mode, scale, reduced-motion preference, system accent
//!   (`Cx::accent`, `Cx::focus_color`) and blend mode (`theme::blends_srgb`, which theme
//!   colours resolve by) are part of every version, so changing any of them builds
//!   again. App state the closure reads (a theme, a locale, a selection) must be part of
//!   `version`. For a text field that's the
//!   `TextEdit` itself, which hashes its text, selection, composition and focus.
//! - **`Cx` reads.** `Cx::layout`, `Cx::layout_with`, `Cx::layout_size`, `Cx::window_layout`,
//!   `Cx::window_layout_with` and `Cx::caret` called inside the closure are recorded with
//!   what they returned. A reuse counts them as read
//!   again, so the view still re-runs when they move, and builds again when one returns
//!   something else. They needn't be in `version`.
//! - **Hot reload** (`rune/hot`) drops every cached subtree, so new code always builds.
//! - **Virtual rows** that scroll out keep their cached subtree as long as the view still
//!   creates them (virtualized rows always are); scrolling back in mounts it again without a
//!   build. A memo the view stops creating is dropped once the commit that stopped creating it
//!   applies (a commit may build its view more than once). Rows `El::virtual_rows` builds count
//!   as part of the view build, so a memo in one is kept while its row stays in the mounted
//!   window and dropped once the row leaves it.
//!
//! The id names the subtree within the window, so it must be unique among the memos of one
//! view build (a message id, say). A memo inside another's closure is named within it, so
//! `memo("b0", ..)` inside each message's memo doesn't collide. A second memo with an id
//! already used in the same build just builds, uncached.
//!
//! Layout isn't stored: a reused subtree still lays out each commit it is mounted, but Taffy
//! keeps each unchanged node's measured size, so this is cheap while width and constraints hold.

use std::cell::RefCell;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use crate::El;
use crate::el::ElFlags;
use crate::runtime::HashMap;
use crate::runtime::layout_read::{LayoutRects, SELECTION, Same, boxes_moved, moved, note_read, seen_rect, size, window_key};

pub(crate) mod snapshot;
pub use snapshot::snapshot;

/// The children a memo's placeholders share.
#[derive(Clone)]
pub(crate) struct Shared {
    pub(crate) children: Vec<El>,
    /// Whether a focused field is among them, so an offscreen virtual row that has none isn't
    /// searched for one.
    pub(crate) focused_input: bool,
    /// Whether two siblings anywhere among them share a key, so the per-commit key check can
    /// skip a memo without any.
    pub(crate) duplicate_keys: bool,
    /// Whether any descendant uses `virtualize` or `virtual_rows`.
    pub(crate) has_virtual: bool,
    /// Whether every descendant can reuse its flattened `Node` and Taffy subtree across commits.
    pub(crate) reusable: bool,
}

impl El {
    /// Every child: those shared with a memo cache, then the element's own.
    pub(crate) fn all_children(&self) -> impl Iterator<Item = &El> {
        self.memo_children().into_iter().flat_map(|c| c.children.iter()).chain(&self.children)
    }

    /// The children shared with a memo cache, which come before `children`.
    #[inline]
    pub(crate) fn memo_children(&self) -> Option<&Rc<Shared>> {
        self.memo.as_ref().map(|b| &b.children)
    }

    /// The cached subtree this placeholder is, unless it was expanded or given extra children.
    #[inline]
    pub(crate) fn memo_built(&self) -> Option<&Rc<Built>> {
        self.memo.as_ref().filter(|_| !self.flags.has(ElFlags::MEMO_EXTENDED))
    }

    /// Copies in the children shared with a memo cache, so the element owns all of them.
    pub(crate) fn expand_memo(&mut self) {
        self.flags.set(ElFlags::MEMO_EXTENDED, false);
        if let Some(built) = self.memo.take() {
            if built.children.has_virtual {
                self.flags.set(ElFlags::HAS_VIRTUAL, true);
            }
            let shared = Rc::unwrap_or_clone(built.children.clone()).children;
            let own = std::mem::replace(&mut self.children, shared);
            self.children.extend(own);
        }
    }
}

fn el_flatten_reusable(e: &El) -> bool {
    let c = &*e.cold;
    c.virtual_estimate.is_none()
        && c.growth.is_none()
        && c.aspect_fit <= 0.0
        && c.above.is_none()
        && c.live.is_empty()
        && c.input.is_none()
        && e.handlers.get().is_none_or(|h| h.modal.is_none())
        && !(c.scroll.is_some() && c.scroll_axes.x())
        && e.text.as_ref().is_none_or(|t| !t.redacted && t.live.is_none())
        && e.vis.pixels.as_ref().is_none_or(|_| {
            let sz = e.style.size();
            e.style.aspect_ratio().is_some() || (sz.width.is_auto() == sz.height.is_auto())
        })
        && e.memo_children().is_none_or(|m| m.reusable && !m.duplicate_keys)
        && e.children.iter().all(el_flatten_reusable)
}

/// Precomputes `shape` on plain, non-input, non-cased text specs at `scale` while the freshly
/// built subtree is still uniquely owned, so flattening never has to copy a shared `TextSpec`.
fn prekey_text_shapes(el: &mut El, scale: f32) {
    if el.cold.input.is_none()
        && let Some(t) = &el.text
        && !t.redacted
        && t.case.is_none()
        && t.rings.is_empty()
        && t.shape.is_none_or(|s| s.0 != scale.to_bits())
        && let Some(t) = &mut el.text
    {
        t.key_shape(scale);
    }
    for child in &mut el.children {
        prekey_text_shapes(child, scale);
    }
}

/// Whether `el` or anything under it is a focused field.
pub(crate) fn has_focused_input(el: &El) -> bool {
    el.cold.input.as_ref().is_some_and(|i| i.edit.focused)
        || el.memo_children().is_some_and(|m| m.focused_input)
        || el.children.iter().any(has_focused_input)
}

/// A built subtree: its root without children (or key, supplied per placeholder), and the
/// children placeholders share.
pub(crate) struct Built {
    pub(crate) id: u64,
    pub(crate) reusable: bool,
    pub(crate) root: El,
    pub(crate) children: Rc<Shared>,
}

static NEXT_BUILT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl Built {
    fn new(mut el: El, scale: f32) -> (Rc<Built>, Option<String>) {
        let key = el.key.take();
        el.flags.set(ElFlags::MEMO_EXTENDED, false);
        let shared = if el.children.is_empty() && let Some(built) = el.memo.take() {
            prekey_text_shapes(&mut el, scale);
            built.children.clone()
        } else {
            el.expand_memo();
            prekey_text_shapes(&mut el, scale);
            let children = std::mem::take(&mut el.children);
            let focused_input = children.iter().any(has_focused_input);
            let duplicate_keys = crate::input::key_diagnostics::memo_children_have_duplicates(&el, &children);
            let has_virtual = children.iter().any(|c| c.flags.has(ElFlags::HAS_VIRTUAL));
            let reusable = !duplicate_keys && !has_virtual && children.iter().all(el_flatten_reusable);
            Rc::new(Shared { children, focused_input, duplicate_keys, has_virtual, reusable })
        };
        el.flags.set(ElFlags::HAS_VIRTUAL, el.cold.virtual_estimate.is_some());
        let reusable = shared.reusable && !shared.duplicate_keys && el_flatten_reusable(&el);
        let id = NEXT_BUILT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        (Rc::new(Built { id, reusable, root: el, children: shared }), key)
    }

    /// The root, with its children shared rather than copied.
    fn placeholder(self: &Rc<Self>, key: Option<String>) -> El {
        let mut el = self.root.clone();
        el.key = key;
        if self.children.has_virtual {
            el.flags.set(ElFlags::HAS_VIRTUAL, true);
        }
        el.memo = Some(self.clone());
        el
    }
}

/// A `Cx` read made while a memo built, with what it returned.
#[derive(Clone)]
enum Read {
    Caret(String, Option<[f32; 4]>),
    Layout(String, Option<[f32; 4]>),
    /// A `Cx::layout_with` read: its function's check, and the rect it was given.
    LayoutWith(String, Same, Option<[f32; 4]>),
    /// A `Cx::selection_rects` read, and the boxes it returned.
    Selection(Rc<[[f32; 4]]>),
}

impl PartialEq for Read {
    fn eq(&self, other: &Read) -> bool {
        match (self, other) {
            (Read::Caret(a, x), Read::Caret(b, y)) | (Read::Layout(a, x), Read::Layout(b, y)) => a == b && x == y,
            (Read::LayoutWith(a, f, x), Read::LayoutWith(b, g, y)) => a == b && f.is(g) && x == y,
            (Read::Selection(a), Read::Selection(b)) => a == b,
            _ => false,
        }
    }
}

/// What the `Cx` of the view being built would return now.
struct Seen {
    caret: Option<(String, [f32; 4])>,
    layout: LayoutRects,
}

impl Seen {
    fn holds(&self, read: &Read) -> bool {
        match read {
            Read::Caret(key, rect) => crate::runtime::caret_of(&self.caret, key) == *rect,
            Read::Layout(key, rect) => !moved(*rect, seen_rect(&self.layout, key)),
            Read::LayoutWith(key, same, rect) => !same.moved(*rect, seen_rect(&self.layout, key)),
            Read::Selection(boxes) => !boxes_moved(boxes, &self.layout.selection),
        }
    }
}

/// `Cx` reads made by memos this build reused, which the view's `Cx` didn't see.
#[derive(Default)]
pub(crate) struct Replayed {
    pub(crate) caret: bool,
    pub(crate) layout: crate::runtime::layout_read::LayoutReads,
}

struct Entry {
    id: Rc<str>,
    version: u64,
    built: Rc<Built>,
    key: Option<String>,
    /// The `Cx` reads its build made, its nested memos' included.
    reads: Vec<Read>,
    /// Memos built inside this one's closure, kept while this one is reused.
    nested: Vec<Rc<str>>,
    /// The view build that last used it.
    pass: u64,
    /// `0` when built in `view`, else [`virtual_row_tag`] of the `virtual_rows` row that built it.
    virtual_row: u64,
}

/// One window's cached subtrees.
#[derive(Default)]
pub(crate) struct MemoCache {
    entries: HashMap<Rc<str>, Entry>,
    pub(crate) flattened: crate::runtime::HashMap<u64, crate::runtime::memo_reuse::FlattenedMemo>,
    pub(crate) pending_scratch: Vec<crate::runtime::memo_reuse::PendingFlat>,
    pub(crate) reused_scratch: Vec<crate::runtime::memo_reuse::ReusedSpan>,
    pub(crate) skip_scratch: Vec<crate::runtime::memo_reuse::CleanApplySpan>,
    pub(crate) split_scratch: Vec<bool>,
    pub(crate) taken_paths_scratch: Vec<(u32, u32)>,
    pub(crate) path_scratch: String,
    id_scratch: String,
    pure_counts: HashMap<(u64, u64, u64), u32>,
    nested_scratch: Vec<(Rc<str>, u64)>,
    pub(crate) present_len: usize,
    pass: u64,
    #[cfg(test)]
    pub(crate) builds: usize,
}

impl MemoCache {
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.flattened.clear();
        self.present_len = 0;
    }

    /// No memos, counting passes on from this cache's, for a build that must start from scratch.
    #[cfg(feature = "hot")]
    pub(crate) fn emptied(&self) -> MemoCache {
        MemoCache { pass: self.pass, ..MemoCache::default() }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// The pass the next view build will be.
    pub(crate) fn next_pass(&self) -> u64 {
        self.pass + 1
    }

    /// Drops the memos whose own id starts with `prefix`, wherever they nest, and those built
    /// inside them.
    pub(crate) fn evict_prefix(&mut self, prefix: &str) {
        let before = self.entries.len();
        self.entries.retain(|id, _| !(id.starts_with(prefix) || id.match_indices('/').any(|(i, _)| id[i + 1..].starts_with(prefix))));
        if self.entries.len() < before {
            let live: crate::runtime::HashSet<u64> = self.entries.values().map(|e| e.built.id).collect();
            self.flattened.retain(|id, _| live.contains(id));
        }
    }

    /// Drops the memos no build since pass `since` (a [`MemoCache::next_pass`]) used: a commit
    /// keeps what any of its view builds used.
    pub(crate) fn keep_since(&mut self, since: u64) {
        // A commit that didn't build the view used no memos, and drops none.
        if self.pass < since { return; }
        let before = self.entries.len();
        self.entries.retain(|_, e| e.pass >= since);
        if self.entries.len() < before && !self.flattened.is_empty() {
            let live: crate::runtime::HashSet<u64> = self.entries.values().map(|e| e.built.id).collect();
            self.flattened.retain(|id, _| live.contains(id));
        }
    }
}

impl<S: 'static> crate::Runtime<S> {
    /// Drops this window's cached subtrees whose memo id starts with `prefix`, wherever they
    /// nest, and those built inside them, then re-runs the view. A library that built them
    /// can unload once the commit after this drops its handlers. A memo enclosing a dropped
    /// one still reuses its own subtree, so it keeps what changed in its version.
    pub fn invalidate_memos_by_prefix(&mut self, prefix: &str) {
        self.memo.evict_prefix(prefix);
        self.invalidate();
    }
}

struct Scope {
    cache: MemoCache,
    env: u64,
    scale: f32,
    virtual_row: u64,
    /// The ids of the memos whose closures are running, innermost last.
    path: Vec<Rc<str>>,
    /// The memos each running closure has created so far.
    nested: Vec<Vec<Rc<str>>>,
    /// The `Cx` reads each running closure has made so far.
    reads: Vec<Vec<Read>>,
    seen: Seen,
    replayed: Replayed,
}

impl Scope {
    /// Counts `reads` toward the running closure, if any.
    fn note(&mut self, reads: &[Read]) {
        let Some(frame) = self.reads.last_mut() else { return };
        for read in reads {
            if !frame.contains(read) {
                frame.push(read.clone());
            }
        }
    }
}

thread_local! {
    static SCOPES: RefCell<Vec<Scope>> = const { RefCell::new(Vec::new()) };
}

/// Makes `cache` the one [`memo`] uses until [`Guard::finish`], for one view build.
/// `caret` and `layout` are what the view's `Cx` returns.
#[allow(clippy::too_many_arguments)]
pub(crate) fn enter(
    mut cache: MemoCache,
    env: crate::Env,
    accent: crate::SystemAccent,
    reduced_motion: bool,
    srgb_blend: bool,
    chrome: [u32; 4],
    caret: Option<(String, [f32; 4])>,
    layout: LayoutRects,
) -> Guard {
    cache.pass += 1;
    cache.pure_counts.clear();
    let accent = [accent.accent(env.system_dark), accent.focus(env.system_dark)].map(|c| [c.r, c.g, c.b, c.a].map(f32::to_bits));
    let scale = env.scale;
    // Theme colours resolve per blend mode (`theme::blends_srgb`), so a switch rebuilds memos.
    // So does a change to what the window's controls take (`Cx::chrome_insets`), and to the icon
    // swap, whose icons can take another size (`text::set_icon_swap_size`).
    let icons = crate::text::icon::swap_state();
    let env = hash_of(&(env.system_dark, scale.to_bits(), reduced_motion, accent, crate::platform::scroll_bars::scoped(), srgb_blend, chrome, icons));
    let scope = Scope {
        cache,
        env,
        scale,
        virtual_row: 0,
        path: Vec::new(),
        nested: Vec::new(),
        reads: Vec::new(),
        seen: Seen { caret, layout },
        replayed: Replayed::default(),
    };
    SCOPES.with_borrow_mut(|s| s.push(scope));
    Guard(true)
}

pub(crate) fn virtual_row_tag(list_path: &str, row_key: &str) -> u64 {
    hash_of(&(list_path, row_key)).max(1)
}

pub(crate) fn in_virtual_row<R>(tag: u64, f: impl FnOnce() -> R) -> R {
    struct Reset(Option<u64>);
    impl Drop for Reset {
        fn drop(&mut self) {
            if let Some(prev) = self.0 {
                SCOPES.with_borrow_mut(|scopes| {
                    if let Some(s) = scopes.last_mut() {
                        s.virtual_row = prev;
                    }
                });
            }
        }
    }
    let prev = SCOPES.with_borrow_mut(|scopes| {
        scopes.last_mut().map(|s| std::mem::replace(&mut s.virtual_row, tag))
    });
    let _reset = Reset(prev);
    f()
}

/// [`enter`] for a commit that skips running `view` and builds only newly entering virtual rows,
/// keeping memos built in `view` (`virtual_row == 0`) or in rows `kept_rows` stays mounted on.
#[allow(clippy::too_many_arguments)]
pub(crate) fn enter_partial(
    mut cache: MemoCache,
    kept_rows: &crate::runtime::HashSet<u64>,
    env: crate::Env,
    accent: crate::SystemAccent,
    reduced_motion: bool,
    srgb_blend: bool,
    chrome: [u32; 4],
    caret: Option<(String, [f32; 4])>,
    layout: LayoutRects,
) -> Guard {
    let next = cache.pass + 1;
    let mut nested = std::mem::take(&mut cache.nested_scratch);
    nested.clear();
    for e in cache.entries.values_mut() {
        if e.virtual_row == 0 || kept_rows.contains(&e.virtual_row) {
            e.pass = next;
            nested.extend(e.nested.iter().map(|id| (id.clone(), e.virtual_row)));
        }
    }
    if !nested.is_empty() {
        keep_nested_rows(&mut cache, &mut nested, next);
    }
    cache.nested_scratch = nested;
    enter(cache, env, accent, reduced_motion, srgb_blend, chrome, caret, layout)
}

/// Records a `Cx::caret` read for the memo building, if any.
pub(crate) fn read_caret(key: &str, rect: Option<[f32; 4]>) {
    record(|| Read::Caret(key.to_owned(), rect));
}

/// Records a `Cx::layout` read for the memo building, if any.
pub(crate) fn read_layout(key: &str, rect: Option<[f32; 4]>) {
    record(|| Read::Layout(key.to_owned(), rect));
}

/// Records a `Cx::selection_rects` read for the memo building, if any.
pub(crate) fn read_selection(boxes: &Rc<[[f32; 4]]>) {
    record(|| Read::Selection(boxes.clone()));
}

/// Records a `Cx::layout_with` read for the memo building, if any.
pub(crate) fn read_layout_with(key: &str, same: &Same, rect: Option<[f32; 4]>) {
    record(|| Read::LayoutWith(key.to_owned(), same.clone(), rect));
}

/// [`Cx::layout`](crate::Cx::layout) for a component that builds inside a view without taking
/// [`Cx`](crate::Cx); `None` outside a view build.
pub fn layout(key: &str) -> Option<[f32; 4]> {
    read_outside_cx(key, |rect| Read::Layout(key.to_owned(), rect)).flatten()
}

/// [`Cx::layout_with`](crate::Cx::layout_with) for a component that builds inside a view
/// without taking [`Cx`](crate::Cx); `None` outside a view build.
pub fn layout_with<T: PartialEq + 'static>(key: &str, f: impl Fn([f32; 4]) -> T + 'static) -> Option<T> {
    let mut value = None;
    let found = read_outside_cx(key, |rect| {
        value = rect.map(&f);
        let same = Same::new(f);
        Read::LayoutWith(key.to_owned(), same, rect)
    });
    found.and(value)
}

/// [`Cx::layout_size`](crate::Cx::layout_size) for a component that builds inside a view
/// without taking [`Cx`](crate::Cx); `None` outside a view build.
pub fn layout_size(key: &str) -> Option<[f32; 2]> {
    layout_with(key, size)
}

/// [`Cx::window_layout`](crate::Cx::window_layout) for a component that builds inside a view
/// without taking [`Cx`](crate::Cx); `None` outside a view build.
pub fn window_layout(key: &str) -> Option<[f32; 4]> {
    layout(&window_key(key))
}

/// [`Cx::window_layout_with`](crate::Cx::window_layout_with) for a component that builds
/// inside a view without taking [`Cx`](crate::Cx); `None` outside a view build.
pub fn window_layout_with<T: PartialEq + 'static>(key: &str, f: impl Fn([f32; 4]) -> T + 'static) -> Option<T> {
    layout_with(&window_key(key), f)
}

/// A layout read of `key` made without a `Cx`, counted toward the view and the memo building,
/// if any: `read` turns the rect the view sees into its record. The rect, or `None` outside a
/// view build.
fn read_outside_cx(key: &str, read: impl FnOnce(Option<[f32; 4]>) -> Read) -> Option<Option<[f32; 4]>> {
    SCOPES.with_borrow_mut(|scopes| {
        let s = scopes.last_mut()?;
        let rect = seen_rect(&s.seen.layout, key);
        let made = read(rect);
        let same = match &made { Read::LayoutWith(_, same, _) => Some(same.clone()), _ => None };
        note_read(&mut s.replayed.layout, key, same);
        if !s.reads.is_empty() {
            s.note(&[made]);
        }
        Some(rect)
    })
}

fn record(read: impl FnOnce() -> Read) {
    SCOPES.with_borrow_mut(|scopes| {
        if let Some(s) = scopes.last_mut()
            && !s.reads.is_empty()
        {
            s.note(&[read()]);
        }
    });
}

/// Ends the view build [`enter`] began; dropped without [`Guard::finish`] (a panicking view),
/// it discards the cache.
pub(crate) struct Guard(bool);

impl Guard {
    /// The cache back, and the `Cx` reads of the memos this build reused. Unused memos stay
    /// until [`MemoCache::keep_since`], since a commit may build its view more than once.
    pub(crate) fn finish(mut self) -> (MemoCache, Replayed) {
        self.0 = false;
        let Scope { cache, replayed, .. } = SCOPES.with_borrow_mut(|s| s.pop()).expect("memo scope");
        (cache, replayed)
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if self.0 {
            SCOPES.with_borrow_mut(|s| s.pop());
        }
    }
}

pub(crate) fn hash_of(v: &impl Hash) -> u64 {
    let mut h = rustc_hash::FxHasher::default();
    v.hash(&mut h);
    h.finish()
}

/// `el` keyed `key`; debug builds warn once per call site when it already had another key.
#[cfg_attr(debug_assertions, track_caller)]
fn keyed(el: El, key: String) -> El {
    #[cfg(debug_assertions)]
    if let Some(existing) = &el.key {
        crate::tooling::perf_warn::memo_key_replaced(&key, existing, std::panic::Location::caller());
    }
    el.key(key)
}

/// The subtree `build` returns, keyed `id`, built again only when `version` (or the window's
/// environment) differs from the last build's. Otherwise `build` isn't called and the
/// previous subtree is reused, with the same node ids and state.
///
/// ```ignore
/// col().growth(GrowthSpec::default()).virtualize(72.0).children(s.messages.iter().map(|m| {
///     memo(format!("msg-{}", m.id), (m.revision, s.theme), || message_view(m))
/// }))
/// ```
///
/// Anything the closure reads that can change belongs in `version`, apart from `Cx::layout`,
/// `Cx::layout_with`, `Cx::layout_size` and `Cx::caret`, which are tracked; see the
/// [module docs](crate::memo) for handlers, live values and what else invalidates. Outside a view (a test calling a view function directly)
/// it just builds.
#[cfg_attr(debug_assertions, track_caller)]
pub fn memo(id: impl Into<String>, version: impl Hash, build: impl FnOnce() -> El) -> El {
    memo_with_hash(id, hash_of(&version), build)
}

/// [`memo`] with a version already hashed to a `u64`, for a caller that keeps its own
/// hashes (a plugin across a C ABI, where `impl Hash` can't cross). The window's environment
/// still joins it, as it does `memo`'s.
#[cfg_attr(debug_assertions, track_caller)]
pub fn memo_with_hash(id: impl Into<String>, version: u64, build: impl FnOnce() -> El) -> El {
    let key = id.into();
    enum Step {
        Reuse(Rc<Built>),
        Build { id: Rc<str>, version: u64, store: bool },
        Plain,
    }
    let step = SCOPES.with_borrow_mut(|scopes| {
        let Some(s) = scopes.last_mut() else { return Step::Plain };
        let version = hash_of(&(version, s.env));
        let name: &str = if let Some(p) = s.path.last() {
            s.cache.id_scratch.clear();
            s.cache.id_scratch.reserve(p.len() + 1 + key.len());
            s.cache.id_scratch.push_str(p);
            s.cache.id_scratch.push('/');
            s.cache.id_scratch.push_str(&key);
            &s.cache.id_scratch
        } else {
            &key
        };
        let pass = s.cache.pass;
        let has_parent = !s.nested.is_empty();
        let mut reused = None;
        let mut store = true;
        if let Some(e) = s.cache.entries.get_mut(name) {
            if e.pass == pass {
                store = false;
            } else if e.version == version && e.reads.iter().all(|r| s.seen.holds(r)) {
                e.pass = pass;
                e.virtual_row = s.virtual_row;
                for read in &e.reads {
                    match read {
                        Read::Caret(..) => s.replayed.caret = true,
                        Read::Layout(k, _) => note_read(&mut s.replayed.layout, k, None),
                        Read::LayoutWith(k, same, _) => note_read(&mut s.replayed.layout, k, Some(same.clone())),
                        Read::Selection(_) => note_read(&mut s.replayed.layout, SELECTION, None),
                    }
                }
                let parent_reads = if s.reads.is_empty() || e.reads.is_empty() { Vec::new() } else { e.reads.clone() };
                s.cache.nested_scratch.extend(e.nested.iter().map(|id| (id.clone(), s.virtual_row)));
                reused = Some((has_parent.then(|| e.id.clone()), e.built.clone(), parent_reads));
            }
        }
        if let Some((parent_id, built, reads)) = reused {
            if !s.cache.nested_scratch.is_empty() {
                let mut scratch = std::mem::take(&mut s.cache.nested_scratch);
                keep_nested_rows(&mut s.cache, &mut scratch, pass);
                s.cache.nested_scratch = scratch;
            }
            if !reads.is_empty() {
                s.note(&reads);
            }
            if let (Some(parent), Some(id)) = (s.nested.last_mut(), parent_id) {
                parent.push(id);
            }
            return Step::Reuse(built);
        }
        let id: Rc<str> = name.into();
        if let Some(parent) = s.nested.last_mut() {
            parent.push(id.clone());
        }
        s.path.push(id.clone());
        s.nested.push(Vec::new());
        s.reads.push(Vec::new());
        Step::Build { id, version, store }
    });
    let (id, version, store) = match step {
        Step::Reuse(built) => return built.placeholder(Some(key)),
        Step::Plain => return keyed(build(), key),
        Step::Build { id, version, store } => (id, version, store),
    };
    let el = keyed(build(), key);
    SCOPES.with_borrow_mut(|scopes| {
        let Some(s) = scopes.last_mut() else { return el };
        s.path.pop();
        let nested = s.nested.pop().unwrap_or_default();
        let reads = s.reads.pop().unwrap_or_default();
        s.note(&reads);
        #[cfg(test)]
        {
            s.cache.builds += 1;
        }
        if !store {
            return el;
        }
        let (built, key) = Built::new(el, s.scale);
        let el = built.placeholder(key.clone());
        let pass = s.cache.pass;
        let virtual_row = s.virtual_row;
        let entry_id = id.clone();
        s.cache.entries.insert(id, Entry { id: entry_id, version, built, key, reads, nested, pass, virtual_row });
        el
    })
}

#[doc(hidden)]
pub fn digest(input: &(impl Hash + ?Sized)) -> (u64, u64) {
    let h1 = hash_of(&input);
    let mut h2 = std::collections::hash_map::DefaultHasher::new();
    input.hash(&mut h2);
    (h1, h2.finish())
}

/// Caches a pure-data subtree by a 128-bit [`digest`] and its occurrence ordinal in the
/// current view pass (or `virtual_rows` row), preserving `el.key` (`None` when unkeyed).
#[doc(hidden)]
pub fn cached((h1, h2): (u64, u64), build: impl FnOnce() -> El) -> El {
    enum Step {
        Reuse(Rc<Built>, Option<String>),
        Build { id: Rc<str>, version: u64 },
        Plain,
    }
    let step = SCOPES.with_borrow_mut(|scopes| {
        let Some(s) = scopes.last_mut() else { return Step::Plain };
        let occ = *s
            .cache
            .pure_counts
            .entry((s.virtual_row, h1, h2))
            .and_modify(|c| *c += 1)
            .or_insert(0);
        let version = hash_of(&(h1, h2, s.env));
        {
            use std::fmt::Write;
            s.cache.id_scratch.clear();
            if let Some(p) = s.path.last() {
                s.cache.id_scratch.push_str(p);
                s.cache.id_scratch.push('/');
            }
            let _ = write!(s.cache.id_scratch, "\0{}:{h1:016x}:{h2:016x}:{occ}", s.virtual_row);
        }
        let pass = s.cache.pass;
        let has_parent = !s.nested.is_empty();
        let mut reused = None;
        if let Some(e) = s.cache.entries.get_mut(s.cache.id_scratch.as_str())
            && e.pass != pass
            && e.version == version
            && e.reads.iter().all(|r| s.seen.holds(r))
        {
            e.pass = pass;
            e.virtual_row = s.virtual_row;
            for read in &e.reads {
                match read {
                    Read::Caret(..) => s.replayed.caret = true,
                    Read::Layout(k, _) => note_read(&mut s.replayed.layout, k, None),
                    Read::LayoutWith(k, same, _) => note_read(&mut s.replayed.layout, k, Some(same.clone())),
                    Read::Selection(_) => note_read(&mut s.replayed.layout, SELECTION, None),
                }
            }
            let parent_reads = if s.reads.is_empty() || e.reads.is_empty() { Vec::new() } else { e.reads.clone() };
            s.cache.nested_scratch.extend(e.nested.iter().map(|id| (id.clone(), s.virtual_row)));
            reused = Some((has_parent.then(|| e.id.clone()), e.built.clone(), e.key.clone(), parent_reads));
        }
        if let Some((parent_id, built, key, reads)) = reused {
            if !s.cache.nested_scratch.is_empty() {
                let mut scratch = std::mem::take(&mut s.cache.nested_scratch);
                keep_nested_rows(&mut s.cache, &mut scratch, pass);
                s.cache.nested_scratch = scratch;
            }
            if !reads.is_empty() {
                s.note(&reads);
            }
            if let (Some(parent), Some(id)) = (s.nested.last_mut(), parent_id) {
                parent.push(id);
            }
            return Step::Reuse(built, key);
        }
        let id: Rc<str> = s.cache.id_scratch.as_str().into();
        if let Some(parent) = s.nested.last_mut() {
            parent.push(id.clone());
        }
        s.path.push(id.clone());
        s.nested.push(Vec::new());
        s.reads.push(Vec::new());
        Step::Build { id, version }
    });
    let (id, version) = match step {
        Step::Reuse(built, key) => return built.placeholder(key),
        Step::Plain => return build(),
        Step::Build { id, version } => (id, version),
    };
    let el = build();
    SCOPES.with_borrow_mut(|scopes| {
        let Some(s) = scopes.last_mut() else { return el };
        s.path.pop();
        let nested = s.nested.pop().unwrap_or_default();
        let reads = s.reads.pop().unwrap_or_default();
        s.note(&reads);
        #[cfg(test)]
        {
            s.cache.builds += 1;
        }
        let (built, key) = Built::new(el, s.scale);
        let el = built.placeholder(key.clone());
        let pass = s.cache.pass;
        let virtual_row = s.virtual_row;
        let entry_id = id.clone();
        s.cache.entries.insert(id, Entry { id: entry_id, version, built, key, reads, nested, pass, virtual_row });
        el
    })
}

/// Marks the memos built inside a reused one as used, so they survive to its next build.
fn keep_nested_rows(cache: &mut MemoCache, ids: &mut Vec<(Rc<str>, u64)>, pass: u64) {
    while let Some((id, virtual_row)) = ids.pop() {
        if let Some(e) = cache.entries.get_mut(&id)
            && e.pass != pass
        {
            e.pass = pass;
            e.virtual_row = virtual_row;
            ids.extend(e.nested.iter().map(|nested| (nested.clone(), virtual_row)));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::{Cx, Env, GrowthSpec, Runtime, ScrollSpec, col, el, text};

    thread_local! {
        static BUILDS: Cell<usize> = const { Cell::new(0) };
    }

    fn counted(e: El) -> El {
        BUILDS.set(BUILDS.get() + 1);
        e
    }

    struct S {
        version: u32,
        ticks: u32,
        presses: u32,
    }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        col().size(200.0, 200.0).children([
            memo("card", s.version, || {
                counted(col().child(el().key("button").size(40.0, 40.0).fill(crate::Color::WHITE).on_press(|s: &mut S| s.presses += 1)))
            }),
            text(format!("tick {}", s.ticks)).key("tick"),
        ])
    }

    fn commit<S>(rt: &mut Runtime<S>) {
        rt.invalidate();
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn an_unchanged_version_reuses_the_subtree_and_a_new_version_or_theme_builds() {
        BUILDS.set(0);
        let mut rt = Runtime::new(S { version: 0, ticks: 0, presses: 0 }, view, (200.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        assert_eq!(BUILDS.get(), 1);
        let id = |rt: &Runtime<S>| rt.nodes.iter().find(|n| n.key.as_deref() == Some("button")).unwrap().id;
        let first = id(&rt);
        for _ in 0..3 {
            rt.state.ticks += 1;
            commit(&mut rt);
        }
        assert_eq!(BUILDS.get(), 1, "other state changing doesn't build the memo");
        assert_eq!(id(&rt), first, "the reused subtree keeps its node ids");
        let (x, y) = rt.find("button").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.presses, 1, "handlers in a reused subtree run");
        assert_eq!(BUILDS.get(), 1);
        rt.state.version += 1;
        commit(&mut rt);
        assert_eq!(BUILDS.get(), 2, "a new version builds");
        rt.set_system_dark(false);
        rt.frame(1.0 / 60.0);
        assert_eq!(BUILDS.get(), 3, "a theme change builds");
        rt.set_viewport((200.0, 200.0), 2.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(BUILDS.get(), 4, "a scale change builds");
        rt.memo.clear();
        commit(&mut rt);
        assert_eq!(BUILDS.get(), 5, "a cleared cache (hot patch, tunables) builds");
        assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("button")), "tools see reused nodes");
    }

    #[test]
    fn a_view_panicking_inside_a_memo_leaves_no_scope_for_the_next_build() {
        let panicked = std::panic::catch_unwind(|| {
            let _scope = enter(MemoCache::default(), Env::new(true, 1.0), crate::SystemAccent::DEFAULT, false, true, [0; 4], None, LayoutRects::default());
            memo("outer", 0, || memo("inner", 0, || panic!("view panicked")))
        });
        assert!(panicked.is_err());
        assert!(SCOPES.with_borrow(|s| s.is_empty()), "unwinding pops the build's scope");
        BUILDS.set(0);
        let mut rt = Runtime::new(S { version: 0, ticks: 0, presses: 0 }, view, (200.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        commit(&mut rt);
        assert_eq!(BUILDS.get(), 1, "the next window's memos cache as usual");
    }

    #[test]
    fn a_memo_inside_a_reused_memo_survives_to_the_outer_ones_next_build() {
        BUILDS.set(0);
        fn view(s: &(u32, u32), _: &mut Cx<(u32, u32)>) -> El {
            col().child(memo("outer", s.0, || {
                counted(col().children([memo("inner", s.1, || counted(text("inner").key("t"))), text(format!("outer {}", s.0))]))
            }))
        }
        let mut rt = Runtime::new((0, 0), view, (200.0, 200.0), Env::default());
        rt.frame(0.0);
        assert_eq!(BUILDS.get(), 2);
        commit(&mut rt);
        commit(&mut rt);
        assert_eq!(BUILDS.get(), 2);
        rt.state.0 += 1;
        commit(&mut rt);
        assert_eq!(BUILDS.get(), 3, "the outer builds and finds the inner unchanged");
        rt.state.1 += 1;
        commit(&mut rt);
        assert_eq!(BUILDS.get(), 3, "the outer's version gates its closure, inner included");
    }

    #[test]
    fn an_id_used_twice_in_one_build_builds_both() {
        BUILDS.set(0);
        fn view(_: &(), _: &mut Cx<()>) -> El {
            col().children([
                col().key("a").child(memo("same", 0, || counted(text("a").key("t")))),
                col().key("b").child(memo("same", 0, || counted(text("b").key("t")))),
            ])
        }
        let mut rt = Runtime::new((), view, (200.0, 200.0), Env::default());
        rt.frame(0.0);
        commit(&mut rt);
        let texts: Vec<_> = rt.elements().into_iter().filter_map(|e| e.text).collect();
        assert_eq!(texts, ["a", "b"]);
        assert_eq!(BUILDS.get(), 3, "the first is reused, the duplicate builds each time");
    }

    struct Rows {
        n: usize,
        tick: u64,
        memo: bool,
    }

    fn row(i: usize) -> El {
        col().w_full().h(72.0).padding(8.0).gap(4.0).children([
            text(format!("Message {i}")).key("title"),
            text(format!("Body of message {i}, a line of text that is long enough to wrap twice")).wrap().w_full().key("body"),
            text(format!("{i} min ago")).key("time"),
        ])
    }

    fn rows(s: &Rows, _: &mut Cx<Rows>) -> El {
        let stack = col().key("rows").w_full().growth(GrowthSpec::default()).virtualize(72.0).children((0..s.n).map(|i| {
            let key = format!("r{i}");
            if s.memo { memo(key, 0, || counted(row(i))) } else { counted(row(i)).key(key) }
        }));
        col().w_full().h_full().children([
            text(format!("tick {}", s.tick)).key("counter"),
            col().key("scroll").w_full().grow(1.0).scroll(ScrollSpec { stick_bottom: true }).child(stack),
        ])
    }

    #[test]
    fn virtual_rows_scroll_out_and_back_in_without_building() {
        BUILDS.set(0);
        let mut rt = Runtime::new(Rows { n: 500, tick: 0, memo: true }, rows, (400.0, 600.0), Env::default());
        rt.frame(0.0);
        assert_eq!(BUILDS.get(), 500, "a miss builds in the view, mounted or not");
        assert!(rt.nodes.len() < 200, "offscreen rows' children never reach the element tree: {}", rt.nodes.len());
        for _ in 0..5 {
            rt.state.tick += 1;
            commit(&mut rt);
        }
        let scroll = |rt: &mut Runtime<Rows>, dy: f32| {
            for _ in 0..40 {
                rt.wheel(200.0, 300.0, dy);
                rt.state.tick += 1;
                commit(rt);
            }
        };
        scroll(&mut rt, 400.0);
        assert!(rt.find("r499/title").is_none(), "scrolled up, the last row unmounts");
        scroll(&mut rt, -400.0);
        assert!(rt.find("r499/title").is_some(), "scrolled back down, it mounts again");
        assert_eq!(BUILDS.get(), 500, "ticks and scrolling build no row");
    }

    #[test]
    fn a_focused_field_in_an_offscreen_memo_row_keeps_focus_and_typing() {
        use crate::{TextEdit, TextStyle, text_input};
        fn view(s: &Vec<TextEdit>, _: &mut Cx<Vec<TextEdit>>) -> El {
            col().key("viewport").size(200.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).clip_soft(0.0)
                .child(col().key("rows").w_full().growth(GrowthSpec::default()).virtualize(40.0)
                    .children(s.iter().enumerate().map(|(i, edit)| {
                        memo(format!("row-{i}"), edit, || {
                            col().size(200.0, 40.0).child(text_input(edit, TextStyle::new(14.0, 400.0)).key("field")
                                .size(200.0, 40.0).on_edit(move |s: &mut Vec<TextEdit>| &mut s[i]))
                        })
                    })))
        }
        let mut state: Vec<_> = (0..100).map(|_| TextEdit::new("")).collect();
        state[0].focused = true;
        let mut rt = Runtime::new(state, view, (200.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.wheel(100.0, 100.0, -2000.0);
        rt.frame(0.0);
        assert!(rt.find("row-0/field").is_none(), "the focused row scrolled out");
        rt.key(crate::KeyEvent { key: crate::Key::Text("offscreen".into()), mods: Default::default() });
        rt.frame(0.0);
        assert_eq!(rt.state[0].text, "offscreen");
    }

    #[test]
    fn a_memoized_field_redraws_its_caret_and_composition() {
        use crate::{Ime, Key, KeyEvent, TextEdit, TextStyle, text_input};
        fn view(s: &TextEdit, _: &mut Cx<TextEdit>) -> El {
            col().size(200.0, 100.0).child(memo("field", s, || {
                text_input(s, TextStyle::new(14.0, 400.0)).size(200.0, 40.0).on_edit(|s: &mut TextEdit| s)
            }))
        }
        let mut edit = TextEdit::new("ab");
        edit.focused = true;
        let mut rt = Runtime::new(edit, view, (200.0, 100.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.key(KeyEvent { key: Key::Left, mods: Default::default() });
        rt.frame(1.0 / 60.0);
        rt.ime(Ime::Preedit("X".into(), None));
        rt.frame(1.0 / 60.0);
        let shown = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("field")).and_then(|e| e.text);
        assert_eq!(shown.as_deref(), Some("aXb"), "the composition draws at the moved caret");
        let mut edit = TextEdit::new("ab");
        let caret = hash_of(&edit);
        edit.anchor = 0;
        assert_ne!(hash_of(&edit), caret, "a selection changes the version");
    }

    struct Probe {
        width: f32,
        ticks: u32,
    }

    fn probe(s: &Probe, cx: &mut Cx<Probe>) -> El {
        col().size(300.0, 300.0).children([
            el().key("probe").w(s.width).h(10.0),
            memo("label", 0, || {
                let width = cx.layout("probe").map_or(-1.0, |r| r[2]);
                counted(text(format!("width {width}")).key("label"))
            }),
            text(format!("tick {}", s.ticks)).key("tick"),
        ])
    }

    fn label<S>(rt: &Runtime<S>) -> Option<String> {
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some("label")).and_then(|e| e.text)
    }

    #[test]
    fn a_layout_read_inside_a_memo_rebuilds_it_when_the_rect_changes() {
        BUILDS.set(0);
        let mut rt = Runtime::new(Probe { width: 100.0, ticks: 0 }, probe, (300.0, 300.0), Env::default());
        rt.frame(0.0);
        assert_eq!(label(&rt).as_deref(), Some("width 100"), "the first commit settles the read");
        let builds = BUILDS.get();
        for _ in 0..3 {
            rt.state.ticks += 1;
            commit(&mut rt);
        }
        assert_eq!(BUILDS.get(), builds, "an unmoved read reuses");
        rt.state.width = 150.0;
        commit(&mut rt);
        assert_eq!(label(&rt).as_deref(), Some("width 150"), "a moved read builds, though the version didn't change");
    }

    struct Moving {
        top: f32,
        width: f32,
        ticks: u32,
    }

    fn moving(s: &Moving, cx: &mut Cx<Moving>) -> El {
        let shown = |key: &str, v: Option<[f32; 2]>| counted(text(format!("{v:?}")).key(key));
        col().size(300.0, 300.0).children([
            el().key("gap").w(10.0).h(s.top),
            el().key("probe").w(s.width).h(10.0),
            memo("cx", 0, || shown("cx", cx.layout_size("probe"))),
            memo("free", 0, || shown("free", layout_size("probe"))),
            memo("rect", 0, || shown("rect", cx.layout("probe").map(|r| [r[2], r[3]]))),
            text(format!("tick {}", s.ticks)).key("tick"),
        ])
    }

    #[test]
    fn a_size_read_inside_a_memo_rebuilds_it_only_when_the_size_changes() {
        BUILDS.set(0);
        let mut rt = Runtime::new(Moving { top: 0.0, width: 100.0, ticks: 0 }, moving, (300.0, 300.0), Env::default());
        rt.frame(0.0);
        let shown = |rt: &Runtime<Moving>, key| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).and_then(|e| e.text);
        assert_eq!(shown(&rt, "cx").as_deref(), Some("Some([100.0, 10.0])"), "settled on the first commit");
        assert_eq!(shown(&rt, "free"), shown(&rt, "cx"));
        let builds = BUILDS.get();
        rt.state.top = 40.0;
        rt.state.ticks += 1;
        commit(&mut rt);
        assert_eq!(BUILDS.get(), builds + 1, "a move rebuilds only the whole-rect read");
        rt.state.width = 150.0;
        commit(&mut rt);
        assert_eq!(BUILDS.get(), builds + 4, "a resize rebuilds all three");
        assert_eq!(shown(&rt, "cx").as_deref(), Some("Some([150.0, 10.0])"));
        assert_eq!(shown(&rt, "free"), shown(&rt, "cx"));
    }

    fn scrolled(s: &Probe, cx: &mut Cx<Probe>) -> El {
        let rows = (0..20).fold(col().w_full(), |c, i| c.child(el().key(format!("r{i}")).w_full().h(30.0)));
        col().size(300.0, 300.0).children([
            col().key("scroll").w_full().h(200.0).scroll(ScrollSpec { stick_bottom: false }).child(rows),
            memo("label", 0, || counted(text(format!("{:?}", cx.window_layout("r3"))).key("label"))),
            text(format!("tick {}", s.ticks)).key("tick"),
        ])
    }

    #[test]
    fn a_whole_rect_read_inside_a_memo_holds_through_a_move_below_the_threshold() {
        BUILDS.set(0);
        let mut rt = Runtime::new(Probe { width: 0.0, ticks: 0 }, scrolled, (300.0, 300.0), Env::default());
        rt.frame(0.0);
        let builds = BUILDS.get();
        rt.wheel(100.0, 100.0, -0.3);
        rt.frame(1.0 / 60.0);
        rt.state.ticks += 1;
        commit(&mut rt);
        assert_eq!(BUILDS.get(), builds, "0.3 pt is within the half point a whole-rect read ignores");
        rt.wheel(100.0, 100.0, -0.3);
        rt.frame(1.0 / 60.0);
        rt.state.ticks += 1;
        commit(&mut rt);
        assert_eq!(BUILDS.get(), builds + 1, "0.6 pt from what it read builds");
    }

    #[test]
    fn a_caret_read_inside_a_reused_memo_keeps_following_the_caret() {
        use crate::{Key, KeyEvent, TextEdit, TextStyle, text_input};
        fn view(s: &TextEdit, cx: &mut Cx<TextEdit>) -> El {
            col().size(300.0, 300.0).children([
                text_input(s, TextStyle::new(14.0, 400.0)).key("field").size(300.0, 40.0).on_edit(|s: &mut TextEdit| s),
                memo("label", 0, || {
                    let x = cx.caret("field").map_or(-1.0, |r| r[0]);
                    text(format!("caret {x}")).key("label")
                }),
            ])
        }
        let mut edit = TextEdit::new("");
        edit.focused = true;
        let mut rt = Runtime::new(edit, view, (300.0, 300.0), Env::default());
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        let at_start = label(&rt);
        assert_ne!(at_start.as_deref(), Some("caret -1"), "{at_start:?}");
        for _ in 0..3 {
            rt.key(KeyEvent { key: Key::Text("w".into()), mods: Default::default() });
            rt.frame(1.0 / 60.0);
            rt.frame(1.0 / 60.0);
        }
        assert_ne!(label(&rt), at_start, "typing moved the caret the label shows");
    }

    #[test]
    fn a_memo_names_its_callers_line_for_key_warnings() {
        let line = line!() + 1;
        let built = memo("id", 0, el);
        assert!(built.key_location().starts_with(&format!("{}:{line}:", file!())), "{}", built.key_location());
    }

    #[test]
    fn cached_unkeyed_siblings_keep_index_paths_and_leave_no_key_warnings() {
        fn view(tick: &u32, _: &mut Cx<u32>) -> El {
            let _ = *tick;
            col().w_full().children([
                cached(digest("same"), || counted(text("one"))),
                cached(digest("same"), || counted(text("one"))),
                cached(digest("other"), || counted(text("two"))).key("named"),
            ])
        }
        BUILDS.set(0);
        let mut rt = Runtime::new(0u32, view, (200.0, 200.0), Env::new(true, 1.0));
        rt.key_warnings = Some(Vec::new());
        rt.frame(0.0);
        assert_eq!(BUILDS.get(), 3);
        assert!(rt.key_warnings.as_ref().unwrap().is_empty(), "{:?}", rt.key_warnings);
        let paths: Vec<_> = rt.elements().into_iter().map(|e| e.path).collect();
        assert_eq!(paths, vec!["#0", "#0/#0", "#0/#1", "#0/named"]);

        rt.update(|t, _| *t += 1);
        rt.frame(1.0 / 120.0);
        assert_eq!(BUILDS.get(), 3, "reused all three cached subtrees");
        assert!(rt.key_warnings.as_ref().unwrap().is_empty());
    }

    /// `cargo test -p rune --profile perf --lib memo_build_time -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn memo_build_time() {
        for memo in [false, true] {
            for n in [100, 500, 2000] {
                let mut rt = Runtime::new(Rows { n, tick: 0, memo }, rows, (400.0, 600.0), Env::new(true, 2.0));
                rt.frame(0.0);
                for _ in 0..30 {
                    rt.state.tick += 1;
                    commit(&mut rt);
                }
                let frames = 300;
                let start = std::time::Instant::now();
                for _ in 0..frames {
                    rt.state.tick += 1;
                    commit(&mut rt);
                }
                let per = start.elapsed().as_secs_f64() * 1e6 / f64::from(frames);
                println!("memo {memo:5} rows {n:5}: {per:8.1} µs per frame");
            }
        }
    }
}
