//! [`Live`]: a window's playground while it runs, and the requests that move it.

use super::*;

/// A window's playground while it runs.
pub(crate) struct Live<S: 'static> {
    pub(super) pages: Vec<Page<S>>,
    /// By the scene they're for.
    pub(super) triggers: Vec<(usize, Trigger<S>)>,
    pub(super) sides: Vec<(usize, Side<S>)>,
    pub(super) stages: Vec<(usize, Stage)>,
    pub(super) variants: Vec<(usize, &'static str, Variant<S>)>,
    /// Each story's variant, by its index into scenes then pages, until the playground closes.
    pub(super) variant: Vec<usize>,
    pub(super) sizes: Vec<(&'static str, (f32, f32))>,
    pub(super) shortcut: Option<Shortcut>,
    /// The View menu's item takes the shortcut, so keys here leave it alone.
    pub(super) menu: bool,
    pub(super) bar: bool,
    pub(super) init: fn() -> S,
    pub(super) fresh: Option<fn(&S) -> S>,
    pub(super) keep: Option<fn(&S, &mut S)>,
    pub(super) typing: Option<fn(&S) -> bool>,
    /// [`Playground::dark`].
    pub(super) app_dark: Option<fn(&S) -> bool>,
    pub(super) scenes: &'static [(&'static str, fn(&mut S))],
    pub(super) poll: Option<fn(&mut S, f64, &mut Tx) -> bool>,
    /// Stories, as indices into scenes then pages, in the order the list shows them.
    pub(super) order: Vec<usize>,
    pub(super) open: bool,
    /// Into scenes then pages.
    pub(super) selected: usize,
    /// A group header is the list's selection rather than `selected`'s row.
    pub(super) header: Option<&'static str>,
    /// Groups whose stories the list hides, until the playground closes.
    pub(super) folded: Vec<&'static str>,
    /// [`Playground::folds`].
    pub(super) folds: Folds,
    /// [`Playground::widths`].
    pub(super) widths: Widths,
    /// The keyboard is on the open story's side controls, which take keys.
    pub(super) side_active: bool,
    /// The column that has the keyboard.
    pub(super) region: Region,
    /// The focused element's path when the region last looked (see [`Live::see_focus`]).
    pub(super) region_seen: Option<String>,
    pub(super) scene: Option<Box<Staged<S>>>,
    pub(super) owned: Vec<Option<Box<dyn Any>>>,
    pub(super) controls: Controls,
    pub(super) dark: Option<bool>,
    pub(super) size: usize,
    pub(super) reduce_motion: bool,
    pub(super) pending: Vec<Request>,
    /// Keys the stage's steps pressed with [`Play::press`], until the runtime routes them.
    pub(super) pressed: Vec<KeyEvent>,
    /// A key a step pressed is being routed: only the stage pages.
    pub(crate) pressing: bool,
    /// How many keys steps have pressed, which a motion gate's run reads as input.
    pub(crate) played: u64,
    /// The clock at the last commit or poll, which triggers start from.
    pub(super) now: f64,
    /// Bumped by every select and replay, so the new stage's elements start fresh: scroll
    /// offsets, focus and animations don't carry over from the last one.
    pub(super) generation: u32,
    /// The node the app's view returned, which holds the window's root handlers.
    pub(crate) root: usize,
    /// The time scale the app had when the playground opened, which every new stage starts
    /// from and Back restores.
    pub(crate) app_time_scale: f32,
}

impl<S: 'static> Live<S> {
    pub(super) fn new(playground: Playground<S>, spec: &crate::AppSpec<S>, menu: bool) -> Self {
        let Playground { pages, triggers, sides, stages, sizes, shortcut, bar, enabled: _, scenes: show, fresh, keep, variants, typing, dark: app_dark, folds, mut widths, group_order } = playground;
        let controls = Controls { save_widths: widths.save.take(), ..Controls::default() };
        let mut live = Live {
            owned: pages.iter().map(|_| None).collect(),
            pages,
            triggers: Vec::new(),
            sides: Vec::new(),
            stages: Vec::new(),
            variants: Vec::new(),
            variant: Vec::new(),
            sizes,
            shortcut,
            menu,
            bar,
            init: spec.init,
            fresh,
            keep,
            typing,
            app_dark,
            scenes: spec.scenes,
            poll: spec.poll,
            order: Vec::new(),
            open: false,
            selected: 0,
            header: None,
            folded: Vec::new(),
            folds,
            widths,
            side_active: false,
            region: Region::List,
            region_seen: None,
            scene: None,
            controls,
            dark: None,
            size: 0,
            reduce_motion: false,
            pending: Vec::new(),
            pressed: Vec::new(),
            pressing: false,
            played: 0,
            now: 0.0,
            generation: 0,
            root: 0,
            app_time_scale: 1.0,
        };
        // Scene names as `find` takes them: a path or a name, matching case if it can.
        let scene = |live: &Self, name: &str| {
            let at = live.find_in(name, &(0..live.scenes.len()).collect::<Vec<_>>());
            if at.is_none() { eprintln!("rune: the playground has a trigger, side column or stage for {name:?}, which isn't one of AppSpec::scenes"); }
            at
        };
        live.triggers = triggers.into_iter().filter_map(|(name, t)| Some((scene(&live, name)?, t))).collect();
        live.sides = sides.into_iter().filter_map(|(name, view)| Some((scene(&live, name)?, view))).collect();
        live.stages = stages.into_iter().filter_map(|(name, stage)| Some((scene(&live, name)?, stage))).collect();
        live.variants = variants.into_iter().filter_map(|(name, variant, apply)| Some((scene(&live, name)?, variant, apply))).collect();
        live.variant = vec![0; live.count()];
        let listed: Vec<usize> = (0..live.count()).filter(|&i| live.scenes.get(i).is_none_or(|(name, _)| show.is_none_or(|show| show(name)))).collect();
        live.order = live.tree_order(&listed, &group_order);
        live.selected = live.order.first().copied().unwrap_or(0);
        if live.folds.start { live.folded = live.all_groups(); }
        live.load_widths();
        live
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    pub(super) fn count(&self) -> usize {
        self.scenes.len() + self.pages.len()
    }

    pub(super) fn path(&self, i: usize) -> &'static str {
        match self.scenes.get(i) {
            Some((name, _)) => name,
            None => self.pages[i - self.scenes.len()].name,
        }
    }

    pub(super) fn group(&self, i: usize) -> &'static str {
        split(self.path(i), if i < self.scenes.len() { "Scenes" } else { "Pages" }).0
    }

    pub(super) fn name(&self, i: usize) -> &'static str {
        split(self.path(i), "").1
    }

    pub(super) fn page(&self) -> Option<(usize, &Page<S>)> {
        let i = self.selected.checked_sub(self.scenes.len())?;
        self.pages.get(i).map(|p| (i, p))
    }

    pub(super) fn own(&self) -> Option<(&dyn OwnStory, &dyn Any)> {
        let (i, page) = self.page()?;
        let Kind::Own(own) = &page.kind else { return None };
        Some((own.as_ref(), self.owned[i].as_deref()?))
    }

    pub(super) fn is_live_page(&self) -> bool {
        matches!(self.page(), Some((_, Page { kind: Kind::Live(_), .. })))
    }

    /// How the stage lays out the open story.
    pub(super) fn layout(&self) -> Stage {
        if self.selected < self.scenes.len() {
            return self.stages.iter().rev().find(|(s, _)| *s == self.selected).map_or(Stage::Center, |(_, stage)| *stage);
        }
        match self.page() {
            Some((_, Page { kind: Kind::Own(own), .. })) => own.layout(),
            _ => Stage::Center,
        }
    }

    /// Whether story `i` has triggers of its own, beside R, which the list marks.
    pub(super) fn has_triggers(&self, i: usize) -> bool {
        match i.checked_sub(self.scenes.len()) {
            None => self.triggers.iter().any(|(s, _)| *s == i),
            Some(p) => matches!(self.pages.get(p), Some(Page { kind: Kind::Own(own), .. }) if !own.triggers().is_empty()),
        }
    }

    /// The open story's triggers, R's first.
    pub(super) fn triggers(&self) -> Vec<(char, &'static str)> {
        if self.is_live_page() { return Vec::new(); }
        let mut keys = match (self.scenes.get(self.selected), self.own()) {
            (Some(_), _) => self.triggers.iter().filter(|(s, _)| *s == self.selected).map(|(_, t)| (t.key, t.label)).collect(),
            (None, Some((own, _))) => own.triggers(),
            (None, None) => Vec::new(),
        };
        let replay = keys.iter().position(|(k, _)| *k == 'r').map_or((('r'), "Replay"), |i| keys.remove(i));
        keys.insert(0, replay);
        keys
    }

    pub(crate) fn request(&mut self, requests: Vec<Request>) {
        self.pending.extend(requests);
    }

    pub(crate) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub(crate) fn pending_swaps_stage(&self) -> bool {
        self.pending.iter().any(Request::swaps_stage)
    }

    /// Whether the only requests that swap the stage are replays, which keep its story.
    pub(crate) fn pending_replays_only(&self) -> bool {
        self.pending.iter().filter(|r| r.swaps_stage()).all(|r| matches!(r, Request::Replay | Request::Variant(_)))
    }

    /// Applies the requests handlers made since the last commit; true when anything changed.
    pub(crate) fn apply(&mut self, clock: f64, real: &mut S) -> bool {
        self.now = clock;
        self.load_folds();
        self.load_widths();
        let mut changed = false;
        for request in std::mem::take(&mut self.pending) {
            changed = true;
            match request {
                Request::Open(name) => self.open_story(name.as_deref(), real),
                Request::OpenAt(name, variant) => {
                    let name = self.resolve(&name, variant.as_deref());
                    self.open_story(name.as_deref(), real);
                }
                Request::Close => self.close(real),
                Request::Toggle if self.open => self.close(real),
                Request::Toggle => {
                    self.open = true;
                    if self.folds.start { self.reveal(self.selected); }
                    self.select(self.selected, real);
                }
                Request::Select(i) => {
                    self.reveal(i);
                    self.select(i, real);
                }
                Request::Replay => self.replay(real),
                Request::Variant(n) if n < self.variants().len() => {
                    self.variant[self.selected] = n;
                    self.replay(real);
                }
                Request::Variant(_) => {}
                Request::Keep => self.keep(real),
                Request::Trigger(key) => self.trigger(key),
                Request::Dark(dark) => self.dark = Some(dark),
                Request::Size(i) => self.size = i,
                Request::ReduceMotion(on) => self.reduce_motion = on,
                Request::Header(group) => self.header = Some(group),
                Request::Fold(group, fold) => self.fold(group, fold),
            }
        }
        // Opened on a story whose group is folded, the header stands in for its row.
        self.settle();
        changed
    }

    /// Folds a group, unfolds it, or toggles it. A filter shows every match, so folds wait
    /// until it's cleared. A selection whose row goes moves to the header that hid it.
    pub(super) fn fold(&mut self, group: &'static str, fold: Option<bool>) {
        if !self.query().is_empty() { return; }
        let folded = self.folded.contains(&group);
        match fold.unwrap_or(!folded) {
            true if !folded => self.refold(|f| f.push(group)),
            false => self.refold(|f| f.retain(|g| *g != group)),
            true => {}
        }
        self.settle();
    }

    /// Story `i`'s row becomes the list's selection, its groups unfolded.
    pub(super) fn reveal(&mut self, i: usize) {
        self.header = None;
        if i < self.count() {
            let groups = self.groups_of(i);
            self.refold(|f| f.retain(|g| !groups.contains(g)));
        }
    }

    /// The open story's stage made again, then its `'r'` trigger.
    pub(super) fn replay(&mut self, real: &mut S) {
        if self.is_live_page() { return; }
        if let Some((i, _)) = self.page() { self.owned[i] = None; }
        self.select(self.selected, real);
    }

    /// [`Playground::keep`], from the scene's stage as it goes.
    pub(super) fn keep(&mut self, real: &mut S) {
        if let (Some(keep), Some(scene)) = (self.keep, &self.scene) { keep(&scene.state, real); }
    }

    pub(super) fn close(&mut self, real: &mut S) {
        self.keep(real);
        self.open = false;
        self.header = None;
        self.region = Region::List;
        self.controls.filter = crate::TextEdit::default();
        self.variant.iter_mut().for_each(|v| *v = 0);
        self.scene = None;
        self.owned.iter_mut().for_each(|o| *o = None);
    }

    /// Shows story `i`. A stage made new, as a scene's always is, runs its `'r'` trigger.
    pub(super) fn select(&mut self, i: usize, real: &mut S) {
        self.keep(real);
        if let Some(left) = self.selected.checked_sub(self.scenes.len())
            && let (Some(Page { kind: Kind::Own(own), .. }), Some(Some(stage))) = (self.pages.get(left), self.owned.get_mut(left))
        {
            own.stop(stage.as_mut());
        }
        self.selected = i.min(self.count().saturating_sub(1));
        self.scene = None;
        self.generation += 1;
        let scene_clock = crate::SceneClock { now: self.now, dt: 1.0 / 60.0 };
        if let Some((_, f)) = self.scenes.get(self.selected) {
            let (init, fresh, variant) = (self.init, self.fresh, self.scene_variant());
            let state = crate::with_scene_clock(scene_clock, || {
                let mut s = fresh.map_or_else(init, |fresh| fresh(real));
                f(&mut s);
                if let Some(variant) = variant { variant(&mut s); }
                s
            });
            self.scene = Some(Box::new(Staged::new(state)));
        } else if let Some((i, page)) = self.page() && let Kind::Own(own) = &page.kind && self.owned[i].is_none() {
            let variant = self.variant[self.selected];
            let stage = crate::with_scene_clock(scene_clock, || own.init(&*real, variant));
            self.owned[i] = Some(stage);
        } else {
            return;
        }
        self.trigger('r');
    }

    /// Queues the open story's trigger on `key`, to run at the next poll from now.
    pub(super) fn trigger(&mut self, key: char) {
        if !self.open { return; }
        let now = self.now;
        if let Some(scene) = &mut self.scene {
            if let Some((_, t)) = self.triggers.iter().find(|(s, t)| *s == self.selected && t.key == key) {
                scene.script.push((now, t.queued()));
            }
            return;
        }
        let Some(i) = self.selected.checked_sub(self.scenes.len()) else { return };
        if let (Some(Page { kind: Kind::Own(own), .. }), Some(Some(stage))) = (self.pages.get(i), self.owned.get_mut(i)) {
            own.trigger(stage.as_mut(), key, now);
        }
    }

    /// Whether `ev` is the shortcut that opens and closes the playground, where no menu item
    /// takes it first.
    pub(crate) fn is_shortcut(&self, ev: &KeyEvent) -> bool {
        !self.menu && self.shortcut.as_ref().is_some_and(|s| s.matches_key(ev))
    }
}
