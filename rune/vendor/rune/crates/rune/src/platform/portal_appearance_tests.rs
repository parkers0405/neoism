use super::*;

#[test]
fn only_an_explicit_preference_picks_an_appearance() {
    assert_eq!(scheme_dark(1), Some(true));
    assert_eq!(scheme_dark(2), Some(false));
    assert_eq!(scheme_dark(0), None);
    assert_eq!(scheme_dark(7), None);
}

#[test]
fn reads_the_scheme_however_deeply_the_portal_wraps_it() {
    assert_eq!(scheme(&Variant::U32(2)), Some(2));
    assert_eq!(scheme(&Variant::Variant(Box::new(Variant::U32(1)))), Some(1));
    assert_eq!(scheme(&Variant::Variant(Box::new(Variant::Variant(Box::new(Variant::U32(2)))))), Some(2));
    assert_eq!(scheme(&Variant::Str("prefer-dark".into())), None);
}
