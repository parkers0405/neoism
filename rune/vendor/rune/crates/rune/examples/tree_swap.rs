//! Tree swap: one frame replaces a big view tree with another, as a launcher does when Escape
//! takes it from an emoji grid back to its root list. Headless, no window.
//!
//! The list is 200 rows of icon, title and subtitle under `virtual_rows`. The grid is 1,800
//! emoji cells, ten to a row, under `virtualize` with every row built, as an app builds it
//! today (`--lazy-grid` builds its rows with `virtual_rows` instead; `--eager-list` builds every
//! list row under `virtualize`, as apps did before `virtual_rows`). `--cells N` sets the grid's
//! cell count; 200 is a launcher's emoji grid, 20 rows of 74.4 points in a 388-point viewport
//! that shows about five. Tab opens the grid and Escape returns to the list. Each direction runs
//! four cases on a fresh runtime per sample:
//! `first`, the target never shown before; `recent`, the target shown, then the source for one
//! commit; `quick`, as `recent` but back one frame after leaving, with both trees still
//! moving; `stale`, the source for four commits, so the target's shaped text has been evicted
//! (what a person who browsed the grid comes back to).
//!
//! cargo build --profile perf -p rune --example tree_swap --features frame-profile
//! target/perf/examples/tree_swap [--samples 15] [--only grid-list] [--lazy-grid] [--eager-list] [--cells 1800]
//!
//! Prints `METRIC dir=.. case=.. metric=.. median=.. p95=..` per measurement. `cpu_ms` is the
//! thread CPU of the key and the frame it commits; `encode_ms` is prepare, draw and submit
//! after it. Phases are thread CPU and overlap: commit includes view, layout and apply, and
//! layout includes text. `tail_ms` is the CPU of the frames after the swap until motion settles.
//! `rows_built` counts row builder calls in the swap frame (an eager list builds every row in
//! its view); `rows_mounted` counts the shown list's rows laid out after it, and
//! `rows_settled` after the frames that follow.
use std::borrow::Cow;
use std::cell::Cell;
use std::time::Instant;

use rune::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
use rune::text::TextSystem;
use rune::{Color, Cx, El, Env, GrowthSpec, Key, KeyEvent, Runtime, ScrollSpec, TextStyle, Tx, col, el, row, text};

#[global_allocator]
static ALLOC: procwatch::alloc::Counting = procwatch::alloc::Counting::system();

const SIZE: (f32, f32) = (760.0, 480.0);
const ROW: f32 = 44.0;
const COLUMNS: usize = 10;
const EMOJI: &[&str] = &[
    "😀", "😃", "😄", "😁", "😆", "😅", "🤣", "😂", "🙂", "🙃", "😉", "😊", "😇", "🥰", "😍", "🤩", "😘", "😗",
    "😚", "😙", "😋", "😛", "😜", "🤪", "😝", "🤑", "🤗", "🤭", "🤫", "🤔", "🤐", "🤨", "😐", "😑", "😶", "😏",
    "😒", "🙄", "😬", "🤥", "😌", "😔", "😪", "🤤", "😴", "😷", "🤒", "🤕", "🤢", "🤮", "🤧", "🥵", "🥶", "🥴",
    "😵", "🤯", "🤠", "🥳", "😎", "🤓", "🧐", "😕", "😟", "🙁", "😮", "😯", "😲", "😳", "🥺", "😦", "😧", "😨",
    "👋", "🤚", "🖐", "✋", "🖖", "👌", "🤏", "✌️", "🤞", "🤟", "🤘", "🤙", "👈", "👉", "👆", "🖕", "👇", "👍",
    "🐶", "🐱", "🐭", "🐹", "🐰", "🦊", "🐻", "🐼", "🐨", "🐯", "🦁", "🐮", "🐷", "🐸", "🐵", "🐔", "🐧", "🐦",
    "🍏", "🍎", "🍐", "🍊", "🍋", "🍌", "🍉", "🍇", "🍓", "🍈", "🍒", "🍑", "🥭", "🍍", "🥥", "🥝", "🍅", "🍆",
    "⚽", "🏀", "🏈", "⚾", "🥎", "🎾", "🏐", "🏉", "🥏", "🎱", "🪀", "🏓", "🏸", "🏒", "🏑", "🥍", "🏏", "🪃",
    "🚗", "🚕", "🚙", "🚌", "🚎", "🏎", "🚓", "🚑", "🚒", "🚐", "🛻", "🚚", "🚛", "🚜", "🦯", "🦽", "🦼", "🛴",
    "❤️", "🧡", "💛", "💚", "💙", "💜", "🖤", "🤍", "🤎", "💔", "❣️", "💕", "💞", "💓", "💗", "💖", "💘", "💝",
];

thread_local! {
    static BUILT: Cell<usize> = const { Cell::new(0) };
}

#[derive(Clone, Copy, PartialEq)]
enum Mode { List, Grid }

struct State {
    mode: Mode,
    selected: usize,
    lazy_grid: bool,
    eager: bool,
    titles: Vec<String>,
    subtitles: Vec<String>,
    keys: Vec<String>,
    cells: Vec<String>,
    row_keys: Vec<String>,
}

impl State {
    fn new(mode: Mode, lazy_grid: bool, eager: bool, cells: usize) -> Self {
        let cells: Vec<String> = (0..cells).map(|i| EMOJI[i % EMOJI.len()].to_owned()).collect();
        State {
            mode,
            selected: 0,
            lazy_grid,
            eager,
            titles: (0..200).map(|i| format!("Demo App {i:03}")).collect(),
            subtitles: (0..200).map(|i| format!("Application · {} uses this week", (i * 37) % 91)).collect(),
            keys: (0..200).map(|i| format!("r{i}")).collect(),
            row_keys: (0..cells.len().div_ceil(COLUMNS)).map(|i| format!("g{i}")).collect(),
            cells,
        }
    }
}

const BG: Color = Color { r: 0.008, g: 0.008, b: 0.009, a: 1.0 };
const TEXT: Color = Color { r: 0.82, g: 0.82, b: 0.84, a: 1.0 };
const FAINT: Color = Color { r: 0.26, g: 0.26, b: 0.29, a: 1.0 };
const LAYER: Color = Color { r: 0.022, g: 0.022, b: 0.027, a: 1.0 };
const HOVER: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.06 };

fn ui() -> TextStyle { TextStyle::new(14.0, 400.0) }

fn list_row(s: &State, i: usize) -> El {
    BUILT.set(BUILT.get() + 1);
    let selected = i == s.selected;
    let letter = s.titles[i].chars().next().unwrap().to_string();
    row().w_full().h(ROW).px(10.0).gap(10.0).center_y().radius(8.0)
        .fill(if selected { LAYER } else { Color::TRANSPARENT })
        .hover(HOVER).role("option").label(s.titles[i].clone())
        .on_press(move |s: &mut State| s.selected = i)
        .child(el().size(22.0, 22.0).radius(5.0).fill(LAYER).center()
            .child(text(letter).font(TextStyle::new(12.0, 600.0)).color(TEXT)))
        .child(col().key("copy").grow(1.0).shrink().gap(2.0).child(
            row().key("line").w_full().gap(8.0).center_y()
                .child(text(s.titles[i].clone()).font(if selected { TextStyle::new(14.0, 500.0) } else { ui() }).color(TEXT).truncate_fade())
                .child(text(s.subtitles[i].clone()).font(ui()).color(FAINT).truncate_fade().grow(1.0))))
}

fn list(s: &State) -> El {
    let rows = col().key("rows").w_full().px(8.0).py(6.0).growth(GrowthSpec::default());
    let rows = if s.eager {
        rows.virtualize(ROW).children((0..s.titles.len()).map(|i| list_row(s, i).key(s.keys[i].clone())))
    } else {
        rows.virtual_rows(s.titles.len(), ROW, |s: &State, i| Cow::Borrowed(s.keys[i].as_str()), list_row)
    };
    col().key("list").w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false })
        .clip_fade(0.0, 12.0, 0.0, 12.0).child(rows)
}

fn grid_row(s: &State, r: usize) -> El {
    BUILT.set(BUILT.get() + 1);
    let cell = (SIZE.0 - 16.0) / COLUMNS as f32;
    let glyph = (cell * 0.5).min(34.0);
    let mut line = row().w_full().h(cell);
    for c in 0..COLUMNS {
        let index = r * COLUMNS + c;
        let Some(emoji) = s.cells.get(index) else { break };
        let selected = index == s.selected;
        line = line.child(el().key(format!("c{index}")).size(cell, cell).center().radius(10.0)
            .fill(if selected { LAYER } else { Color::TRANSPARENT })
            .border(if selected { 1.0 } else { 0.0 }, FAINT)
            .hover(HOVER).role("option").label(emoji.clone())
            .on_press(move |s: &mut State| s.selected = index)
            .child(el().size(glyph, glyph).center()
                .child(text(emoji.clone()).font(TextStyle::new(glyph * 0.78, 400.0).lh(glyph)).color(TEXT))));
    }
    line
}

fn grid(s: &State) -> El {
    let cell = (SIZE.0 - 16.0) / COLUMNS as f32;
    let rows = col().key("rows").w_full().px(8.0).py(6.0).growth(GrowthSpec::default());
    let rows = if s.lazy_grid {
        rows.virtual_rows(s.row_keys.len(), cell, |s: &State, r| Cow::Borrowed(s.row_keys[r].as_str()), grid_row)
    } else {
        rows.virtualize(cell).children((0..s.row_keys.len()).map(|r| grid_row(s, r).key(s.row_keys[r].clone())))
    };
    col().key("grid").w_full().grow(1.0).shrink().scroll(ScrollSpec { stick_bottom: false })
        .clip_fade(0.0, 12.0, 0.0, 12.0).child(rows)
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let (body, prompt) = match s.mode {
        Mode::List => (list(s), "Search for apps and commands…"),
        Mode::Grid => (grid(s), "Search emoji…"),
    };
    col().w_full().h_full().fill(BG)
        .child(row().key("search").w_full().h(56.0).px(16.0).gap(10.0).center_y()
            .child(el().size(18.0, 18.0).radius(9.0).border(1.5, FAINT))
            .child(text(prompt).font(TextStyle::new(17.0, 400.0)).color(FAINT)))
        .child(body)
        .child(row().key("footer").w_full().h(36.0).px(16.0).center_y().gap(8.0)
            .child(text("Open").font(TextStyle::new(12.0, 500.0)).color(FAINT).key("open"))
            .child(text("Actions ⌘K").font(TextStyle::new(12.0, 500.0)).color(FAINT).key("actions")))
}

fn on_key(s: &mut State, k: &KeyEvent, _: &mut Tx) -> bool {
    match k.key {
        Key::Escape => { s.mode = Mode::List; s.selected = 0; true }
        Key::Tab => { s.mode = Mode::Grid; s.selected = 0; true }
        Key::Down => { s.selected += 1; true }
        _ => false,
    }
}

fn press(rt: &mut Runtime<State>, key: Key) { rt.key(KeyEvent { key, mods: Default::default() }); }

struct Draw { gpu: Gpu, renderer: Renderer, out: Offscreen }

impl Draw {
    fn draw(&mut self, rt: &mut Runtime<State>) {
        self.renderer.upload_images(&self.gpu.device, &self.gpu.queue, &mut rt.images);
        self.renderer.prepare(&self.gpu.device, &self.gpu.queue, &mut rt.text.atlas, &rt.instances, (self.out.width, self.out.height));
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        self.renderer.draw(&mut encoder, &self.out.view, rt.clear_color());
        self.renderer.submit(&self.gpu.queue, encoder);
    }
    fn wait(&self) { self.gpu.device.poll(wgpu::PollType::wait_indefinitely()).ok(); }
}

/// Frames until motion settles, drawing the last; returns their CPU.
fn settle(rt: &mut Runtime<State>, d: &mut Draw) -> (f64, usize) {
    let (start, mut frames) = (rune::thread_cpu_ms(), 0);
    while frames < 240 && rt.frame(1.0 / 120.0) { frames += 1; }
    let cpu = rune::thread_cpu_ms() - start;
    d.draw(rt);
    d.wait();
    (cpu, frames)
}

const METRICS: &[&str] = &["cpu_ms", "frame_ms", "encode_ms", "view_ms", "commit_ms", "layout_ms", "text_ms", "paint_ms",
    "apply_ms", "shaped", "layout_nodes", "allocations", "tail_ms", "tail_frames", "rows_built", "rows_mounted", "rows_settled"];

/// How many of the shown list's rows are laid out.
fn mounted(rt: &Runtime<State>) -> f64 {
    let (list, keys) = match rt.state.mode {
        Mode::List => ("list", &rt.state.keys),
        Mode::Grid => ("grid", &rt.state.row_keys),
    };
    keys.iter().filter(|k| rt.find(&format!("#0/{list}/rows/{k}")).is_some()).count() as f64
}

fn sample(d: &mut Draw, from: Mode, case: &str, lazy_grid: bool, eager: bool, cells: usize) -> Vec<f64> {
    let to = if from == Mode::Grid { Mode::List } else { Mode::Grid };
    let key = |m: Mode| if m == Mode::List { Key::Escape } else { Key::Tab };
    // Fonts load outside the timed frame; the warm-up layouts are evicted before it.
    let mut text = TextSystem::default();
    for style in [ui(), TextStyle::new(28.0, 400.0)] {
        text.layout_rich("Warm 😀 fonts", &[], style, 2.0, None);
    }
    for _ in 0..3 { text.end_commit(); }
    let start = if case == "first" { from } else { to };
    let mut rt = Runtime::with_text(State::new(start, lazy_grid, eager, cells), view, SIZE, Env::new(true, 2.0), text);
    rt.on_key = Some(on_key);
    rt.frame(0.0);
    settle(&mut rt, d);
    if case == "quick" {
        press(&mut rt, key(from));
        rt.frame(1.0 / 120.0);
        d.draw(&mut rt);
        d.wait();
    } else if case != "first" {
        press(&mut rt, key(from));
        settle(&mut rt, d);
        if case == "stale" {
            for _ in 0..3 {
                press(&mut rt, Key::Down);
                settle(&mut rt, d);
            }
        }
    }
    rune::take_frame_phases();
    BUILT.set(0);
    let (alloc0, cpu0, wall0) = (procwatch::alloc::thread_allocations(), rune::thread_cpu_ms(), Instant::now());
    press(&mut rt, key(to));
    rt.frame(1.0 / 120.0);
    let (cpu, frame) = (rune::thread_cpu_ms() - cpu0, wall0.elapsed().as_secs_f64() * 1000.0);
    let allocations = (procwatch::alloc::thread_allocations() - alloc0) as f64;
    let phases = rune::take_frame_phases();
    let encode0 = Instant::now();
    d.draw(&mut rt);
    let encode = encode0.elapsed().as_secs_f64() * 1000.0;
    d.wait();
    let (shaped, nodes) = (rt.stats.shaped as f64, rt.stats.layout_nodes as f64);
    let (built, mounted_now) = (BUILT.get() as f64, mounted(&rt));
    let (tail, tail_frames) = settle(&mut rt, d);
    let p = rune::PHASES;
    vec![cpu, frame, encode, phases[p], phases[p + 1], phases[p + 2], phases[p + 3], phases[p + 4], phases[p + 5],
        shaped, nodes, allocations, tail, tail_frames as f64, built, mounted_now, mounted(&rt)]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let samples: usize = value("--samples").map_or(15, |s| s.parse().expect("--samples N"));
    let only = value("--only");
    let lazy_grid = args.iter().any(|a| a == "--lazy-grid");
    let eager = args.iter().any(|a| a == "--eager-list");
    let cells: usize = value("--cells").map_or(1800, |s| s.parse().expect("--cells N"));
    if cfg!(debug_assertions) { eprintln!("tree_swap: debug build; time a --profile perf build"); }
    let gpu = Gpu::headless();
    let renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let out = Offscreen::new(&gpu.device, (SIZE.0 * 2.0) as u32, (SIZE.1 * 2.0) as u32);
    let mut d = Draw { gpu, renderer, out };
    for (dir, from) in [("grid-list", Mode::Grid), ("list-grid", Mode::List)] {
        if only.as_ref().is_some_and(|o| !dir.starts_with(o.as_str()) && !o.contains(&format!("{dir}/"))) { continue; }
        for case in ["first", "quick", "recent", "stale"] {
            if only.as_ref().is_some_and(|o| o.contains('/') && *o != format!("{dir}/{case}")) { continue; }
            let rows: Vec<Vec<f64>> = (0..=samples).map(|_| sample(&mut d, from, case, lazy_grid, eager, cells)).skip(1).collect();
            for (m, name) in METRICS.iter().enumerate() {
                let mut v: Vec<f64> = rows.iter().map(|r| r[m]).collect();
                v.sort_by(f64::total_cmp);
                let p95 = v[((v.len() - 1) as f64 * 0.95).round() as usize];
                println!("METRIC dir={dir} case={case} metric={name} median={:.4} p95={p95:.4} n={}", v[v.len() / 2], v.len());
            }
        }
    }
}
