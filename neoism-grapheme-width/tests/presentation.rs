use neoism_grapheme_width::emoji::Presentation::{self, Emoji, Text};

#[test]
fn character_defaults_distinguish_text_and_emoji() {
    for c in ['A', '\u{2764}', '\u{fe0e}', '\u{fe0f}', '\u{e0b0}'] {
        assert_eq!(Presentation::for_char(c), Text, "{c:?}");
    }
    for c in ['\u{231a}', '\u{1f600}'] {
        assert_eq!(Presentation::for_char(c), Emoji, "{c:?}");
    }
}

#[test]
fn variation_selectors_preserve_the_base_default() {
    // Heart defaults to text; watch defaults to emoji. Both support overrides.
    for (base, default) in [("\u{2764}", Text), ("\u{231a}", Emoji)] {
        assert_eq!(Presentation::for_grapheme(base), (default, None));
        assert_eq!(
            Presentation::for_grapheme(&format!("{base}\u{fe0e}")),
            (default, Some(Text))
        );
        assert_eq!(
            Presentation::for_grapheme(&format!("{base}\u{fe0f}")),
            (default, Some(Emoji))
        );
    }
}

#[test]
fn unsupported_variation_sequences_do_not_force_emoji() {
    for s in ["A\u{fe0e}", "A\u{fe0f}", "\u{fe0e}", "\u{fe0f}"] {
        assert_eq!(Presentation::for_grapheme(s), (Text, None), "{s:?}");
    }
}

#[test]
fn fallback_scans_the_entire_grapheme_for_emoji() {
    // A keycap and an accented letter contain no Emoji_Presentation character.
    for s in ["", "e\u{301}", "1\u{fe0f}\u{20e3}"] {
        assert_eq!(Presentation::for_grapheme(s), (Text, None), "{s:?}");
    }
    // Rainbow flag starts with a text-default flag; the rainbow later in the
    // ZWJ sequence must still make the fallback classify the cluster as emoji.
    for s in [
        "\u{1f3f3}\u{fe0f}\u{200d}\u{1f308}",
        "\u{1f469}\u{200d}\u{1f4bb}",
        "\u{1f1fa}\u{1f1f8}",
    ] {
        assert_eq!(Presentation::for_grapheme(s), (Emoji, None), "{s:?}");
    }
}
