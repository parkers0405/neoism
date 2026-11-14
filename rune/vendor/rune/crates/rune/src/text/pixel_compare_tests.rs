use super::*;

/// Loose bounds for now: they catch a broken pipeline (missing glyphs, wrong blend, shifted
/// rows), not the remaining distance to CoreText. Tighten as the text path converges.
fn check(o: Options, ink: std::ops::RangeInclusive<f64>) {
    let rows = match run(o, |_, _, _, _| {}) {
        Ok(rows) => rows,
        Err(e) if o.set == Set::Sf || e.contains("adapter") || e.contains("GPU") => return eprintln!("skipped: {e}"),
        Err(e) => panic!("{e}"),
    };
    eprintln!("{}", table(&o, &rows));
    for r in &rows {
        let s = &r.stats;
        let what = format!("{} {}x {}", r.pair.name, r.scale, r.case.name);
        assert!(s.pixels > 0, "{what}: no ink in either image");
        assert!(ink.contains(&s.ink_ratio()), "{what}: ink ratio {:.3}", s.ink_ratio());
        let (over8, meanabs) = if r.case.whole_region { (0.2, 8.0) } else { (0.45, 25.0) };
        assert!(s.over8_share() < over8, "{what}: {:.1}% of pixels off by more than 8", 100.0 * s.over8_share());
        assert!(s.meanabs() < meanabs, "{what}: meanabs {:.2}", s.meanabs());
    }
}

#[test]
fn macos_smoothing_stays_near_smoothed_coretext() {
    for set in [Set::Inter, Set::Sf] {
        check(Options { smoothing: Some(FontSmoothing::On), ..Options::new(set) }, 0.75..=1.25);
    }
}

#[test]
fn smoothing_off_stays_near_unsmoothed_coretext() {
    for set in [Set::Inter, Set::Sf] {
        check(Options { smoothing: Some(FontSmoothing::Off), ..Options::new(set) }, 0.9..=1.1);
    }
}
