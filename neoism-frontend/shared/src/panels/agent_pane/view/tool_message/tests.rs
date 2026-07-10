
use super::diff::snapshot_section_from_text;
use super::*;

#[test]
fn snapshot_diff_pairs_replacement_rows_by_line_number() {
    let before = "keep\nold one\nold two\ntail\n";
    let after = "keep\nnew one\nnew two\ntail\n";

    let section = snapshot_section_from_text("src/lib.rs".to_string(), before, after);
    let rows = section.lines;

    let remove_one = rows
        .iter()
        .position(|row| row.kind == DiffLineKind::Remove && row.text == "-old one")
        .expect("old one remove row");
    assert_eq!(rows[remove_one].line_number, Some(2));
    assert_eq!(rows[remove_one + 1].kind, DiffLineKind::Add);
    assert_eq!(rows[remove_one + 1].text, "+new one");
    assert_eq!(rows[remove_one + 1].line_number, Some(2));
    assert_eq!(rows[remove_one + 2].kind, DiffLineKind::Remove);
    assert_eq!(rows[remove_one + 2].line_number, Some(3));
    assert_eq!(rows[remove_one + 3].kind, DiffLineKind::Add);
    assert_eq!(rows[remove_one + 3].line_number, Some(3));
}
