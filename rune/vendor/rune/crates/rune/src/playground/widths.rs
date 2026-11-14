//! Resizable widths for the playground's story list and right-hand column: dragged by their
//! separator edges, reset by double-clicking, and remembered across launches.

use super::*;
use crate::prefs::Prefs;
use crate::{Cursor, Drag, Phase};

pub(super) const SIDEBAR_MIN: f32 = 150.0;
pub(super) const SIDEBAR_MAX: f32 = 520.0;
pub(super) const SIDE_MIN: f32 = 180.0;
pub(super) const SIDE_MAX: f32 = 560.0;
pub(super) const STAGE_MIN: f32 = 200.0;

const PREF_SIDEBAR: &str = "playground_sidebar_w";
const PREF_SIDE: &str = "playground_side_w";

#[derive(Default)]
pub(super) struct Widths {
    pub(super) load: Option<Box<dyn FnOnce() -> (f32, f32)>>,
    pub(super) save: Option<Rc<dyn Fn(f32, f32)>>,
}

impl Widths {
    pub(super) fn prefs(app_id: &str) -> Self {
        let load_id = app_id.to_owned();
        let save_id = app_id.to_owned();
        Self {
            load: Some(Box::new(move || {
                let p = Prefs::open(&load_id);
                (p.get(PREF_SIDEBAR).unwrap_or(SIDEBAR), p.get(PREF_SIDE).unwrap_or(SIDE))
            })),
            save: Some(Rc::new(move |sidebar, side| {
                let mut p = Prefs::open(&save_id);
                if (sidebar - SIDEBAR).abs() < 0.5 {
                    let _ = p.remove(PREF_SIDEBAR);
                } else {
                    let _ = p.set(PREF_SIDEBAR, &sidebar);
                }
                if (side - SIDE).abs() < 0.5 {
                    let _ = p.remove(PREF_SIDE);
                } else {
                    let _ = p.set(PREF_SIDE, &side);
                }
            })),
        }
    }
}

impl<S: 'static> Playground<S> {
    /// Keeps the story list's and side column's widths from one run to the next. Windowed apps
    /// that don't set this store them in [`Prefs`] automatically.
    pub fn widths(mut self, load: impl FnOnce() -> (f32, f32) + 'static, save: impl Fn(f32, f32) + 'static) -> Self {
        self.widths = Widths { load: Some(Box::new(load)), save: Some(Rc::new(save)) };
        self
    }
}

pub(super) fn clamp_pair(sidebar: f32, side: f32, viewport_w: f32) -> (f32, f32) {
    let side = side.clamp(SIDE_MIN, SIDE_MAX.min((viewport_w - SIDEBAR_MIN - STAGE_MIN - 2.0).max(SIDE_MIN)));
    let sidebar = sidebar.clamp(SIDEBAR_MIN, SIDEBAR_MAX.min((viewport_w - side - STAGE_MIN - 2.0).max(SIDEBAR_MIN)));
    (sidebar, side)
}

impl Controls {
    pub(super) fn set_widths(&mut self, sidebar: f32, side: f32) {
        if sidebar.is_finite() && sidebar > 0.0 {
            self.sidebar_w = sidebar.clamp(SIDEBAR_MIN, SIDEBAR_MAX).round();
        }
        if side.is_finite() && side > 0.0 {
            self.side_w = side.clamp(SIDE_MIN, SIDE_MAX).round();
        }
    }

    pub(super) fn save_widths(&self) {
        if let Some(save) = &self.save_widths {
            save(self.sidebar_w, self.side_w);
        }
    }
}

impl<S: 'static> Live<S> {
    pub(super) fn load_widths(&mut self) {
        if let Some(load) = self.widths.load.take() {
            let (sidebar, side) = load();
            self.controls.set_widths(sidebar, side);
        }
    }

    pub(super) fn widths(&self, viewport_w: f32) -> (f32, f32) {
        clamp_pair(self.controls.sidebar_w, self.controls.side_w, viewport_w)
    }

    pub(super) fn sidebar_edge(&self, sidebar_w: f32, viewport: (f32, f32)) -> El {
        let vw = viewport.0;
        el().key("playground-sidebar-edge")
            .absolute(sidebar_w - 3.0, 0.0)
            .w(7.0)
            .h(viewport.1)
            .cursor(Cursor::ResizeColumn)
            .help("Drag to resize; double-click for the default width")
            .on_drag(move |c: &mut Controls, d: &mut Drag, tx: &mut Tx| {
                tx.instant();
                match d.phase {
                    Phase::Down => d.data = clamp_pair(c.sidebar_w, c.side_w, vw).0,
                    Phase::Move => {
                        let (_, side_w) = clamp_pair(c.sidebar_w, c.side_w, vw);
                        let max = SIDEBAR_MAX.min((vw - side_w - STAGE_MIN - 2.0).max(SIDEBAR_MIN));
                        c.sidebar_w = (d.data + d.dx).clamp(SIDEBAR_MIN, max).round();
                    }
                    Phase::Up if d.cancelled() => c.sidebar_w = d.data,
                    Phase::Up => c.save_widths(),
                }
            })
            .on_double_press_tx(|c: &mut Controls, tx: &mut Tx| {
                tx.instant();
                c.sidebar_w = SIDEBAR;
                c.save_widths();
            })
    }

    pub(super) fn side_edge(&self, side_w: f32, viewport: (f32, f32)) -> El {
        let vw = viewport.0;
        el().key("playground-side-edge")
            .absolute(vw - side_w - 4.0, 0.0)
            .w(7.0)
            .h(viewport.1)
            .cursor(Cursor::ResizeColumn)
            .help("Drag to resize; double-click for the default width")
            .on_drag(move |c: &mut Controls, d: &mut Drag, tx: &mut Tx| {
                tx.instant();
                match d.phase {
                    Phase::Down => d.data = clamp_pair(c.sidebar_w, c.side_w, vw).1,
                    Phase::Move => {
                        let (sidebar_w, _) = clamp_pair(c.sidebar_w, c.side_w, vw);
                        let max = SIDE_MAX.min((vw - sidebar_w - STAGE_MIN - 2.0).max(SIDE_MIN));
                        c.side_w = (d.data - d.dx).clamp(SIDE_MIN, max).round();
                    }
                    Phase::Up if d.cancelled() => c.side_w = d.data,
                    Phase::Up => c.save_widths(),
                }
            })
            .on_double_press_tx(|c: &mut Controls, tx: &mut Tx| {
                tx.instant();
                c.side_w = SIDE;
                c.save_widths();
            })
    }
}
