//! A single bounded edit between authoritative document snapshots.
use std::ops::Range;

pub(super) fn changed_ranges(before: &str, after: &str) -> (Range<usize>, Range<usize>) {
    let mut start = before
        .bytes()
        .zip(after.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !before.is_char_boundary(start)
        || !after.is_char_boundary(start)
        || splits_crlf(before, start)
        || splits_crlf(after, start)
    {
        start -= 1;
    }
    let mut suffix = before.as_bytes()[start..]
        .iter()
        .rev()
        .zip(after.as_bytes()[start..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    while !before.is_char_boundary(before.len() - suffix)
        || !after.is_char_boundary(after.len() - suffix)
        || splits_crlf(before, before.len() - suffix)
        || splits_crlf(after, after.len() - suffix)
    {
        suffix -= 1;
    }
    (start..before.len() - suffix, start..after.len() - suffix)
}

fn splits_crlf(text: &str, offset: usize) -> bool {
    offset > 0
        && text.as_bytes().get(offset - 1) == Some(&b'\r')
        && text.as_bytes().get(offset) == Some(&b'\n')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_character_edit_does_not_resend_large_document() {
        let before = format!("{}old{}", "a".repeat(100_000), "z".repeat(100_000));
        let after = before.replacen("old", "older", 1);
        let (old, new) = changed_ranges(&before, &after);
        assert_eq!(old, 100_003..100_003);
        assert_eq!(&after[new], "er");
    }

    #[test]
    fn edits_reconstruct_unicode_and_all_line_endings() {
        let atoms = ["", "a", "é", "ê", "😀", "😁", "\r", "\n", "\r\n"];
        let strings: Vec<_> = atoms
            .iter()
            .flat_map(|a| atoms.iter().map(move |b| format!("{a}{b}")))
            .collect();
        for before in &strings {
            for after in &strings {
                let (old, new) = changed_ranges(before, after);
                for (text, range) in [(before, &old), (after, &new)] {
                    assert!(
                        text.is_char_boundary(range.start)
                            && text.is_char_boundary(range.end)
                    );
                    assert!(
                        !splits_crlf(text, range.start) && !splits_crlf(text, range.end)
                    );
                }
                let mut reconstructed = before.clone();
                reconstructed.replace_range(old, &after[new]);
                assert_eq!(&reconstructed, after);
            }
        }
    }
}
