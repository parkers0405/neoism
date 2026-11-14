use super::Presence;

#[test]
fn a_prewarmed_panel_is_not_excluded_until_it_shows() {
    // A scoped prewarm orders in only for acquisition and orders out on drop.
    let mut p = Presence::new(true);
    assert!(!p.ordered());
    assert!(p.requested() && !p.excluded(), "a hidden prewarmed panel would blank every Space's thumbnail");
    let g = p.show();
    assert!(p.ordered());
    assert!(p.excluded(), "applied before the alpha starts rising");
    assert!(p.fade_done(g, false));
    assert!(p.excluded());
}

#[test]
fn a_hide_without_a_fade_drops_the_exclusion_at_once() {
    let mut p = Presence::new(true);
    p.show();
    p.hide(false);
    assert!(!p.ordered(), "resetting sharing alone cannot remove a panel from Spaces");
    assert!(!p.excluded() && p.requested());
}

#[test]
fn a_fading_hide_keeps_the_exclusion_until_its_fade_finishes() {
    let mut p = Presence::new(true);
    p.show();
    let g = p.hide(true);
    assert!(p.ordered(), "keep the fade on screen");
    assert!(p.excluded(), "the fade-out is still on screen");
    assert!(p.fade_done(g, true));
    assert!(!p.ordered(), "order out before restoring sharing");
    assert!(!p.excluded());
}

#[test]
fn a_show_during_a_fade_out_wins() {
    let mut p = Presence::new(true);
    p.show();
    let out = p.hide(true);
    let back = p.show();
    assert!(!p.fade_done(out, true), "the replaced fade-out's completion is stale");
    assert!(p.ordered(), "a stale completion must not order out the new show");
    assert!(p.excluded());
    assert!(p.fade_done(back, false));
    assert!(p.excluded());
}

#[test]
fn a_show_fade_completing_after_a_hide_changes_nothing() {
    let mut p = Presence::new(true);
    let shown = p.show();
    p.hide(false);
    assert!(!p.fade_done(shown, false));
    assert!(!p.excluded());
}

#[test]
fn a_toggle_while_hidden_waits_for_the_next_show() {
    let mut p = Presence::new(false);
    p.request(true);
    assert!(p.requested() && !p.excluded());
    p.show();
    assert!(p.excluded());
    p.request(false);
    assert!(!p.excluded(), "a toggle while shown applies at once");
    p.hide(false);
    p.request(true);
    p.request(false);
    p.show();
    assert!(!p.excluded(), "the last request wins");
}

#[test]
fn excluded_iff_shown_and_requested_across_every_sequence() {
    // Every sequence of up to five steps, checked against an independent model.
    #[derive(Clone, Copy, Debug)]
    enum Step { Show, Hide, HideFading, FinishFade, Request(bool) }
    let steps = [Step::Show, Step::Hide, Step::HideFading, Step::FinishFade, Step::Request(true), Step::Request(false)];
    let mut sequence = vec![0usize; 5];
    loop {
        for start in [false, true] {
            let mut p = Presence::new(start);
            let (mut shown, mut requested, mut pending): (bool, bool, Option<(u64, bool)>) = (false, start, None);
            for &i in &sequence {
                match steps[i] {
                    Step::Show => { pending = Some((p.show(), false)); shown = true; }
                    Step::Hide => { p.hide(false); pending = None; shown = false; }
                    Step::HideFading => pending = Some((p.hide(true), true)),
                    Step::FinishFade => if let Some((g, hiding)) = pending.take() {
                        assert!(p.fade_done(g, hiding));
                        if hiding { shown = false; }
                    },
                    Step::Request(on) => { p.request(on); requested = on; }
                }
                assert_eq!(p.excluded(), shown && requested, "{:?}", sequence.iter().map(|&i| steps[i]).collect::<Vec<_>>());
                assert_eq!(p.ordered(), shown, "hidden panels must leave Spaces regardless of their sharing history");
                assert_eq!(p.requested(), requested);
            }
        }
        let Some(i) = sequence.iter().rposition(|&i| i + 1 < steps.len()) else { break };
        sequence[i] += 1;
        sequence[i + 1..].iter_mut().for_each(|i| *i = 0);
    }
}
