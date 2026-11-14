use super::*;

#[test]
fn first_seen_and_previous_values_are_stable_for_every_pass_of_one_commit() {
    let mut seen = false;
    let mut count = 9;
    {
        let _commit = enter();
        for _ in 0..4 {
            assert!(!*snapshot("item", &seen));
            assert_eq!(*snapshot("item", &count), 9, "types have separate histories");
            seen = true;
            count = 8;
        }
    }
    let _next_commit = enter();
    assert!(*snapshot("item", &seen));
    assert_eq!(*snapshot("item", &count), 8);
    assert_eq!(*snapshot("other", &12), 12);
}

#[test]
fn nested_commits_and_unwinds_restore_the_outer_scope() {
    let _outer = enter();
    assert_eq!(*snapshot("item", &9), 9);
    let _ = std::panic::catch_unwind(|| {
        let _inner = enter();
        assert_eq!(*snapshot("item", &8), 8);
        panic!("discard inner scope");
    });
    assert_eq!(*snapshot("item", &7), 9);
}
