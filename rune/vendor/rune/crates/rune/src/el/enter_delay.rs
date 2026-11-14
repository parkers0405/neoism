//! `El::enter_delay`: a new element holds its whole entrance, then plays it.
use crate::el::El;

impl El {
    /// Hold this element's entrance `seconds` after it mounts, then play all of it on its
    /// [`transition`](Self::transition)'s enter spring. Until then it waits at the insertion
    /// effect: offset, scale, blur, opacity and reveal stay at their absent values and leave
    /// together, so a row that slides and fades is still where it starts when it begins to
    /// show. It takes its place in layout at once, as any entrance does. Stagger a list with
    /// [`Stagger`](crate::Stagger):
    ///
    /// ```
    /// # use rune::{El, Stagger, Transition, col, el};
    /// # let items = ["Welcome", "Pick a theme", "Sign in"];
    /// let stagger = Stagger::new(0.06);
    /// let rows = items.iter().enumerate().map(|(i, item)| {
    ///     el().key(*item).h(44.0)
    ///         .transition(Transition::offset(0.0, 12.0).combined(Transition::opacity()))
    ///         .enter_delay(stagger.delay(i, items.len()))
    /// });
    /// let list: El = col().children(rows);
    /// ```
    ///
    /// The delay runs on the element's clock, so [`El::time_scale`] and the window's time scale
    /// stretch it with the entrance. Under Reduce Motion it is kept: the entrance turns into
    /// the short fade, at the same time. Only a fresh mount waits. An entrance nobody would
    /// see lands at once and ignores it: the first frame, a [`Tx::instant`](crate::Tx::instant)
    /// or [`Tx::settle`](crate::Tx::settle) commit (`Runtime::settle_all` also lands one still
    /// waiting), a hidden window, an element out of sight. So does an element that comes back
    /// while it's leaving. Removed while it waits, it goes at once when its insertion effect
    /// is invisible (it fades in), or else leaves from that effect on the exit spring. Changing
    /// the delay after it mounts does nothing.
    ///
    /// [`opacity_motion`](Self::opacity_motion) animates the element's own opacity, which
    /// multiplies the entrance's: its own delay applies to that change only.
    pub fn enter_delay(mut self, seconds: f32) -> Self {
        let seconds = if seconds.is_finite() { seconds.max(0.0) } else { 0.0 };
        self.vis.cold_mut().enter_delay = seconds;
        self
    }
}
