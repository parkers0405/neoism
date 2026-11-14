//! Explicit state machines: one `enum State` whose variants carry only the data valid in them,
//! and one transition, `fn step(state: &mut State, event: Event) -> Effects<Effect>`, that
//! returns what should happen instead of doing it. The caller carries the effects out
//! (`tx.wake_at(due)`, `tx.instant()`, a request on a worker), so a transition is plain data in
//! and out, and a combination of flags that means nothing can't be built.
//!
//! Timers are events too: a timed variant keeps its `Attempt` and due time on
//! [`Tx::clock`](crate::Tx::clock), entering it returns an effect the caller turns into
//! [`Tx::wake_at`](crate::Tx::wake_at), and `poll` feeds the timer back as an event when it
//! comes due. A timer, reply or dialog answer whose attempt the state no longer holds is stale
//! and changes nothing.
//!
//! ```ignore
//! use rune::machine::{Attempt, Effects, Seq};
//!
//! enum State { Idle, Waiting { attempt: Attempt, due: f64 }, Open }
//! enum Event { Enter { now: f64 }, Leave, Due { attempt: Attempt, now: f64 } }
//! enum Effect { WakeAt(f64) }
//!
//! fn step(seq: &mut Seq, state: &mut State, event: Event) -> Effects<Effect> {
//!     match (&*state, event) {
//!         (State::Idle, Event::Enter { now }) => {
//!             let (attempt, due) = (seq.next(), now + 0.4);
//!             *state = State::Waiting { attempt, due };
//!             Effects::one(Effect::WakeAt(due))
//!         }
//!         (State::Waiting { attempt, due }, Event::Due { attempt: a, now }) if *attempt == a && now >= *due => {
//!             *state = State::Open;
//!             Effects::none()
//!         }
//!         (_, Event::Leave) => {
//!             *state = State::Idle;
//!             Effects::none()
//!         }
//!         _ => Effects::none(),
//!     }
//! }
//!
//! // The caller: a handler steps and carries the effects out.
//! for effect in step(&mut s.seq, &mut s.tip, Event::Enter { now: tx.clock() }) {
//!     match effect { Effect::WakeAt(due) => tx.wake_at(due) }
//! }
//! ```

/// A fencing token for an async request, a platform dialog or a timer: the state that started
/// it keeps the attempt, and its answer carries it back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Attempt(pub u32);

/// Issues fresh [`Attempt`]s, so an answer to an earlier one never matches a later state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Seq(u32);

impl Seq {
    /// A new attempt, different from every one this sequence issued before (until it wraps
    /// after 2³² attempts).
    pub fn next(&mut self) -> Attempt {
        self.0 = self.0.wrapping_add(1);
        Attempt(self.0)
    }

    /// The last attempt issued.
    pub fn current(self) -> Attempt {
        Attempt(self.0)
    }
}

/// What a transition asks for, in order. Up to two effects (nearly every transition) stay
/// inline; more go on the heap.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Effects<E> {
    #[default]
    None,
    One(E),
    Two(E, E),
    Many(Vec<E>),
}

impl<E> Effects<E> {
    pub const fn none() -> Self { Self::None }

    pub const fn one(e: E) -> Self { Self::One(e) }

    pub const fn two(a: E, b: E) -> Self { Self::Two(a, b) }

    pub const fn is_empty(&self) -> bool { matches!(self, Self::None) }

    /// Adds `e` after the effects already asked for.
    pub fn push(&mut self, e: E) {
        *self = match std::mem::take(self) {
            Self::None => Self::One(e),
            Self::One(a) => Self::Two(a, e),
            Self::Two(a, b) => Self::Many(vec![a, b, e]),
            Self::Many(mut v) => {
                v.push(e);
                Self::Many(v)
            }
        };
    }
}

impl<E> Extend<E> for Effects<E> {
    fn extend<I: IntoIterator<Item = E>>(&mut self, iter: I) {
        for e in iter { self.push(e); }
    }
}

impl<E> IntoIterator for Effects<E> {
    type Item = E;
    type IntoIter = EffectsIter<E>;

    fn into_iter(self) -> EffectsIter<E> {
        match self {
            Self::None => EffectsIter::Inline(None, None),
            Self::One(a) => EffectsIter::Inline(Some(a), None),
            Self::Two(a, b) => EffectsIter::Inline(Some(a), Some(b)),
            Self::Many(v) => EffectsIter::Many(v.into_iter()),
        }
    }
}

/// The effects of an [`Effects`], in order.
pub enum EffectsIter<E> {
    Inline(Option<E>, Option<E>),
    Many(std::vec::IntoIter<E>),
}

impl<E> Iterator for EffectsIter<E> {
    type Item = E;

    fn next(&mut self) -> Option<E> {
        match self {
            Self::Inline(a, b) => a.take().or_else(|| b.take()),
            Self::Many(it) => it.next(),
        }
    }
}
