//! Complete wrapping results, not individual measurements. Hits avoid tokenization,
//! shaping and rebuilding caret offsets during otherwise animated frames.
use std::cell::RefCell;
use std::collections::VecDeque;

use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::panels::agent_pane::input_controller::InputWrapRow;

const ENTRY_LIMIT: usize = 32;
const BYTE_LIMIT: usize = 512 * 1024;

thread_local! {
    static PROMPTS: RefCell<WrapCache<Vec<InputWrapRow>>> = RefCell::new(WrapCache::default());
    static BUBBLES: RefCell<WrapCache<Vec<String>>> = RefCell::new(WrapCache::default());
}

#[derive(Clone, Debug, PartialEq, Eq)]
// Color, extrusion, clip rect, caret position and animation time are paint-only;
// excluding them lets layout survive blinking, hover and animated backgrounds.
struct LayoutKey {
    width: u32,
    font_size: u32,
    scale: u32,
    bold: bool,
    italic: bool,
    font_id: Option<usize>,
    library: usize,
    // Face keys are globally unique, also detecting replacements within a library
    // and preventing a recycled library address from hitting an obsolete entry.
    faces: Vec<(usize, u64)>,
    hinting: bool,
    symbol_maps: Vec<(usize, char, char)>,
}

impl LayoutKey {
    fn new(sugarloaf: &Sugarloaf, width: f32, opts: &DrawOpts) -> Self {
        let library = sugarloaf.font_library();
        let fonts = library.inner.read();
        let mut faces: Vec<_> = fonts
            .inner
            .iter()
            .map(|(id, face)| (*id, face.key.value()))
            .collect();
        faces.sort_unstable();
        Self {
            width: width.to_bits(),
            font_size: opts.font_size.to_bits(),
            scale: sugarloaf.scale_factor().to_bits(),
            bold: opts.bold,
            italic: opts.italic,
            font_id: opts.font_id,
            library: std::sync::Arc::as_ptr(&library.inner) as usize,
            faces,
            hinting: fonts.hinting,
            symbol_maps: fonts
                .symbol_maps
                .as_ref()
                .map(|maps| {
                    maps.iter()
                        .map(|map| (map.font_index, map.range.start, map.range.end))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    fn bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.faces.len() * std::mem::size_of::<(usize, u64)>()
            + self.symbol_maps.len() * std::mem::size_of::<(usize, char, char)>()
    }
}

struct Entry<T> {
    key: LayoutKey,
    text: String,
    value: T,
    bytes: usize,
}

struct WrapCache<T> {
    entries: VecDeque<Entry<T>>,
    bytes: usize,
}

impl<T> Default for WrapCache<T> {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            bytes: 0,
        }
    }
}

impl<T: Clone> WrapCache<T> {
    fn get(&mut self, key: &LayoutKey, text: &str) -> Option<T> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.key == *key && entry.text == text)?;
        let entry = self.entries.remove(index)?;
        let value = entry.value.clone();
        self.entries.push_back(entry);
        Some(value)
    }

    fn insert(&mut self, key: LayoutKey, text: &str, value: T, value_bytes: usize) {
        let bytes = key
            .bytes()
            .saturating_add(
                std::mem::size_of::<Entry<T>>() - std::mem::size_of::<LayoutKey>(),
            )
            .saturating_add(text.len())
            .saturating_add(value_bytes);
        // Huge pastes still wrap normally, but cannot monopolize retained memory.
        if bytes > BYTE_LIMIT {
            return;
        }
        while self.entries.len() >= ENTRY_LIMIT || self.bytes + bytes > BYTE_LIMIT {
            let Some(old) = self.entries.pop_front() else {
                break;
            };
            self.bytes -= old.bytes;
        }
        self.bytes += bytes;
        self.entries.push_back(Entry {
            key,
            text: text.to_owned(),
            value,
            bytes,
        });
    }
}

fn cached<T: Clone>(
    cache: &'static std::thread::LocalKey<RefCell<WrapCache<T>>>,
    key: LayoutKey,
    text: &str,
    build: impl FnOnce() -> T,
    size: impl FnOnce(&T) -> usize,
) -> T {
    if let Some(value) = cache.with(|cache| cache.borrow_mut().get(&key, text)) {
        return value;
    }
    // Never hold a RefCell borrow while shaping: fallback-font discovery can
    // mutate the font library, and measurement has its own independent caches.
    let value = build();
    let bytes = size(&value);
    cache.with(|cache| cache.borrow_mut().insert(key, text, value.clone(), bytes));
    value
}

pub(super) fn prompt(
    sugarloaf: &mut Sugarloaf,
    text: &str,
    width: f32,
    opts: &DrawOpts,
    build: impl FnOnce(&mut Sugarloaf) -> Vec<InputWrapRow>,
) -> Vec<InputWrapRow> {
    let key = LayoutKey::new(sugarloaf, width, opts);
    cached(
        &PROMPTS,
        key,
        text,
        || build(sugarloaf),
        |rows| {
            rows.len() * std::mem::size_of::<InputWrapRow>()
                + rows
                    .iter()
                    .map(|row| row.offsets.len() * std::mem::size_of::<f32>())
                    .sum::<usize>()
        },
    )
}

pub(super) fn bubble(
    sugarloaf: &mut Sugarloaf,
    text: &str,
    width: f32,
    opts: &DrawOpts,
    build: impl FnOnce(&mut Sugarloaf) -> Vec<String>,
) -> Vec<String> {
    let key = LayoutKey::new(sugarloaf, width, opts);
    cached(
        &BUBBLES,
        key,
        text,
        || build(sugarloaf),
        |lines| {
            lines.len() * std::mem::size_of::<String>()
                + lines.iter().map(String::len).sum::<usize>()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> LayoutKey {
        LayoutKey {
            width: 100_f32.to_bits(),
            font_size: 16_f32.to_bits(),
            scale: 1_f32.to_bits(),
            bold: false,
            italic: false,
            font_id: None,
            library: 1,
            faces: vec![(0, 42)],
            hinting: true,
            symbol_maps: vec![],
        }
    }

    #[test]
    fn exact_text_and_every_layout_input_invalidate() {
        let mut cache = WrapCache::default();
        cache.insert(key(), "界 e\u{301}\n👩‍💻", vec!["result"], 16);
        assert_eq!(cache.get(&key(), "界 e\u{301}\n👩‍💻"), Some(vec!["result"]));
        assert!(cache.get(&key(), "界 e\u{301}\n👩‍💻!").is_none());
        let mut variants = Vec::new();
        let mut k = key();
        k.width += 1;
        variants.push(k);
        let mut k = key();
        k.font_size += 1;
        variants.push(k);
        let mut k = key();
        k.scale += 1;
        variants.push(k);
        let mut k = key();
        k.bold = true;
        variants.push(k);
        let mut k = key();
        k.italic = true;
        variants.push(k);
        let mut k = key();
        k.font_id = Some(0);
        variants.push(k);
        let mut k = key();
        k.library += 1;
        variants.push(k);
        let mut k = key();
        k.faces[0].1 += 1;
        variants.push(k);
        let mut k = key();
        k.hinting = false;
        variants.push(k);
        let mut k = key();
        k.symbol_maps.push((1, 'a', 'z'));
        variants.push(k);
        for variant in variants {
            assert!(cache.get(&variant, "界 e\u{301}\n👩‍💻").is_none());
        }
    }

    #[test]
    fn lru_and_byte_budget_are_bounded() {
        let mut cache = WrapCache::default();
        for i in 0..ENTRY_LIMIT {
            cache.insert(key(), &i.to_string(), i, 8);
        }
        assert_eq!(cache.get(&key(), "0"), Some(0));
        cache.insert(key(), "new", 99, 8);
        assert!(cache.get(&key(), "1").is_none());
        assert_eq!(cache.get(&key(), "0"), Some(0));
        cache.insert(key(), "large", 100, BYTE_LIMIT / 2);
        cache.insert(key(), "another large", 101, BYTE_LIMIT / 2);
        assert!(cache.bytes <= BYTE_LIMIT);
        assert!(cache.entries.len() <= ENTRY_LIMIT);
        let bytes = cache.bytes;
        cache.insert(key(), "oversized", 102, BYTE_LIMIT);
        assert_eq!(cache.bytes, bytes);
        assert!(cache.get(&key(), "oversized").is_none());
    }

    #[test]
    fn bubble_hits_preserve_graphemes_and_resize_rewraps() {
        use unicode_segmentation::UnicodeSegmentation;
        thread_local! {
            static TEST: RefCell<WrapCache<Vec<String>>> = RefCell::new(WrapCache::default());
        }
        let text = "e\u{301}e\u{301}e\u{301}\n👩‍💻👩‍💻👩‍💻";
        let builds = std::cell::Cell::new(0);
        let wrap = |width: f32| {
            let mut layout = key();
            layout.width = width.to_bits();
            cached(
                &TEST,
                layout,
                text,
                || {
                    builds.set(builds.get() + 1);
                    super::super::wrap_user_message_with(text, width, 30, |part| {
                        part.graphemes(true).count() as f32
                    })
                },
                |lines| lines.iter().map(String::len).sum(),
            )
        };
        for _ in 0..10 {
            assert_eq!(wrap(2.0), ["e\u{301}e\u{301}", "e\u{301}", "👩‍💻👩‍💻", "👩‍💻"]);
        }
        assert_eq!(builds.get(), 1);
        assert_eq!(wrap(3.0), ["e\u{301}e\u{301}e\u{301}", "👩‍💻👩‍💻👩‍💻"]);
        assert_eq!(builds.get(), 2);
    }

    #[test]
    fn prompt_core_preserves_exact_fill_newline_and_atomic_attachment() {
        let rows = super::super::wrap_agent_prompt_rows_with("界a", 2.0, |s| {
            s.chars().count() as f32
        });
        assert_eq!((rows[0].start, rows[0].end), (0, 4));
        assert_eq!(rows[0].offsets, [0.0, 1.0, 2.0]);
        assert_eq!(rows.len(), 1);
        let text = "a [Image1]\n";
        let rows = super::super::wrap_agent_prompt_rows_with(text, 3.0, |s| {
            s.chars().count() as f32
        });
        assert_eq!(&text[rows[1].start..rows[1].end], "[Image1]");
        assert_eq!(rows[1].offsets.len(), "[Image1]".chars().count() + 1);
        let last = rows.last().unwrap();
        assert_eq!((last.start, last.end), (text.len(), text.len()));
        assert_eq!(last.offsets, [0.0]);
    }

    #[test]
    fn identical_prompt_hit_calls_measurement_zero_times() {
        thread_local! {
            static TEST: RefCell<WrapCache<Vec<InputWrapRow>>> = RefCell::new(WrapCache::default());
        }
        let calls = std::cell::Cell::new(0);
        let wrap = |layout: LayoutKey, text: &str| {
            cached(
                &TEST,
                layout.clone(),
                text,
                || {
                    super::super::wrap_agent_prompt_rows_with(
                        text,
                        f32::from_bits(layout.width),
                        |part| {
                            calls.set(calls.get() + 1);
                            part.chars().count() as f32
                        },
                    )
                },
                |_| 100,
            )
        };
        let text = "界 e\u{301} [Image1]\n👩‍💻";
        let original = wrap(key(), text);
        assert!(calls.get() > 0);
        calls.set(0);
        let repeated = wrap(key(), text);
        assert_eq!(
            calls.get(),
            0,
            "identical hit must not measure any glyph/token"
        );
        assert_eq!(original.len(), repeated.len());
        for (a, b) in original.iter().zip(&repeated) {
            assert_eq!((a.start, a.end, &a.offsets), (b.start, b.end, &b.offsets));
        }
        for (layout, changed_text) in [
            (key(), "edited draft"),
            (
                {
                    let mut k = key();
                    k.width = 10_f32.to_bits();
                    k
                },
                text,
            ),
            (
                {
                    let mut k = key();
                    k.font_id = Some(1);
                    k
                },
                text,
            ),
            (
                {
                    let mut k = key();
                    k.font_size = 18_f32.to_bits();
                    k
                },
                text,
            ),
            (
                {
                    let mut k = key();
                    k.faces[0].1 += 1;
                    k
                },
                text,
            ),
            (
                {
                    let mut k = key();
                    k.scale = 2_f32.to_bits();
                    k
                },
                text,
            ),
        ] {
            calls.set(0);
            wrap(layout, changed_text);
            assert!(calls.get() > 0, "changed layout/text must measure again");
        }
    }

    #[test]
    #[ignore = "debug GPU-free microbenchmark; run explicitly with --ignored --nocapture"]
    fn long_draft_cold_vs_warm_microbenchmark() {
        use std::hint::black_box;
        use std::time::Instant;
        thread_local! {
            static TEST: RefCell<WrapCache<Vec<InputWrapRow>>> = RefCell::new(WrapCache::default());
        }
        let text = "A long unchanged draft with 界 and e\u{301}, links, and [Image1]. "
            .repeat(100);
        let layout = key();
        let run = || {
            cached(
                &TEST,
                layout.clone(),
                &text,
                || {
                    super::super::wrap_agent_prompt_rows_with(&text, 100.0, |part| {
                        black_box(part).chars().count() as f32
                    })
                },
                |rows| {
                    rows.len() * std::mem::size_of::<InputWrapRow>()
                        + rows.iter().map(|row| row.offsets.len() * 4).sum::<usize>()
                },
            )
        };
        const ITERATIONS: usize = 2000;
        let start = Instant::now();
        for _ in 0..ITERATIONS {
            TEST.with(|cache| *cache.borrow_mut() = WrapCache::default());
            black_box(run());
        }
        let cold = start.elapsed();
        black_box(run());
        let start = Instant::now();
        for _ in 0..ITERATIONS {
            black_box(run());
        }
        let warm = start.elapsed();
        eprintln!("debug GPU-free synthetic-measure composer wrap: {} bytes, {} chars, {ITERATIONS} iterations; cold {:.2} us/iter, warm {:.2} us/iter, {:.2}x", text.len(), text.chars().count(), cold.as_secs_f64() * 1e6 / ITERATIONS as f64, warm.as_secs_f64() * 1e6 / ITERATIONS as f64, cold.as_secs_f64() / warm.as_secs_f64());
    }

    #[test]
    fn repeated_frames_skip_build_and_preserve_caret_offsets() {
        thread_local! { static TEST: RefCell<WrapCache<Vec<InputWrapRow>>> = RefCell::new(WrapCache::default()); }
        let builds = std::cell::Cell::new(0);
        for _ in 0..10 {
            let rows = cached(
                &TEST,
                key(),
                "界\n",
                || {
                    builds.set(builds.get() + 1);
                    vec![
                        InputWrapRow {
                            start: 0,
                            end: 3,
                            offsets: vec![0.0, 12.5],
                        },
                        InputWrapRow {
                            start: 4,
                            end: 4,
                            offsets: vec![0.0],
                        },
                    ]
                },
                |_| 100,
            );
            assert_eq!((rows[0].start, rows[0].end), (0, 3));
            assert_eq!(rows[0].offsets, [0.0, 12.5]);
            assert_eq!((rows[1].start, rows[1].end), (4, 4));
        }
        assert_eq!(builds.get(), 1);
        let mut resized = key();
        resized.width += 1;
        cached(
            &TEST,
            resized,
            "界\n",
            || {
                builds.set(builds.get() + 1);
                vec![]
            },
            |_| 0,
        );
        assert_eq!(builds.get(), 2);
    }
}
