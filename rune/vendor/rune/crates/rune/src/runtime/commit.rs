//! View-to-scene commit orchestration (`Runtime::commit`, `Measurement`, `Measured`).
use super::*;

pub(super) mod remount;

/// Measurement capabilities: text/layout caches and virtual geometry only. In particular this
/// type has no Scene, growth tracks, reveal tracks, draw instances, or mutable app state.
pub(super) struct Measurement<'a, S: 'static> {
    pub(super) state: &'a S,
    pub(super) view: fn(&S, &mut Cx<S>) -> El,
    pub(super) playground: Option<&'a crate::playground::Live<S>>,
    pub(super) env: Env,
    pub(super) viewport: (f32, f32),
    pub(super) fit_height: bool,
    pub(super) system_reduced_motion: bool,
    pub(super) titlebar_inset: f32,
    pub(super) chrome: crate::ChromeInsets,
    /// Caption buttons drawn over the view's root.
    pub(super) caption: Option<crate::titlebar::Caption>,
    pub(super) system_accent: crate::SystemAccent,
    pub(super) system_scroll_bars: crate::platform::scroll_bars::SystemScrollBars,
    pub(super) reduced_motion: bool,
    pub(super) time_scale: f32,
    /// `Cx::foreground` and `Cx::focus_ring`: the last view's, then this one's once it returns.
    pub(super) defaults: cx::ViewDefaults,
    /// `Cx::set_titlebar` and `Cx::titlebar_height`, as the view sets them.
    pub(super) titlebar: crate::titlebar::TitlebarSet,
    pub(super) commits: u64,
    /// The last commit's nodes. A pass that commits drops the text layouts of the ones it keeps.
    pub(super) nodes: &'a mut [Node],
    pub(super) committing: bool,
    pub(super) spare_nodes: &'a mut Vec<Node>,
    pub(super) text: &'a mut TextSystem,
    pub(super) layout: &'a mut LayoutEngine,
    pub(super) node_index: &'a mut HashMap<NodeId, usize>,
    pub(super) path_ids: &'a mut HashMap<String, NodeId>,
    pub(super) memo: &'a mut crate::memo::MemoCache,
    pub(super) next_id: &'a mut u64,
    pub(super) scrolls: &'a mut HashMap<NodeId, ScrollState>,
    pub(super) virtuals: &'a mut HashMap<String, crate::el::virtual_list::VirtualList>,
    pub(super) virtual_culled: &'a mut HashSet<String>,
    pub(super) virtual_inputs: &'a mut HashMap<String, Box<InputSpec>>,
    pub(super) stats: &'a mut FrameStats,
    pub(super) key_warnings: &'a mut Option<Vec<String>>,
    /// The focused element's path, so an index-built row holding it is still built.
    pub(super) focused: Option<&'a str>,
    /// True when `self.state` was not mutated since the previous commit.
    pub(super) state_unchanged: bool,
    /// Whether this pass measures again because estimates left part of a viewport blank.
    pub(super) retry: bool,
    /// This commit is a live-only check's: virtual lists keep the rows they mount now.
    pub(super) hold_windows: bool,
    pub(super) caret: Option<(String, [f32; 4])>,
    pub(super) caret_read: bool,
    pub(super) layout_seen: layout_read::LayoutRects,
    pub(super) layout_reads: layout_read::LayoutReads,
}

/// Owned result of completed measurement. Scene mutation consumes this once; retries never
/// return it. Keeping this separate from El makes unmeasured views invalid commit inputs.
pub(super) struct Measured {
    pub(super) layout: LayoutEngine,
    pub(super) nodes: Vec<Node>,
    pub(super) reduced_motion: bool,
    pub(super) time_scale: f32,
    pub(super) defaults: cx::ViewDefaults,
    pub(super) titlebar: crate::titlebar::TitlebarSet,
    pub(super) caret_read: bool,
    pub(super) layout_reads: layout_read::LayoutReads,
    pub(super) memo_flattened: Option<Vec<memo_reuse::PendingFlat>>,
    pub(super) memo_reused: Vec<memo_reuse::ReusedSpan>,
    pub(super) taken_paths: Vec<(u32, u32)>,
    /// The selection as the measure began, before reused `memo` subtrees left the last
    /// commit's nodes.
    pub(super) selection: Vec<layout_read::SelectedRange>,
}

impl<S: 'static> Runtime<S> {
    pub(super) fn commit(&mut self, tx: &Tx) {
        let _commit_time = crate::frame::profile::Scope::new(crate::frame::profile::Phase::Commit);
        let _first_view = crate::tooling::perf_warn::launch::FirstView::start(self.commits == 0);
        let budget = crate::tooling::perf_warn::budget::Commit::start();
        self.apply_playground();
        let _history = crate::memo::snapshot::enter();
        COMMITS.set(COMMITS.get() + 1);
        self.last_commit_clock = self.clock;
        self.memo_since = self.memo.next_pass();
        for r in &self.scroll_reveals {
            if r.at_end && let Some(&id) = self.path_ids.get(&r.viewport)
                && let Some(scroll) = self.scrolls.get_mut(&id)
            {
                scroll.pin();
            }
        }
        self.note_drawn_rows();
        let mut measured = self.measure_commit(tx);
        self.focus_before_commit = self.focus.owned();
        // Settle layout reads first so a field that mounts once a rect is known is in `nodes`.
        measured = self.settle_layout_reads(measured, tx);
        if self.sync_requested_focus(&mut measured.nodes) {
            self.discard_measured(measured);
            measured = self.measure(tx);
            measured = self.settle_layout_reads(measured, tx);
        }
        if measured.caret_read && self.caret_moved(Some(&measured.nodes)) {
            self.discard_measured(measured);
            measured = self.measure(tx);
            measured = self.settle_layout_reads(measured, tx);
        }
        self.apply(measured, tx);
        self.scene.keep_flights(false);
        self.memo.keep_since(self.memo_since);
        self.track_focused_rows();
        self.prune_text_selection();
        budget.end(self.watch.cause);
    }

    /// Restores any nodes moved out of `self.nodes` and recycles `measured`'s buffers before a
    /// commit pass re-measures.
    pub(super) fn discard_measured(&mut self, mut measured: Measured) {
        memo_reuse::restore_moved_nodes(&mut self.nodes, &mut measured.nodes, &measured.memo_reused, &measured.taken_paths);
        measured.nodes.clear();
        self.spare_nodes = measured.nodes;
        if let Some(mut pending) = measured.memo_flattened {
            pending.clear();
            self.memo.pending_scratch = pending;
        }
        measured.memo_reused.clear();
        self.memo.reused_scratch = measured.memo_reused;
        measured.taken_paths.clear();
        self.memo.taken_paths_scratch = measured.taken_paths;
        self.layout = measured.layout;
    }
}

/// A horizontal scroll viewport's content without a width of its own, or one filling the
/// viewport (`w_full`), takes its natural one, unwrapped, and at least the viewport's (CSS
/// `width: max-content; min-width: 100%`). Any other width stays as set, narrower than the
/// viewport too. The content never shrinks. The viewport lays it out as a row, where taffy
/// measures a max-content main size unwrapped (a column measures its height wrapped at the
/// viewport's width). With vertical scrolling too it keeps its natural height instead of
/// stretching to the viewport's.
pub(super) fn unwrapped_width(style: &mut CompactStyle, vertical: bool) {
    let w = style.size().width;
    if w.is_auto() || w == Dimension::percent(1.0) {
        style.set_width(Dimension::max_content());
        if style.min_size().width.is_auto() { style.set_min_width(percent(1.0)); }
    }
    style.set_flex_shrink(0.0);
    if vertical && style.align_self().is_none() { style.set_align_self(Some(AlignSelf::FLEX_START)); }
}
