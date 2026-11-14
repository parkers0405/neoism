//! [`Story`], and [`OwnStory`], which erases its state type so the playground holds stories
//! of any.

use super::*;

/// A story with its own state, made by `init` when it first shows and kept, timers and
/// animations included, until the playground closes. R makes it again. Handlers built for `P`
/// change it.
///
/// ```ignore
/// Story::new("Progress/Upload", Upload::new, upload_view)
///     .poll(Upload::tick)
///     .trigger('r', "Run", |u, play| play.after(0.5, |u, _| u.done = true))
///     .trigger('f', "Finish", |u, _| u.done = true)
///     .side(upload_knobs)
/// ```
pub struct Story<P: 'static> {
    pub(super) name: &'static str,
    init: Init<P>,
    view: View<P>,
    poll: Option<Box<dyn Fn(&mut P, f64, &mut Tx) -> bool>>,
    frame: Option<(f32, f32)>,
    stage: Stage,
    triggers: Vec<Trigger<P>>,
    /// Its view, told whether the keyboard is on it.
    side: Option<SideView<P>>,
    side_key: Option<SideKey<P>>,
    variants: Vec<(&'static str, Variant<P>)>,
}

impl<P: 'static> Story<P> {
    /// `name` may be a `"Group/Name"` path. `init` and `view` may be closures, so a story
    /// table can make its stories in a loop, each capturing its row.
    pub fn new(name: &'static str, init: impl Fn() -> P + 'static, view: impl Fn(&P, &mut Cx<P>) -> El + 'static) -> Self {
        Story { name, init: Init::New(Box::new(init)), view: stage::view(view), poll: None, frame: None, stage: Stage::Center, triggers: Vec::new(), side: None, side_key: None, variants: Vec::new() }
    }

    /// A story whose state `derive` makes from the app's real state, each time it's made: when
    /// it first shows, and on R. `S` is the playground's app state.
    pub fn from_real<S: 'static>(name: &'static str, derive: impl Fn(&S) -> P + 'static, view: impl Fn(&P, &mut Cx<P>) -> El + 'static) -> Self {
        let init = Init::Real(std::any::TypeId::of::<S>(), Box::new(move |real: &dyn Any| derive(real.downcast_ref().expect("checked by Playground::story"))));
        Story { init, ..Story::new(name, || unreachable!(), view) }
    }

    /// An overlay's view at its size, in a dashed frame over the stage (the overlay's clear
    /// parts stay clear), with a state made by `init`.
    pub fn overlay(name: &'static str, spec: &OverlaySpec<P>, init: impl Fn() -> P + 'static) -> Self {
        Story { frame: Some(spec.size), ..Story::new(name, init, spec.view) }
    }

    /// Called every frame while the story shows, as `AppSpec::poll` is, after its due steps:
    /// true when it changed the state.
    pub fn poll(mut self, poll: impl Fn(&mut P, f64, &mut Tx) -> bool + 'static) -> Self {
        self.poll = Some(Box::new(poll));
        self
    }

    /// A trigger: `key` (a letter, or `' '` for Space) runs `run` on the story's state, as its
    /// button in the side column does. [`Play::after`] queues the steps that follow, on the
    /// logical clock. An `'r'` trigger runs whenever the state is made: when the story first
    /// shows, and on R, which always replays. J and K are taken.
    pub fn trigger(mut self, key: char, label: &'static str, run: impl Fn(&mut P, &mut Play<'_, P>) + 'static) -> Self {
        self.triggers.push(trigger(key, label, run));
        self
    }

    /// [`Story::trigger`] with a step that takes just the [`Tx`], for a handler the app already
    /// has: `.trigger_tx('s', "Send", Chat::send)` for `fn send(&mut Chat, &mut Tx)`.
    pub fn trigger_tx(mut self, key: char, label: &'static str, run: impl Fn(&mut P, &mut Tx) + 'static) -> Self {
        self.triggers.push(trigger_tx(key, label, run));
        self
    }

    /// The app's own controls for this story, at the top of the side column, drawn from and
    /// acting on its state: knobs, say.
    pub fn side(mut self, view: impl Fn(&P, &mut Cx<P>) -> El + 'static) -> Self {
        let view = stage::view(view);
        self.side = Some(Box::new(move |p, _, cx| view(p, cx)));
        self.side_key = None;
        self
    }

    /// [`Story::side`] for controls that take keys: knobs, say. → or Tab puts the keyboard on
    /// them, and `view` gets `true` while it's there, to mark what the keys move. `key` gets
    /// each key first, and returns true when it took it. Esc and Tab always leave, and ←
    /// doesn't. Keys it leaves do what they do anywhere in the side column: ↑ and ↓ move on to
    /// the controls below, and letters run triggers.
    ///
    /// ```ignore
    /// .side_keys(
    ///     |s, active, cx| rune_knobs::inline(&s.knobs, GROUPS, style(cx), |s: &mut State| &mut s.knobs, active),
    ///     rune_knobs::side_key(GROUPS, |s: &mut State| &mut s.knobs),
    /// )
    /// ```
    pub fn side_keys(mut self, view: impl Fn(&P, bool, &mut Cx<P>) -> El + 'static, key: impl Fn(&mut P, &KeyEvent, &mut Tx) -> bool + 'static) -> Self {
        self.side = Some(Box::new(view));
        self.side_key = Some(Box::new(key));
        self
    }

    /// How the stage lays it out: in its middle at the size picked beside it, by default;
    /// [`Stage::Scroll`] for a page taller than the stage, or [`Stage::Fill`] for a story the
    /// size of a window.
    ///
    /// ```ignore
    /// Story::new("Settings/All", Settings::default, settings).stage(Stage::Scroll)
    /// ```
    pub fn stage(mut self, stage: Stage) -> Self {
        self.stage = stage;
        self
    }

    /// The app state `init` derives from, for [`Story::from_real`].
    pub(super) fn real(&self) -> Option<std::any::TypeId> {
        match &self.init {
            Init::New(_) => None,
            Init::Real(t, _) => Some(*t),
        }
    }

    /// A variant: `apply` runs on the state after `init`. Keys 1 to 9, or its row in the side
    /// column, switch between them, making the state again as R does. A story with variants
    /// starts on its first.
    pub fn variant(mut self, name: &'static str, apply: impl Fn(&mut P) + 'static) -> Self {
        self.variants.push((name, Box::new(apply)));
        self
    }

    fn staged(stage: &dyn Any) -> &Staged<P> {
        stage.downcast_ref().expect("a story's stage is its own")
    }

    fn staged_mut(stage: &mut dyn Any) -> &mut Staged<P> {
        stage.downcast_mut().expect("a story's stage is its own")
    }
}

/// How a story makes its state: from nothing, or from the real state, of the type named.
enum Init<P> {
    New(Box<dyn Fn() -> P>),
    Real(std::any::TypeId, Box<dyn Fn(&dyn Any) -> P>),
}

/// A [`Story`] with its state type erased. Its stage is a `Staged<P>`.
pub(super) trait OwnStory {
    /// A new stage, from the app's real state if the story derives from it.
    fn init(&self, real: &dyn Any, variant: usize) -> Box<dyn Any>;
    fn state<'a>(&self, stage: &'a mut dyn Any) -> &'a mut dyn Any;
    fn state_ref<'a>(&self, stage: &'a dyn Any) -> &'a dyn Any;
    fn view(&self, stage: &dyn Any, cx: &mut Cx<()>) -> El;
    /// `active`: the keyboard is on it.
    fn side(&self, stage: &dyn Any, active: bool, cx: &mut Cx<()>) -> Option<El>;
    /// Whether its side controls take keys.
    fn side_keys(&self) -> bool;
    /// Offers `ev` to its side controls; true when they took it.
    fn side_key(&self, stage: &mut dyn Any, ev: &KeyEvent, tx: &mut Tx) -> bool;
    fn poll(&self, stage: &mut dyn Any, t: f64, tx: &mut Tx, pressed: &mut Vec<KeyEvent>) -> bool;
    fn polls(&self, stage: &dyn Any) -> bool;
    /// Queues the trigger on `key` at `now`; false when there's none.
    fn trigger(&self, stage: &mut dyn Any, key: char, now: f64) -> bool;
    /// Drops the steps still queued, so a story left part way doesn't burst on return.
    fn stop(&self, stage: &mut dyn Any);
    fn triggers(&self) -> Vec<(char, &'static str)>;
    fn variants(&self) -> Vec<&'static str>;
    fn frame(&self) -> Option<(f32, f32)>;
    fn layout(&self) -> Stage;
}

impl<P: 'static> OwnStory for Story<P> {
    fn init(&self, real: &dyn Any, variant: usize) -> Box<dyn Any> {
        let mut state = match &self.init {
            Init::New(init) => init(),
            Init::Real(_, derive) => derive(real),
        };
        if let Some((_, apply)) = self.variants.get(variant) { apply(&mut state); }
        Box::new(Staged::new(state))
    }
    fn state<'a>(&self, stage: &'a mut dyn Any) -> &'a mut dyn Any {
        &mut Self::staged_mut(stage).state
    }
    fn state_ref<'a>(&self, stage: &'a dyn Any) -> &'a dyn Any {
        &Self::staged(stage).state
    }
    fn view(&self, stage: &dyn Any, cx: &mut Cx<()>) -> El {
        let mut own = cx.retype::<P>();
        let root = (self.view)(&Self::staged(stage).state, &mut own);
        cx.absorb(own);
        root
    }
    fn side(&self, stage: &dyn Any, active: bool, cx: &mut Cx<()>) -> Option<El> {
        let side = self.side.as_ref()?;
        let mut own = cx.retype::<P>();
        let root = side(&Self::staged(stage).state, active, &mut own);
        cx.absorb(own);
        Some(root)
    }
    fn side_keys(&self) -> bool {
        self.side_key.is_some()
    }
    fn side_key(&self, stage: &mut dyn Any, ev: &KeyEvent, tx: &mut Tx) -> bool {
        self.side_key.as_ref().is_some_and(|key| key(&mut Self::staged_mut(stage).state, ev, tx))
    }
    fn poll(&self, stage: &mut dyn Any, t: f64, tx: &mut Tx, pressed: &mut Vec<KeyEvent>) -> bool {
        let stage = Self::staged_mut(stage);
        let ran = stage.run(t, tx, pressed);
        self.poll.as_ref().is_some_and(|poll| poll(&mut stage.state, t, tx)) || ran
    }
    fn polls(&self, stage: &dyn Any) -> bool {
        self.poll.is_some() || !Self::staged(stage).script.is_empty()
    }
    fn trigger(&self, stage: &mut dyn Any, key: char, now: f64) -> bool {
        let Some(t) = self.triggers.iter().find(|t| t.key == key) else { return false };
        Self::staged_mut(stage).script.push((now, t.queued()));
        true
    }
    fn stop(&self, stage: &mut dyn Any) {
        Self::staged_mut(stage).script.clear();
    }
    fn triggers(&self) -> Vec<(char, &'static str)> {
        self.triggers.iter().map(|t| (t.key, t.label)).collect()
    }
    fn variants(&self) -> Vec<&'static str> {
        self.variants.iter().map(|(name, _)| *name).collect()
    }
    fn frame(&self) -> Option<(f32, f32)> {
        self.frame
    }
    fn layout(&self) -> Stage {
        self.stage
    }
}
