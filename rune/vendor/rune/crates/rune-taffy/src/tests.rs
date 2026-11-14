use crate::prelude::*;
use crate::{
    BaselineLayoutStore, BaselineTaffyTree, BoxLayout, CompactCache, CompactStyle, LayoutStore, SlotId,
    baseline_storage_stats,
};
use core::mem::size_of;
use taffy::geometry::{Line, Point, Rect, Size};
use taffy::style::{AvailableSpace, CoreStyle, FlexboxContainerStyle, FlexboxItemStyle, Overflow};
use taffy::tree::{Baselines, CollapsibleMarginSet, LayoutInput, LayoutOutput, RequestedAxis, RunMode, SizingMode};

#[test]
fn compact_types_stay_within_size_budgets() {
    assert_eq!(size_of::<CompactStyle>(), 64);
    assert_eq!(size_of::<CompactCache>(), 96);
    assert_eq!(size_of::<BoxLayout>(), 32);
}

#[test]
fn compact_style_round_trips_all_length_variants() {
    let hot_dims = [
        Dimension::auto(),
        Dimension::ZERO,
        Dimension::length(42.5),
        Dimension::percent(0.375),
        Dimension::max_content(),
    ];
    for &w in &hot_dims {
        for &h in &hot_dims {
            let s = Style {
                display: Display::Flex,
                position: Position::Relative,
                flex_direction: FlexDirection::ColumnReverse,
                flex_wrap: FlexWrap::Wrap,
                overflow: Point { x: Overflow::Hidden, y: Overflow::Scroll },
                size: Size { width: w, height: h },
                min_size: Size { width: length(10.0), height: percent(0.25) },
                max_size: Size { width: auto(), height: length(500.0) },
                padding: Rect {
                    left: length(4.0),
                    right: percent(0.125),
                    top: length(8.0),
                    bottom: length(12.0),
                },
                gap: Size { width: length(6.0), height: percent(0.25) },
                flex_basis: Dimension::ZERO,
                flex_grow: 1.5,
                flex_shrink: 0.25,
                align_items: AlignItems::CENTER,
                align_self: Some(AlignSelf::FLEX_END),
                justify_content: JustifyContent::SPACE_BETWEEN,
                align_content: AlignContent::STRETCH,
                ..Style::DEFAULT
            };
            let compact = CompactStyle::from_style(s.clone());
            assert_eq!(compact.heap_bytes(), 0, "hot Rune style should stay 100% inline in 64 bytes");
            assert!(compact == s);
            assert_eq!(compact.to_style(), s);
        }
    }

    let all_dims = [
        Dimension::auto(),
        Dimension::ZERO,
        Dimension::length(42.5),
        Dimension::percent(0.375),
        Dimension::min_content(),
        Dimension::max_content(),
        Dimension::fit_content_px(120.0),
        Dimension::fit_content_percent(0.8),
    ];
    for &w in &all_dims {
        for &h in &all_dims {
            let s = Style {
                display: Display::Flex,
                position: Position::Relative,
                flex_direction: FlexDirection::ColumnReverse,
                flex_wrap: FlexWrap::Wrap,
                overflow: Point { x: Overflow::Hidden, y: Overflow::Scroll },
                size: Size { width: w, height: h },
                min_size: Size { width: length(10.0), height: percent(0.25) },
                max_size: Size { width: auto(), height: length(500.0) },
                padding: Rect {
                    left: length(4.0),
                    right: percent(0.1),
                    top: length(8.0),
                    bottom: length(12.0),
                },
                gap: Size { width: length(6.0), height: percent(0.05) },
                flex_basis: w,
                flex_grow: 1.5,
                flex_shrink: 0.25,
                aspect_ratio: Some(1.75),
                align_items: AlignItems::CENTER,
                align_self: Some(AlignSelf::FLEX_END),
                justify_content: JustifyContent::SPACE_BETWEEN,
                align_content: AlignContent::STRETCH,
                ..Style::DEFAULT
            };
            let compact = CompactStyle::from_style(s.clone());
            assert!(compact == s);
            assert_eq!(compact.to_style(), s);
            assert_eq!(CoreStyle::size(&compact), s.size);
            assert_eq!(CoreStyle::min_size(&compact), s.min_size);
            assert_eq!(CoreStyle::max_size(&compact), s.max_size);
            assert_eq!(CoreStyle::padding(&compact), s.padding);
            assert_eq!(FlexboxContainerStyle::gap(&compact), s.gap);
            assert_eq!(FlexboxItemStyle::flex_basis(&compact), s.flex_basis);
        }
    }
}

#[test]
fn compact_cache_preserves_second_chance_and_overflow_semantics() {
    let make_input = |w: f32| LayoutInput {
        run_mode: RunMode::ComputeSize,
        sizing_mode: SizingMode::InherentSize,
        axis: RequestedAxis::Both,
        known_dimensions: Size { width: Some(w), height: None },
        known_dimensions_are_definite: Size { width: true, height: true },
        parent_size: Size::NONE,
        available_space: Size { width: AvailableSpace::MaxContent, height: AvailableSpace::MaxContent },
        vertical_margins_are_collapsible: Line::FALSE,
    };
    let make_out = |w: f32| LayoutOutput::from_outer_size(Size { width: w, height: w * 2.0 });

    let mut cache = CompactCache::new();
    cache.store(&make_input(0.0), make_out(0.0));
    cache.store(&make_input(1.0), make_out(1.0));
    assert_eq!(cache.heap_bytes(), 0);

    for w in 2..9 {
        cache.store(&make_input(w as f32), make_out(w as f32));
    }
    assert!(cache.heap_bytes() > 0);
    cache.store(&make_input(9.0), make_out(9.0));
    assert_eq!(cache.get(&make_input(1.0)), Some(make_out(1.0)));
    cache.store(&make_input(10.0), make_out(10.0));
    assert_eq!(cache.get(&make_input(1.0)), Some(make_out(1.0)));
    assert_eq!(cache.get(&make_input(2.0)), None);

    let mut collapse = make_out(99.0);
    collapse.top_margin = CollapsibleMarginSet::from_margin(8.0);
    cache.store(&make_input(99.0), collapse);
    assert_eq!(cache.get(&make_input(99.0)), None);
}

#[test]
fn compact_style_setters_match_taffy_style() {
    for dir in [
        FlexDirection::Row,
        FlexDirection::Column,
        FlexDirection::RowReverse,
        FlexDirection::ColumnReverse,
    ] {
        let mut cs = CompactStyle::rune_base(dir);
        let mut ts = Style {
            display: Display::Flex,
            flex_direction: dir,
            flex_shrink: 0.0,
            position: Position::Relative,
            ..Style::DEFAULT
        };
        assert_eq!(cs.to_style(), ts);
        assert_eq!(cs, CompactStyle::from_style(ts.clone()));

        cs.set_width(percent(1.0));
        ts.size.width = percent(1.0);
        cs.set_height(length(48.5));
        ts.size.height = length(48.5);
        cs.set_min_width(length(0.0));
        ts.min_size.width = length(0.0);
        cs.set_max_height(length(240.0));
        ts.max_size.height = length(240.0);
        cs.set_padding_all(12.0);
        ts.padding = Rect { left: length(12.0), right: length(12.0), top: length(12.0), bottom: length(12.0) };
        cs.set_pad_left(6.0);
        ts.padding.left = length(6.0);
        cs.set_gap(8.0, 4.0);
        ts.gap = Size { width: length(8.0), height: length(4.0) };
        cs.set_grow(1.5);
        ts.flex_grow = 1.5;
        ts.flex_shrink = 1.0;
        ts.flex_basis = length(0.0);
        cs.set_flex_wrap(FlexWrap::Wrap);
        ts.flex_wrap = FlexWrap::Wrap;
        cs.set_overflow(Overflow::Hidden, Overflow::Scroll);
        ts.overflow = Point { x: Overflow::Hidden, y: Overflow::Scroll };
        cs.set_align_items(AlignItems::CENTER);
        ts.align_items = AlignItems::CENTER;
        cs.set_align_self(Some(AlignSelf::STRETCH));
        ts.align_self = Some(AlignSelf::STRETCH);
        cs.set_justify_content(JustifyContent::SPACE_BETWEEN);
        ts.justify_content = JustifyContent::SPACE_BETWEEN;
        assert_eq!(cs.heap_bytes(), 0, "hot setters should stay inline");
        assert_eq!(cs.to_style(), ts);
        assert_eq!(cs, CompactStyle::from_style(ts.clone()));

        cs.set_aspect_ratio(Some(1.75));
        ts.aspect_ratio = Some(1.75);
        assert_eq!(cs.to_style(), ts);
        cs.set_aspect_ratio(None);
        ts.aspect_ratio = None;
        assert_eq!(cs.heap_bytes(), 0, "clearing last cold field drops extra Box");
        assert_eq!(cs.to_style(), ts);
    }

    for pos in [Position::Static, Position::Relative, Position::Absolute] {
        let s = Style { position: pos, ..Style::DEFAULT };
        let c = CompactStyle::from_style(s.clone());
        assert_eq!(c.position(), pos);
        assert_eq!(c.to_style(), s);
    }
}

#[test]
fn a_taken_box_placement_keeps_the_overflow_on_the_wrapper() {
    for cold in [false, true] {
        let mut cs = CompactStyle::rune_base(FlexDirection::Column);
        cs.set_overflow(Overflow::Hidden, Overflow::Scroll);
        if cold {
            cs.set_aspect_ratio(Some(2.0));
        }
        let outer = cs.take_box_placement();
        assert_eq!(outer.overflow(), Point { x: Overflow::Hidden, y: Overflow::Scroll }, "cold {cold}");
        assert_eq!(cs.overflow(), Point { x: Overflow::Hidden, y: Overflow::Scroll }, "cold {cold}");
    }
}

#[derive(Clone)]
struct SpecNode {
    key: u64,
    style: Style,
    leaf: Option<(u64, f32, f32, f32)>, // (measure_hash, intrinsic_w, line_h, baseline)
    children: Vec<SpecNode>,
}

struct CommitEntry {
    key: u64,
    parent: Option<usize>,
    comp_slot: SlotId,
    base_slot: SlotId,
    leaf: Option<(f32, f32, f32)>,
}

fn sync_pair(
    spec: &SpecNode,
    parent: Option<usize>,
    comp: &mut LayoutStore,
    base: &mut BaselineLayoutStore,
    out: &mut Vec<CommitEntry>,
) -> (SlotId, bool, SlotId) {
    let me = out.len();
    out.push(CommitEntry {
        key: spec.key,
        parent,
        comp_slot: SlotId(0),
        base_slot: SlotId(0),
        leaf: spec.leaf.map(|(_, w, h, b)| (w, h, b)),
    });
    let c_mark = comp.begin_children();
    let b_mark = base.begin_children();
    let mut kids_dirty = false;
    let mut c_kids = Vec::with_capacity(spec.children.len());
    let mut b_kids = Vec::with_capacity(spec.children.len());
    for child in &spec.children {
        let (cs, cd, bs) = sync_pair(child, Some(me), comp, base, out);
        kids_dirty |= cd;
        c_kids.push(cs);
        b_kids.push(bs);
    }
    for cs in c_kids {
        comp.push_child(cs);
    }
    for bs in b_kids {
        base.push_child(bs);
    }
    let measure = spec.leaf.map(|(h, ..)| h);
    let cs_style = CompactStyle::from_style(spec.style.clone());
    let (c_slot, dirty) = comp.sync_node(spec.key, me as u32, cs_style.clone(), measure, false, c_mark, kids_dirty);
    let (b_slot, _) = base.sync_node(spec.key, me as u32, cs_style, measure, false, b_mark, kids_dirty);
    out[me].comp_slot = c_slot;
    out[me].base_slot = b_slot;
    (c_slot, dirty, b_slot)
}

fn commit_and_assert_parity(
    root: &SpecNode,
    room: Size<AvailableSpace>,
    comp: &mut LayoutStore,
    base: &mut BaselineLayoutStore,
) -> (Vec<CommitEntry>, Vec<u64>, Vec<u64>) {
    comp.begin_commit();
    base.begin_commit();
    let mut entries = Vec::new();
    let (mut c_root, _, mut b_root) = sync_pair(root, None, comp, base, &mut entries);
    comp.end_commit(&mut c_root, |me, slot| entries[me as usize].comp_slot = slot);
    base.end_commit(&mut b_root, |me, slot| entries[me as usize].base_slot = slot);
    assert_eq!(comp.live_count(), entries.len());
    assert_eq!(base.live_count(), entries.len());

    let eval_leaf = |inputs: LayoutInput, ctx: Option<usize>, style: &CompactStyle, log: &mut Vec<u64>| {
        let mut baselines = Baselines::NONE;
        let mut out = taffy::compute_leaf_layout(inputs, style, |_, _| 0.0, |known, avail| {
            let Some(i) = ctx else { return Size::ZERO };
            log.push(entries[i].key);
            let (full_w, line_h, baseline) = entries[i].leaf.expect("ctx must point to measured leaf");
            let max_w = known.width.or(match avail.width {
                AvailableSpace::Definite(w) => Some(w),
                AvailableSpace::MinContent => Some((full_w * 0.25).max(20.0)),
                AvailableSpace::MaxContent => None,
            });
            let (w, lines) = match max_w {
                Some(limit) if full_w > limit && limit > 0.0 => (limit, (full_w / limit).ceil().max(1.0)),
                _ => (full_w, 1.0),
            };
            if baseline > 0.0 {
                baselines = Baselines { first: Some(baseline), last: Some((lines - 1.0) * line_h + baseline) };
            }
            Size {
                width: known.width.unwrap_or(w),
                height: known.height.unwrap_or(lines * line_h),
            }
        });
        out.baselines = baselines;
        out
    };

    let mut c_log = Vec::new();
    let mut b_log = Vec::new();
    comp.compute_layout(c_root, room, |inputs, ctx, style| eval_leaf(inputs, ctx, style, &mut c_log));
    base.compute_layout(b_root, room, |inputs, ctx, style| eval_leaf(inputs, ctx, style, &mut b_log));

    for (i, e) in entries.iter().enumerate() {
        assert_eq!(
            comp.box_layout(e.comp_slot),
            base.box_layout(e.base_slot),
            "BoxLayout mismatch at entry {i} (key={})",
            e.key
        );
    }
    (entries, c_log, b_log)
}

#[test]
fn differential_flex_grid_oof_scroll_text_baselines_reorder_and_removal() {
    let mut comp = LayoutStore::new();
    let mut base = BaselineLayoutStore::new();

    let text_style = Style {
        flex_shrink: 1.0,
        min_size: Size { width: length(0.0), height: auto() },
        max_size: Size { width: length(180.0), height: length(100.0) },
        ..Style::DEFAULT
    };
    let build_scene = |reordered: bool, scaled: bool, removed: bool| {
        let t1 = SpecNode {
            key: 11,
            style: text_style.clone(),
            leaf: Some(if scaled { (2, 310.0, 20.0, 15.5) } else { (1, 240.0, 18.0, 14.0) }),
            children: vec![],
        };
        let t2 = SpecNode {
            key: 12,
            style: text_style.clone(),
            leaf: Some((3, 95.5, 22.0, 17.0)),
            children: vec![],
        };
        let flex_row = SpecNode {
            key: 10,
            style: Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                flex_wrap: FlexWrap::Wrap,
                align_items: AlignItems::BASELINE,
                gap: Size { width: length(8.25), height: length(4.5) },
                padding: Rect { left: length(10.5), right: length(10.5), top: length(6.25), bottom: length(6.25) },
                ..Style::DEFAULT
            },
            leaf: None,
            children: if reordered { vec![t2, t1] } else { vec![t1, t2] },
        };

        let mut grid_kids = vec![SpecNode {
            key: 21,
            style: Style {
                grid_column: Line { start: line(1), end: span(2) },
                grid_row: Line { start: line(1), end: span(1) },
                size: Size { width: auto(), height: length(28.5) },
                ..Style::DEFAULT
            },
            leaf: None,
            children: vec![],
        }];
        if !removed {
            grid_kids.push(SpecNode {
                key: 22,
                style: Style {
                    grid_column: Line { start: line(3), end: span(1) },
                    grid_row: Line { start: line(1), end: span(1) },
                    aspect_ratio: Some(1.5),
                    ..Style::DEFAULT
                },
                leaf: None,
                children: vec![],
            });
        }
        let grid = SpecNode {
            key: 20,
            style: Style {
                display: Display::Grid,
                grid_template_columns: vec![fr(1.0), fr(2.0), minmax(length(40.0), fr(1.0))],
                gap: Size { width: length(6.0), height: length(6.0) },
                size: Size { width: percent(1.0), height: auto() },
                ..Style::DEFAULT
            },
            leaf: None,
            children: grid_kids,
        };

        let scroll = SpecNode {
            key: 30,
            style: Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                position: Position::Relative,
                overflow: Point { x: Overflow::Hidden, y: Overflow::Scroll },
                flex_grow: 1.0,
                flex_shrink: 1.0,
                flex_basis: length(0.0),
                min_size: Size { width: length(0.0), height: length(0.0) },
                ..Style::DEFAULT
            },
            leaf: None,
            children: vec![
                SpecNode {
                    key: 31,
                    style: Style { size: Size { width: percent(1.0), height: length(420.0) }, flex_shrink: 0.0, ..Style::DEFAULT },
                    leaf: None,
                    children: vec![],
                },
                SpecNode {
                    key: 32,
                    style: Style {
                        position: Position::Absolute,
                        inset: Rect { left: length(12.25), top: length(8.75), right: auto(), bottom: auto() },
                        size: Size { width: length(64.0), height: length(24.0) },
                        ..Style::DEFAULT
                    },
                    leaf: None,
                    children: vec![],
                },
            ],
        };

        SpecNode {
            key: 1,
            style: Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                position: Position::Relative,
                size: Size { width: length(360.0), height: length(240.0) },
                gap: Size { width: length(0.0), height: length(8.0) },
                ..Style::DEFAULT
            },
            leaf: None,
            children: vec![flex_row, grid, scroll],
        }
    };

    let room = Size {
        width: AvailableSpace::Definite(360.0),
        height: AvailableSpace::Definite(240.0),
    };
    let (_, c1, b1) = commit_and_assert_parity(&build_scene(false, false, false), room, &mut comp, &mut base);
    assert_eq!(c1, b1);

    let (_, c2, b2) = commit_and_assert_parity(&build_scene(true, true, true), room, &mut comp, &mut base);
    assert_eq!(c2, b2);
}

#[test]
fn large_to_small_swap_shrinks_retained_memory_and_matches_baseline() {
    let mut comp = LayoutStore::new();
    let mut base = BaselineLayoutStore::new();

    let make_list = |rows: usize| {
        let mut children = Vec::with_capacity(rows);
        for i in 0..rows {
            let base_key = 100 + (i as u64) * 10;
            children.push(SpecNode {
                key: base_key,
                style: Style {
                    display: Display::Flex,
                    flex_direction: FlexDirection::Row,
                    size: Size { width: percent(1.0), height: length(32.0) },
                    ..Style::DEFAULT
                },
                leaf: None,
                children: vec![
                    SpecNode {
                        key: base_key + 1,
                        style: Style { size: Size { width: length(120.0), height: length(18.0) }, ..Style::DEFAULT },
                        leaf: Some((i as u64 + 1, 120.0, 18.0, 14.0)),
                        children: vec![],
                    },
                    SpecNode {
                        key: base_key + 2,
                        style: Style { size: Size { width: length(20.0), height: length(20.0) }, ..Style::DEFAULT },
                        leaf: None,
                        children: vec![],
                    },
                ],
            });
        }
        SpecNode {
            key: 1,
            style: Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                size: Size { width: length(1000.0), height: length(900.0) },
                ..Style::DEFAULT
            },
            leaf: None,
            children,
        }
    };

    let room = Size {
        width: AvailableSpace::Definite(1000.0),
        height: AvailableSpace::Definite(900.0),
    };
    commit_and_assert_parity(&make_list(500), room, &mut comp, &mut base);
    let b_stats_500 = base.storage_stats();
    let c_stats_500 = comp.storage_stats();
    assert!(
        c_stats_500.total_retained_bytes * 2 < b_stats_500.total_retained_bytes,
        "expected >50% reduction at 500 rows: compact={} vs baseline={}",
        c_stats_500.total_retained_bytes,
        b_stats_500.total_retained_bytes
    );

    commit_and_assert_parity(&make_list(10), room, &mut comp, &mut base);
    let b_stats_10 = base.storage_stats();
    let c_stats_10 = comp.storage_stats();
    assert!(
        c_stats_10.slot_capacity <= 64,
        "LayoutStore slot capacity should shrink after 500->10 swap, got {}",
        c_stats_10.slot_capacity
    );
    assert!(
        c_stats_10.total_retained_bytes * 10 < b_stats_10.total_retained_bytes,
        "expected >90% reduction after 500->10 swap: compact={} vs baseline={}",
        c_stats_10.total_retained_bytes,
        b_stats_10.total_retained_bytes
    );
}

#[test]
fn differential_hoisted_oof_aspect_fit_anchored_fit_and_cache_hits() {
    let mut comp = LayoutStore::new();
    let mut base = BaselineLayoutStore::new();

    let scene = SpecNode {
        key: 1,
        style: Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            position: Position::Relative,
            size: Size { width: length(400.0), height: length(300.0) },
            ..Style::DEFAULT
        },
        leaf: None,
        children: vec![
            SpecNode {
                key: 10,
                style: Style {
                    display: Display::Flex,
                    position: Position::Static,
                    padding: Rect { left: length(7.25), right: length(7.25), top: length(5.5), bottom: length(5.5) },
                    ..Style::DEFAULT
                },
                leaf: None,
                children: vec![SpecNode {
                    key: 11,
                    style: Style {
                        position: Position::Absolute,
                        inset: Rect { left: auto(), right: length(14.5), top: length(9.25), bottom: auto() },
                        size: Size { width: length(88.0), height: length(36.0) },
                        ..Style::DEFAULT
                    },
                    leaf: None,
                    children: vec![],
                }],
            },
            SpecNode {
                key: 20,
                style: Style {
                    flex_grow: 1.0,
                    size: Size { width: percent(1.0), height: auto() },
                    ..Style::DEFAULT
                },
                leaf: None,
                children: vec![],
            },
            SpecNode {
                key: 30,
                style: Style {
                    position: Position::Absolute,
                    inset: Rect { left: length(0.0), top: length(0.0), right: auto(), bottom: auto() },
                    size: Size { width: length(160.0), height: auto() },
                    ..Style::DEFAULT
                },
                leaf: None,
                children: vec![SpecNode {
                    key: 31,
                    style: Style { size: Size { width: percent(1.0), height: auto() }, ..Style::DEFAULT },
                    leaf: Some((99, 160.0, 320.0, 0.0)),
                    children: vec![],
                }],
            },
        ],
    };

    let room = Size {
        width: AvailableSpace::Definite(400.0),
        height: AvailableSpace::Definite(300.0),
    };
    let (_, c_cold, b_cold) = commit_and_assert_parity(&scene, room, &mut comp, &mut base);
    assert!(!c_cold.is_empty());
    assert_eq!(c_cold, b_cold, "measure call count mismatch on cold pass");

    // Warm commit with zero changes hits root cache (0 leaf measurements).
    let (entries, c_warm, b_warm) = commit_and_assert_parity(&scene, room, &mut comp, &mut base);
    assert!(c_warm.is_empty());
    assert!(b_warm.is_empty());

    // Pass 2 intra-commit refit: clamp `aspect_fit` (key 20) to 16:9 and `anchored_fit` (key 30) max_size.height.
    let aspect_idx = entries.iter().position(|e| e.key == 20).unwrap();
    let ov_idx = entries.iter().position(|e| e.key == 30).unwrap();
    let room_sz = comp.box_layout(entries[aspect_idx].comp_slot).size();
    let fit_w = room_sz.width.min(room_sz.height * (16.0 / 9.0));
    let fit_h = fit_w / (16.0 / 9.0);
    let fitted = CompactStyle::from_style(Style {
        size: Size { width: length(fit_w), height: length(fit_h) },
        flex_shrink: 0.0,
        ..Style::DEFAULT
    });
    comp.set_style_and_mark_ancestors(
        entries[aspect_idx].comp_slot,
        fitted.clone(),
        core::iter::successors(entries[aspect_idx].parent, |&p| entries[p].parent).map(|p| entries[p].comp_slot),
    );
    base.set_style_and_mark_ancestors(
        entries[aspect_idx].base_slot,
        fitted,
        core::iter::successors(entries[aspect_idx].parent, |&p| entries[p].parent).map(|p| entries[p].base_slot),
    );

    let mut ov_style = comp.style(entries[ov_idx].comp_slot).clone();
    ov_style.set_max_height(length(180.0));
    comp.set_style_and_mark_ancestors(
        entries[ov_idx].comp_slot,
        ov_style.clone(),
        core::iter::successors(entries[ov_idx].parent, |&p| entries[p].parent).map(|p| entries[p].comp_slot),
    );
    base.set_style_and_mark_ancestors(
        entries[ov_idx].base_slot,
        ov_style,
        core::iter::successors(entries[ov_idx].parent, |&p| entries[p].parent).map(|p| entries[p].base_slot),
    );

    let mut c_pass2 = 0usize;
    let mut b_pass2 = 0usize;
    comp.compute_layout(entries[0].comp_slot, room, |inputs, _ctx, style| {
        taffy::compute_leaf_layout(inputs, style, |_, _| 0.0, |known, _| {
            c_pass2 += 1;
            Size { width: known.width.unwrap_or(160.0), height: known.height.unwrap_or(320.0) }
        })
    });
    base.compute_layout(entries[0].base_slot, room, |inputs, _ctx, style| {
        taffy::compute_leaf_layout(inputs, style, |_, _| 0.0, |known, _| {
            b_pass2 += 1;
            Size { width: known.width.unwrap_or(160.0), height: known.height.unwrap_or(320.0) }
        })
    });
    assert_eq!(c_pass2, b_pass2, "measure call count mismatch after aspect_fit + anchored_fit refit");
    for (i, e) in entries.iter().enumerate() {
        assert_eq!(comp.box_layout(e.comp_slot), base.box_layout(e.base_slot), "refit mismatch at {i}");
    }
}

#[test]
fn layout_store_carries_cache_across_commits_and_matches_upstream_bit_for_bit() {
    let make_row = |id_base: u64, text_w: f32, hash: u64| SpecNode {
        key: id_base,
        style: Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Row,
            flex_shrink: 0.0,
            position: Position::Relative,
            size: Size { width: percent(1.0), height: auto() },
            padding: Rect { left: length(8.0), right: length(8.0), top: length(4.0), bottom: length(4.0) },
            gap: Size { width: length(6.0), height: length(0.0) },
            align_items: AlignItems::CENTER,
            ..Style::DEFAULT
        },
        leaf: None,
        children: vec![
            SpecNode {
                key: id_base + 1,
                style: Style { size: Size { width: length(16.0), height: length(16.0) }, flex_shrink: 0.0, ..Style::DEFAULT },
                leaf: None,
                children: vec![],
            },
            SpecNode {
                key: id_base + 2,
                style: Style {
                    flex_grow: 1.0,
                    flex_shrink: 1.0,
                    flex_basis: length(0.0),
                    min_size: Size { width: length(0.0), height: auto() },
                    ..Style::DEFAULT
                },
                leaf: Some((hash, text_w, 16.0, 0.0)),
                children: vec![],
            },
        ],
    };

    let make_tree = |rows: Vec<SpecNode>, vp_w: f32| {
        let grid_header = SpecNode {
            key: 10,
            style: Style {
                display: Display::Grid,
                position: Position::Relative,
                size: Size { width: percent(1.0), height: auto() },
                gap: Size { width: length(8.0), height: length(4.0) },
                grid_template_columns: vec![fr(1.0), fr(2.0)],
                ..Style::DEFAULT
            },
            leaf: None,
            children: vec![
                SpecNode {
                    key: 11,
                    style: Style { size: Size { width: auto(), height: length(24.0) }, ..Style::DEFAULT },
                    leaf: None,
                    children: vec![],
                },
                SpecNode {
                    key: 12,
                    style: Style { size: Size { width: auto(), height: length(24.0) }, ..Style::DEFAULT },
                    leaf: Some((999, 180.0, 18.0, 0.0)),
                    children: vec![],
                },
                SpecNode {
                    key: 13,
                    style: Style {
                        position: Position::Absolute,
                        inset: Rect { left: auto(), top: length(2.0), right: length(4.0), bottom: auto() },
                        size: Size { width: length(20.0), height: length(12.0) },
                        ..Style::DEFAULT
                    },
                    leaf: None,
                    children: vec![],
                },
            ],
        };
        let static_oof = SpecNode {
            key: 20,
            style: Style { position: Position::Static, size: Size { width: percent(1.0), height: length(10.0) }, ..Style::DEFAULT },
            leaf: None,
            children: vec![SpecNode {
                key: 21,
                style: Style {
                    position: Position::Absolute,
                    inset: Rect { left: length(12.0), top: auto(), right: length(12.0), bottom: length(6.0) },
                    size: Size { width: auto(), height: length(18.0) },
                    ..Style::DEFAULT
                },
                leaf: None,
                children: vec![],
            }],
        };
        let hidden = SpecNode {
            key: 30,
            style: Style { display: Display::None, size: Size { width: length(90.0), height: length(90.0) }, ..Style::DEFAULT },
            leaf: None,
            children: vec![make_row(31, 100.0, 31)],
        };
        let mut children = vec![grid_header, static_oof, hidden];
        children.extend(rows);
        SpecNode {
            key: 1,
            style: Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                position: Position::Relative,
                size: Size { width: length(vp_w), height: length(500.0) },
                padding: Rect { left: length(10.0), right: length(10.0), top: length(8.0), bottom: length(8.0) },
                gap: Size { width: length(0.0), height: length(4.0) },
                ..Style::DEFAULT
            },
            leaf: None,
            children,
        }
    };

    let mut comp = LayoutStore::new();
    let mut base = BaselineLayoutStore::new();
    let room = |vp_w: f32| Size {
        width: AvailableSpace::Definite(vp_w),
        height: AvailableSpace::Definite(500.0),
    };
    let rows_1: Vec<SpecNode> = (0..8).map(|i| make_row(100 + i * 10, 220.0 + (i as f32) * 35.0, 1000 + i)).collect();

    // Commit 1 (Cold): measures leaves and matches BaselineLayoutStore bit-for-bit.
    let (entries_1, keys_1, _) = commit_and_assert_parity(&make_tree(rows_1.clone(), 360.0), room(360.0), &mut comp, &mut base);
    assert!(keys_1.len() >= 9);
    let mut raw_base = BaselineTaffyTree::<usize>::new();
    let raw_leaf = raw_base.new_leaf(Style::DEFAULT).unwrap();
    let base_bytes = baseline_storage_stats(&raw_base, Some(raw_leaf), entries_1.len()).total_retained_bytes;
    assert!(comp.storage_stats().total_retained_bytes * 2 < base_bytes);

    // Commit 2 (Warm identical): hits root cache — 0 leaf measurements!
    let (_, keys_2, _) = commit_and_assert_parity(&make_tree(rows_1.clone(), 360.0), room(360.0), &mut comp, &mut base);
    assert!(keys_2.is_empty(), "warm commit with no changes must invoke measure 0 times");

    // Commit 3 (Insert row 90 at front — shifts every existing row's `me` index by +3!):
    let mut rows_3 = vec![make_row(90, 310.0, 9999)];
    rows_3.extend(rows_1);
    let (_, keys_3, _) = commit_and_assert_parity(&make_tree(rows_3.clone(), 360.0), room(360.0), &mut comp, &mut base);
    assert!(!keys_3.is_empty());
    assert!(keys_3.iter().all(|&k| k == 92), "only newly inserted row leaf (key=92) may measure; got {keys_3:?}");

    // Commit 4 (Narrow viewport width from 360 -> 280 after index shift):
    let (_, keys_4, _) = commit_and_assert_parity(&make_tree(rows_3.clone(), 280.0), room(280.0), &mut comp, &mut base);
    assert!(keys_4.len() >= 9, "narrowed viewport must re-measure shifted leaves");

    // Commit 5 (Remove 3 rows, reorder remaining rows, insert 2 new rows recycling freed slots, mutate 1 leaf hash):
    let mut rows_5 = vec![
        rows_3[5].clone(),
        make_row(500, 410.0, 5001),
        rows_3[2].clone(),
        make_row(510, 145.0, 5002),
        rows_3[0].clone(),
    ];
    rows_5[2].children[1].leaf = Some((77777, 480.0, 16.0, 0.0));
    let (_, keys_5, _) = commit_and_assert_parity(&make_tree(rows_5, 280.0), room(280.0), &mut comp, &mut base);
    assert!(keys_5.contains(&502) && keys_5.contains(&512) && keys_5.contains(&112));
    assert!(!keys_5.contains(&92) && !keys_5.contains(&142), "unchanged reordered rows must hit cache");
}

#[test]
fn memo_reuse_and_retry_mutation_guard_match_baseline() {
    let mut comp = LayoutStore::new();
    let mut base = BaselineLayoutStore::new();

    // Commit 1 (applied): create slot for key 10 with width 120.
    comp.begin_commit();
    base.begin_commit();
    let mut s1 = CompactStyle::rune_base(FlexDirection::Row);
    s1.set_width(length(120.0));
    s1.set_height(length(40.0));
    let m_c = comp.begin_children();
    let (slot_c, _) = comp.sync_node(10, 0, s1.clone(), None, false, m_c, false);
    comp.finish_flatten();
    comp.compact_if_sparse(|_, _| {});
    let m_b = base.begin_children();
    let (slot_b, _) = base.sync_node(10, 0, s1.clone(), None, false, m_b, false);
    base.finish_flatten();
    base.compact_if_sparse(|_, _| {});

    // Commit 2, Pass 1 (discarded before apply): key 10 temporarily mutates to width 300.
    comp.begin_commit();
    base.begin_commit();
    assert!(comp.can_reuse(10, slot_c));
    assert!(base.can_reuse(10, slot_b));
    let mut s_tmp = CompactStyle::rune_base(FlexDirection::Row);
    s_tmp.set_width(length(300.0));
    s_tmp.set_height(length(40.0));
    let m_c = comp.begin_children();
    comp.sync_node(10, 0, s_tmp.clone(), None, false, m_c, false);
    comp.finish_flatten();
    let m_b = base.begin_children();
    base.sync_node(10, 0, s_tmp, None, false, m_b, false);
    base.finish_flatten();

    // Commit 2, Pass 2 (retry without apply): `can_reuse` must reject `slot` because Pass 1 mutated it!
    comp.begin_commit();
    base.begin_commit();
    assert!(!comp.can_reuse(10, slot_c), "mutated slot must not be reused across intra-commit retry");
    assert!(!base.can_reuse(10, slot_b), "baseline store must match retry mutation guard");

    // Normal flatten restores `s1` and applies.
    let m_c = comp.begin_children();
    comp.sync_node(10, 0, s1.clone(), None, false, m_c, false);
    comp.finish_flatten();
    comp.compact_if_sparse(|_, _| {});
    let m_b = base.begin_children();
    base.sync_node(10, 0, s1, None, false, m_b, false);
    base.finish_flatten();
    base.compact_if_sparse(|_, _| {});

    // Commit 3: clean slot is reusable again and can rekey cleanly to key 20.
    comp.begin_commit();
    base.begin_commit();
    assert!(comp.can_reuse(10, slot_c));
    assert!(base.can_reuse(10, slot_b));
    comp.rekey_and_reuse_batch(core::iter::once((10, 20, slot_c, 3)));
    base.rekey_and_reuse_batch(core::iter::once((10, 20, slot_b, 3)));
    assert_eq!(comp.mounted_node_index(20), Some(3));
    assert_eq!(base.mounted_node_index(20), Some(3));
    assert_eq!(comp.mounted_node_index(10), None);
    assert_eq!(base.mounted_node_index(10), None);
}
