use super::*;

#[test]
fn ink_sits_inside_the_box_and_spaces_have_none() {
    let style = TextStyle::new(12.0, 400.0).lh(16.0);
    let (w, h) = super::super::measure("H", &[], style, None);
    let h_ink = ink("H", style).expect("H draws");
    let [x, y, iw, ih] = h_ink.bounds;
    assert!(x >= 0.0 && y >= 0.0 && x + iw <= w && y + ih <= h, "{h_ink:?} in {w}×{h}");
    assert!(ih > 7.0 && ih < 10.0, "an H is about cap height: {ih}");
    let [cx, _] = h_ink.centre();
    assert!((h_ink.mass[0] - cx).abs() < 0.2, "H is symmetric: {h_ink:?}");
    assert_eq!(ink(" ", style), None);
}

#[test]
fn a_wider_word_has_wider_ink() {
    let style = TextStyle::new(12.0, 400.0);
    assert!(ink("HH", style).unwrap().bounds[2] > ink("H", style).unwrap().bounds[2] * 1.8);
}

#[test]
fn the_cache_stays_bounded_in_entries_and_bytes() {
    let style = TextStyle::new(12.0, 400.0);
    let mut cache = Cache::default();
    for i in 0..MAX_ENTRIES + 50 {
        let text = format!("t{i}");
        cache.insert(Cache::key(&text, style), &text, style, None);
    }
    assert_eq!(cache.entries.len(), MAX_ENTRIES);
    assert_eq!(cache.order.len(), MAX_ENTRIES);
    assert_eq!(cache.get(Cache::key("t0", style), "t0", style), None, "the oldest went");
    let last = format!("t{}", MAX_ENTRIES + 49);
    assert_eq!(cache.get(Cache::key(&last, style), &last, style), Some(None), "the newest stays");

    let long = "x".repeat(MAX_BYTES / 8);
    for i in 0..16 {
        let text = format!("{long}{i}");
        cache.insert(Cache::key(&text, style), &text, style, None);
    }
    assert!(cache.bytes <= MAX_BYTES, "{} bytes", cache.bytes);
    assert_eq!(cache.bytes, cache.entries.values().map(Entry::bytes).sum::<usize>());
    let huge = "x".repeat(MAX_BYTES);
    cache.insert(Cache::key(&huge, style), &huge, style, None);
    assert_eq!(cache.get(Cache::key(&huge, style), &huge, style), None, "one text over the budget isn't kept");
}

#[test]
fn a_hit_checks_the_text_and_style_not_only_the_hash() {
    let style = TextStyle::new(12.0, 400.0);
    let mut cache = Cache::default();
    let ink = Some(TextInk { bounds: [1.0, 2.0, 3.0, 4.0], mass: [2.0, 3.0] });
    cache.insert(7, "H", style, ink);
    assert_eq!(cache.get(7, "H", style), Some(ink));
    assert_eq!(cache.get(7, "I", style), None);
    assert_eq!(cache.get(7, "H", TextStyle::new(13.0, 400.0)), None);
    cache.insert(7, "I", style, None);
    assert_eq!((cache.get(7, "I", style), cache.entries.len(), cache.order.len()), (Some(None), 1, 1));
}
