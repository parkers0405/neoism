//! Byte-for-byte equivalence tests between `memo`-reusing builds (across Stages a, b and c) and
//! fresh un-memoized builds across exiting nodes, focus, hover, selection, scroll, layout reads,
//! env/theme/blend/scale/accent/reduced-motion changes, live values, keys, `virtual_rows` inside
//! and outside memos, parent-path rebasing, and hot-reload cache clears.

use std::borrow::Cow;

use rune_motion::Transition;

use crate::render::Instance;
use crate::text::Span;
use crate::{
    Color, Cx, El, Env, GrowthSpec, Ime, Key, KeyEvent, Live, LiveText, Mods, Pixels, Runtime,
    ScrollSpec, SystemAccent, TextEdit, TextStyle, col, el, memo, rich, row, text, text_input,
};

fn maybe_memo(use_memo: bool, id: impl Into<String>, ver: impl std::hash::Hash, build: impl FnOnce() -> El) -> El {
    let key = id.into();
    if use_memo {
        memo(key, ver, build)
    } else {
        build().key(key)
    }
}

fn assert_same_state<S: 'static>(memo_rt: &Runtime<S>, fresh_rt: &Runtime<S>, step: &str) {
    assert_eq!(memo_rt.nodes.len(), fresh_rt.nodes.len(), "{step}: node count");
    for (idx, (a, b)) in memo_rt.nodes.iter().zip(&fresh_rt.nodes).enumerate() {
        assert_eq!(a.id, b.id, "{step}: node[{idx}] ({}) id", a.path);
        assert_eq!(a.path, b.path, "{step}: node[{idx}] path");
        assert_eq!(a.key, b.key, "{step}: node[{idx}] key");
        assert_eq!(a.role, b.role, "{step}: node[{idx}] role");
        assert_eq!(a.label, b.label, "{step}: node[{idx}] label");
        assert_eq!(a.parent, b.parent, "{step}: node[{idx}] parent");
        assert_eq!(a.local, b.local, "{step}: node[{idx}] local ({})", a.path);
        assert_eq!(a.target, b.target, "{step}: node[{idx}] target ({})", a.path);
        assert_eq!(a.inset, b.inset, "{step}: node[{idx}] inset ({})", a.path);
        assert_eq!(a.inert, b.inert, "{step}: node[{idx}] inert ({})", a.path);
        assert_eq!(a.scale_track, b.scale_track, "{step}: node[{idx}] scale_track ({})", a.path);
        assert_eq!(a.group_fade, b.group_fade, "{step}: node[{idx}] group_fade ({})", a.path);
        assert_eq!(a.elided, b.elided, "{step}: node[{idx}] elided ({})", a.path);
        assert_eq!(a.vis.opacity.to_bits(), b.vis.opacity.to_bits(), "{step}: node[{idx}] opacity ({})", a.path);
        assert_eq!(a.vis.paint_alpha.to_bits(), b.vis.paint_alpha.to_bits(), "{step}: node[{idx}] paint_alpha ({})", a.path);
        assert_eq!(a.vis.fill, b.vis.fill, "{step}: node[{idx}] fill ({})", a.path);
        assert_eq!(a.vis.radii(), b.vis.radii(), "{step}: node[{idx}] radius ({})", a.path);
        assert_eq!(
            a.text.as_ref().map(|t| (&t.content, t.color, t.shape)),
            b.text.as_ref().map(|t| (&t.content, t.color, t.shape)),
            "{step}: node[{idx}] text ({})",
            a.path,
        );
    }

    assert_eq!(memo_rt.hits, fresh_rt.hits, "{step}: hit rects");

    let (ea, eb) = (memo_rt.elements(), fresh_rt.elements());
    assert_eq!(ea.len(), eb.len(), "{step}: ElementInfo count");
    for (i, (a, b)) in ea.iter().zip(&eb).enumerate() {
        assert_eq!(
            (&a.path, &a.key, a.exiting, a.focused, a.focus_visible, a.inert, &a.text, &a.selected, a.caret),
            (&b.path, &b.key, b.exiting, b.focused, b.focus_visible, b.inert, &b.text, &b.selected, b.caret),
            "{step}: ElementInfo[{i}] identity/state",
        );
        assert_eq!(
            (a.rect, a.target, a.visible, a.opacity.to_bits(), a.scale.to_bits(), a.hover.to_bits(), a.press.to_bits(), a.moving),
            (b.rect, b.target, b.visible, b.opacity.to_bits(), b.scale.to_bits(), b.hover.to_bits(), b.press.to_bits(), b.moving),
            "{step}: ElementInfo[{i}] ({}) geometry/motion",
            a.path,
        );
    }

    assert_eq!(memo_rt.instances.len(), fresh_rt.instances.len(), "{step}: instance count");
    let bytes_a: &[u8] = bytemuck::cast_slice::<Instance, u8>(&memo_rt.instances);
    let bytes_b: &[u8] = bytemuck::cast_slice::<Instance, u8>(&fresh_rt.instances);
    if bytes_a != bytes_b {
        for (k, (ia, ib)) in memo_rt.instances.iter().zip(&fresh_rt.instances).enumerate() {
            assert_eq!(
                bytemuck::bytes_of(ia),
                bytemuck::bytes_of(ib),
                "{step}: instance[{k}] differs:\n  memo:  {ia:?}\n  fresh: {ib:?}",
            );
        }
    }
}

fn step_both<S: 'static>(
    a: &mut Runtime<S>,
    b: &mut Runtime<S>,
    dt: f64,
    label: &str,
    mut f: impl FnMut(&mut Runtime<S>),
) {
    f(a);
    f(b);
    let anim_a = a.frame(dt);
    let anim_b = b.frame(dt);
    assert_eq!(anim_a, anim_b, "{label}: animating mismatch");
    assert_same_state(a, b, label);
}

#[derive(Clone)]
struct CardsState {
    use_memo: bool,
    banner_h: f32,
    tick: u32,
    card_ver: [u32; 4],
    show_card: [bool; 4],
    swap_order: bool,
    nest_in_right: bool,
    dim_first: bool,
}

fn card_body(i: usize, ver: u32, use_memo: bool) -> El {
    let title = format!("Card {i} v{ver}");
    let body = format!("Detailed body copy for card {i} at version {ver} with wrapping words.");
    col()
        .w_full()
        .padding(10.0)
        .gap(6.0)
        .radius(8.0)
        .fill(Color { r: 0.12, g: 0.14, b: 0.18, a: 1.0 })
        .border(1.0, Color::WHITE.alpha(0.15))
        .hover(Color::WHITE.alpha(0.06))
        .press(Color::WHITE.alpha(0.12))
        .press_scale(0.97)
        .hover_group()
        .focusable(true)
        .on_press(move |s: &mut CardsState| s.tick += (i as u32) + 1)
        .transition(Transition::opacity())
        .children([
            row().key("head").w_full().gap(8.0).center_y().children([
                el().key("badge").size(10.0, 10.0).radius(5.0).fill(Color { r: 0.3, g: 0.7, b: 0.9, a: 1.0 }).group_opacity(0.6, 1.0),
                text(title).key("title").font(TextStyle::new(14.0, 600.0)).color(Color::WHITE).grow(1.0).shrink().truncate(),
                maybe_memo(use_memo, "nested-pill", ver, || {
                    let overlay = crate::system_scroll_bars().is_overlay(crate::system_scroller_overlay());
                    row().padding(if overlay { 4.0 } else { 6.0 }).radius(4.0).fill(Color::WHITE.alpha(0.08))
                        .child(text(format!("#{ver}")).key("v").font(TextStyle::new(11.0, 500.0)))
                }),
            ]),
            rich(body, vec![Span { start: 0, end: 8, weight: Some(700.0), color: Some(Color { r: 0.9, g: 0.8, b: 0.4, a: 1.0 }), ..Default::default() }])
                .key("desc")
                .w_full()
                .selectable(true),
        ])
        .children((ver % 2 == 0).then(|| {
            text("Optional footer note").key("footer").font(TextStyle::new(11.0, 400.0)).color(Color::WHITE.alpha(0.6)).transition(Transition::opacity())
        }))
}

fn cards_view(s: &CardsState, cx: &mut Cx<CardsState>) -> El {
    let m = s.use_memo;
    let indices: Vec<usize> = if s.swap_order { vec![2, 1, 0, 3] } else { vec![0, 1, 2, 3] };
    let mut left_kids = Vec::new();
    let mut right_kids = Vec::new();
    for i in indices {
        if !s.show_card[i] {
            continue;
        }
        let ver = s.card_ver[i];
        let mut c = maybe_memo(m, format!("card-{i}"), ver, || card_body(i, ver, m));
        if i == 0 && s.dim_first {
            c = c.opacity(0.45);
        }
        if i == 1 && s.nest_in_right {
            right_kids.push(c);
        } else {
            left_kids.push(c);
        }
    }
    if !s.nest_in_right {
        // Occupy `#0/columns/right/card-1` before `card-1` rebases onto that exact path.
        right_kids.push(el().key("card-1").w_full().h(18.0).fill(Color::WHITE.alpha(0.05)));
    }
    let accent = cx.accent();
    col().w_full().h_full().padding(12.0).gap(8.0).selection_group(true).children([
        el().key("banner").w_full().h(s.banner_h).radius(6.0).fill(accent),
        text(format!("tick {}", s.tick)).key("tick").font(TextStyle::new(12.0, 400.0)),
        row().key("columns").w_full().grow(1.0).min_h(0.0).gap(12.0).children([
            col().key("left").grow(1.0).shrink().gap(8.0).children(left_kids),
            col().key("right").w(180.0).gap(8.0).children(right_kids),
        ]),
    ])
}

#[test]
fn steady_commits_moving_ancestors_rebase_exits_and_modifiers_match_fresh_build() {
    let init = |use_memo| CardsState {
        use_memo,
        banner_h: 24.0,
        tick: 0,
        card_ver: [0, 0, 0, 0],
        show_card: [true; 4],
        swap_order: false,
        nest_in_right: false,
        dim_first: false,
    };
    let mut rt_m = Runtime::new(init(true), cards_view, (560.0, 520.0), Env::new(true, 2.0));
    let mut rt_f = Runtime::new(init(false), cards_view, (560.0, 520.0), Env::new(true, 2.0));
    step_both(&mut rt_m, &mut rt_f, 0.0, "initial", |_| {});
    let first_builds = rt_m.memo.builds;
    assert!(first_builds >= 4);

    // Steady commits: nothing moves, Stage (b) + Stage (c) skip flatten and apply on all cards.
    for k in 0..5 {
        step_both(&mut rt_m, &mut rt_f, 1.0 / 120.0, &format!("steady-{k}"), |rt| {
            rt.update(|s, _| s.tick += 1);
        });
    }
    assert_eq!(rt_m.memo.builds, first_builds, "no memo rebuilt during steady ticks");

    // Ancestor move: banner expands, shifting every card's window rect while keeping interior local rects.
    step_both(&mut rt_m, &mut rt_f, 1.0 / 120.0, "banner-grow", |rt| {
        rt.update(|s, _| s.banner_h = 68.0);
    });
    for f in 0..30 {
        step_both(&mut rt_m, &mut rt_f, 1.0 / 120.0, &format!("banner-spring-{f}"), |rt| {
            if f == 5 {
                rt.update(|s, _| s.tick += 1);
            }
        });
    }

    // Sibling reorder + parent-path rebase onto a path previously occupied by another node.
    step_both(&mut rt_m, &mut rt_f, 1.0 / 120.0, "reorder-and-rebase", |rt| {
        rt.update(|s, _| {
            s.swap_order = true;
            s.nest_in_right = true;
        });
    });
    for f in 0..25 {
        step_both(&mut rt_m, &mut rt_f, 1.0 / 120.0, &format!("rebase-spring-{f}"), |_| {});
    }

    // Caller root modifier toggled outside `memo(...)` without changing `ver`.
    step_both(&mut rt_m, &mut rt_f, 1.0 / 120.0, "root-modifier-on", |rt| {
        rt.update(|s, _| s.dim_first = true);
    });
    for f in 0..15 {
        step_both(&mut rt_m, &mut rt_f, 1.0 / 120.0, &format!("dim-spring-{f}"), |_| {});
    }
    step_both(&mut rt_m, &mut rt_f, 1.0 / 120.0, "root-modifier-off", |rt| {
        rt.update(|s, _| s.dim_first = false);
    });

    // Interior child exit (bumping card_ver[2] to odd drops its footer) + whole-card exit + prune + revive.
    step_both(&mut rt_m, &mut rt_f, 1.0 / 120.0, "exits-begin", |rt| {
        rt.update(|s, _| {
            s.card_ver[2] += 1;
            s.show_card[1] = false;
        });
    });
    for f in 0..45 {
        step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, &format!("exits-step-{f}"), |rt| {
            if f == 4 {
                rt.update(|s, _| s.tick += 1);
            }
        });
    }
    // Commits after `prune_exited` compacted `self.nodes`.
    for k in 0..3 {
        step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, &format!("after-prune-{k}"), |rt| {
            rt.update(|s, _| s.tick += 1);
        });
    }
    // Revive during an exit.
    step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, "remove-card-3", |rt| {
        rt.update(|s, _| s.show_card[3] = false);
    });
    step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, "mid-exit", |_| {});
    step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, "revive-card-3", |rt| {
        rt.update(|s, _| s.show_card[3] = true);
    });
    for f in 0..20 {
        step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, &format!("revive-settle-{f}"), |_| {});
    }
}

#[test]
fn hover_press_focus_selection_env_blend_and_hot_clear_match_fresh_build() {
    let init = |use_memo| CardsState {
        use_memo,
        banner_h: 24.0,
        tick: 0,
        card_ver: [0, 0, 0, 0],
        show_card: [true; 4],
        swap_order: false,
        nest_in_right: false,
        dim_first: false,
    };
    let mut rt_m = Runtime::new(init(true), cards_view, (560.0, 520.0), Env::new(true, 1.0));
    let mut rt_f = Runtime::new(init(false), cards_view, (560.0, 520.0), Env::new(true, 1.0));
    step_both(&mut rt_m, &mut rt_f, 0.0, "init", |_| {});

    // Hover + group_opacity + press + press_scale while commits fire.
    let (cx, cy) = rt_m.find("card-0").unwrap();
    step_both(&mut rt_m, &mut rt_f, 1.0 / 120.0, "hover-enter", |rt| rt.pointer_move(cx, cy));
    for f in 0..8 {
        step_both(&mut rt_m, &mut rt_f, 1.0 / 120.0, &format!("hover-spring-{f}"), |rt| {
            if f == 3 {
                rt.update(|s, _| s.tick += 1);
            }
        });
    }
    step_both(&mut rt_m, &mut rt_f, 1.0 / 120.0, "press-down", |rt| rt.pointer_down(cx, cy));
    step_both(&mut rt_m, &mut rt_f, 1.0 / 120.0, "press-held-commit", |rt| rt.update(|s, _| s.tick += 1));
    step_both(&mut rt_m, &mut rt_f, 1.0 / 120.0, "press-up", |rt| rt.pointer_up(cx, cy));
    assert_eq!(rt_m.state.tick, rt_f.state.tick);

    // Keyboard Tab focus across focusable memo cards.
    for t in 0..3 {
        step_both(&mut rt_m, &mut rt_f, 1.0 / 120.0, &format!("tab-{t}"), |rt| {
            rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
        });
    }

    // Programmatic text selection across memoized selectable paragraphs + unrelated commit.
    step_both(&mut rt_m, &mut rt_f, 1.0 / 120.0, "select-all", |rt| {
        rt.update(|_, tx| tx.select_all_text("columns"));
    });
    step_both(&mut rt_m, &mut rt_f, 1.0 / 120.0, "selection-commit", |rt| {
        rt.update(|s, _| s.tick += 1);
    });

    // Env, dark mode, scale, system accent, sRGB blend, reduced motion, and hot-reload cache clear.
    step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, "dark-false", |rt| rt.set_system_dark(false));
    step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, "scale-2x", |rt| rt.set_viewport((560.0, 520.0), 2.0));
    step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, "srgb-on", |rt| rt.set_srgb_blend(true));
    step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, "srgb-off", |rt| rt.set_srgb_blend(false));
    step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, "accent", |rt| {
        let c = Color { r: 0.9, g: 0.3, b: 0.4, a: 1.0 };
        rt.set_system_accent(SystemAccent::new(c, c));
    });
    step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, "scroll-bars-always", |rt| {
        rt.set_system_scroll_bars(crate::ScrollBars::Always);
    });
    step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, "scroll-bars-auto-pad", |rt| {
        rt.set_system_scroll_bars(crate::ScrollBars::Automatic);
        rt.set_system_scroller_overlay(true);
    });
    step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, "scroll-bars-auto-mouse", |rt| {
        rt.set_system_scroller_overlay(false);
    });
    step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, "reduced-motion", |rt| rt.set_system_reduced_motion(true));

    let before_hot = rt_m.memo.builds;
    rt_m.memo.clear();
    step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, "hot-clear-rebuild", |rt| rt.update(|s, _| s.tick += 1));
    assert!(rt_m.memo.builds > before_hot, "hot clear rebuilds memos cleanly");
}

struct WorkspaceState {
    use_memo: bool,
    probe_w: f32,
    panel_ver: u32,
    rows: Vec<(String, u32)>,
    composer: TextEdit,
    live_opacity: Live<f32>,
    live_label: LiveText,
    pixels: Pixels,
    dup_keys: bool,
}

fn workspace_view(s: &WorkspaceState, cx: &mut Cx<WorkspaceState>) -> El {
    let m = s.use_memo;
    let probe_sz = cx.layout_size("probe").unwrap_or([0.0, 0.0]);
    let inner_sz = cx.layout_size("live-card").unwrap_or([0.0, 0.0]);

    // A `virtual_rows` list nested INSIDE an outer `memo`, whose rows are ALSO `memo`s.
    let panel = maybe_memo(m, "outer-panel", s.panel_ver, || {
        col().w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(
            col().key("vrows").w_full().gap(4.0).growth(GrowthSpec::default()).virtual_rows(
                s.rows.len(),
                36.0,
                |_: &WorkspaceState, i| Cow::Owned(format!("vr-{i}")),
                |s: &WorkspaceState, i| {
                    let (label, ver) = &s.rows[i];
                    let label = label.clone();
                    maybe_memo(s.use_memo, format!("vr-{i}"), *ver, move || {
                        row().w_full().h(36.0).px(8.0).gap(6.0).center_y()
                            .fill(Color::WHITE.alpha(0.05))
                            .children([
                                el().key("dot").size(6.0, 6.0).radius(3.0).fill(Color::WHITE),
                                text(label).key("txt").font(TextStyle::new(13.0, 400.0)).grow(1.0).shrink().truncate(),
                            ])
                    })
                },
            ),
        )
    });

    let live_op = s.live_opacity.clone();
    let live_txt = s.live_label.clone();
    let px = s.pixels.clone();
    let dup = s.dup_keys;
    let live_card = maybe_memo(m, "live-card", dup, move || {
        row().w_full().h(30.0).gap(8.0).center_y()
            .live_opacity(&live_op)
            .children([
                el().key("avatar").size(16.0, 16.0).pixels(&px),
                text("init").key("clock").font(TextStyle::new(12.0, 400.0)).live_text(&live_txt),
                el().key("dup").size(8.0, 8.0),
                el().key(if dup { "dup" } else { "unique" }).size(8.0, 8.0),
            ])
    });

    let size_reader = maybe_memo(m, "size-reader", 0u32, move || {
        let w = crate::memo::layout_size("probe").map_or(-1.0, |r| r[0]);
        text(format!("probe={w:.0}")).key("probe-txt").font(TextStyle::new(12.0, 400.0))
    });

    col().w_full().h_full().padding(8.0).gap(6.0).children([
        el().key("probe").w(s.probe_w).h(10.0).fill(Color::WHITE.alpha(0.2)),
        text(format!("seen {:?} {:?}", probe_sz, inner_sz)).key("summary").font(TextStyle::new(11.0, 400.0)),
        size_reader,
        live_card,
        panel,
        text_input(&s.composer, TextStyle::new(13.0, 400.0))
            .key("composer")
            .w_full()
            .h(32.0)
            .on_edit(|s: &mut WorkspaceState| &mut s.composer),
    ])
}

#[test]
fn virtual_rows_inside_memos_layout_reads_live_values_and_duplicate_keys_match_fresh_build() {
    let make = |use_memo| {
        let live_opacity = Live::new(0.85);
        let live_label = LiveText::new("00:01");
        let pixels = Pixels::new();
        pixels.set(4, 4, vec![200u8; 64]);
        let mut composer = TextEdit::new("hello ");
        composer.focused = true;
        composer.set_caret(composer.text.len());
        WorkspaceState {
            use_memo,
            probe_w: 120.0,
            panel_ver: 0,
            rows: (0..80).map(|i| (format!("Row item {i}"), 0)).collect(),
            composer,
            live_opacity,
            live_label,
            pixels,
            dup_keys: false,
        }
    };
    let mut rt_m = Runtime::new(make(true), workspace_view, (420.0, 380.0), Env::new(true, 2.0));
    let mut rt_f = Runtime::new(make(false), workspace_view, (420.0, 380.0), Env::new(true, 2.0));
    rt_m.set_live_visible(true);
    rt_f.set_live_visible(true);
    rt_m.key_warnings = Some(Vec::new());
    rt_f.key_warnings = Some(Vec::new());

    step_both(&mut rt_m, &mut rt_f, 0.0, "init", |_| {});
    for f in 0..8 {
        step_both(&mut rt_m, &mut rt_f, 1.0 / 120.0, &format!("overscan-fill-{f}"), |_| {});
    }

    // Typing into the sibling composer while outer-panel and virtual rows stay memoized.
    for ch in ["a", "b", "c"] {
        step_both(&mut rt_m, &mut rt_f, 1.0 / 120.0, &format!("type-{ch}"), |rt| {
            rt.ime(Ime::Commit(ch.into()));
        });
    }

    // Scrolling the `virtual_rows` list INSIDE `memo("outer-panel")` without changing `panel_ver`.
    for s in 0..12 {
        step_both(&mut rt_m, &mut rt_f, 1.0 / 120.0, &format!("scroll-down-{s}"), |rt| {
            rt.wheel(200.0, 220.0, -140.0);
        });
    }
    // Mutating one mounted row's version inside the unchanged outer memo, then scrolling back up.
    step_both(&mut rt_m, &mut rt_f, 1.0 / 120.0, "bump-row", |rt| {
        rt.update(|s, _| {
            for (lbl, ver) in &mut s.rows {
                *lbl = format!("{lbl}!");
                *ver += 1;
            }
        });
    });
    for s in 0..12 {
        step_both(&mut rt_m, &mut rt_f, 1.0 / 120.0, &format!("scroll-up-{s}"), |rt| {
            rt.wheel(200.0, 220.0, 140.0);
        });
    }

    // Layout size read invalidates `size-reader` when `probe_w` changes, and reuses when unchanged.
    step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, "resize-probe", |rt| {
        rt.update(|s, _| s.probe_w = 210.0);
    });
    for f in 0..15 {
        step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, &format!("probe-spring-{f}"), |_| {});
    }

    // Live values (`Live`, `LiveText`, `Pixels`) update across frames and across unrelated commits.
    step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, "live-update", |rt| {
        rt.state.live_opacity.set(0.5);
        rt.state.live_label.set("00:42");
        rt.state.pixels.set(4, 4, vec![90u8; 64]);
    });
    step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, "commit-after-live", |rt| {
        rt.ime(Ime::Commit("!".into()));
    });

    // Duplicate sibling keys inside a memo repair identically.
    step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, "dup-keys", |rt| {
        rt.update(|s, _| s.dup_keys = true);
    });
    assert_eq!(rt_m.key_warnings, rt_f.key_warnings, "duplicate-key warnings match");
}

#[test]
fn self_prefix_rebase_box_into_box_box_and_back_matches_fresh_build() {
    fn view(s: &(bool, bool), _: &mut Cx<(bool, bool)>) -> El {
        let leaf = maybe_memo(s.0, "box", 0u32, || {
            col().w(80.0).h(30.0).fill(Color::WHITE).child(text("inner").key("box"))
        });
        col().size(200.0, 200.0).child(if s.1 {
            col().key("box").w(120.0).h(60.0).child(leaf)
        } else {
            leaf
        })
    }
    let mut rt_m = Runtime::new((true, false), view, (200.0, 200.0), Env::new(true, 1.0));
    let mut rt_f = Runtime::new((false, false), view, (200.0, 200.0), Env::new(true, 1.0));
    step_both(&mut rt_m, &mut rt_f, 0.0, "flat", |_| {});
    step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, "into-wrapper", |rt| rt.update(|s, _| s.1 = true));
    for f in 0..20 {
        step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, &format!("wrap-settle-{f}"), |_| {});
    }
    step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, "out-of-wrapper", |rt| rt.update(|s, _| s.1 = false));
    for f in 0..20 {
        step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, &format!("unwrap-settle-{f}"), |_| {});
    }
}

struct MoveHarness {
    use_memo: bool,
    tick: u32,
    theme_ver: u32,
    header_h: f32,
    show_modal: bool,
    show_banner_chip: bool,
    show_mid_memo: bool,
    list_ver: u32,
    vrows_len: usize,
    editor: TextEdit,
    modal_input: TextEdit,
}

fn move_harness_view(s: &MoveHarness, cx: &mut Cx<MoveHarness>) -> El {
    let m = s.use_memo;
    let dark = cx.env().system_dark;
    let hdr_rect = cx.layout("dyn-hdr").unwrap_or([0.0; 4]);
    let caret = cx.caret("editor").unwrap_or([0.0; 4]);
    let card_bg = if dark && s.theme_ver % 2 == 0 {
        Color { r: 0.13, g: 0.15, b: 0.20, a: 1.0 }
    } else {
        Color { r: 0.88, g: 0.89, b: 0.93, a: 1.0 }
    };
    let ink = if dark && s.theme_ver % 2 == 0 { Color::WHITE } else { Color { r: 0.1, g: 0.12, b: 0.16, a: 1.0 } };

    // Styled memo cards with shadows, gradients, radial stops, outlines and selectable text,
    // exercising `apply`'s paint-track `prev_node` path whenever `skip_spans` is bypassed.
    let styled_card = |idx: usize| {
        maybe_memo(m, format!("styled-{idx}"), s.theme_ver, move || {
            col()
                .w_full()
                .padding(8.0)
                .gap(4.0)
                .radius(6.0)
                .fill(card_bg)
                .border(1.0, ink.alpha(0.2))
                .shadow(2.0, 6.0, Color::BLACK.alpha(0.25))
                .gradient(card_bg, card_bg.alpha(0.85))
                .radial_gradient([0.5, 0.5], 40.0, [(0.0, ink.alpha(0.08)), (1.0, Color::TRANSPARENT)])
                .outline(1.0, 1.0, ink.alpha(0.15))
                .hover(ink.alpha(0.06))
                .press(ink.alpha(0.12))
                .focusable(true)
                .transition(Transition::opacity())
                .children([
                    text(format!("Styled memo {idx}")).key("h").font(TextStyle::new(13.0, 600.0)).color(ink),
                    text(format!("Selectable line inside moved memo {idx} for drag & copy")).key("p").font(TextStyle::new(12.0, 400.0)).color(ink).selectable(true),
                ])
        })
    };

    // A single `memo` owning a long scrollable list (140 items, >420 nodes moved in one slice).
    let long_static_list = maybe_memo(m, "long-list-memo", (s.list_ver, s.theme_ver), || {
        col()
            .w_full()
            .h(120.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .child(
                col().key("items").w_full().gap(2.0).children((0..140).map(|i| {
                    row()
                        .key(format!("li-{i}"))
                        .w_full()
                        .h(22.0)
                        .px(6.0)
                        .gap(6.0)
                        .center_y()
                        .fill(card_bg.alpha(0.5))
                        .focusable(i % 25 == 0)
                        .children([
                            el().key("b").size(6.0, 6.0).radius(3.0).fill(ink),
                            text(format!("Long memo item #{i}")).key("t").font(TextStyle::new(12.0, 400.0)).color(ink).selectable(true),
                        ])
                })),
            )
    });

    // A long `virtual_rows` list inside a `memo` whose true row height (64 pt) intentionally
    // exceeds its 14 pt estimate so `measure_virtuals` exercises multi-pass retry restoration.
    let long_virtual_memo = maybe_memo(m, "long-vlist-memo", s.vrows_len, || {
        col()
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .scroll(ScrollSpec { stick_bottom: true })
            .child(
                col().key("vstack").w_full().gap(4.0).growth(GrowthSpec::default()).virtual_rows(
                    s.vrows_len,
                    14.0,
                    |_: &MoveHarness, i| Cow::Owned(format!("mv-{i}")),
                    |s: &MoveHarness, i| {
                        maybe_memo(s.use_memo, format!("mv-{i}"), s.theme_ver, move || {
                            col().w_full().h(64.0).padding(6.0).fill(Color::WHITE.alpha(0.04)).children([
                                text(format!("Virtual row {i}")).key("vt").font(TextStyle::new(12.0, 500.0)),
                                text("Second line keeping row taller than estimate").key("vs").font(TextStyle::new(11.0, 400.0)),
                            ])
                        })
                    },
                ),
            )
    });

    let mut body = col()
        .w_full()
        .h_full()
        .padding(10.0)
        .gap(6.0)
        .selection_group(true)
        .child_exit(Transition::opacity())
        .children([
            el().key("dyn-hdr").w_full().h(s.header_h).fill(ink.alpha(0.1)),
            text(format!("t={} hdr={:.0} caret={:.1}", s.tick, hdr_rect[3], caret[0]))
                .key("readout")
                .font(TextStyle::new(11.0, 400.0)),
            styled_card(0),
        ]);
    if s.show_banner_chip {
        body = body.child(
            row().key("leaving-chip").w_full().h(20.0).fill(ink.alpha(0.15)).transition(Transition::opacity())
                .child(text("Transient banner").key("lbl").font(TextStyle::new(11.0, 400.0))),
        );
    }
    if s.show_mid_memo {
        body = body.child(styled_card(1));
    }
    body = body.children([
        long_static_list,
        long_virtual_memo,
        text_input(&s.editor, TextStyle::new(13.0, 400.0))
            .key("editor")
            .w_full()
            .h(30.0)
            .on_edit(|s: &mut MoveHarness| &mut s.editor),
    ]);
    if s.show_modal {
        body = body.child(
            col()
                .key("popover")
                .w_full()
                .padding(6.0)
                .focus_return("styled-0")
                .transition(Transition::opacity())
                .child(
                    text_input(&s.modal_input, TextStyle::new(13.0, 400.0))
                        .key("modal-field")
                        .w_full()
                        .h(28.0)
                        .on_edit(|s: &mut MoveHarness| &mut s.modal_input),
                ),
        );
    }
    body
}

#[test]
fn move_path_preserves_allocations_and_matches_fresh_across_exits_focus_selection_scroll_theme_and_long_lists() {
    let make = |use_memo| {
        let mut editor = TextEdit::new("seed ");
        editor.focused = true;
        editor.set_caret(editor.text.len());
        MoveHarness {
            use_memo,
            tick: 0,
            theme_ver: 0,
            header_h: 18.0,
            show_modal: false,
            show_banner_chip: true,
            show_mid_memo: true,
            list_ver: 0,
            vrows_len: 320,
            editor,
            modal_input: TextEdit::new("modal"),
        }
    };
    let mut rt_m = Runtime::new(make(true), move_harness_view, (500.0, 620.0), Env::new(true, 2.0));
    let mut rt_f = Runtime::new(make(false), move_harness_view, (500.0, 620.0), Env::new(true, 2.0));
    step_both(&mut rt_m, &mut rt_f, 0.0, "init", |_| {});
    for f in 0..8 {
        step_both(&mut rt_m, &mut rt_f, 1.0 / 120.0, &format!("init-fill-{f}"), |_| {});
    }

    // Proof that `same_path` memo reuse moves `Node`s rather than cloning their owned heap strings.
    let path_ptr = |rt: &Runtime<MoveHarness>, path: &str| {
        rt.nodes.iter().find(|n| n.path == path).unwrap().path.as_ptr() as usize
    };
    let p_styled = path_ptr(&rt_m, "#0/styled-0/p");
    let p_long = path_ptr(&rt_m, "#0/long-list-memo/items/li-95/t");

    // Non-committing `content_height()` must leave `rt_m.nodes` intact AND recycle `spare_nodes`.
    assert_eq!(rt_m.content_height(), rt_f.content_height(), "content_height matches");
    assert!(rt_m.spare_nodes.capacity() >= rt_m.nodes.len(), "content_height recycled spare_nodes");
    let (buf_a, buf_b) = (rt_m.nodes.as_ptr() as usize, rt_m.spare_nodes.as_ptr() as usize);

    for k in 0..4 {
        step_both(&mut rt_m, &mut rt_f, 1.0 / 120.0, &format!("steady-move-{k}"), |rt| {
            rt.update(|s, _| s.tick += 1);
        });
        assert_eq!(path_ptr(&rt_m, "#0/styled-0/p"), p_styled, "styled memo path moved without reallocation");
        assert_eq!(path_ptr(&rt_m, "#0/long-list-memo/items/li-95/t"), p_long, "long-list node moved without reallocation");
        let pair = (rt_m.nodes.as_ptr() as usize, rt_m.spare_nodes.as_ptr() as usize);
        assert!(pair == (buf_a, buf_b) || pair == (buf_b, buf_a), "nodes and spare_nodes ping-pong without reallocating");
    }

    // Typing moves `cx.caret("editor")`, triggering `caret_moved` -> `discard_measured` -> re-measure
    // after pass 1 already moved `styled-0`, `styled-1` and `long-list-memo`.
    for ch in ["x", "y", "z"] {
        step_both(&mut rt_m, &mut rt_f, 1.0 / 120.0, &format!("caret-remeasure-{ch}"), |rt| {
            rt.ime(Ime::Commit(ch.into()));
        });
    }
    assert_eq!(path_ptr(&rt_m, "#0/styled-0/p"), p_styled, "caret re-measure restored and re-moved node");

    // Changing `header_h` shifts `cx.layout("dyn-hdr")` AND shifts `styled-0`'s local rect (bypassing
    // `skip_spans` so `apply`'s `prev_node` runs on shadows/gradients/radials/outlines) AND triggers
    // `settle_layout_reads` -> `discard_measured` -> re-measure.
    step_both(&mut rt_m, &mut rt_f, 1.0 / 120.0, "layout-read-remeasure", |rt| {
        rt.update(|s, _| s.header_h = 54.0);
    });
    for f in 0..10 {
        step_both(&mut rt_m, &mut rt_f, 1.0 / 120.0, &format!("hdr-spring-{f}"), |rt| {
            if f == 4 {
                rt.update(|s, tx| {
                    s.tick += 1;
                    tx.settle();
                });
            }
        });
    }

    // Elements leaving: both an un-memoized sibling (`leaving-chip`) and a moved memo (`styled-1`)
    // exit while commits continue, then prune, then `styled-1` revives.
    step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, "elements-leave", |rt| {
        rt.update(|s, _| {
            s.show_banner_chip = false;
            s.show_mid_memo = false;
        });
    });
    for f in 0..25 {
        step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, &format!("leaving-fade-{f}"), |rt| {
            if f == 3 || f == 12 {
                rt.update(|s, _| s.tick += 1);
            }
        });
    }
    step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, "revive-mid-memo", |rt| {
        rt.update(|s, _| s.show_mid_memo = true);
    });

    // Focus request (`sync_requested_focus` -> `discard_measured` -> re-measure) + `focus_return` on close,
    // followed by Tab traversal and text selection across moved memos.
    step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, "open-focused-modal", |rt| {
        rt.update(|s, _| {
            s.show_modal = true;
            s.modal_input.focused = true;
        });
    });
    step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, "close-modal-focus-return", |rt| {
        rt.update(|s, tx| {
            s.show_modal = false;
            s.modal_input.focused = false;
            tx.instant();
        });
    });
    for t in 0..4 {
        step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, &format!("focus-tab-{t}"), |rt| {
            rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
        });
    }
    step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, "select-all-moved", |rt| {
        rt.update(|_, tx| tx.select_all_text(""));
    });
    let (sx, sy) = rt_m.find("styled-0/p").unwrap();
    let (ex, ey) = rt_m.find("styled-1/p").unwrap();
    step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, "drag-sel-down", |rt| rt.pointer_down(sx, sy));
    step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, "drag-sel-move", |rt| rt.pointer_move(ex, ey));
    step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, "drag-sel-up", |rt| rt.pointer_up(ex, ey));
    step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, "commit-with-selection", |rt| rt.update(|s, _| s.tick += 1));

    // Scrolling BOTH the long static list inside `long-list-memo` and the long `virtual_rows` list
    // inside `long-vlist-memo` while commits run.
    let (lx, ly) = rt_m.find("long-list-memo").unwrap();
    for s in 0..8 {
        step_both(&mut rt_m, &mut rt_f, 1.0 / 120.0, &format!("scroll-long-static-{s}"), |rt| {
            rt.wheel(lx, ly, -120.0);
            if s % 2 == 1 {
                rt.update(|st, _| st.tick += 1);
            }
        });
    }
    let (vx, vy) = rt_m.find("long-vlist-memo").unwrap();
    for s in 0..10 {
        step_both(&mut rt_m, &mut rt_f, 1.0 / 120.0, &format!("scroll-long-virtual-{s}"), |rt| {
            rt.wheel(vx, vy, 220.0);
            if s % 3 == 0 {
                rt.update(|st, _| st.tick += 1);
            }
        });
    }

    // Theme changes: both `Env::system_dark` flip and app-versioned theme crossfade across moved memos.
    step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, "system-light", |rt| rt.set_system_dark(false));
    for f in 0..10 {
        step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, &format!("light-crossfade-{f}"), |rt| {
            if f == 2 {
                rt.update(|s, _| s.tick += 1);
            }
        });
    }
    step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, "app-theme-bump", |rt| {
        rt.update(|s, _| s.theme_ver += 1);
    });
    for f in 0..10 {
        step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, &format!("app-theme-fade-{f}"), |_| {});
    }
    step_both(&mut rt_m, &mut rt_f, 1.0 / 60.0, "system-dark-again", |rt| rt.set_system_dark(true));
}
