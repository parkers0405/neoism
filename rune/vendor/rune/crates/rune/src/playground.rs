//! [`App::playground`](crate::App::playground): a storybook built into the window (⌘⇧D). Its
//! stories, grouped by type, are the app's scenes and its own [`Story`]s and pages. One shows
//! at a time on a plain stage, live and interactive, with a column beside it for that story's
//! triggers and controls. Nothing touches the app's real state.
//!
//! While it's open, handlers in the stage act on the stage's state: a fresh scene state, a
//! story's own state, or the real state for a [`Playground::page`]. Its own controls act on
//! [`Controls`], beside it, through [`crate::states::States`]. The real state keeps polling
//! throughout and is back, untouched, after Back.

use std::any::Any;
use std::rc::Rc;
use std::time::Duration;

use crate::states::{States, StatesRef};
use crate::tok::{self, Theme, radius, type_};
use crate::{Color, Cx, El, Icon, Key, KeyEvent, OverlaySpec, ScrollSpec, Shortcut, Tx, col, el, icon, row, spacer, text, text_input};

/// The bar's height when the playground is closed, hairline included.
pub(crate) const BAR: f32 = 26.0;
const SIDEBAR: f32 = 200.0;
const SIDE: f32 = 264.0;
const HEADER: f32 = 38.0;
const BAND: f32 = 28.0;
const ROW: f32 = 28.0;
const CONTROL: f32 = 24.0;
/// The story list's filter field.
pub(crate) const FILTER: &str = "playground-filter";
/// How far each level of the story list indents.
const INDENT: f32 = 12.0;

mod folds;
mod list;
mod live;
mod nav;
mod open;
mod region;
mod stage;
mod story;
mod testing;
mod tree;
mod variants;
mod view;
mod widths;

pub(crate) use live::Live;
use folds::Folds;
use widths::Widths;
pub use open::slug;
pub(crate) use region::Region;
pub(crate) use stage::call;
pub use stage::{Play, Stage, Step};
use stage::{SCROLL_MARGIN, Staged, Trigger, View, trigger, trigger_tx};
pub use story::Story;
use story::OwnStory;
pub(crate) use testing::stage_element;
use tree::Row;

/// A storybook for one window, given to [`App::playground`](crate::App::playground):
///
/// ```ignore
/// App::new(spec).playground(
///     Playground::<State>::new()
///         .page("Lists/Live devices", devices)
///         .trigger("Lists/Many devices", 'a', "Add device", |s, _| s.devices.push("New"))
///         .story(Story::new("Progress/Upload", Upload::new, upload).trigger('r', "Run", run))
///         .sizes(&[("Compact", (360.0, 240.0)), ("Wide", (900.0, 600.0))]),
/// )
/// ```
///
/// Name the app's state, as `Playground::<State>::new()`, when a closure comes before anything
/// that does: `Playground::new().trigger(.., |s, _| ..)` can't infer it (E0282).
///
/// Its stories are the window's `AppSpec::scenes`, then its pages and [`Story`]s, listed under
/// their groups: a `"Group/Name"` name puts a story in a group, and each further `/` nests a
/// group inside it (`"Sidebar/Filter buttons/Compact"`), indented a step, as deep as the path
/// goes. Groups and stories keep the order they first appear in, scenes first, unless
/// [`Playground::group_order`] names the top-level groups to list first;
/// [`Playground::start_folded`] starts them folded. Scenes without a group list under
/// "Scenes", pages and stories under "Pages". A filter field above the list (/ or a click
/// focuses it) narrows it to the stories whose path has every word typed, with their groups
/// unfolded: ↓ goes down into the matches, Return opens the first, and Esc clears it, then
/// leaves.
///
/// A scene opens as a fresh `AppSpec::init` with the scene fn applied (`App::init`'s closure
/// isn't used), polled by `AppSpec::poll` but never given `AppSpec::setup`, so it gets no
/// waker and starts nothing; with [`Playground::fresh`] it starts from the real state instead. Back drops every stage state and returns to the real one.
///
/// Keys, while it's open, go to one column at a time: the story list, the stage or the side
/// column. Its selection takes the accent (a ring for the stage), and the hint under the side
/// column lists its keys. A click in a column gives it the keyboard.
///
/// - **The list** (where it starts): ↑↓ move along it, group headers included, and J K between
///   stories (⌘↑ ⌘↓ to the first and last). On a header ← folds its group, or goes to the group
///   around it once folded, → unfolds it or steps in, and ↵ toggles it, as a click on it does;
///   ← on a story goes to its group. Groups stay folded while the app runs (see
///   [`Playground::folds`] to keep them longer). R replays the story from the start and a
///   trigger's letter runs it. → on a story, or Tab, moves to the side column, ⇧Tab to the
///   stage, and Esc or ⌘⇧D go back to the app.
/// - **The side column**: ↑↓ move between its controls, 1 to 9 pick the story's variant, a
///   trigger's letter runs it, and ←, Tab or Esc go back to the list. Side controls from
///   `side_keys` get every key first, and only Tab and Esc leave them.
/// - **The stage**: every key is the story's, through its focused element or the stage's
///   `AppSpec::on_key`, and Esc it leaves goes back to the list.
///
/// Keys the list and side column leave, ⌘-chords and digits included, go to the stage's
/// `on_key` too, so a story's shortcuts work wherever the keyboard is. From the list, Esc goes
/// to it first, and [`Playground::typing`] gives it every key first. A focused text field
/// keeps all its keys.
pub struct Playground<S: 'static> {
    pages: Vec<Page<S>>,
    triggers: Vec<(&'static str, Trigger<S>)>,
    sides: Vec<(&'static str, Side<S>)>,
    stages: Vec<(&'static str, Stage)>,
    sizes: Vec<(&'static str, (f32, f32))>,
    shortcut: Option<Shortcut>,
    bar: bool,
    enabled: bool,
    scenes: Option<fn(&str) -> bool>,
    fresh: Option<fn(&S) -> S>,
    keep: Option<fn(&S, &mut S)>,
    variants: Vec<(&'static str, &'static str, Variant<S>)>,
    typing: Option<fn(&S) -> bool>,
    dark: Option<fn(&S) -> bool>,
    folds: Folds,
    widths: Widths,
    group_order: Vec<&'static str>,
}

impl<S: 'static> Default for Playground<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: 'static> Playground<S> {
    /// No pages, a bar, ⌘⇧D (Ctrl+Shift+D off macOS), and enabled in debug builds only.
    pub fn new() -> Self {
        let shortcut = if cfg!(target_os = "macos") { "Shift+Cmd+D" } else { "Ctrl+Shift+D" };
        Playground {
            pages: Vec::new(),
            triggers: Vec::new(),
            sides: Vec::new(),
            stages: Vec::new(),
            sizes: Vec::new(),
            shortcut: shortcut.parse().ok(),
            bar: true,
            enabled: cfg!(debug_assertions),
            scenes: None,
            fresh: None,
            keep: None,
            variants: Vec::new(),
            typing: None,
            dark: None,
            folds: Folds::default(),
            widths: Widths::default(),
            group_order: Vec::new(),
        }
    }

    /// A page drawn by `view` from the app's live state. Its handlers change the real state,
    /// and it has no triggers or replay.
    pub fn page(mut self, name: &'static str, view: impl Fn(&S, &mut Cx<S>) -> El + 'static) -> Self {
        self.pages.push(Page { name, kind: Kind::Live(stage::view(view)) });
        self
    }

    /// A story with its own state: see [`Story`].
    pub fn story<P: 'static>(mut self, story: Story<P>) -> Self {
        assert!(story.real().is_none_or(|t| t == std::any::TypeId::of::<S>()), "Story::from_real({:?}) derives from another app state than this playground's", story.name);
        self.pages.push(Page { name: story.name, kind: Kind::Own(Box::new(story)) });
        self
    }

    /// A trigger for the scene named `scene` (its path, or just its name): `key` runs `run` on the scene's stage state, as
    /// its button in the side column does. An `'r'` trigger runs when the scene is chosen and on
    /// each replay, so its motion plays at once. See
    /// [`Story::trigger`].
    pub fn trigger(mut self, scene: &'static str, key: char, label: &'static str, run: impl Fn(&mut S, &mut Play<'_, S>) + 'static) -> Self {
        self.triggers.push((scene, trigger(key, label, run)));
        self
    }

    /// [`Playground::trigger`] with a step that takes just the [`Tx`]. See [`Story::trigger_tx`].
    pub fn trigger_tx(mut self, scene: &'static str, key: char, label: &'static str, run: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.triggers.push((scene, trigger_tx(key, label, run)));
        self
    }

    /// The app's own controls for the scene named `scene` (its path, or just its name), at the
    /// top of the side column, drawn from and acting on the scene's stage state: knobs, say.
    /// See [`Story::side`].
    pub fn side(mut self, scene: &'static str, view: impl Fn(&S, &mut Cx<S>) -> El + 'static) -> Self {
        self.sides.push((scene, Side::Stage(stage::view(view))));
        self
    }

    /// A variant of the scene named `scene` (its path, or just its name): `apply` runs on its
    /// state after the scene fn. See [`Story::variant`].
    pub fn variant(mut self, scene: &'static str, name: &'static str, apply: impl Fn(&mut S) + 'static) -> Self {
        self.variants.push((scene, name, Box::new(apply)));
        self
    }

    /// [`Playground::side`], drawn from the stage and the real state both: to mark the app's
    /// own choice beside the one the stage previews, say, with a control that calls
    /// [`Tx::keep_playground`] to write the preview back through [`Playground::keep`].
    pub fn side_real(mut self, scene: &'static str, view: impl Fn(&S, &S, &mut Cx<S>) -> El + 'static) -> Self {
        self.sides.push((scene, Side::Real(Box::new(view))));
        self
    }

    /// [`Playground::side`] for controls that take keys, for the scene named `scene`. See
    /// [`Story::side_keys`].
    pub fn side_keys(mut self, scene: &'static str, view: impl Fn(&S, bool, &mut Cx<S>) -> El + 'static, key: impl Fn(&mut S, &KeyEvent, &mut Tx) -> bool + 'static) -> Self {
        self.sides.push((scene, Side::Keys(Box::new(view), Box::new(key))));
        self
    }

    /// How the stage lays out the scene named `scene` (its path, or just its name). See
    /// [`Story::stage`].
    pub fn stage(mut self, scene: &'static str, stage: Stage) -> Self {
        self.stages.push((scene, stage));
        self
    }

    /// Which of `AppSpec::scenes` the list shows, by name: `|name| name.contains('/')` keeps
    /// ungrouped scenes to `--scene`. By default, all of them.
    pub fn scenes(mut self, show: fn(&str) -> bool) -> Self {
        self.scenes = Some(show);
        self
    }

    /// Scenes start from `fresh(&real)` rather than `AppSpec::init`, then their scene fn applies:
    /// `|real| real.clone()` previews them on a copy of the app as it is, its waker and handles
    /// included: a wake from another thread polls the stage.
    pub fn fresh(mut self, fresh: fn(&S) -> S) -> Self {
        self.fresh = Some(fresh);
        self
    }

    /// Runs before a scene's stage goes, on R, another story or Back, with the stage and the
    /// real state: what it copies over stays in the app, and the next [`Playground::fresh`]
    /// starts from it, so tuning in the side column sticks.
    pub fn keep(mut self, keep: fn(&S, &mut S)) -> Self {
        self.keep = Some(keep);
        self
    }

    /// While `typing` is true of the stage's state (a scene's, or the real state on a page),
    /// `AppSpec::on_key` gets every key before the playground and focused fields do, letters
    /// and arrows included, and what it refuses goes on as usual: for a view that takes text
    /// itself. A focused Rune field needs none of this; it keeps its keys anyway.
    pub fn typing(mut self, typing: fn(&S) -> bool) -> Self {
        self.typing = Some(typing);
        self
    }

    /// Whether the app is dark, read from its real state, for the playground's bar and chrome
    /// and the stage's appearance until the switch beside it changes it: for an app with its own
    /// light and dark setting. By default, the system's.
    pub fn dark(mut self, dark: fn(&S) -> bool) -> Self {
        self.dark = Some(dark);
        self
    }

    /// Stage sizes offered beside "Window" (the stage fills what the playground leaves).
    pub fn sizes(mut self, sizes: &[(&'static str, (f32, f32))]) -> Self {
        self.sizes = sizes.to_vec();
        self
    }

    /// The shortcut that opens and closes the playground, or `None` for none: by default ⌘⇧D,
    /// Ctrl+Shift+D off macOS. On macOS it's the View menu's Playground item's.
    pub fn shortcut(mut self, shortcut: Option<Shortcut>) -> Self {
        self.shortcut = shortcut;
        self
    }

    /// The bar along the window's bottom edge that opens the playground. With it, the app lays
    /// out 26 points shorter, as in a window that much shorter. Without it the app's layout is
    /// untouched and the playground opens from its shortcut, the View menu's Playground item,
    /// or [`Tx::open_playground`].
    pub fn bar(mut self, on: bool) -> Self {
        self.bar = on;
        self
    }

    /// Whether the window offers the playground at all; by default only in debug builds.
    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    pub(crate) fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn shortcut_key(&self) -> Option<&Shortcut> {
        self.shortcut.as_ref()
    }
}

/// A scene's side column view, from [`Playground::side`], [`Playground::side_real`] or
/// [`Playground::side_keys`].
enum Side<S: 'static> {
    Stage(View<S>),
    Real(Box<dyn Fn(&S, &S, &mut Cx<S>) -> El>),
    Keys(SideView<S>, SideKey<S>),
}

/// A variant's setup, run on a new stage state.
type Variant<X> = Box<dyn Fn(&mut X)>;

/// A side column view that takes keys, told whether the keyboard is on it.
type SideView<X> = Box<dyn Fn(&X, bool, &mut Cx<X>) -> El>;
/// Its keys: true when it took one.
type SideKey<X> = Box<dyn Fn(&mut X, &KeyEvent, &mut Tx) -> bool>;

struct Page<S: 'static> {
    name: &'static str,
    kind: Kind<S>,
}

enum Kind<S: 'static> {
    Live(View<S>),
    Own(Box<dyn OwnStory>),
}

/// A story's group and name, from its path: everything before the last `/` names its group
/// (`"Sidebar/Filter buttons"`, a group inside "Sidebar"), the rest the story.
fn split(path: &'static str, default: &'static str) -> (&'static str, &'static str) {
    path.rsplit_once('/').unwrap_or((default, path))
}

/// What a handler asks of the playground (`Tx::open_playground`, its keys and its controls),
/// applied at the start of the next commit, before the view runs against the new stage.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Request {
    Open(Option<String>),
    /// [`Tx::open_playground_at`]: a story by path, name or slug, and a variant.
    OpenAt(String, Option<String>),
    Close,
    Toggle,
    Select(usize),
    /// R: the stage's state made again, then its `'r'` trigger.
    Replay,
    /// [`Tx::keep_playground`].
    Keep,
    /// A digit: the open story's variant, by index, made as R makes it.
    Variant(usize),
    Trigger(char),
    Dark(bool),
    Size(usize),
    ReduceMotion(bool),
    /// A group's header becomes the list's selection; the stage keeps its story.
    Header(&'static str),
    /// Folds a group (`Some(true)`), unfolds it, or toggles it (`None`).
    Fold(&'static str, Option<bool>),
}

impl Request {
    /// Whether this one changes the stage's state, so handlers built for the old one must go.
    fn swaps_stage(&self) -> bool {
        matches!(self, Request::Open(_) | Request::OpenAt(..) | Request::Close | Request::Toggle | Request::Select(_) | Request::Replay | Request::Variant(_))
    }
}

/// What a key does in the open playground, for the runtime to carry out.
pub(crate) enum Nav {
    Pass,
    /// The playground's key, which does nothing here.
    Stay,
    Request(Request),
    /// Focus the side column's first control.
    IntoSide,
    /// The stage takes the keyboard.
    IntoStage,
    /// Focus the story list's filter field.
    Filter,
    /// Focus leaves the side column.
    OutOfSide,
    /// Focus moves this many controls along the side column.
    SideStep(isize),
}

/// The playground's own controls' state: the story list's filter field and column widths. Its
/// other controls change the playground through requests.
pub(crate) struct Controls {
    pub(crate) filter: crate::TextEdit,
    pub(super) sidebar_w: f32,
    pub(super) side_w: f32,
    pub(super) save_widths: Option<Rc<dyn Fn(f32, f32)>>,
}

impl Default for Controls {
    fn default() -> Self {
        Self { filter: Default::default(), sidebar_w: SIDEBAR, side_w: SIDE, save_widths: None }
    }
}

/// Shows `playground` in `rt`'s window when this run offers it: an enabled playground, in a
/// window unless `RUNE_PLAYGROUND=0`, or wherever `--playground` asks for it, headless included,
/// opened at the story it names, on the variant `--variant` names.
pub(crate) fn attach<S: 'static>(rt: &mut crate::Runtime<S>, playground: Option<Playground<S>>, spec: &crate::AppSpec<S>, open: Option<Option<String>>, variant: Option<String>, windowed: bool) {
    let Some(mut playground) = playground.filter(Playground::is_enabled) else { return };
    if open.is_none() && (!windowed || hidden_by_env()) { return; }
    if windowed && !cfg!(test) && playground.widths.load.is_none() && playground.widths.save.is_none() {
        playground.widths = Widths::prefs(crate::tooling::app_id().unwrap_or(spec.title));
    }
    let mut live = Live::new(playground, spec, windowed && cfg!(target_os = "macos"));
    if let Some(name) = open {
        live.open_at(name.as_deref(), variant.as_deref()).unwrap_or_else(|e| crate::tooling::die(&e));
        live.pending.push(Request::Open(name));
    }
    rt.playground = Some(Box::new(live));
}

/// `--playground [NAME]`'s optional story name, taking it when the next argument isn't a flag.
pub(crate) fn arg(argv: &[String], i: &mut usize) -> Option<String> {
    let name = argv.get(*i + 1).filter(|v| !v.starts_with("--")).cloned();
    if name.is_some() { *i += 1; }
    name
}

fn hidden_by_env() -> bool {
    std::env::var("RUNE_PLAYGROUND").is_ok_and(|v| v == "0")
}

pub(crate) fn offered_by_env() -> bool {
    !hidden_by_env()
}

impl Tx {
    /// Show [`App::playground`](crate::App::playground) in this window. Without one, or in a
    /// run that doesn't show it (a release build, `RUNE_PLAYGROUND=0`, a headless render
    /// without `--playground`), nothing happens.
    pub fn open_playground(&mut self) {
        self.playground.push(Request::Open(None));
    }

    /// Back to the app from the playground, as its Back button does.
    pub fn close_playground(&mut self) {
        self.playground.push(Request::Close);
    }

    /// Runs [`Playground::keep`] now, with the open scene's stage and the real state, as a
    /// switch would before the stage goes. The stage stays.
    pub fn keep_playground(&mut self) {
        self.playground.push(Request::Keep);
    }

    pub fn toggle_playground(&mut self) {
        self.playground.push(Request::Toggle);
    }
}

/// Whether `key` is the stage's, around the story's root.
pub(crate) fn is_stage(key: &str) -> bool {
    key.strip_prefix("playground-stage-").is_some_and(|g| g.parse::<u32>().is_ok())
}

#[cfg(test)]
#[path = "playground_tests.rs"]
mod tests;
