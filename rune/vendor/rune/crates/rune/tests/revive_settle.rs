use rune::{Color, Cx, El, Env, Runtime, Transition, col, el};

fn view(shown: &(bool, f32), _: &mut Cx<(bool, f32)>) -> El {
    let mut c = col().size(200.0, 200.0).child(el().h(shown.1));
    if shown.0 {
        c = c.child(el().key("card").size(80.0, 40.0).fill(Color::WHITE).transition(Transition::opacity()));
    }
    c
}

/// Removes the card, runs most of its fade, then brings it back (moved down by 20 pt).
fn return_mid_exit(settle: bool) -> Runtime<(bool, f32)> {
    let mut rt = Runtime::new((true, 0.0), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| s.0 = false);
    for _ in 0..24 {
        rt.frame(1.0 / 120.0);
    }
    let (_, o) = rt.presented("card").expect("still fading out");
    assert!(o > 0.0 && o < 0.5, "in the exit tail: {o}");
    rt.update(|s, tx| {
        *s = (true, 20.0);
        if settle {
            tx.settle();
        }
    });
    rt.frame(1.0 / 120.0);
    rt
}

#[test]
fn a_settled_return_mid_exit_appears_at_rest() {
    let mut rt = return_mid_exit(true);
    let (r, o) = rt.presented("card").unwrap();
    assert_eq!((r, o), ([0.0, 20.0, 80.0, 40.0], 1.0));
    for _ in 0..120 {
        rt.frame(1.0 / 120.0);
        assert_eq!(rt.presented("card").unwrap(), ([0.0, 20.0, 80.0, 40.0], 1.0));
    }
}

#[test]
fn an_animated_return_mid_exit_still_reverses_from_where_it_is() {
    let mut rt = return_mid_exit(false);
    let (_, o) = rt.presented("card").unwrap();
    assert!(o > 0.0 && o < 0.5, "reverses from the faded look: {o}");
    for _ in 0..240 {
        rt.frame(1.0 / 120.0);
    }
    assert_eq!(rt.presented("card").unwrap(), ([0.0, 20.0, 80.0, 40.0], 1.0));
}
