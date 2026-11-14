//! Paired benchmark and differential harness for `rune-taffy` (`LayoutStore` vs `taffy::TaffyTree`).
//!
//! Run isolated A/B + runtime benchmark:
//!   cargo build --profile perf -p rune --example taffy_adapter_bench --features frame-profile
//!   target/perf/examples/taffy_adapter_bench [--samples 25]

use std::borrow::Cow;
use std::hint::black_box;
use std::time::Instant;

use rune::text::TextSystem;
use rune::{
    Align, Color, Cx, El, Env, GrowthSpec, Key, KeyEvent, Runtime, ScrollSpec, Side, TextStyle, Tx, col, el, row, text,
};
use rune_taffy::prelude::*;
use rune_taffy::{BaselineTaffyTree, LayoutStore, baseline_storage_stats};

#[global_allocator]
static ALLOC: procwatch::alloc::Counting = procwatch::alloc::Counting::system();

const SIZE: (f32, f32) = (760.0, 480.0);
const ROW: f32 = 44.0;
const COLUMNS: usize = 10;

fn live_heap_bytes() -> i64 {
    procwatch::alloc::heap().map_or(0, |h| h.live)
}

fn stats(mut v: Vec<f64>) -> (f64, f64) {
    v.sort_by(f64::total_cmp);
    let p50 = v[v.len() / 2];
    let p95 = v[((v.len() - 1) as f64 * 0.95).round() as usize];
    (p50, p95)
}

fn build_baseline_list(rows: usize) -> (BaselineTaffyTree<usize>, NodeId, Vec<[NodeId; 5]>) {
    let mut tree = BaselineTaffyTree::new();
    tree.disable_rounding();
    let icon_s = Style { size: Size { width: length(22.0), height: length(22.0) }, ..Style::DEFAULT };
    let title_s = Style { flex_shrink: 1.0, size: Size { width: length(140.0), height: length(18.0) }, ..Style::DEFAULT };
    let sub_s = Style { flex_grow: 1.0, flex_shrink: 1.0, size: Size { width: length(220.0), height: length(18.0) }, ..Style::DEFAULT };
    let copy_s = Style {
        display: Display::Flex,
        flex_direction: FlexDirection::Row,
        flex_grow: 1.0,
        flex_shrink: 1.0,
        gap: Size { width: length(8.0), height: length(0.0) },
        align_items: AlignItems::CENTER,
        ..Style::DEFAULT
    };
    let row_s = Style {
        display: Display::Flex,
        flex_direction: FlexDirection::Row,
        size: Size { width: percent(1.0), height: length(ROW) },
        padding: Rect { left: length(10.0), right: length(10.0), top: length(0.0), bottom: length(0.0) },
        gap: Size { width: length(10.0), height: length(0.0) },
        align_items: AlignItems::CENTER,
        ..Style::DEFAULT
    };
    let root_s = Style {
        display: Display::Flex,
        flex_direction: FlexDirection::Column,
        size: Size { width: length(SIZE.0), height: length(SIZE.1) },
        padding: Rect { left: length(8.0), right: length(8.0), top: length(6.0), bottom: length(6.0) },
        ..Style::DEFAULT
    };

    let mut row_ids = Vec::with_capacity(rows);
    let mut handles = Vec::with_capacity(rows);
    for i in 0..rows {
        let icon = tree.new_leaf(icon_s.clone()).unwrap();
        let title = tree.new_leaf_with_context(title_s.clone(), i * 2).unwrap();
        let sub = tree.new_leaf_with_context(sub_s.clone(), i * 2 + 1).unwrap();
        let copy = tree.new_with_children(copy_s.clone(), &[title, sub]).unwrap();
        let r = tree.new_with_children(row_s.clone(), &[icon, copy]).unwrap();
        row_ids.push(r);
        handles.push([r, icon, copy, title, sub]);
    }
    let root = tree.new_with_children(root_s, &row_ids).unwrap();
    (tree, root, handles)
}

fn sync_store_list(store: &mut LayoutStore, rows: usize, key_base: u64, dirty_row: Option<usize>) -> SlotId {
    let mut icon_s = CompactStyle::default();
    icon_s.set_width(length(22.0));
    icon_s.set_height(length(22.0));

    let mut title_s = CompactStyle::default();
    title_s.set_flex_shrink(1.0);
    title_s.set_width(length(140.0));
    title_s.set_height(length(18.0));

    let mut sub_s = CompactStyle::default();
    sub_s.set_grow(1.0);
    sub_s.set_width(length(220.0));
    sub_s.set_height(length(18.0));

    let mut copy_s = CompactStyle::rune_base(FlexDirection::Row);
    copy_s.set_grow(1.0);
    copy_s.set_gap(8.0, 0.0);
    copy_s.set_align_items(AlignItems::CENTER);

    let mut row_s = CompactStyle::rune_base(FlexDirection::Row);
    row_s.set_width(percent(1.0));
    row_s.set_height(length(ROW));
    row_s.set_pad_left(10.0);
    row_s.set_pad_right(10.0);
    row_s.set_gap(10.0, 0.0);
    row_s.set_align_items(AlignItems::CENTER);

    let mut root_s = CompactStyle::rune_base(FlexDirection::Column);
    root_s.set_width(length(SIZE.0));
    root_s.set_height(length(SIZE.1));
    root_s.set_pad_left(8.0);
    root_s.set_pad_right(8.0);
    root_s.set_pad_top(6.0);
    root_s.set_pad_bottom(6.0);

    store.begin_commit();
    let root_mark = store.begin_children();
    let mut root_dirty = false;
    let mut me = 1u32;
    for i in 0..rows {
        let kb = key_base + (i as u64) * 8;
        let m_icon = store.begin_children();
        let (icon, d_icon) = store.sync_node(kb + 1, me, icon_s.clone(), None, false, m_icon, false);
        me += 1;

        let m_title = store.begin_children();
        let m_hash = if dirty_row == Some(i) { 999_999 } else { (i * 2 + 1) as u64 };
        let (title, d_title) = store.sync_node(kb + 2, me, title_s.clone(), Some(m_hash), false, m_title, false);
        me += 1;

        let m_sub = store.begin_children();
        let (sub, d_sub) = store.sync_node(kb + 3, me, sub_s.clone(), Some((i * 2 + 2) as u64), false, m_sub, false);
        me += 1;

        let m_copy = store.begin_children();
        store.push_child(title);
        store.push_child(sub);
        let (copy, d_copy) = store.sync_node(kb + 4, me, copy_s.clone(), None, false, m_copy, d_title || d_sub);
        me += 1;

        let m_row = store.begin_children();
        store.push_child(icon);
        store.push_child(copy);
        let (r, d_row) = store.sync_node(kb + 5, me, row_s.clone(), None, false, m_row, d_icon || d_copy);
        me += 1;

        store.push_child(r);
        root_dirty |= d_row;
    }
    let (mut root, _) = store.sync_node(1, 0, root_s, None, false, root_mark, root_dirty);
    store.end_commit(&mut root, |_, _| {});
    root
}

fn solve_store(store: &mut LayoutStore, root: SlotId, room: Size<AvailableSpace>) {
    store.compute_layout(root, room, |inputs, _ctx, style| {
        taffy::compute_leaf_layout(inputs, style, |_, _| 0.0, |known, _| Size {
            width: known.width.unwrap_or(0.0),
            height: known.height.unwrap_or(0.0),
        })
    });
}

fn run_isolated_paired_bench(samples: usize) {
    let room = Size {
        width: AvailableSpace::Definite(SIZE.0),
        height: AvailableSpace::Definite(SIZE.1),
    };

    let mut base_mem_500 = Vec::with_capacity(samples);
    let mut comp_mem_500 = Vec::with_capacity(samples);
    let mut base_mem_swap = Vec::with_capacity(samples);
    let mut comp_mem_swap = Vec::with_capacity(samples);
    let mut base_allocs = Vec::with_capacity(samples);
    let mut comp_allocs = Vec::with_capacity(samples);
    let mut base_cold_ms = Vec::with_capacity(samples);
    let mut comp_cold_ms = Vec::with_capacity(samples);
    let mut base_warm_ms = Vec::with_capacity(samples);
    let mut comp_warm_ms = Vec::with_capacity(samples);
    let mut base_dirty_ms = Vec::with_capacity(samples);
    let mut comp_dirty_ms = Vec::with_capacity(samples);

    for iter in 0..=samples {
        for which in 0..2 {
            let run_compact = (iter + which) % 2 == 1;
            if !run_compact {
                let h0 = live_heap_bytes();
                let a0 = procwatch::alloc::thread_allocations();
                let t0 = Instant::now();
                let (mut tree, root, handles) = build_baseline_list(500);
                tree.compute_layout(root, room).unwrap();
                let cold = t0.elapsed().as_secs_f64() * 1000.0;
                let allocs = (procwatch::alloc::thread_allocations() - a0) as f64;
                let mem_500 = (live_heap_bytes() - h0).max(0) as f64;

                let tw = Instant::now();
                tree.compute_layout(root, room).unwrap();
                let warm = tw.elapsed().as_secs_f64() * 1000.0;

                let td = Instant::now();
                tree.mark_dirty(handles[250][3]).unwrap();
                tree.compute_layout(root, room).unwrap();
                let dirty = td.elapsed().as_secs_f64() * 1000.0;

                // Simulate Rune's flatten-then-retain swap from 500 rows down to 12 mounted rows:
                // allocate 12 new rows first, attach to root, then remove the 500 old rows.
                let mut new_rows = Vec::with_capacity(12);
                let s = Style::DEFAULT;
                for i in 0..12 {
                    let a = tree.new_leaf_with_context(s.clone(), i).unwrap();
                    let b = tree.new_leaf(s.clone()).unwrap();
                    let r = tree.new_with_children(s.clone(), &[a, b]).unwrap();
                    new_rows.push(r);
                }
                tree.set_children(root, &new_rows).unwrap();
                for grp in &handles {
                    for &id in grp.iter().rev() {
                        tree.remove(id).unwrap();
                    }
                }
                tree.compute_layout(root, room).unwrap();
                let mem_swap = (live_heap_bytes() - h0).max(0) as f64;
                black_box(&tree);

                if iter > 0 {
                    base_mem_500.push(mem_500);
                    base_mem_swap.push(mem_swap);
                    base_allocs.push(allocs);
                    base_cold_ms.push(cold);
                    base_warm_ms.push(warm);
                    base_dirty_ms.push(dirty);
                }
            } else {
                let h0 = live_heap_bytes();
                let a0 = procwatch::alloc::thread_allocations();
                let t0 = Instant::now();
                let mut store = LayoutStore::new();
                let root = sync_store_list(&mut store, 500, 100, None);
                solve_store(&mut store, root, room);
                let cold = t0.elapsed().as_secs_f64() * 1000.0;
                let allocs = (procwatch::alloc::thread_allocations() - a0) as f64;
                let mem_500 = (live_heap_bytes() - h0).max(0) as f64;

                let tw = Instant::now();
                solve_store(&mut store, root, room);
                let warm = tw.elapsed().as_secs_f64() * 1000.0;

                let td = Instant::now();
                let root = sync_store_list(&mut store, 500, 100, Some(250));
                solve_store(&mut store, root, room);
                let dirty = td.elapsed().as_secs_f64() * 1000.0;

                let root = sync_store_list(&mut store, 12, 100_000, None);
                solve_store(&mut store, root, room);
                let mem_swap = (live_heap_bytes() - h0).max(0) as f64;
                black_box(&store);

                if iter > 0 {
                    comp_mem_500.push(mem_500);
                    comp_mem_swap.push(mem_swap);
                    comp_allocs.push(allocs);
                    comp_cold_ms.push(cold);
                    comp_warm_ms.push(warm);
                    comp_dirty_ms.push(dirty);
                }
            }
        }
    }

    let (b_tree, b_root, _) = build_baseline_list(500);
    let mut c_store = LayoutStore::new();
    let _ = sync_store_list(&mut c_store, 500, 100, None);
    let b_struct = baseline_storage_stats(&b_tree, Some(b_root), 2501);
    let c_struct = c_store.storage_stats();
    println!(
        "STRUCTURAL_500_ROWS baseline_slot_B={} compact_slot_B={} baseline_retained_B={} compact_retained_B={} reduction_pct={:.2}",
        b_struct.bytes_per_slot,
        c_struct.bytes_per_slot,
        b_struct.total_retained_bytes,
        c_struct.total_retained_bytes,
        (1.0 - c_struct.total_retained_bytes as f64 / b_struct.total_retained_bytes as f64) * 100.0
    );

    for (name, b_vec, c_vec) in [
        ("heap_live_500_rows_B", base_mem_500, comp_mem_500),
        ("heap_live_after_swap_12_rows_B", base_mem_swap, comp_mem_swap),
        ("cold_allocations_500_rows", base_allocs, comp_allocs),
        ("cold_build_solve_ms", base_cold_ms, comp_cold_ms),
        ("warm_solve_ms", base_warm_ms, comp_warm_ms),
        ("single_dirty_solve_ms", base_dirty_ms, comp_dirty_ms),
    ] {
        let (b50, b95) = stats(b_vec);
        let (c50, c95) = stats(c_vec);
        let delta_pct = (c50 - b50) / b50.max(1e-9) * 100.0;
        println!(
            "ISOLATED metric={name} n={samples} baseline_p50={b50:.4} baseline_p95={b95:.4} compact_p50={c50:.4} compact_p95={c95:.4} delta_pct={delta_pct:+.2}"
        );
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    List,
    Grid,
    KitchenSink,
}

struct AppState {
    mode: Mode,
    eager_list: bool,
    scale_bump: bool,
    reordered: bool,
    removed: bool,
    titles: Vec<String>,
    subtitles: Vec<String>,
    keys: Vec<String>,
    cells: Vec<String>,
    row_keys: Vec<String>,
}

impl AppState {
    fn new(mode: Mode, eager_list: bool, cell_count: usize) -> Self {
        let cells: Vec<String> = (0..cell_count).map(|i| format!("E{}", i % 90)).collect();
        Self {
            mode,
            eager_list,
            scale_bump: false,
            reordered: false,
            removed: false,
            titles: (0..200).map(|i| format!("Demo App {i:03}")).collect(),
            subtitles: (0..200).map(|i| format!("Application · {} uses this week", (i * 37) % 91)).collect(),
            keys: (0..200).map(|i| format!("r{i}")).collect(),
            row_keys: (0..cells.len().div_ceil(COLUMNS)).map(|i| format!("g{i}")).collect(),
            cells,
        }
    }
}

fn list_row(s: &AppState, i: usize) -> El {
    let ui = TextStyle::new(14.0, 400.0);
    row()
        .w_full()
        .h(ROW)
        .px(10.0)
        .gap(10.0)
        .center_y()
        .child(el().size(22.0, 22.0).center().child(text("D").font(TextStyle::new(12.0, 600.0))))
        .child(
            col().key("copy").grow(1.0).shrink().gap(2.0).child(
                row()
                    .key("line")
                    .w_full()
                    .gap(8.0)
                    .center_y()
                    .child(text(s.titles[i].clone()).font(ui).truncate_fade())
                    .child(text(s.subtitles[i].clone()).font(ui).truncate_fade().grow(1.0)),
            ),
        )
}

fn grid_row(s: &AppState, r: usize) -> El {
    let cell = (SIZE.0 - 16.0) / COLUMNS as f32;
    let glyph = (cell * 0.5).min(34.0);
    let mut line = row().w_full().h(cell);
    for c in 0..COLUMNS {
        let index = r * COLUMNS + c;
        let Some(label) = s.cells.get(index) else { break };
        line = line.child(
            el().key(format!("c{index}"))
                .size(cell, cell)
                .center()
                .child(el().size(glyph, glyph).center().child(text(label.clone()).font(TextStyle::new(14.0, 400.0)))),
        );
    }
    line
}

fn kitchen_sink_view(s: &AppState) -> El {
    let fs = if s.scale_bump { 18.0 } else { 14.0 };
    let mut top_row = row().key("baseline_row").w_full().gap(12.0).items_center();
    let a = text("Intrinsic wrapped text with min/max bounds")
        .key("txt_a")
        .font(TextStyle::new(fs, 500.0))
        .min_w(60.0)
        .max_w(180.0);
    let b = text("Second baseline item")
        .key("txt_b")
        .font(TextStyle::new(fs + 4.0, 400.0));
    if s.reordered {
        top_row = top_row.child(b).child(a);
    } else {
        top_row = top_row.child(a).child(b);
    }

    let mut grid = el()
        .key("css_grid")
        .equal_columns(3)
        .gap(8.0)
        .w_full()
        .child(el().key("gc_span").grid_cell(0).h(32.0))
        .child(el().key("gc_mid").grid_cell(1).h(32.0));
    if !s.removed {
        grid = grid.child(el().key("gc_single").grid_cell(2).h(32.0));
    }

    let anchor_btn = el().key("anchor_target").size(120.0, 34.0).fill(Color::WHITE);
    let anchored_pop = el()
        .key("anchored_pop")
        .anchored("anchor_target", Side::Bottom, Align::Start, 6.0)
        .size(160.0, 48.0);
    let anchored_fit_pop = col()
        .key("anchored_fit_pop")
        .anchored_fit("anchor_target", Side::Bottom, Align::Start, 6.0)
        .w(180.0)
        .child(el().key("tall_pop_body").w_full().h(600.0));

    let aspect_box = el()
        .key("aspect_room")
        .w(240.0)
        .h(180.0)
        .child(el().key("aspect_inner").w_full().grow(1.0).aspect_fit(16.0 / 9.0));

    let scroll_vp = col()
        .key("scroll_vp")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .child(
            col()
                .key("scroll_content")
                .w_full()
                .gap(6.0)
                .child(el().key("scroll_item_1").w_full().h(220.0))
                .child(el().key("scroll_item_2").w_full().h(220.0)),
        )
        .child(el().key("pinned_badge").size(72.0, 24.0).pin(None, Some(10.0), Some(14.0), None));

    col()
        .key("sink_root")
        .w_full()
        .h_full()
        .padding(12.0)
        .gap(10.0)
        .child(top_row)
        .child(grid)
        .child(anchor_btn)
        .child(aspect_box)
        .child(scroll_vp)
        .child(anchored_pop)
        .child(anchored_fit_pop)
}

fn app_view(s: &AppState, _: &mut Cx<AppState>) -> El {
    match s.mode {
        Mode::List => {
            let rows = col().key("rows").w_full().px(8.0).py(6.0).growth(GrowthSpec::default());
            let rows = if s.eager_list {
                rows.virtualize(ROW).children((0..s.titles.len()).map(|i| list_row(s, i).key(s.keys[i].clone())))
            } else {
                rows.virtual_rows(s.titles.len(), ROW, |s: &AppState, i| Cow::Borrowed(s.keys[i].as_str()), list_row)
            };
            col()
                .key("root")
                .w_full()
                .h_full()
                .child(col().key("list").w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(rows))
        }
        Mode::Grid => {
            let cell = (SIZE.0 - 16.0) / COLUMNS as f32;
            let rows = col()
                .key("rows")
                .w_full()
                .px(8.0)
                .py(6.0)
                .growth(GrowthSpec::default())
                .virtualize(cell)
                .children((0..s.row_keys.len()).map(|r| grid_row(s, r).key(s.row_keys[r].clone())));
            col()
                .key("root")
                .w_full()
                .h_full()
                .child(col().key("grid").w_full().grow(1.0).shrink().scroll(ScrollSpec { stick_bottom: false }).child(rows))
        }
        Mode::KitchenSink => kitchen_sink_view(s),
    }
}

fn on_key(s: &mut AppState, k: &KeyEvent, _: &mut Tx) -> bool {
    match k.key {
        Key::Escape => {
            s.mode = Mode::List;
            true
        }
        Key::Tab => {
            s.mode = Mode::Grid;
            true
        }
        Key::Enter => {
            s.scale_bump = !s.scale_bump;
            s.reordered = !s.reordered;
            s.removed = !s.removed;
            true
        }
        _ => false,
    }
}

fn run_runtime_suite(samples: usize) {
    let adapter = if cfg!(feature = "baseline-taffy") { "baseline" } else { "compact" };

    // 1. Kitchen-sink layout digest across initial frame + reorder/scale/remove mutation frame.
    let text = TextSystem::default();
    let mut rt = Runtime::with_text(AppState::new(Mode::KitchenSink, false, 1800), app_view, SIZE, Env::new(true, 2.0), text);
    rt.on_key = Some(on_key);
    rt.frame(0.0);
    let keys = [
        "sink_root",
        "baseline_row",
        "txt_a",
        "txt_b",
        "css_grid",
        "gc_span",
        "gc_mid",
        "gc_single",
        "anchor_target",
        "aspect_room",
        "aspect_inner",
        "scroll_vp",
        "pinned_badge",
        "anchored_pop",
        "anchored_fit_pop",
    ];
    for step in 0..2 {
        if step == 1 {
            rt.key(KeyEvent { key: Key::Enter, mods: Default::default() });
            while rt.frame(1.0 / 120.0) {}
        }
        let mut rects = Vec::new();
        for k in keys {
            if let Some((r, _)) = rt.presented(k) {
                rects.push(format!("{k}:{:.2},{:.2},{:.2},{:.2}", r[0], r[1], r[2], r[3]));
            }
        }
        println!("RECT_DIGEST adapter={adapter} step={step} rects={}", rects.join(";"));
    }

    // 2. Combined retained-tree memory and commit/layout CPU across eager list (200 rows),
    // emoji grid (1,800 cells), and grid -> virtual-list swap.
    let make_warm_text = || {
        let mut t = TextSystem::default();
        t.layout_rich("Warm Demo App 000 E0", &[], TextStyle::new(14.0, 400.0), 2.0, None);
        for _ in 0..3 {
            t.end_commit();
        }
        t
    };

    let mut mem_eager_list = Vec::with_capacity(samples);
    let mut mem_grid_1800 = Vec::with_capacity(samples);
    let mut mem_after_swap = Vec::with_capacity(samples);
    let mut swap_commit_ms = Vec::with_capacity(samples);
    let mut swap_layout_ms = Vec::with_capacity(samples);
    let mut grid_commit_ms = Vec::with_capacity(samples);
    let mut grid_layout_ms = Vec::with_capacity(samples);

    for i in 0..=samples {
        // Eager list 200 rows
        {
            let t = make_warm_text();
            let state = AppState::new(Mode::List, true, 1800);
            let h0 = live_heap_bytes();
            let mut rt = Runtime::with_text(state, app_view, SIZE, Env::new(true, 2.0), t);
            rt.frame(0.0);
            let live = (live_heap_bytes() - h0).max(0) as f64;
            black_box(&rt);
            if i > 0 {
                mem_eager_list.push(live);
            }
        }
        // Grid 1800 cells -> swap to virtual list
        {
            let t = make_warm_text();
            let state = AppState::new(Mode::Grid, false, 1800);
            let h0 = live_heap_bytes();
            rune::take_frame_phases();
            let mut rt = Runtime::with_text(state, app_view, SIZE, Env::new(true, 2.0), t);
            rt.on_key = Some(on_key);
            rt.frame(0.0);
            let phases_grid = rune::take_frame_phases();
            let p = rune::PHASES;
            let live_grid = (live_heap_bytes() - h0).max(0) as f64;

            // Settle motion then swap to virtual list (3,790 layout nodes -> ~75 layout nodes).
            while rt.frame(1.0 / 120.0) {}
            rune::take_frame_phases();
            rt.key(KeyEvent { key: Key::Escape, mods: Default::default() });
            rt.frame(1.0 / 120.0);
            let phases_swap = rune::take_frame_phases();
            while rt.frame(1.0 / 120.0) {}
            let live_swap = (live_heap_bytes() - h0).max(0) as f64;
            black_box(&rt);

            if i > 0 {
                mem_grid_1800.push(live_grid);
                mem_after_swap.push(live_swap);
                grid_commit_ms.push(phases_grid[p + 1]);
                grid_layout_ms.push(phases_grid[p + 2]);
                swap_commit_ms.push(phases_swap[p + 1]);
                swap_layout_ms.push(phases_swap[p + 2]);
            }
        }
    }

    let raw = std::env::args().any(|a| a == "--raw");
    for (name, vec) in [
        ("combined_retained_eager_list_200_B", mem_eager_list),
        ("combined_retained_grid_1800_B", mem_grid_1800),
        ("combined_retained_after_grid_to_list_swap_B", mem_after_swap),
        ("grid_1800_commit_ms", grid_commit_ms),
        ("grid_1800_layout_ms", grid_layout_ms),
        ("grid_to_list_swap_commit_ms", swap_commit_ms),
        ("grid_to_list_swap_layout_ms", swap_layout_ms),
    ] {
        if raw {
            for v in &vec {
                println!("RAW adapter={adapter} metric={name} val={v:.6}");
            }
        }
        let (p50, p95) = stats(vec);
        println!("RUNTIME adapter={adapter} metric={name} n={samples} p50={p50:.4} p95={p95:.4}");
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let samples: usize = value("--samples").map_or(25, |s| s.parse().expect("--samples N"));
    if ! args.iter().any(|a| a == "--runtime-only") {
        run_isolated_paired_bench(samples);
    }
    run_runtime_suite(samples);
}
