//! Complete wrapping results, not individual measurements. Hits avoid tokenization,
//! shaping and rebuilding caret offsets during otherwise animated frames.
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use sugarloaf::text::DrawOpts;

use crate::panels::agent_pane::input_controller::InputWrapRow;

const ENTRY_LIMIT: usize = 32;
const BYTE_LIMIT: usize = 512 * 1024;

thread_local! {
    static PROMPTS: RefCell<WrapCache<Rc<Vec<InputWrapRow>>>> = RefCell::new(WrapCache::default());
    static BUBBLES: RefCell<WrapCache<Vec<String>>> = RefCell::new(WrapCache::default());
    static FONT_KEYS: RefCell<Option<FontKey>> = const { RefCell::new(None) };
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FontKey {
    library: usize,
    faces: Rc<Vec<(usize, u64)>>,
    hinting: bool,
    symbol_maps: Rc<Vec<(usize, char, char)>>,
}

impl FontKey {
    fn new(sugarloaf: &dyn crate::primitives::paint::Paint) -> Self {
        let library = sugarloaf.font_library();
        let address = std::sync::Arc::as_ptr(&library.inner) as usize;
        let fonts = library.inner.read();
        Self::from_fonts(address, &fonts)
    }

    // The public library is mutable without a revision counter. Validate every
    // face and symbol mapping, not just the Arc address/face count. Unchanged
    // calls reuse allocations and skip enumeration/sorting; validation remains
    // O(faces + mappings) under the read lock until Sugarloaf owns a revision.
    fn from_fonts(address: usize, fonts: &sugarloaf::font::FontLibraryData) -> Self {
        FONT_KEYS.with(|cached| {
            let mut cached = cached.borrow_mut();
            if let Some(key) = cached.as_ref().filter(|key| {
                key.library == address
                    && key.hinting == fonts.hinting
                    && key.faces.len() == fonts.inner.len()
                    && key.faces.iter().all(|(id, face)| {
                        fonts
                            .inner
                            .get(id)
                            .is_some_and(|font| font.key.value() == *face)
                    })
                    && key.symbol_maps.iter().copied().eq(fonts
                        .symbol_maps
                        .iter()
                        .flatten()
                        .map(|map| (map.font_index, map.range.start, map.range.end)))
            }) {
                return key.clone();
            }
            let mut faces: Vec<_> = fonts
                .inner
                .iter()
                .map(|(id, face)| (*id, face.key.value()))
                .collect();
            faces.sort_unstable();
            let key = Self {
                library: address,
                faces: Rc::new(faces),
                hinting: fonts.hinting,
                symbol_maps: Rc::new(
                    fonts
                        .symbol_maps
                        .iter()
                        .flatten()
                        .map(|map| (map.font_index, map.range.start, map.range.end))
                        .collect(),
                ),
            };
            *cached = Some(key.clone());
            key
        })
    }
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
    faces: Rc<Vec<(usize, u64)>>,
    hinting: bool,
    symbol_maps: Rc<Vec<(usize, char, char)>>,
}

impl LayoutKey {
    fn new(sugarloaf: &dyn crate::primitives::paint::Paint, width: f32, opts: &DrawOpts) -> Self {
        let fonts = FontKey::new(sugarloaf);
        Self {
            width: width.to_bits(),
            font_size: opts.font_size.to_bits(),
            scale: sugarloaf.scale_factor().to_bits(),
            bold: opts.bold,
            italic: opts.italic,
            font_id: opts.font_id,
            library: fonts.library,
            faces: fonts.faces,
            hinting: fonts.hinting,
            symbol_maps: fonts.symbol_maps,
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
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    text: &str,
    width: f32,
    opts: &DrawOpts,
    build: impl FnOnce(&mut dyn crate::primitives::paint::Paint) -> Vec<InputWrapRow>,
) -> Rc<Vec<InputWrapRow>> {
    let key = LayoutKey::new(sugarloaf, width, opts);
    cached(
        &PROMPTS,
        key,
        text,
        || Rc::new(build(sugarloaf)),
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
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    text: &str,
    width: f32,
    opts: &DrawOpts,
    build: impl FnOnce(&mut dyn crate::primitives::paint::Paint) -> Vec<String>,
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
            faces: Rc::new(vec![(0, 42)]),
            hinting: true,
            symbol_maps: Rc::default(),
        }
    }

    #[test]
    fn font_fingerprint_reuses_allocations_but_detects_in_place_mutation() {
        use sugarloaf::font::{FontData, FontLibraryData, SymbolMap};
        let mut fonts = FontLibraryData::default();
        let bytes = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../sugarloaf/resources/test-fonts/DejaVuSans.ttf"
        ));
        fonts.inner.insert(0, FontData::from_slice(bytes).unwrap());
        let first = FontKey::from_fonts(7, &fonts);
        let hit = FontKey::from_fonts(7, &fonts);
        assert!(Rc::ptr_eq(&first.faces, &hit.faces));
        assert!(Rc::ptr_eq(&first.symbol_maps, &hit.symbol_maps));
        // Same address, same face count: replacement must not hit stale rows.
        fonts.inner.insert(0, FontData::from_slice(bytes).unwrap());
        let replacement = FontKey::from_fonts(7, &fonts);
        assert_ne!(replacement, first);
        fonts.hinting = !fonts.hinting;
        let hinting = FontKey::from_fonts(7, &fonts);
        assert_ne!(hinting, replacement);
        fonts.symbol_maps = Some(vec![SymbolMap {
            font_index: 0,
            range: 'a'..'z',
        }]);
        let mapped = FontKey::from_fonts(7, &fonts);
        assert_ne!(mapped, hinting);
        fonts.symbol_maps.as_mut().unwrap()[0].range.end = 'y';
        let remapped = FontKey::from_fonts(7, &fonts);
        assert_ne!(remapped, mapped);
        fonts.inner.clear();
        assert_ne!(FontKey::from_fonts(7, &fonts), remapped);
        assert_ne!(
            FontKey::from_fonts(8, &fonts),
            FontKey::from_fonts(7, &fonts)
        );
    }

    #[test]
    fn prompt_hits_share_rows_and_caret_offsets_without_deep_clones() {
        thread_local! {
            static TEST: RefCell<WrapCache<Rc<Vec<InputWrapRow>>>> = RefCell::new(WrapCache::default());
        }
        let builds = std::cell::Cell::new(0);
        let text = "界 e\u{301} [Image1]\n👩‍💻";
        let wrap = || {
            cached(
                &TEST,
                key(),
                text,
                || {
                    builds.set(builds.get() + 1);
                    Rc::new(super::super::wrap_agent_prompt_rows_with(text, 4.0, |s| {
                        s.chars().count() as f32
                    }))
                },
                |_| 256,
            )
        };
        let original = wrap();
        let offsets = original[0].offsets.as_ptr();
        for _ in 0..100 {
            let hit = wrap();
            assert!(Rc::ptr_eq(&original, &hit));
            assert_eq!(offsets, hit[0].offsets.as_ptr());
        }
        assert_eq!(builds.get(), 1);
        // Pane/prepared-layout owners survive eviction of the global entry.
        TEST.with(|cache| *cache.borrow_mut() = WrapCache::default());
        assert!(!original.is_empty());
        assert_eq!(original[0].offsets[0], 0.0);
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
        Rc::make_mut(&mut k.faces)[0].1 += 1;
        variants.push(k);
        let mut k = key();
        k.hinting = false;
        variants.push(k);
        let mut k = key();
        Rc::make_mut(&mut k.symbol_maps).push((1, 'a', 'z'));
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
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[1].start, rows[1].end), (4, 4));
        assert_eq!(rows[1].offsets, [0.0]);
        assert_eq!(
            crate::panels::agent_pane::input_controller::visual_row_index(&rows, 4),
            Some(1),
        );
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
    fn prompt_newlines_and_exact_fill_preserve_the_eof_insertion_row() {
        for (text, width, spans) in [
            ("", 2.0, vec![(0, 0)]),
            ("\n", 2.0, vec![(0, 0), (1, 1)]),
            ("a\n", 2.0, vec![(0, 1), (2, 2)]),
            ("a\n\n", 2.0, vec![(0, 1), (2, 2), (3, 3)]),
            ("界\n", 2.0, vec![(0, 3), (4, 4)]),
            ("a", 2.0, vec![(0, 1)]),
            ("ab", 2.0, vec![(0, 2), (2, 2)]),
            ("abc", 2.0, vec![(0, 2), (2, 3)]),
            ("ab\n", 2.0, vec![(0, 2), (3, 3)]),
        ] {
            let rows = super::super::wrap_agent_prompt_rows_with(text, width, |s| {
                s.chars().count() as f32
            });
            assert_eq!(
                rows.iter()
                    .map(|row| (row.start, row.end))
                    .collect::<Vec<_>>(),
                spans
            );
            assert_eq!(
                crate::panels::agent_pane::input_controller::visual_row_index(
                    &rows,
                    text.len()
                ),
                Some(rows.len() - 1),
            );
            for row in rows.iter().filter(|row| row.start == row.end) {
                assert_eq!(row.offsets, [0.0]);
            }
        }
    }

    fn slot_rows(text: &str, width: f32, slot: f32) -> Vec<InputWrapRow> {
        super::super::wrap_agent_prompt_rows_with_slot(text, width, slot, |part| {
            if part == "[Image1]" {
                140.0
            } else {
                part.chars()
                    .map(|ch| match ch {
                        'W' => 23.0,
                        'i' => 2.0,
                        _ => 19.0,
                    })
                    .sum()
            }
        })
    }

    #[test]
    fn prompt_slot_closes_proportional_remainder_before_narrow_or_wide_append() {
        use crate::panels::agent_pane::input_controller::visual_row_index;
        // Five proportional glyphs leave 5px: text fits, the 9.6px caret does not.
        let rows = slot_rows("aaaaa", 100.0, 9.6);
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].start, rows[0].end), (0, 5));
        assert_eq!(rows[0].offsets.last(), Some(&95.0));
        assert_eq!((rows[1].start, rows[1].end), (5, 5));
        assert_eq!(visual_row_index(&rows, 5), Some(1));
        for next in ["i", "W"] {
            let text = format!("aaaaa{next}");
            let appended = slot_rows(&text, 100.0, 9.6);
            assert_eq!(appended.len(), 2);
            assert_eq!((appended[0].start, appended[0].end), (0, 5));
            assert_eq!(appended[0].offsets, rows[0].offsets);
            assert_eq!((appended[1].start, appended[1].end), (5, 6));
            assert_eq!(visual_row_index(&appended, 6), Some(1));
            // Editing an interior boundary keeps the existing end affinity.
            assert_eq!(visual_row_index(&appended, 5), Some(0));
        }
    }

    #[test]
    fn prompt_closed_row_newline_consumes_pending_soft_break() {
        for (text, spans) in [
            ("aaaaa\n", vec![(0, 5), (6, 6)]),
            ("aaaaa\ni", vec![(0, 5), (6, 7)]),
            ("aaaaa\n\n", vec![(0, 5), (6, 6), (7, 7)]),
            ("[Image1]\n", vec![(0, 8), (9, 9)]),
        ] {
            let rows = slot_rows(text, 100.0, 9.6);
            assert_eq!(
                rows.iter()
                    .map(|row| (row.start, row.end))
                    .collect::<Vec<_>>(),
                spans
            );
        }
    }

    #[test]
    fn prompt_slot_eof_successor_scrolls_at_capacity_five() {
        use crate::panels::agent_pane::input_controller::visual_row_index;
        use crate::panels::agent_pane::input_viewport::InputViewport;
        let text = "aaaaa".repeat(5);
        let rows = slot_rows(&text, 100.0, 9.6);
        assert_eq!(rows.len(), 6);
        let caret = visual_row_index(&rows, text.len()).unwrap();
        assert_eq!(caret, 5);
        let mut viewport = InputViewport::default();
        let first = viewport.update(
            &text,
            text.len(),
            caret,
            rows.len(),
            5,
            [18.0, 0.0, 100.0, 120.0],
            24.0,
        );
        assert_eq!(first, 1);
        assert_eq!(caret - first, 4);
    }

    #[test]
    fn prompt_exact_slot_fit_and_fractional_scale_thresholds() {
        for scale in [0.6, 0.75, 0.95, 1.0, 1.2, 1.5, 2.0] {
            let slot = super::super::prompt_caret_width(16.0 * scale);
            let glyph = 71.0 * scale;
            let exact = glyph + slot;
            let below = f32::from_bits(exact.to_bits() - 1);
            let above = f32::from_bits(exact.to_bits() + 1);
            for (width, count) in [(below, 2), (exact, 1), (above, 1)] {
                let rows = super::super::wrap_agent_prompt_rows_with_slot(
                    "a",
                    width,
                    slot,
                    |_| glyph,
                );
                assert_eq!(rows.len(), count, "scale={scale}, width={width}");
                assert_eq!(rows[0].offsets, [0.0, glyph]);
            }
            let rect = super::super::prompt_caret_rect(
                18.0 * scale,
                0.0,
                glyph,
                exact,
                slot,
                20.0 * scale,
            );
            assert_eq!(rect[0], 18.0 * scale + glyph);
            assert!((rect[2] - slot).abs() < 0.0001);
        }
    }

    #[test]
    fn prompt_caret_uses_true_insertion_x_and_clips_oversized_atomic_attachment() {
        // The old independent box-right-minus-28 clamp shifted this caret left.
        let rect = super::super::prompt_caret_rect(18.0, 0.0, 90.0, 100.0, 9.6, 20.0);
        assert_eq!(rect, [108.0, 0.0, 9.6, 20.0]);
        assert_eq!(
            super::super::prompt_caret_rect(18.0, 0.0, 95.0, 100.0, 9.6, 20.0),
            [113.0, 0.0, 5.0, 20.0]
        );
        let text = "i [Image1]i";
        let rows = slot_rows(text, 100.0, 9.6);
        assert_eq!(
            rows.iter()
                .map(|row| &text[row.start..row.end])
                .collect::<Vec<_>>(),
            ["i ", "[Image1]", "i"]
        );
        assert_eq!(rows[1].offsets.len(), 9);
        assert_eq!(rows[1].offsets.last(), Some(&140.0));
        assert!(rows[1].offsets.windows(2).all(|pair| pair[0] < pair[1]));
        let clipped = super::super::prompt_caret_rect(18.0, 0.0, 140.0, 100.0, 9.6, 20.0);
        assert_eq!(clipped, [158.0, 0.0, 0.0, 20.0]);
        let eof = slot_rows("[Image1]", 100.0, 9.6);
        assert_eq!(eof.len(), 2);
        assert_eq!((eof[1].start, eof[1].end), (8, 8));
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
                    Rc::make_mut(&mut k.faces)[0].1 += 1;
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
