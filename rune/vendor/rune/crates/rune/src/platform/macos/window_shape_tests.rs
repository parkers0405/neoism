use super::Resample;

/// A window's drawables: presents number them, Metal reports them on screen later.
#[derive(Default)]
struct Sim { state: Resample, acquired: u64, on_screen: u64, resamples: usize }

impl Sim {
    fn present(&mut self) -> u64 { self.acquired += 1; self.state.presented(self.acquired); self.acquired }
    fn land(&mut self, n: u64) { self.on_screen = self.on_screen.max(n); }
    fn turn(&mut self) { if self.state.turn(self.on_screen) { self.resamples += 1; } }
    /// Present a frame, let a turn pass before it lands, then land it and turn again.
    fn frame(&mut self) {
        let n = self.present();
        self.turn();
        self.land(n);
        self.turn();
    }
}

#[test]
fn first_show_resamples_once_its_first_frame_is_on_screen() {
    let mut sim = Sim::default();
    sim.state.shown(0);
    sim.turn();
    assert_eq!(sim.resamples, 0, "nothing presented yet");
    let n = sim.present();
    sim.turn();
    assert_eq!(sim.resamples, 0, "presented but not on screen: the old pixels would be sampled");
    sim.land(n);
    sim.turn();
    assert_eq!(sim.resamples, 1);
    sim.turn();
    assert_eq!(sim.resamples, 1, "once per show");
}

#[test]
fn ordinary_frames_never_resample() {
    let mut sim = Sim::default();
    for _ in 0..20 { sim.frame(); }
    assert_eq!(sim.resamples, 0);
    sim.state.shown(sim.acquired);
    sim.turn();
    assert_eq!(sim.resamples, 1);
    for _ in 0..20 { sim.frame(); }
    assert_eq!(sim.resamples, 1, "frames after the resample don't repeat it");
}

#[test]
fn resize_and_scale_wait_for_a_frame_drawn_after_them() {
    for change in ["resize", "scale"] {
        let mut sim = Sim::default();
        sim.state.shown(0);
        sim.frame();
        assert_eq!(sim.resamples, 1);
        // A frame from before the change lands after it: not good enough.
        let stale = sim.present();
        sim.state.redraw();
        sim.land(stale);
        sim.turn();
        assert_eq!(sim.resamples, 1, "{change}: a frame presented before it is the old shape");
        sim.frame();
        assert_eq!(sim.resamples, 2, "{change}: the first frame after it");
        sim.frame();
        assert_eq!(sim.resamples, 2, "{change}: once");
    }
}

#[test]
fn a_live_resize_resamples_after_it_settles() {
    let mut sim = Sim::default();
    for _ in 0..10 {
        sim.state.redraw();
        let n = sim.present();
        sim.turn();
        // The next resize arrives before this frame lands.
        sim.state.redraw();
        sim.land(n);
        sim.turn();
    }
    assert_eq!(sim.resamples, 0, "each frame was already stale when it landed");
    sim.frame();
    assert_eq!(sim.resamples, 1);
}

#[test]
fn prewarm_resamples_after_its_hidden_frame_and_reveal_after_the_cached_one() {
    let mut sim = Sim::default();
    // The launcher's prewarm: its first frame draws at alpha zero.
    sim.state.redraw();
    sim.frame();
    assert_eq!(sim.resamples, 1, "after the prewarm frame lands");
    // Hidden refreshes change the cached pixels without a resample.
    sim.frame();
    sim.frame();
    assert_eq!(sim.resamples, 1);
    // Revealing cached pixels draws nothing new; the last frame is already on screen.
    sim.state.shown(sim.acquired);
    sim.turn();
    assert_eq!(sim.resamples, 2, "the reveal resamples on the next turn");
    sim.state.hidden();
    // A reveal right after a hidden refresh waits for that frame to land.
    let n = sim.present();
    sim.state.shown(n);
    sim.turn();
    assert_eq!(sim.resamples, 2);
    sim.land(n);
    sim.turn();
    assert_eq!(sim.resamples, 3);
}

#[test]
fn hiding_cancels_a_pending_resample() {
    let mut sim = Sim::default();
    sim.state.shown(0);
    let n = sim.present();
    sim.state.hidden();
    sim.land(n);
    sim.turn();
    assert_eq!(sim.resamples, 0);
}

#[test]
fn a_show_keeps_waiting_for_the_frame_a_resize_asked_for() {
    let mut sim = Sim::default();
    sim.frame();
    sim.state.redraw();
    sim.state.shown(sim.acquired);
    sim.turn();
    assert_eq!(sim.resamples, 0, "the frame on screen predates the resize");
    sim.frame();
    assert_eq!(sim.resamples, 1);
}
