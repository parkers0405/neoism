//! Long note: one multi-line `text_input` holding a whole Markdown note, inside a scroll
//! viewport, as a notes app edits its source. Headless, no window.
//!
//! The note is generated (fictional words): headings, long wrapping paragraphs, bullet and
//! numbered lists, quotes and fenced code, about 40 bytes a line. `--sizes 5,50,200` picks the
//! note sizes in KB. For each size it times, on a fresh runtime:
//!
//! - `type_start`, `type_mid`, `type_end`: one typed character (an IME commit) and the frame it
//!   commits, keystroke after keystroke at the same place;
//! - `enter_mid`, `backspace_mid`: Return, and Backspace of a character just typed, in the
//!   middle (Return types a newline through the field's `on_submit`, as an editor's must:
//!   without one it goes to the default button);
//! - `paste_2k`, `undo`: a fresh 2 KB paste in the middle, then Cmd+Z, alternating;
//! - `undo_depth`: how many of 100 small pastes Cmd+Z can take back;
//! - `scroll`: each frame of a top-to-bottom wheel scroll in 120-point steps;
//! - `down`: Down arrow from the top, one line at a time (checks the caret stays in view and the
//!   viewport never moves more than a line per press);
//! - `cmd_down`: Cmd+Down from the top to the end;
//! - `click`: a click near the bottom of the viewport after a wheel scroll (the caret lands
//!   there and the viewport doesn't move);
//! - `memory`: heap and footprint growth for opening the note, then after 50 and 200 typed
//!   characters.
//!
//! `--blocks` splits the note at blank lines into one field per block, as an app that keys its
//! Markdown blocks, and runs the cases that don't move the caret between fields.
//!
//! cargo build --profile perf -p rune --example long_note --features frame-profile
//! target/perf/examples/long_note [--sizes 5,50,200] [--samples 60] [--only type_mid,scroll] [--gpu] [--blocks]
//!
//! Prints `METRIC size=.. case=.. metric=.. median=.. p95=.. max=..` per measurement, and
//! `MEMORY size=..` lines. `cpu_ms` is thread CPU of the input and the frame it commits;
//! phases are thread CPU and overlap: commit includes view, layout and apply, layout includes
//! text (shaping). `shaped` counts layouts shaped that frame and `glyphs` glyph quads painted.
//! `--gpu` adds `encode_ms`: renderer prepare, draw and submit on a headless GPU.
use std::time::Instant;

use rune::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
use rune::text::TextSystem;
use rune::{Color, Cx, El, Env, Ime, Key, KeyEvent, Mods, Runtime, ScrollSpec, TextEdit, TextStyle, col, text_input};

#[global_allocator]
static ALLOC: procwatch::alloc::Counting = procwatch::alloc::Counting::system();

const SIZE: (f32, f32) = (760.0, 900.0);
const LINE: f32 = 22.0;
const BG: Color = Color { r: 0.008, g: 0.008, b: 0.009, a: 1.0 };

struct State { note: TextEdit, blocks: Vec<TextEdit> }

fn style() -> TextStyle { TextStyle::new(15.0, 400.0).lh(LINE) }

fn view(s: &State, _: &mut Cx<State>) -> El {
    col().w_full().h_full().fill(BG).child(
        col().key("viewport").w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false })
            .child(col().w_full().px(32.0).py(24.0)
                .child(text_input(&s.note, style()).key("editor").w_full().on_edit(|s: &mut State| &mut s.note)
                    // Return in a field without `on_submit` goes to the default button; an editor types a newline.
                    .on_submit(|s: &mut State, _| s.note.insert("\n")))),
    )
}

/// `--blocks`: one field per blank-line-separated block, as an app that keys its Markdown blocks.
fn block_view(s: &State, _: &mut Cx<State>) -> El {
    let fields = s.blocks.iter().enumerate().map(|(i, b)| {
        text_input(b, style()).key(format!("b{i}")).w_full()
            .on_edit(move |s: &mut State| &mut s.blocks[i])
            .on_submit(move |s: &mut State, _| s.blocks[i].insert("\n"))
    });
    col().w_full().h_full().fill(BG).child(
        col().key("viewport").w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false })
            .child(col().key("editor").w_full().px(32.0).py(24.0).gap(LINE).children(fields)),
    )
}

/// A deterministic fictional Markdown note of at least `bytes` bytes.
fn note(bytes: usize) -> String {
    const WORDS: &[&str] = &["amber", "lantern", "orchard", "quiet", "harbor", "copper", "meadow", "signal", "drift",
        "pebble", "willow", "ember", "tide", "cinder", "fable", "glacier", "hollow", "juniper", "kestrel", "lumen",
        "marrow", "nimbus", "opal", "prism", "quill", "ripple", "sable", "thistle", "umber", "vellum", "wander",
        "yarrow", "zephyr", "bramble", "cobalt", "dune", "fern", "grove", "heron", "isle", "the", "a", "of", "and",
        "to", "in", "with", "under", "over", "before"];
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n as u64) as usize
    };
    let words = |next: &mut dyn FnMut(usize) -> usize, n: usize| {
        (0..n).map(|_| WORDS[next(WORDS.len())]).collect::<Vec<_>>().join(" ")
    };
    let mut out = String::new();
    let mut section = 0;
    while out.len() < bytes {
        section += 1;
        out += &format!("# Section {section}: {}\n\n", words(&mut next, 3));
        for sub in 0..3 {
            out += &format!("## {} {sub}\n\n", words(&mut next, 2));
            for _ in 0..2 {
                let n = 25 + next(60);
                out += &words(&mut next, n);
                out += ".\n\n";
            }
            for i in 0..4 + next(5) {
                let n = 4 + next(10);
                if sub % 2 == 0 { out += &format!("- {}\n", words(&mut next, n)); } else { out += &format!("{}. {}\n", i + 1, words(&mut next, n)); }
                if next(4) == 0 { let n = 3 + next(5); out += &format!("  - {}\n", words(&mut next, n)); }
            }
            out += "\n";
            match next(3) {
                0 => {
                    out += "```rust\n";
                    for i in 0..5 + next(6) {
                        let (indent, name, call) = ("    ".repeat(next(3)), WORDS[next(WORDS.len())], WORDS[next(WORDS.len())]);
                        out += &format!("{indent}let {name}_{i} = {call}(\"{}\");\n", words(&mut next, 2));
                    }
                    out += "```\n\n";
                }
                1 => {
                    let n = 12 + next(20);
                    out += &format!("> {}\n\n", words(&mut next, n));
                }
                _ => {
                    for _ in 0..3 { let n = 3 + next(6); out += &format!("- [ ] {}\n", words(&mut next, n)); }
                    out += "\n";
                }
            }
        }
    }
    out
}

fn paste_text() -> String {
    let mut p = String::new();
    let all = note(4096);
    for line in all.lines() {
        if p.len() + line.len() + 1 > 2048 { break; }
        p += line;
        p.push('\n');
    }
    p
}

struct Draw { gpu: Gpu, renderer: Renderer, out: Offscreen }

impl Draw {
    fn draw(&mut self, rt: &mut Runtime<State>) -> f64 {
        let t = Instant::now();
        self.renderer.upload_images(&self.gpu.device, &self.gpu.queue, &mut rt.images);
        self.renderer.prepare(&self.gpu.device, &self.gpu.queue, &mut rt.text.atlas, &rt.instances, (self.out.width, self.out.height));
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        self.renderer.draw(&mut encoder, &self.out.view, rt.clear_color());
        self.renderer.submit(&self.gpu.queue, encoder);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        self.gpu.device.poll(wgpu::PollType::wait_indefinitely()).ok();
        ms
    }
}

const METRICS: &[&str] = &["cpu_ms", "view_ms", "commit_ms", "layout_ms", "text_ms", "paint_ms", "shaped", "glyphs", "allocations", "encode_ms"];

/// Times `input` and the one frame that follows it.
fn timed(rt: &mut Runtime<State>, d: &mut Option<Draw>, input: impl FnOnce(&mut Runtime<State>)) -> Vec<f64> {
    rune::take_frame_phases();
    let (alloc0, cpu0) = (procwatch::alloc::thread_allocations(), rune::thread_cpu_ms());
    input(rt);
    rt.frame(1.0 / 120.0);
    let cpu = rune::thread_cpu_ms() - cpu0;
    let allocations = (procwatch::alloc::thread_allocations() - alloc0) as f64;
    let p = rune::take_frame_phases();
    let encode = d.as_mut().map_or(0.0, |d| d.draw(rt));
    let q = rune::PHASES;
    vec![cpu, p[q], p[q + 1], p[q + 2], p[q + 3], p[q + 4], rt.stats.shaped as f64, rt.stats.glyphs as f64, allocations, encode]
}

fn settle(rt: &mut Runtime<State>, d: &mut Option<Draw>) {
    let mut n = 0;
    while n < 240 && rt.frame(1.0 / 120.0) { n += 1; }
    if let Some(d) = d { d.draw(rt); }
}

fn press(rt: &mut Runtime<State>, key: Key) { rt.key(KeyEvent { key, mods: Mods::default() }); }
fn cmd(rt: &mut Runtime<State>, key: Key) { rt.key(KeyEvent { key, mods: Mods { cmd: true, ..Mods::default() } }); }

fn runtime(text: &str, caret: usize) -> Runtime<State> {
    let mut fonts = TextSystem::default();
    fonts.layout_rich("Warm fonts", &[], style(), 2.0, None);
    for _ in 0..3 { fonts.end_commit(); }
    let blocks = BLOCKS.get();
    let mut note = TextEdit::new(if blocks { "" } else { text });
    let mut fields = Vec::new();
    if blocks {
        // The caret's block takes focus, the caret at the same place within it.
        let mut start = 0;
        for part in text.split("\n\n") {
            let mut b = TextEdit::new(part);
            if (start..=start + part.len()).contains(&caret) && !fields.iter().any(|f: &TextEdit| f.focused) {
                b.focused = true;
                b.set_caret(caret - start);
            }
            start += part.len() + 2;
            fields.push(b);
        }
    } else {
        note.focused = true;
        note.set_caret(caret);
    }
    let v = if blocks { block_view } else { view };
    let mut rt = Runtime::with_text(State { note, blocks: fields }, v, SIZE, Env::new(true, 2.0), fonts);
    rt.frame(0.0);
    rt
}

thread_local! {
    static BLOCKS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn find(rt: &Runtime<State>, key: &str) -> rune::runtime::ElementInfo {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap()
}

/// The focused field's key.
fn field(rt: &Runtime<State>) -> String {
    match rt.state.blocks.iter().position(|b| b.focused) {
        Some(i) => format!("b{i}"),
        None => "editor".into(),
    }
}

/// The note's length in bytes.
fn doc_len(s: &State) -> usize {
    if s.blocks.is_empty() { s.note.text.len() } else { s.blocks.iter().map(|b| b.text.len() + 2).sum::<usize>() - 2 }
}

/// The caret's top relative to the viewport, or `None` if it's outside it.
fn caret_in_view(rt: &Runtime<State>) -> Option<f32> {
    let c = find(rt, &field(rt)).caret?;
    let v = find(rt, "viewport").rect;
    (c[1] >= v[1] - 0.5 && c[1] + c[3] <= v[1] + v[3] + 0.5).then_some(c[1] - v[1])
}

fn editor_y(rt: &Runtime<State>) -> f32 { find(rt, "editor").rect[1] }

fn report(size: usize, case: &str, rows: &[Vec<f64>]) {
    if rows.is_empty() { return; }
    for (m, name) in METRICS.iter().enumerate() {
        let mut v: Vec<f64> = rows.iter().map(|r| r[m]).collect();
        v.sort_by(f64::total_cmp);
        let p95 = v[((v.len() - 1) as f64 * 0.95).round() as usize];
        println!("METRIC size={size}k case={case} metric={name} median={:.4} p95={p95:.4} max={:.4} n={}",
            v[v.len() / 2], v[v.len() - 1], v.len());
    }
}

fn heap_mb() -> f64 { procwatch::alloc::heap().map_or(0.0, |h| h.live as f64 / 1_048_576.0) }
fn footprint_mb() -> f64 { procwatch::snapshot().map_or(0.0, |s| s.footprint as f64 / 1_048_576.0) }

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let samples: usize = value("--samples").map_or(60, |s| s.parse().expect("--samples N"));
    let sizes: Vec<usize> = value("--sizes").map_or(vec![5, 50, 200], |s| s.split(',').map(|n| n.parse().expect("--sizes 5,50")).collect());
    let only = value("--only");
    let run = |case: &str| only.as_deref().is_none_or(|o| o.split(',').any(|o| o == case));
    if cfg!(debug_assertions) { eprintln!("long_note: debug build; time a --profile perf build"); }
    let mut d = args.iter().any(|a| a == "--gpu").then(|| {
        let gpu = Gpu::headless();
        let renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
        let out = Offscreen::new(&gpu.device, (SIZE.0 * 2.0) as u32, (SIZE.1 * 2.0) as u32);
        Draw { gpu, renderer, out }
    });
    BLOCKS.set(args.iter().any(|a| a == "--blocks"));
    let paste = paste_text();
    for &kb in &sizes {
        let text = note(kb * 1024);
        let mid = text[..text.len() / 2].rfind(' ').unwrap_or(0) + 1;
        println!("NOTE size={kb}k bytes={} lines={}", text.len(), text.lines().count());

        if run("memory") {
            let (heap0, foot0) = (heap_mb(), footprint_mb());
            let mut rt = runtime(&text, 0);
            settle(&mut rt, &mut d);
            let (heap1, foot1) = (heap_mb(), footprint_mb());
            let mut typed = |rt: &mut Runtime<State>, n: usize| {
                for _ in 0..n { rt.ime(Ime::Commit("x".into())); rt.frame(1.0 / 120.0); }
                settle(rt, &mut d);
                (heap_mb() - heap0, footprint_mb() - foot0)
            };
            let (heap50, foot50) = typed(&mut rt, 50);
            let (heap200, foot200) = typed(&mut rt, 150);
            println!("MEMORY size={kb}k heap_open_mb={:.2} heap_50_keys_mb={heap50:.2} heap_200_keys_mb={heap200:.2} \
                footprint_open_mb={:.2} footprint_50_keys_mb={foot50:.2} footprint_200_keys_mb={foot200:.2}", heap1 - heap0, foot1 - foot0);
        }

        for (case, at) in [("type_start", 0), ("type_mid", mid), ("type_end", text.len())] {
            if !run(case) { continue; }
            let mut rt = runtime(&text, at);
            for _ in 0..3 { rt.ime(Ime::Commit("w".into())); rt.frame(1.0 / 120.0); }
            settle(&mut rt, &mut d);
            let rows: Vec<_> = (0..samples).map(|i| {
                let ch = ["a", "b", "c", " "][i % 4];
                timed(&mut rt, &mut d, |rt| rt.ime(Ime::Commit(ch.into())))
            }).collect();
            assert!(caret_in_view(&rt).is_some(), "{case}: caret in view after typing");
            report(kb, case, &rows);
        }

        for case in ["enter_mid", "backspace_mid"] {
            if !run(case) { continue; }
            let mut rt = runtime(&text, mid);
            rt.ime(Ime::Commit("w".into()));
            settle(&mut rt, &mut d);
            let enter = case == "enter_mid";
            let before = doc_len(&rt.state);
            let rows: Vec<_> = (0..samples).map(|_| {
                // Two characters typed in one untimed frame, so each Backspace reaches a text
                // the layout cache hasn't seen (and a block's field never runs out).
                if !enter { rt.ime(Ime::Commit("qz".into())); rt.frame(1.0 / 120.0); }
                timed(&mut rt, &mut d, |rt| press(rt, if enter { Key::Enter } else { Key::Backspace }))
            }).collect();
            let after = doc_len(&rt.state);
            assert_eq!(after - before, samples, "{case}: one byte per press");
            report(kb, case, &rows);
        }

        if run("paste_2k") || run("undo") {
            let mut rt = runtime(&text, mid);
            rt.ime(Ime::Commit("w".into()));
            settle(&mut rt, &mut d);
            // Each paste is text the layout cache hasn't seen; the Cmd+Z after it goes back to
            // the note just shown, whose layout the cache still holds.
            let (mut pastes, mut undos) = (Vec::new(), Vec::new());
            for i in 0..samples.div_ceil(2) {
                pastes.push(timed(&mut rt, &mut d, |rt| rt.paste(format!("{i:03}{paste}"))));
                undos.push(timed(&mut rt, &mut d, |rt| cmd(rt, Key::Command('z'))));
            }
            assert_eq!(doc_len(&rt.state), text.len() + 1, "every paste undone");
            report(kb, "paste_2k", &pastes);
            report(kb, "undo", &undos);
        }

        if run("undo_depth") {
            let mut rt = runtime(&text, mid);
            settle(&mut rt, &mut d);
            for i in 0..100 {
                rt.paste(format!("edit {i:03} "));
                rt.frame(1.0 / 120.0);
            }
            let mut steps = 0;
            loop {
                let before = doc_len(&rt.state);
                cmd(&mut rt, Key::Command('z'));
                rt.frame(1.0 / 120.0);
                if doc_len(&rt.state) == before { break; }
                steps += 1;
            }
            println!("UNDO size={kb}k edits=100 undo_steps_kept={steps}");
        }

        if run("scroll") {
            let mut rt = runtime(&text, 0);
            settle(&mut rt, &mut d);
            let mut rows = Vec::new();
            let top = editor_y(&rt);
            loop {
                let before = editor_y(&rt);
                rows.push(timed(&mut rt, &mut d, |rt| rt.wheel(SIZE.0 / 2.0, SIZE.1 / 2.0, -120.0)));
                if (editor_y(&rt) - before).abs() < 0.01 { break; }
            }
            println!("SCROLL size={kb}k frames={} travel={:.0}", rows.len(), top - editor_y(&rt));
            report(kb, "scroll", &rows);
        }

        if BLOCKS.get() { continue; }

        if run("down") {
            let mut rt = runtime(&text, 0);
            settle(&mut rt, &mut d);
            let (mut rows, mut worst_jump, mut hidden) = (Vec::new(), 0.0f32, 0);
            for _ in 0..samples.max(120) {
                let before = editor_y(&rt);
                rows.push(timed(&mut rt, &mut d, |rt| press(rt, Key::Down)));
                worst_jump = worst_jump.max((editor_y(&rt) - before).abs());
                if caret_in_view(&rt).is_none() { hidden += 1; }
            }
            println!("CARET size={kb}k case=down moves={} hidden={hidden} worst_jump_pt={worst_jump:.1}", rows.len());
            assert_eq!(hidden, 0, "down: caret left the viewport");
            assert!(worst_jump <= 2.0 * LINE, "down: the viewport jumped {worst_jump} points in one press");
            report(kb, "down", &rows);
        }

        if run("cmd_down") {
            let mut rows = Vec::new();
            let mut visible = true;
            for _ in 0..samples.min(15) {
                let mut rt = runtime(&text, 0);
                settle(&mut rt, &mut d);
                rows.push(timed(&mut rt, &mut d, |rt| cmd(rt, Key::Down)));
                settle(&mut rt, &mut d);
                visible &= rt.state.note.focus == rt.state.note.text.len() && caret_in_view(&rt).is_some();
            }
            println!("CARET size={kb}k case=cmd_down at_end_in_view={visible}");
            assert!(visible, "cmd_down: caret at the end and in view");
            report(kb, "cmd_down", &rows);
        }

        if run("click") {
            let mut rt = runtime(&text, 0);
            settle(&mut rt, &mut d);
            let (mut rows, mut worst_jump, mut moved) = (Vec::new(), 0.0f32, 0);
            let v = find(&rt, "viewport").rect;
            for i in 0..samples.min(30) {
                rt.wheel(SIZE.0 / 2.0, SIZE.1 / 2.0, -900.0);
                settle(&mut rt, &mut d);
                let before = (editor_y(&rt), rt.state.note.focus);
                let y = v[1] + v[3] - 40.0 - (i % 5) as f32 * LINE;
                rows.push(timed(&mut rt, &mut d, |rt| { rt.pointer_down(200.0, y); rt.pointer_up(200.0, y); }));
                settle(&mut rt, &mut d);
                worst_jump = worst_jump.max((editor_y(&rt) - before.0).abs());
                if rt.state.note.focus != before.1 && caret_in_view(&rt).is_some() { moved += 1; }
            }
            println!("CARET size={kb}k case=click clicks={} caret_landed_in_view={moved} worst_jump_pt={worst_jump:.1}", rows.len());
            report(kb, "click", &rows);
        }
    }
}
