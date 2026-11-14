use super::*;

fn glyph_ids(text: &str) -> Vec<u32> {
    let mut ts = TextSystem::default();
    let layout = ts.layout_rich(text, &[], TextStyle::new(13.0, 400.0), 2.0, None);
    let mut ids = Vec::new();
    for line in layout.lines() {
        for item in line.items() {
            if let PositionedLayoutItem::GlyphRun(run) = item {
                ids.extend(run.glyphs().map(|g| g.id));
            }
        }
    }
    ids
}

/// Han text shapes with real glyphs (not .notdef) whenever a CJK font is installed; Latin
/// text keeps the app's own family.
#[test]
fn cjk_falls_back_to_an_installed_font() {
    if system_font_files(&CJK_FONT_FILES).is_empty() {
        eprintln!("skipping: no CJK font installed (set XDG_DATA_DIRS to a dir with fonts/)");
        return;
    }
    let han = glyph_ids("你好世界");
    assert_eq!(han.len(), 4);
    assert!(han.iter().all(|id| *id != 0), "tofu: {han:?}");
    assert_eq!(glyph_ids("Hello"), glyph_ids("Hello"));
}

/// Korean, Japanese and Chinese each shape without missing glyphs where a font for them is
/// installed (Hex #820: Hangul drew boxes on macOS, which ships Apple SD Gothic Neo).
#[test]
fn hangul_kana_and_han_fall_back_to_installed_fonts() {
    let installed = |fragments: &[&str]| !system_font_files(fragments).is_empty();
    let korean = installed(&["applesdgothicneo", "malgun", "notosanskr", "notosanscjk", "applegothic"]);
    let japanese = installed(&["hiragino", "yugoth", "meiryo", "notosansjp", "notosanscjk"]);
    for (text, has_font) in [("안녕하세요 세계", korean), ("こんにちは世界", japanese), ("你好世界", installed(&CJK_FONT_FILES))] {
        if !has_font {
            eprintln!("skipping {text}: no font for it installed");
            continue;
        }
        let ids = glyph_ids(text);
        assert!(ids.iter().all(|id| *id != 0), "tofu in {text}: {ids:?}");
    }
}

/// Han text falls back to the language's fonts: with Japanese and Simplified Chinese fonts both
/// installed, `ja` and `zh` draw the same characters from different fonts (Hex #1382: Japanese
/// interface text was drawn in a Chinese font's forms).
#[test]
fn han_text_falls_back_to_the_languages_font() {
    let japanese = !system_font_files(&["notosanscjkjp", "notosansjp", "yugoth", "meiryo", "hiragino"]).is_empty();
    let chinese = !system_font_files(&["notosanscjksc", "notosanssc", "msyh", "pingfang"]).is_empty();
    if !(japanese && chinese) {
        eprintln!("skipping: needs a Japanese and a Simplified Chinese font");
        return;
    }
    let fonts = |tag: &str| {
        let mut ts = TextSystem::default();
        ts.set_language(parley::fontique::Language::parse(tag).ok());
        let layout = ts.layout_rich("直骨", &[], TextStyle::new(13.0, 400.0), 2.0, None);
        let mut fonts = Vec::new();
        for line in layout.lines() {
            for item in line.items() {
                if let PositionedLayoutItem::GlyphRun(run) = item {
                    let font = run.run().font();
                    fonts.push((font.data.id(), font.index));
                }
            }
        }
        fonts
    };
    assert_ne!(fonts("ja"), fonts("zh"));
}

/// A tab shapes as a space: no missing-glyph box, and the same advance.
#[test]
fn a_tab_draws_as_a_space() {
    assert_eq!(glyph_ids("a\tb"), glyph_ids("a b"));
    let mut ts = TextSystem::default();
    let sans = TextStyle::new(13.0, 400.0);
    assert_eq!(ts.measure("\tx", sans, 2.0), ts.measure(" x", sans, 2.0));
}

/// In monospace text (a mono style or mono spans) a tab reaches the next 4-column stop,
/// counted from the line or the mono run, and stays one byte.
#[test]
fn mono_tabs_fill_to_four_column_stops() {
    let mut ts = TextSystem::default();
    let mono = TextStyle::new(13.0, 400.0).family(Family::Mono);
    let width = |ts: &mut TextSystem, text: &str, spans: &[Span], style: TextStyle| ts.layout_rich(text, spans, style, 2.0, None).full_width();
    for (tabbed, spaced) in [("\tx", "    x"), ("ab\tx", "ab  x"), ("abc\tx", "abc x"), ("abcd\tx", "abcd    x"), ("\t\tx", "        x"), ("  \tx", "    x")] {
        let (a, b) = (width(&mut ts, tabbed, &[], mono), width(&mut ts, spaced, &[], mono));
        assert!((a - b).abs() < 0.01, "{tabbed:?}: {a} vs {spaced:?}: {b}");
    }
    // A mono span counts from its own start, and from each line in it.
    let sans = TextStyle::new(13.0, 400.0);
    let span = |text: &str, from: usize| [Span::new(from, text.len()).mono()];
    let (tabbed, spaced) = ("Run a\tb", "Run a   b");
    let (a, b) = (width(&mut ts, tabbed, &span(tabbed, 4), sans), width(&mut ts, spaced, &span(spaced, 4), sans));
    assert!((a - b).abs() < 0.01, "{a} vs {b}");
    let layout = ts.layout_rich("x\n\ty", &span("x\n\ty", 0), sans, 2.0, None);
    let y = parley::Cluster::from_byte_index(&layout, 3).unwrap();
    let spaced = ts.layout_rich("x\n    y", &span("x\n    y", 0), sans, 2.0, None);
    let y_spaced = parley::Cluster::from_byte_index(&spaced, 6).unwrap();
    let x = |c: &parley::Cluster<'_, Ink>| c.visual_offset().unwrap_or_default();
    assert!((x(&y) - x(&y_spaced)).abs() < 0.01, "{} vs {}", x(&y), x(&y_spaced));
    // Proportional text keeps a tab one space wide.
    assert_eq!(ts.measure("\tx", sans, 2.0), ts.measure(" x", sans, 2.0));
}

/// Monospace text never takes ligatures or contextual alternates: Inter draws `->` as an
/// arrow in prose, but code keeps both characters.
#[test]
fn mono_text_has_no_ligatures() {
    let mut ts = TextSystem::default();
    let ids = |ts: &mut TextSystem, text: &str, spans: &[Span], style: TextStyle| -> Vec<u32> {
        let layout = ts.layout_rich(text, spans, style, 2.0, None);
        layout
            .lines()
            .flat_map(|line| line.items())
            .filter_map(|item| match item {
                PositionedLayoutItem::GlyphRun(run) => Some(run.glyphs().map(|g| g.id).collect::<Vec<_>>()),
                _ => None,
            })
            .flatten()
            .collect()
    };
    let sans = TextStyle::new(13.0, 400.0);
    let mono = sans.family(Family::Mono);
    let apart: Vec<u32> = ["a", "-", ">", "b"].iter().flat_map(|c| ids(&mut ts, c, &[], mono)).collect();
    assert_ne!(ids(&mut ts, "a->b", &[], sans), apart, "fixture: Inter's arrow in prose");
    assert_eq!(ids(&mut ts, "a->b", &[], mono), apart);
    assert_eq!(ids(&mut ts, "a->b", &[Span::new(0, 4).mono()], sans), apart);
    // Under `trak` correction too, which turns optional ligatures back on for prose.
    assert_eq!(ids(&mut ts, "a->b", &[Span::new(0, 4).mono().size(20.0)], sans), apart);
    // Prose around an inline mono span keeps its ligatures.
    let prose_arrow = ids(&mut ts, "a->b", &[], sans);
    let mixed = ids(&mut ts, "a->ba->ba->b", &[Span::new(4, 8).mono()], sans);
    let expected: Vec<u32> = [prose_arrow.as_slice(), apart.as_slice(), prose_arrow.as_slice()].concat();
    assert_eq!(mixed, expected);
}

/// `Family::SystemMono` shapes with the platform's monospaced font when one is installed, so
/// every character advances the same; without one it is the app's mono slot.
#[test]
fn system_mono_is_monospaced_or_the_mono_slot() {
    let mut ts = TextSystem::default();
    let style = TextStyle::new(13.0, 400.0).family(Family::SystemMono);
    let width = |ts: &mut TextSystem, text: &str, style: TextStyle| ts.layout_rich(text, &[], style, 2.0, None).full_width();
    if ts.system_mono().is_empty() {
        eprintln!("skipping the monospace check: no system monospaced font installed");
        assert_eq!(width(&mut ts, "iiii MMMM", style), width(&mut ts, "iiii MMMM", style.mono()));
        return;
    }
    let (narrow, wide) = (width(&mut ts, "iiiiiiii", style), width(&mut ts, "MMMMMMMM", style));
    assert!((narrow - wide).abs() < 0.5, "{narrow} vs {wide}");
    assert!(width(&mut ts, "iiiiiiii", TextStyle::new(13.0, 400.0)) < narrow * 0.7, "the sans slot is proportional");
    let italic = width(&mut ts, "iiiiiiii", style.italic());
    assert!((italic - narrow).abs() < 0.5, "italic keeps the cell: {italic} vs {narrow}");
}

/// `ui-monospace` in the mono slot puts mono spans and `.mono()` in `Family::SystemMono`'s
/// font, so inline code matches a code block set in it.
#[test]
fn ui_monospace_in_the_mono_slot_is_system_mono() {
    let fonts = |ts: &mut TextSystem, text: &str, spans: &[Span], style: TextStyle| {
        let mut ids = Vec::new();
        runs(ts, text, spans, style, |_, font| ids.push((font.data.as_ptr() as usize, font.offset)));
        ids
    };
    let code = &[Span { mono: true, ..Span::new(4, 8) }];
    let mut ts = TextSystem::new(&[], ("Inter", "ui-monospace, Inter"));
    let block = fonts(&mut ts, "code", &[], TextStyle::new(13.0, 400.0).family(Family::SystemMono));
    let spans = fonts(&mut ts, "see code", code, TextStyle::new(13.0, 400.0));
    assert_eq!(spans.last(), block.last(), "the span shapes with the code block's font");
    assert_eq!(fonts(&mut ts, "code", &[], TextStyle::new(13.0, 400.0).mono()), block);
    if !ts.system_mono().is_empty() {
        let mut plain = TextSystem::new(&[], ("Inter", "Inter"));
        assert_ne!(fonts(&mut plain, "see code", code, TextStyle::new(13.0, 400.0)).last(), block.last());
    }
}

/// With no monospace font to stand for `ui-monospace`, the mono slot falls back to sans
/// rather than to no named family at all.
#[test]
fn ui_monospace_with_no_mono_font_falls_back_to_sans() {
    let names = |list: &str, mono: &[String]| {
        family_list(list, mono, "Inter, sans-serif")
            .into_iter()
            .map(|f| match f { FontFamilyName::Named(n) => n.into_owned(), FontFamilyName::Generic(g) => format!("{g:?}") })
            .collect::<Vec<_>>()
    };
    assert_eq!(names("ui-monospace", &[]), ["Inter", "SansSerif"]);
    assert_eq!(names("ui-monospace", &["Menlo".into()]), ["Menlo"]);
    assert_eq!(names("ui-monospace, Iosevka", &[]), ["Iosevka"], "a named family after it still wins");
    assert_eq!(names("monospace", &[]), ["Monospace"], "only ui-monospace falls back");
}

/// Each glyph run of `text` with the font it shaped with.
fn runs(ts: &mut TextSystem, text: &str, spans: &[Span], style: TextStyle, mut f: impl FnMut(&parley::GlyphRun<'_, Ink>, FontRef<'_>)) {
    let layout = ts.layout_rich(text, spans, style, 2.0, None);
    for line in layout.lines() {
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(run) = item else { continue };
            let font = run.run().font();
            f(&run, FontRef::from_index(font.data.as_ref(), font.index as usize).unwrap());
        }
    }
}

/// `(text range, synthetic skew, face style)` of each glyph run of `text`.
fn run_skews(ts: &mut TextSystem, text: &str, spans: &[Span], style: TextStyle) -> Vec<(std::ops::Range<usize>, Option<f32>, String)> {
    let mut out = Vec::new();
    runs(ts, text, spans, style, |run, font| {
        out.push((run.run().text_range(), run.run().synthesis().skew(), format!("{:?}", font.attributes().style())));
    });
    out
}

/// A face without italics (bundled Inter) leans synthetically, in the whole style or one
/// span; a family with an italic face (system SF, DejaVu) uses it unskewed.
#[test]
fn italic_uses_the_real_face_or_a_synthetic_oblique() {
    let mut ts = TextSystem::default();
    let upright = TextStyle::new(13.0, 400.0);
    assert!(run_skews(&mut ts, "Hello", &[], upright).iter().all(|r| r.1.is_none()));
    assert!(run_skews(&mut ts, "Hello", &[], upright.italic()).iter().all(|r| r.1.is_some()), "Inter has no italic");
    let spans = [Span { italic: true, ..Span::new(6, 10) }];
    for (range, skew, _) in run_skews(&mut ts, "plain emph", &spans, upright) {
        assert_eq!(skew.is_some(), range.start >= 6, "{range:?}");
    }
    // The synthetic lean widens an upright stem's ink and moves its top right.
    let mut quads = |style| {
        let l = ts.layout_rich("l", &[], style, 2.0, None);
        let mut q = Vec::new();
        ts.glyphs_for_layout(&l, 2.0, [0.0, 0.0], false, &mut q);
        q[0]
    };
    let (a, b) = (quads(upright), quads(upright.italic()));
    assert!(b.rect[2] >= a.rect[2] + 3.0, "upright {:?}, oblique {:?}", a.rect, b.rect);

    let real = if cfg!(target_os = "macos") { "System Font" } else { "DejaVu Sans" };
    if ts.font_cx.collection.family_id(real).is_none() {
        eprintln!("skipping real italic: {real} not loaded");
        return;
    }
    let style = upright.family(Family::Named(real)).italic();
    for (range, skew, face) in run_skews(&mut ts, "Hello", &[], style) {
        assert!(skew.is_none() && face != "Normal", "{range:?} of {real}: skew {skew:?}, face {face}");
    }
}

/// Hebrew, Arabic, Devanagari, Bengali and Thai load their system fonts on first use and
/// shape with real glyphs; Latin text loads none of them.
#[test]
fn scripts_load_their_system_fonts_on_first_use() {
    let mut ts = TextSystem::default();
    ts.layout_rich("Hello, “café” — naïve…", &[], TextStyle::new(13.0, 400.0), 2.0, None);
    assert_eq!(ts.scripts, 0, "Latin text must not load script fallbacks");
    let samples = ["שלום עולם", "مرحبا بالعالم", "नमस्ते दुनिया", "ওহে বিশ্ব", "สวัสดีชาวโลก"];
    for (fonts, sample) in fallback::SCRIPT_FONTS.iter().zip(samples) {
        assert!(sample.chars().any(|ch| fonts.covers(ch)), "{sample} is not {:?}", fonts.script);
        if system_font_files(fonts.files).is_empty() {
            eprintln!("skipping {}: no font installed", String::from_utf8_lossy(fonts.script));
            continue;
        }
        let glyphs = faces(&mut ts, &format!("a {sample}"), &[], TextStyle::new(13.0, 400.0));
        assert!(glyphs.iter().all(|(_, id)| *id != 0), "tofu in {sample}: {glyphs:?}");
    }
    assert_eq!(ts.scripts.count_ones() as usize, fallback::SCRIPT_FONTS.len());
}

/// Fonts fontconfig knows outside the scanned directories (NixOS keeps them in its store) are
/// found too.
#[cfg(target_os = "linux")]
#[test]
fn system_fonts_include_fontconfig_files() {
    let Ok(out) = std::process::Command::new("fc-list").arg("--format=%{file}\n").output() else {
        eprintln!("skipping: no fc-list");
        return;
    };
    let found = system_font_files(&[""]);
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let font = [".ttf", ".otf", ".ttc", ".otc"].iter().any(|ext| line.to_lowercase().ends_with(ext));
        assert!(!font || found.iter().any(|path| path.as_os_str() == line), "{line} not found");
    }
}

#[test]
fn plex_keyboard_symbols_use_real_glyphs() {
    const PLEX: &[u8] = include_bytes!("../../../../themes/opencode/assets/fonts/IBMPlexMono-Regular.ttf");
    let mut ts = TextSystem::new(&[PLEX], ("IBM Plex Mono", "IBM Plex Mono"));
    if ts.symbol_families.is_empty() {
        eprintln!("skipping: no symbol font installed");
        return;
    }
    let plex = FontRef::from_index(PLEX, 0).unwrap();
    assert_eq!(plex.charmap().map('⌘'), 0, "fixture must need fallback");
    // Exercise standalone Common script, surrounding Latin and a mono rich-text span.
    for text in ["⌘⇧⏎⌥⌃⎋⌫←↑→↓↵", "Copy ⌘K ⇧⏎ ⌥⌃⎋⌫←↑→↓↵"] {
        for style in [TextStyle::new(13.0, 400.0), TextStyle::new(13.0, 400.0).mono()] {
            let spans = [Span { mono: true, ..Span::new(0, text.len()) }];
            for spans in [&[][..], &spans[..]] {
                let layout = ts.layout_rich(text, spans, style, 2.0, None);
                let mut count = 0;
                for line in layout.lines() {
                    for item in line.items() {
                        if let PositionedLayoutItem::GlyphRun(run) = item {
                            for glyph in run.glyphs() {
                                assert_ne!(glyph.id, 0, "missing symbol in {text:?}");
                                count += 1;
                            }
                        }
                    }
                }
                assert_eq!(count, text.chars().count());
            }
        }
    }
    let layout = ts.layout_rich("Hello", &[], TextStyle::new(13.0, 400.0), 2.0, None);
    let ids: Vec<_> = layout
        .lines()
        .flat_map(|line| line.items())
        .filter_map(|item| match item {
            PositionedLayoutItem::GlyphRun(run) => Some(run.glyphs().map(|glyph| glyph.id).collect::<Vec<_>>()),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(ids, "Hello".chars().map(|ch| u32::from(plex.charmap().map(ch))).collect::<Vec<_>>());
}

/// `(family, glyph id)` for every glyph of `text`, in visual order.
fn faces(ts: &mut TextSystem, text: &str, spans: &[Span], style: TextStyle) -> Vec<(String, u32)> {
    let mut out = Vec::new();
    runs(ts, text, spans, style, |run, font| {
        let family = font.localized_strings().find_by_id(swash::StringId::Family, None).map(|s| s.to_string()).unwrap_or_default();
        out.extend(run.glyphs().map(|g| (family.clone(), g.id)));
    });
    out
}

/// Math alphanumerics load the math fallback on first use and shape from one math font,
/// mono spans included; Latin around them stays on the app's font.
#[test]
fn math_alphanumerics_use_one_math_font() {
    let mut ts = TextSystem::default();
    ts.layout_rich("Hello, “café” — naïve…", &[], TextStyle::new(13.0, 400.0), 2.0, None);
    assert!(ts.math.is_none(), "Latin text must not load the math fallback");
    if system_font_files(fallback::MATH_FONT_FILES).is_empty() {
        eprintln!("skipping: no math font installed");
        return;
    }
    let formula = "𝐀𝐁𝐂 𝑥𝑦𝑧 𝒜ℬ𝒞 𝔄𝔅 𝕒𝕓 ℝℂ 𝟎𝟏𝟐 𝛼𝛽𝛾";
    let latin = faces(&mut ts, "Let ", &[], TextStyle::new(13.0, 400.0));
    for (style, spans) in [
        (TextStyle::new(13.0, 400.0), vec![]),
        (TextStyle::new(13.0, 400.0).mono(), vec![]),
        (TextStyle::new(13.0, 400.0), vec![Span { mono: true, ..Span::new(4, 4 + formula.len()) }]),
    ] {
        let text = format!("Let {formula}");
        let glyphs = faces(&mut ts, &text, &spans, style);
        assert!(ts.math.as_ref().is_some_and(|m| !m.is_empty()));
        assert!(glyphs.iter().all(|(_, id)| *id != 0), "tofu in {text:?}: {glyphs:?}");
        if spans.is_empty() && style.family == Family::Sans {
            assert_eq!(glyphs[..4], latin[..], "Latin before a formula keeps the app font");
        }
        let math_glyphs: Vec<_> = text[4..].chars().zip(&glyphs[4..]).filter(|(ch, _)| !ch.is_whitespace()).collect();
        let family = &math_glyphs[0].1.0;
        assert!(fallback::MATH_FAMILIES.contains(&family.as_str()), "{family} is not a math font");
        assert!(math_glyphs.iter().all(|(_, g)| &g.0 == family), "mixed math designs: {math_glyphs:?}");
    }
}

#[test]
fn routes_merge_math_and_emoji_presentation() {
    assert_eq!(fallback::routes("a 𝐀𝐁 b 𝟎"), vec![(2..10, Route::Math), (13..17, Route::Math)]);
    assert!(fallback::routes("plain ascii").is_empty());
    let (heart, flag) = ("❤\u{FE0F}", "🏳\u{FE0F}\u{200D}🌈");
    let flag_at = 1 + heart.len() + '❤'.len_utf8();
    assert_eq!(fallback::routes(&format!("a{heart}❤{flag}")), vec![(1..1 + heart.len(), Route::Emoji), (flag_at..flag_at + flag.len(), Route::Emoji)]);
}

/// `U+FE0F` asks for the colour emoji even when a text font has the base character; without
/// it the text form stays.
#[test]
fn emoji_presentation_selector_draws_in_colour() {
    let mut ts = TextSystem::default();
    if ts.emoji.is_none() {
        eprintln!("skipping: no colour emoji font installed");
        return;
    }
    let colour = |ts: &mut TextSystem, text: &str| {
        let layout = ts.layout_rich(text, &[], TextStyle::new(13.0, 400.0), 2.0, None);
        let mut quads = Vec::new();
        ts.glyphs_for_layout(&layout, 2.0, [0.0, 0.0], false, &mut quads);
        quads.iter().filter(|q| q.color != 0).count()
    };
    assert_eq!(colour(&mut ts, "a❤\u{FE0F}b ☀\u{FE0F} ✔\u{FE0F}"), 3);
    assert_eq!(colour(&mut ts, "a❤b ☺\u{FE0E}"), 0);
    assert_eq!(colour(&mut ts, "🏳\u{FE0F}\u{200D}🌈 👩🏽\u{200D}💻"), 2, "ZWJ sequences stay one image each");
}

/// Each glyph's ink top and bottom in px above the baseline, with its run's weight axis.
fn ink_extents(ts: &mut TextSystem, text: &str, style: TextStyle) -> Vec<(f32, f32)> {
    let layout = ts.layout_rich(text, &[], style, 2.0, None);
    let mut out = Vec::new();
    for line in layout.lines() {
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(glyph_run) = item else { continue };
            let run = glyph_run.run();
            let font = run.font();
            let font_ref = FontRef::from_index(font.data.as_ref(), font.index as usize).unwrap();
            let mut scaler = ts
                .scale_cx
                .builder(font_ref)
                .size(run.font_size())
                .normalized_coords(run.normalized_coords().iter().map(|c| c.to_bits()))
                .build();
            for g in glyph_run.glyphs() {
                let b = scaler.scale_outline(g.id as u16).unwrap().bounds();
                out.push((b.max.y, b.min.y));
            }
        }
    }
    out
}

/// Keyboard symbols a primary font lacks fall back to glyphs that stand on the baseline at
/// about cap height, so `⇧` and `⏎` don't shrink and sink next to the font's capitals.
#[test]
fn keyboard_symbol_fallbacks_match_cap_height() {
    const PLEX: &[u8] = include_bytes!("../../../../themes/opencode/assets/fonts/IBMPlexMono-Regular.ttf");
    let mut ts = TextSystem::new(&[PLEX], ("IBM Plex Mono", "IBM Plex Mono"));
    if !cfg!(target_os = "macos") || system_font_files(&["sfnsttf"]).is_empty() {
        eprintln!("skipping: no macOS system font");
        return;
    }
    for size in [13.0, 15.0] {
        let glyphs = ink_extents(&mut ts, "H⌘⇧⏎⌫⎋⌥", TextStyle::new(size, 400.0).mono());
        let (cap, _) = glyphs[0];
        for (ch, (top, bottom)) in "H⌘⇧⏎⌫⎋⌥".chars().zip(&glyphs).skip(1) {
            assert!((top / cap - 1.0).abs() < 0.12, "{ch} at {size}pt: top {top:.1} vs cap {cap:.1}");
            assert!(bottom.abs() < cap * 0.06, "{ch} at {size}pt: bottom {bottom:.1} off the baseline");
        }
    }
}

/// Emoji shape with the system's colour emoji font and draw as colour images about the
/// size of the text around them, not as blank alpha glyphs.
#[test]
fn emoji_draw_as_colour_images_beside_text() {
    let mut ts = TextSystem::default();
    if ts.emoji.is_none() {
        eprintln!("skipping: no colour emoji font installed");
        return;
    }
    let style = TextStyle::new(13.0, 400.0);
    let layout = ts.layout_rich("Done 🎉", &[], style, 2.0, None);
    let mut quads = Vec::new();
    ts.glyphs_for_layout(&layout, 2.0, [0.0, 0.0], false, &mut quads);
    let letters: Vec<_> = quads.iter().filter(|q| q.color == 0).collect();
    let emoji: Vec<_> = quads.iter().filter(|q| q.color != 0).collect();
    assert_eq!((letters.len(), emoji.len()), (4, 1), "{quads:?}");
    let e = emoji[0];
    let d = letters[0];
    assert!(e.rect[0] > letters[3].rect[0] + letters[3].rect[2], "after the text: {e:?}");
    assert!((20.0..40.0).contains(&e.rect[3]), "about 13pt at 2x: {e:?}");
    // It sits on the line: overlapping the capital D's ink, not above or below it.
    assert!(e.rect[1] < d.rect[1] + d.rect[3] && e.rect[1] + e.rect[3] > d.rect[1], "{e:?} vs {d:?}");
    let pixels = ts.color_glyph(e.color);
    let (w, h) = pixels.size();
    assert_eq!((w as f32, h as f32), (e.rect[2], e.rect[3]));
    let coloured = pixels.rgba().chunks(4).filter(|p| p[3] > 128 && (p[0].abs_diff(p[1]) > 40 || p[1].abs_diff(p[2]) > 40)).count();
    assert!(coloured > (w * h / 10) as usize, "only {coloured} coloured pixels");
    // Drawing it again reuses the image.
    let mut again = Vec::new();
    ts.glyphs_for_layout(&layout, 2.0, [0.0, 0.0], false, &mut again);
    assert_eq!(again.iter().filter(|q| q.color != 0).map(|q| q.color).collect::<Vec<_>>(), vec![e.color]);
    assert_eq!(ts.color_pixels.len(), 1);
}

#[test]
fn run_cells_draw_the_run_glyphs_and_marks_stay_with_their_base() {
    let mut ts = TextSystem::default();
    let style = crate::tok::type_::BODY;
    // No precomposed x-acute, so the accent is its own zero-advance glyph after "x".
    let run = Run::new("x\u{301}yAV");
    let r = ts.run_layout(&run, style, 2.0);
    let mut whole = Vec::new();
    ts.glyphs_for_layout(&r.layout, 2.0, [0.0, 0.0], false, &mut whole);
    let mut cells = Vec::new();
    for at in [0, 3, 4, 5] {
        let mut one = Vec::new();
        ts.glyphs_in(&r, at, 2.0, [0.0, 0.0], &mut one);
        cells.push(one);
    }
    assert_eq!(cells.iter().map(Vec::len).collect::<Vec<_>>(), [2, 1, 1, 1], "the accent draws in the x cell");
    let rects: Vec<_> = cells.concat().iter().map(|q| q.rect).collect();
    assert_eq!(rects, whole.iter().map(|q| q.rect).collect::<Vec<_>>(), "cells draw exactly the run's glyphs");
    // Widths tile the run and sum to its measured width.
    let sum: f32 = [0, 3, 4, 5].iter().map(|&at| r.advance(at, 2.0).unwrap()).sum();
    assert!((sum - ts.measure(run.as_str(), style, 2.0).0).abs() < 1e-3, "{sum}");
}

#[test]
fn run_lookups_off_a_grapheme_start_find_nothing() {
    let mut ts = TextSystem::default();
    let run = Run::new("x\u{301}y");
    let r = ts.run_layout(&run, crate::tok::type_::BODY, 2.0);
    for at in [1, 2, 4, 99] {
        assert_eq!((r.x(at), r.advance(at, 2.0)), (None, None), "byte {at}");
        let mut out = Vec::new();
        ts.glyphs_in(&r, at, 2.0, [0.0, 0.0], &mut out);
        assert!(out.is_empty());
    }
    // A cell whose byte is off the run measures and draws as plain text.
    use crate::{Cx, El, Env, Runtime};
    fn view(_: &(), _: &mut Cx<()>) -> El {
        crate::el::row().child(crate::el::text("y").advance_in(&Run::new("x\u{301}y"), 2)).child(crate::el::text("y").advance_in(&Run::new("y"), 99))
    }
    let mut rt = Runtime::new((), view, (200.0, 100.0), Env::new(true, 2.0));
    rt.frame(0.0);
    assert_eq!(rt.instances.iter().filter(|i| i.params[1] == crate::render::KIND_GLYPH).count(), 2);
}
