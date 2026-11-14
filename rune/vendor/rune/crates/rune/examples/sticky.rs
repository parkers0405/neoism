//! Sticky section headers in a timeline (`El::sticky_top`, `El::stuck_shadow`,
//! `Tx::keep_on_screen`).
//!
//! Each tool group's header holds to the top of the scroll viewport while its group scrolls
//! under it, draws a hairline + soft shadow while stuck, and is pushed up and out by its
//! group's bottom edge when the next group arrives. Clicking a header collapses or expands
//! its group with `Tx::keep_on_screen`, keeping the header fixed under the pointer.
//!
//! Headless trace + motion audit, and the still in `demo/sticky-headers.png`:
//!   cargo run -p rune --example sticky -- --script crates/rune/examples/sticky.json --trace sticky.jsonl --trace-all
//!   cargo run -p rune --example sticky -- --script crates/rune/examples/sticky.json --at 0.65 --snap demo/sticky-headers.png
//!
//! Scroll frame benchmark (50 sticky sections, `--profile perf`):
//!   cargo run --profile perf -p rune --example sticky -- --bench
use std::time::Instant;

use rune::tok::{self, type_};
use rune::{
    AppSpec, Color, Cx, El, Env, Runtime, ScrollSpec, Shadow, TextStyle, Tx, col, oklch, row, text,
};

struct Group {
    title: &'static str,
    badge: &'static str,
    steps: &'static [(&'static str, &'static str)],
}

const GROUPS: &[Group] = &[
    Group {
        title: "Inspect layout & scroll viewport",
        badge: "5 calls · 120 ms",
        steps: &[
            ("read", "crates/rune/src/el.rs (lines 2560–2620)"),
            ("read", "crates/rune/src/runtime.rs (lines 4690–4835)"),
            ("grep", "update_layer_paint_order in crates/rune/src"),
            ("read", "crates/rune/src/scroll_offset.rs (lines 1–96)"),
            ("read", "crates/rune/src/layout_read.rs (lines 1–65)"),
        ],
    },
    Group {
        title: "Verify sticky header clamping & push-out",
        badge: "5 calls · 210 ms",
        steps: &[
            ("edit", "crates/rune/src/sticky.rs — shift & keep_on_screen"),
            ("edit", "crates/rune/src/runtime.rs — paint pass sticky dy"),
            ("test", "sticky_top_and_bottom_hold_push_out_hit_above_siblings"),
            ("test", "nested_scroll_viewports_stick_each_header"),
            ("test", "virtual_rows_keep_stuck_header_on_screen"),
        ],
    },
    Group {
        title: "Audit 120 Hz scroll trace & frame cost",
        badge: "4 calls · 85 ms",
        steps: &[
            ("trace", "scripts/motion-audit/analyze.py (0 findings)"),
            ("bench", "50 sticky sections scroll frame CPU"),
            ("snap", "demo/sticky-headers.png"),
            ("check", "scripts/check-reference.sh --gpu"),
        ],
    },
    Group {
        title: "Finalize docs & changelog",
        badge: "3 calls · 40 ms",
        steps: &[
            ("edit", "crates/rune/README.md — sticky headers section"),
            ("edit", "CHANGELOG.md — ## dev entry"),
            ("test", "cargo test -p rune -p rune-kit"),
        ],
    },
];

struct State {
    collapsed: [bool; 4],
    stuck_px: [f32; 4],
}

impl Default for State {
    fn default() -> Self {
        Self { collapsed: [false; 4], stuck_px: [0.0; 4] }
    }
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let theme = tok::theme(true);
    let bg = oklch(0.16, 0.012, 265.0);
    let card_bg = oklch(0.20, 0.014, 265.0);
    let hdr_bg = oklch(0.23, 0.018, 265.0);
    let accent = oklch(0.72, 0.14, 250.0);
    let border = Color::WHITE.alpha(0.08);
    let divider = Shadow { y: 1.0, color: Color::WHITE.alpha(0.11), ..Default::default() };
    let drop = Shadow { y: 6.0, blur: 14.0, color: Color::BLACK.alpha(0.45), ..Default::default() };

    let mut timeline = col().key("timeline").w_full().py(8.0).gap(10.0);
    for (i, g) in GROUPS.iter().enumerate() {
        let collapsed = s.collapsed[i];
        let hdr_key = format!("hdr-{i}");
        let keep_key = hdr_key.clone();
        let chevron = if collapsed { "▸" } else { "▾" };
        let header = row()
            .key(hdr_key)
            .w_full()
            .h(38.0)
            .px(16.0)
            .gap(8.0)
            .center_y()
            .fill(hdr_bg)
            .sticky_top(0.0)
            .stuck_shadow(divider)
            .stuck_shadow(drop)
            .on_drawn_layout(move |st: &mut State, r| st.stuck_px[i] = r[1])
            .on_press_tx(move |st: &mut State, tx: &mut Tx| {
                st.collapsed[i] = !st.collapsed[i];
                tx.keep_on_screen(&keep_key);
            })
            .child(text(chevron).font(TextStyle::new(13.0, 600.0)).color(accent))
            .child(text(g.title).font(TextStyle::new(13.0, 600.0)).color(theme.text).grow(1.0))
            .child(text(g.badge).font(type_::LABEL).color(theme.text_secondary));

        let mut sec = col()
            .key(format!("sec-{i}"))
            .w_full()
            .pb(6.0)
            .child(header);

        if !collapsed {
            let mut body = col().key(format!("body-{i}")).w_full().px(16.0).pt(10.0).gap(8.0);
            for (k, (kind, detail)) in g.steps.iter().enumerate() {
                body = body.child(
                    row()
                        .key(format!("step-{i}-{k}"))
                        .w_full()
                        .h(34.0)
                        .px(12.0)
                        .gap(10.0)
                        .center_y()
                        .radius(8.0)
                        .fill(card_bg)
                        .border(1.0, border)
                        .child(
                            text(*kind)
                                .font(TextStyle::new(11.0, 600.0).mono())
                                .color(accent),
                        )
                        .child(
                            text(*detail)
                                .font(TextStyle::new(12.0, 400.0).mono())
                                .color(theme.text_secondary),
                        ),
                );
            }
            sec = sec.child(body);
        }
        timeline = timeline.child(sec);
    }

    col()
        .w_full()
        .h_full()
        .fill(bg)
        .child(
            col()
                .key("scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(timeline),
        )
}

fn bench_view(sticky: &bool, _: &mut Cx<bool>) -> El {
    let divider = Shadow { y: 1.0, color: Color::WHITE.alpha(0.11), ..Default::default() };
    let mut list = col().key("list").w_full();
    for i in 0..50 {
        let mut hdr = row()
            .key(format!("hdr-{i}"))
            .w_full()
            .h(32.0)
            .px(12.0)
            .center_y()
            .fill(oklch(0.23, 0.018, 265.0))
            .child(text(format!("Tool group {i:02} · 6 steps")).font(TextStyle::new(13.0, 600.0)));
        if *sticky {
            hdr = hdr.sticky_top(0.0).stuck_shadow(divider);
        }
        let mut sec = col().key(format!("sec-{i}")).w_full().pb(8.0).child(hdr);
        for r in 0..6 {
            sec = sec.child(
                row()
                    .key(format!("row-{i}-{r}"))
                    .w_full()
                    .h(28.0)
                    .px(12.0)
                    .center_y()
                    .child(text(format!("step {r}: crates/rune/src/runtime.rs")).font(TextStyle::new(12.0, 400.0))),
            );
        }
        list = list.child(sec);
    }
    col()
        .w_full()
        .h_full()
        .child(col().key("scroll").w_full().h_full().scroll(ScrollSpec { stick_bottom: false }).child(list))
}

fn run_bench() {
    for (label, is_sticky) in [("plain_50_sections", false), ("sticky_50_sections", true)] {
        let mut rt = Runtime::new(is_sticky, bench_view, (640.0, 480.0), Env::new(true, 2.0));
        rt.frame(0.0);
        // Warm up across the full scroll range.
        for _ in 0..60 {
            rt.wheel(320.0, 240.0, -40.0);
            rt.frame(1.0 / 120.0);
        }
        rt.update(|_, tx| tx.scroll_to_y("scroll", 0.0));
        rt.frame(1.0 / 120.0);

        let mut us = Vec::with_capacity(240);
        for step in 0..240 {
            let dy = if (step / 120) % 2 == 0 { -35.0 } else { 35.0 };
            let t0 = Instant::now();
            rt.wheel(320.0, 240.0, dy);
            rt.frame(1.0 / 120.0);
            us.push(t0.elapsed().as_secs_f64() * 1e6);
        }
        us.sort_by(|a, b| a.total_cmp(b));
        let median = us[us.len() / 2];
        let p95 = us[(us.len() * 95) / 100];
        println!("BENCH case={label} frames={} median_us={median:.1} p95_us={p95:.1}", us.len());
    }
}

fn main() {
    if std::env::args().any(|a| a == "--bench") {
        run_bench();
        return;
    }
    rune::run(AppSpec::new("Sticky headers", (560.0, 420.0), State::default, view));
}
