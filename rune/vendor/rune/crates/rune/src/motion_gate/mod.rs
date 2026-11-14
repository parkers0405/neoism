//! A motion gate for an app's own tests: drive the real app headless on the virtual clock, record
//! every element's presented rect, visible part and opacity each frame, and list what a person
//! would see go wrong (see [`check`] and [`RULES`]).
//!
//! An app lists cells, one scenario in one circumstance each, and runs them in a test:
//!
//! ```ignore
//! use rune::motion_gate::{self, AllowRow, Job, Run};
//!
//! const ALLOW: &[AllowRow] = &[
//!     AllowRow::new("fade-snap", "badge", "add-row", "the badge swaps at once, by design").max(1),
//! ];
//!
//! fn add_row() -> motion_gate::Recording {
//!     let mut r = Run::new(Runtime::new(State::default(), view, (800.0, 600.0), Env::new(true, 1.0)), 120.0);
//!     r.idle(1.0);
//!     r.step(); // frame 0: the settled start
//!     r.act("tap add", |rt| Run::tap(rt, "add"));
//!     r.play(1.0);
//!     r.rec
//! }
//!
//! #[test]
//! fn motion_gate() {
//!     motion_gate::gate_rows(vec![("add-row/wide".into(), Box::new(add_row) as Job)], ALLOW);
//! }
//! ```
//!
//! [`gate`] reads its settings from the environment, which `scripts/motion-audit/gate` sets:
//! `MOTION_GATE` keeps cells whose name contains it, `MOTION_GATE_OUT` names a file for the full
//! report, `MOTION_GATE_ALLOWED` lists allowed findings too, `MOTION_GATE_DUMP=<cell>|<path
//! part>[|from|to]` writes those tracks' frames to `MOTION_GATE_OUT`.dump, and
//! `MOTION_GATE_SHEET=<cell>|<frame>[|<path part>]` writes a contact strip of the frames around
//! `frame` to `MOTION_GATE_OUT`.sheet.png. An empty cell part matches any cell, and headless
//! `rune` under `RUNE_MOTION_GATE` writes them beside its report.
//!
//! Beside geometry and opacity, each element's painted properties are followed frame by frame
//! (its colours in OKLab, corners, blurs, shadow, effect payload, image and place in the paint
//! order), and the composited window as a grid of cells a few points across: a CPU raster of
//! the painter stream, or a GPU frame through [`Run::pixels`]. `snap`, `flash` and
//! `changes-twice` find a property or a region that steps, blinks or changes twice. An input
//! ([`Run::act`]), a hover or press highlight landing, a `Tx::instant` commit,
//! `El::instant_changes` and `El::motion_intent` marks keep a deliberate change from counting.
//! So does `El::motion(Spring::NONE)` written on the element itself, for the properties that
//! spring governs there (colours but text, corners, shadow, blur; `El::blur_motion(Spring::NONE)`
//! for blur alone). A `Spring::NONE` it inherits, takes from a theme or token
//! (`El::token_motion`) or gets under Reduce Motion marks nothing, since no one chose it there.

pub mod check;
mod gate;

pub use check::{Finding, RULES, Recording, Rule, Snap, Track, check, dump, rule, sheet, short};
pub use gate::{Allow, AllowRow, Cell, Job, Report, allowed, gate, gate_rows, report, report_rows, run};
pub(crate) use gate::evidence;

use std::sync::Arc;

use crate::{ElementInfo, Runtime};

/// A frame's pixels, for [`Run::pixels`]: width, height and 8-bit sRGB RGBA rows.
pub type Pixels = (u32, u32, Vec<u8>);

/// A runtime being recorded: every [`step`](Run::step) frames it on the virtual clock and samples
/// its element tree, what each element paints and the composited window into `rec`.
pub struct Run<S: 'static> {
    pub rt: Runtime<S>,
    pub rec: Recording,
    /// Seconds per frame.
    pub dt: f64,
    /// Reads a drawn frame for the region checks in place of the CPU raster ([`Run::pixels`]).
    draw: Option<Box<dyn FnMut(&mut Runtime<S>) -> Option<Pixels>>>,
    raster: Raster,
}

/// The CPU raster of a runtime's painter stream, reused while the stream is unchanged.
#[derive(Default)]
struct Raster(Option<(u64, Arc<check::raster::Grid>)>);

impl Raster {
    fn of<S>(&mut self, rt: &Runtime<S>, size: (f32, f32)) -> Arc<check::raster::Grid> {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        bytemuck::cast_slice::<_, u8>(&rt.instances).hash(&mut hasher);
        let key = hasher.finish();
        if let Some((k, g)) = &self.0 && *k == key { return g.clone(); }
        let g = Arc::new(check::raster::rasterize_with(&rt.instances, size, rt.env.scale, &rt.icon_aprons, &rt.text.glyph_shifts()));
        self.0 = Some((key, g.clone()));
        g
    }
}

/// Samples a frame `rt` just took into `rec`: why it committed, its elements, what they paint
/// and the composited window (`grid`, else the CPU raster).
fn sample<S>(rt: &mut Runtime<S>, rec: &mut Recording, raster: &mut Raster, grid: Option<Arc<check::raster::Grid>>) {
    let why = std::mem::take(&mut rt.commit_why);
    rec.commits(rt.scrolling(), why);
    let els: Vec<ElementInfo> = rt.elements();
    let paints = rt.paints();
    rec.sample_with(&els, Some(&paints));
    let grid = grid.unwrap_or_else(|| raster.of(rt, rec.size));
    rec.grid(Some(grid));
    rec.tips.resize(rec.frames.saturating_sub(1), None);
    rec.tips.push(rt.help_tooltip().map(|t| t.1));
}

/// Records a runtime someone else steps, as headless `rune` does under `RUNE_MOTION_GATE`:
/// call [`input`](Sampler::input) for each input before its frame and
/// [`frame`](Sampler::frame) after each frame.
pub struct Sampler {
    pub rec: Recording,
    raster: Raster,
}

impl Sampler {
    pub fn new<S>(rt: &mut Runtime<S>, hz: f64) -> Self {
        rt.paint_rank = Some(Vec::new());
        Sampler { rec: Recording::new(rt.viewport, hz), raster: Raster::default() }
    }

    /// An input on the next frame.
    pub fn input(&mut self, what: &str) {
        self.rec.input(what);
    }

    /// Clears what the last frame left, before `rt.frame`.
    pub fn before<S>(&mut self, rt: &mut Runtime<S>) {
        rt.commit_why = 0;
    }

    /// Samples the frame `rt` just took; `awake` is what `rt.frame` returned.
    pub fn frame<S>(&mut self, rt: &mut Runtime<S>, awake: bool) {
        self.rec.awake.push(awake);
        sample(rt, &mut self.rec, &mut self.raster, None);
    }
}

impl<S: 'static> Run<S> {
    /// Records `rt` at `hz` frames a second, after its first frame. Settle it with
    /// [`idle`](Run::idle) and set the scene up, then the first [`step`](Run::step) is the settled
    /// start the first action changes.
    pub fn new(mut rt: Runtime<S>, hz: f64) -> Self {
        rt.paint_rank = Some(Vec::new());
        rt.frame(0.0);
        let rec = Recording::new(rt.viewport, hz);
        Run { rt, rec, dt: 1.0 / hz, draw: None, raster: Raster::default() }
    }

    /// Reads each recorded frame's pixels with `draw` (a GPU render of the runtime) for the
    /// region checks, in place of rasterizing its painter stream on the CPU. `None` from it
    /// falls back to the raster for that frame.
    pub fn pixels(&mut self, draw: impl FnMut(&mut Runtime<S>) -> Option<Pixels> + 'static) {
        self.draw = Some(Box::new(draw));
    }

    /// Steps `secs` unrecorded (setting up, before the run).
    pub fn idle(&mut self, secs: f64) {
        for _ in 0..(secs / self.dt).round() as usize {
            self.rt.frame(self.dt);
        }
    }

    /// Records one frame. A key a playground story's step pressed on it (`Play::press`) is an
    /// input there, as [`act`](Run::act)'s are.
    pub fn step(&mut self) {
        self.rt.commit_why = 0;
        let played = self.rt.played_keys();
        let awake = self.rt.frame(self.dt);
        if self.rt.played_keys() > played { self.rec.input("a story step's key"); }
        self.rec.awake.push(awake);
        let size = self.rec.size;
        let drawn = self.draw.as_mut().and_then(|draw| draw(&mut self.rt))
            .map(|(w, h, rgba)| Arc::new(check::raster::from_pixels(&rgba, w, h, size)));
        sample(&mut self.rt, &mut self.rec, &mut self.raster, drawn);
    }

    pub fn step_n(&mut self, n: usize) {
        for _ in 0..n {
            self.step();
        }
    }

    /// Records `secs` of frames.
    pub fn play(&mut self, secs: f64) {
        self.step_n((secs / self.dt).round() as usize);
    }

    /// An input on the next frame: `f` runs, then the frame it lands on is recorded. Motion
    /// retargeted by an input isn't held to one path.
    pub fn act(&mut self, what: &str, f: impl FnOnce(&mut Runtime<S>)) {
        self.rec.input(what);
        f(&mut self.rt);
        self.step();
    }

    /// Marks the cell's motion as over from the next frame: frames asked for well after it are
    /// `wakes-after-end`.
    pub fn quiet(&mut self) {
        self.rec.quiet_from = Some(self.rec.frames);
    }

    /// A click at the middle of the element keyed `key`.
    pub fn tap(rt: &mut Runtime<S>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no {key}"));
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
    }

    /// What the checker finds in the recording.
    pub fn finish(&self) -> Vec<Finding> {
        check(&self.rec)
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod loop_tests;
#[cfg(test)]
mod paint_tests;
#[cfg(test)]
mod spring_none_tests;
