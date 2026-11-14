//! The stage: triggers and their steps on the logical clock, and the state handlers, keys and
//! polls reach while the playground is open.

use super::*;

/// A step a trigger runs, now or later on the logical clock, as a fn: see [`Play::after`].
/// Triggers and `Play::after` take closures too.
pub type Step<X> = fn(&mut X, &mut Play<'_, X>);

/// How the stage lays a story out, from [`Story::stage`] or [`Playground::stage`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Stage {
    /// Sized as a window sizes its root, to the stage or the size picked beside it, in the
    /// stage's middle.
    #[default]
    Center,
    /// For content taller than the stage: laid out at the stage's width inside a 24 pt margin,
    /// at its own height, from the top, and the stage scrolls.
    Scroll,
    /// Edge to edge across the whole stage, whatever size is picked beside it: for a story the
    /// size of a window.
    Fill,
}

/// [`Stage::Scroll`]'s margin.
pub(super) const SCROLL_MARGIN: f32 = 24.0;

/// A step queued on a stage, which runs once.
pub(super) type Queued<X> = Box<dyn FnOnce(&mut X, &mut Play<'_, X>)>;

/// A story's view, or a scene's side view, of state `X`.
pub(super) type View<X> = Box<dyn Fn(&X, &mut Cx<X>) -> El>;

/// What a trigger's steps get beside their state: the frame's [`Tx`] (`Play` derefs to it),
/// [`Play::after`] for the steps that follow and [`Play::press`] for keys.
pub struct Play<'a, X> {
    tx: &'a mut Tx,
    script: &'a mut Vec<(f64, Queued<X>)>,
    pressed: &'a mut Vec<KeyEvent>,
    now: f64,
}

impl<X> Play<'_, X> {
    /// Runs `step` `secs` after this one, on the runtime's logical clock. Times count from when
    /// this step was due, not from the frame that ran it, so a replay runs the same every time,
    /// headless runs included.
    pub fn after(&mut self, secs: f64, step: impl FnOnce(&mut X, &mut Play<'_, X>) + 'static) {
        self.script.push((self.now + secs.max(0.0), Box::new(step)));
    }

    /// Drops every step still queued on this stage, a loop's next one included, so a trigger
    /// can take over from a running one; steps queued after it in this step still run. The
    /// stage's state and its running animations are left alone.
    pub fn clear(&mut self) {
        self.script.clear();
    }

    /// When this step was due, on the runtime's clock.
    pub fn now(&self) -> f64 {
        self.now
    }

    /// Presses `key` on the stage, as a person would with the stage holding the keyboard: it
    /// goes the way a real key does, through focus, `on_key` handlers and shortcuts, on the
    /// frame this step runs, after the step's own changes. That frame counts as input, so a
    /// scroll the key asks for moves as one under a hand does. Pass a [`Key`] for a plain key,
    /// or a [`KeyEvent`] for one with modifiers. It stands in for a person's key in the app, so
    /// a letter reaches the app and not the story's triggers: a step that wants another
    /// trigger's behaviour calls that code directly.
    pub fn press(&mut self, key: impl Into<KeyEvent>) {
        self.pressed.push(key.into());
    }
}

impl<X> std::ops::Deref for Play<'_, X> {
    type Target = Tx;
    fn deref(&self) -> &Tx {
        self.tx
    }
}

impl<X> std::ops::DerefMut for Play<'_, X> {
    fn deref_mut(&mut self) -> &mut Tx {
        self.tx
    }
}

/// A trigger: a step on a single-letter key, shown as a button in the side column.
pub(super) struct Trigger<X> {
    pub(super) key: char,
    pub(super) label: &'static str,
    pub(super) run: Rc<dyn Fn(&mut X, &mut Play<'_, X>)>,
}

impl<X: 'static> Trigger<X> {
    /// Its step, to queue.
    pub(super) fn queued(&self) -> Queued<X> {
        let run = self.run.clone();
        Box::new(move |x, play| run(x, play))
    }
}

/// Runs `view`, through the hot-patch table in a hot build, unless the frame is rebuilding from
/// the startup code after a patched view panicked.
pub(crate) fn call<X: 'static>(view: fn(&X, &mut Cx<X>) -> El, state: &X, cx: &mut Cx<X>) -> El {
    #[cfg(feature = "hot")]
    if !crate::hot::originals_only() { return subsecond::HotFn::current(view).call((state, cx)); }
    view(state, cx)
}

/// `view` as the playground keeps it: called through the hot-patch table in a hot build when
/// it's a fn or a closure that captures nothing, as [`call`] does. A closure that captures
/// runs as built: subsecond would take a pointer-sized one's captures for code.
pub(super) fn view<X: 'static>(view: impl Fn(&X, &mut Cx<X>) -> El + 'static) -> View<X> {
    Box::new(move |x, cx| hot_view(&view, x, cx))
}

fn hot_view<X: 'static, F: Fn(&X, &mut Cx<X>) -> El + 'static>(view: &F, state: &X, cx: &mut Cx<X>) -> El {
    #[cfg(feature = "hot")]
    if !crate::hot::originals_only() {
        if let Some(f) = (view as &dyn Any).downcast_ref::<fn(&X, &mut Cx<X>) -> El>() { return call(*f, state, cx); }
        if size_of::<F>() == 0 {
            // SAFETY: a zero-sized `F` has no bytes to copy; the copy is forgotten, not dropped.
            let mut hot = subsecond::HotFn::current(unsafe { std::ptr::read(view) });
            let root = hot.call((state, cx));
            std::mem::forget(hot);
            return root;
        }
    }
    view(state, cx)
}

pub(super) fn trigger<X>(key: char, label: &'static str, run: impl Fn(&mut X, &mut Play<'_, X>) + 'static) -> Trigger<X> {
    let key = key.to_ascii_lowercase();
    assert!(!matches!(key, 'j' | 'k'), "J and K move between stories; give the {label:?} trigger another key");
    Trigger { key, label, run: Rc::new(run) }
}

/// A trigger whose step takes just the [`Tx`].
pub(super) fn trigger_tx<X>(key: char, label: &'static str, run: impl Fn(&mut X, &mut Tx) + 'static) -> Trigger<X> {
    trigger(key, label, move |x: &mut X, play: &mut Play<'_, X>| run(x, play))
}

/// How far past the clock a step can be due and still run on this frame, in seconds.
const DUE_SLACK: f64 = 1e-6;

/// A stage's state and its triggers' queued steps.
pub(super) struct Staged<X> {
    pub(super) state: X,
    pub(super) script: Vec<(f64, Queued<X>)>,
}

impl<X> Staged<X> {
    pub(super) fn new(state: X) -> Self {
        Staged { state, script: Vec::new() }
    }

    /// Runs the steps due by `t`, earliest first, each seeing the time it was due, adding the
    /// keys they press to `pressed`. Asks for a frame when the next one is due.
    pub(super) fn run(&mut self, t: f64, tx: &mut Tx, pressed: &mut Vec<KeyEvent>) -> bool {
        // The clock is a running sum of frame times, so a step due on a frame can sit a rounding
        // error past it; without the slack a replay later in the clock lands a frame late.
        let t = t + DUE_SLACK;
        let mut ran = false;
        // Bounded, in case steps keep queueing more with no delay.
        for _ in 0..64 {
            let due = self.script.iter().enumerate().filter(|(_, (at, _))| *at <= t).min_by(|a, b| a.1.0.total_cmp(&b.1.0)).map(|(i, _)| i);
            let Some(i) = due else { break };
            let (at, step) = self.script.remove(i);
            step(&mut self.state, &mut Play { tx: &mut *tx, script: &mut self.script, pressed: &mut *pressed, now: at });
            ran = true;
        }
        if let Some(next) = self.script.iter().map(|(at, _)| *at).min_by(f64::total_cmp) {
            tx.wake_after(Duration::from_secs_f64((next - t).max(0.0)));
        }
        ran
    }
}

impl<S: 'static> Live<S> {
    /// What one handler call can reach: the stage's state, or the app's while closed (or on a
    /// live page), and the playground's controls.
    pub(crate) fn states<'a>(&'a mut self, app: &'a mut S) -> States<'a> {
        let app = crate::runtime::handler_state(app);
        if !self.open { return States::two(app, &mut self.controls); }
        if let Some(scene) = &mut self.scene { return States::two(&mut scene.state, &mut self.controls); }
        let Some(i) = self.selected.checked_sub(self.scenes.len()) else { return States::two(app, &mut self.controls) };
        match (self.pages.get(i), self.owned.get_mut(i)) {
            (Some(Page { kind: Kind::Own(own), .. }), Some(Some(stage))) => States::two(own.state(stage.as_mut()), &mut self.controls),
            _ => States::two(app, &mut self.controls),
        }
    }

    pub(crate) fn states_ref<'a>(&'a self, app: &'a S) -> StatesRef<'a> {
        let app = crate::runtime::handler_state_ref(app);
        if !self.open { return StatesRef::two(app, &self.controls); }
        if let Some(scene) = &self.scene { return StatesRef::two(&scene.state, &self.controls); }
        match self.own() {
            Some((own, stage)) => StatesRef::two(own.state_ref(stage), &self.controls),
            None => StatesRef::two(app, &self.controls),
        }
    }

    /// Whether the stage's `App::on_ime` hears the input method now: only while its column has
    /// the keyboard, unless [`Playground::typing`] asks for every key: in the list, a letter may
    /// be J, K or a trigger's.
    pub(crate) fn stage_hears(&self, app: &S) -> bool {
        !self.open || self.region == Region::Stage || self.typing(app)
    }

    /// The state `AppSpec::on_key` gets: the stage's when it's an app state, none on a story
    /// with its own.
    pub(crate) fn key_state<'a>(&'a mut self, app: &'a mut S) -> Option<&'a mut S> {
        if !self.open { return Some(app); }
        if matches!(self.page(), Some((_, Page { kind: Kind::Own(_), .. }))) { return None; }
        match &mut self.scene {
            Some(scene) => Some(&mut scene.state),
            None => Some(app),
        }
    }

    /// Whether [`Playground::typing`] gives the stage's `on_key` every key first.
    pub(crate) fn typing(&self, app: &S) -> bool {
        let Some(typing) = self.typing.filter(|_| self.open) else { return false };
        match (&self.scene, self.page()) {
            (Some(scene), _) => typing(&scene.state),
            (None, Some((_, Page { kind: Kind::Own(_), .. }))) => false,
            (None, _) => typing(app),
        }
    }

    /// The keys this frame's steps pressed, to route to the stage, counting them in `played`.
    pub(crate) fn take_pressed(&mut self) -> Vec<KeyEvent> {
        self.played += self.pressed.len() as u64;
        std::mem::take(&mut self.pressed)
    }

    pub(crate) fn polls(&self) -> bool {
        if !self.open { return false; }
        match (&self.scene, self.own()) {
            (Some(scene), _) => self.poll.is_some() || !scene.script.is_empty(),
            (None, Some((own, stage))) => own.polls(stage),
            (None, None) => false,
        }
    }

    /// Polls the stage's state after the app's own poll: its due steps, then `AppSpec::poll`
    /// for a scene or the story's own. The keys the steps press wait in [`Live::take_pressed`].
    pub(crate) fn poll_stage(&mut self, t: f64, tx: &mut Tx) -> bool {
        self.now = t;
        if !self.open { return false; }
        if let Some(scene) = &mut self.scene {
            let ran = scene.run(t, tx, &mut self.pressed);
            return self.poll.is_some_and(|poll| poll(&mut scene.state, t, tx)) || ran;
        }
        let Some(i) = self.selected.checked_sub(self.scenes.len()) else { return false };
        match (self.pages.get(i).map(|p| &p.kind), self.owned.get_mut(i).and_then(Option::as_mut)) {
            (Some(Kind::Own(own)), Some(stage)) => own.poll(stage.as_mut(), t, tx, &mut self.pressed),
            _ => false,
        }
    }
}
