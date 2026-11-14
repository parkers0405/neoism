//! Files dropped on an element from another app: `El::on_drop_files` and the hover, move
//! and reject callbacks around it, and `Tx::drop_point`. `runtime/input/drop.rs` routes the drag.
use std::rc::Rc;

use crate::el::{El, PointerEvent, Tx};
use crate::states::downcast;

pub(crate) type DropFilesFn = Rc<dyn Fn(crate::states::States<'_>, &[std::path::PathBuf], &mut Tx)>;

/// [`El::on_drop_files`], [`El::accepts_files`], [`El::on_drop_hover`], [`El::on_drop_move`],
/// [`El::on_drop_rejected`] and [`El::on_drop_rejected_hover`].
#[derive(Clone, Default)]
pub(crate) struct DropSpec {
    pub on_drop: Option<DropFilesFn>,
    pub accepts: Option<AcceptsFn>,
    pub on_hover: Option<DropHoverFn>,
    pub on_reject: Option<DropFilesFn>,
    pub on_reject_hover: Option<DropHoverFn>,
    pub on_move: Option<DropMoveFn>,
}
pub(crate) type AcceptsFn = Rc<dyn Fn(&std::path::Path) -> bool>;
pub(crate) type DropHoverFn = Rc<dyn Fn(crate::states::States<'_>, bool, &mut Tx)>;
pub(crate) type DropMoveFn = Rc<dyn Fn(crate::states::States<'_>, PointerEvent, &mut Tx)>;

impl DropSpec {
    /// The files of `paths` this element takes, when it's a drop target, and the rest.
    pub(crate) fn split(&self, paths: &[std::path::PathBuf]) -> (Vec<std::path::PathBuf>, Vec<std::path::PathBuf>) {
        if self.on_drop.is_none() { return (Vec::new(), paths.to_vec()); }
        paths.iter().cloned().partition(|p| self.accepts.as_ref().is_none_or(|f| f(p)))
    }
    /// It stops a drag it takes none of, rather than passing it to the target around it.
    pub(crate) fn claims_rejects(&self) -> bool { self.on_reject.is_some() || self.on_reject_hover.is_some() }
    /// Its hover callback for a drag it takes (`accepted`) or rejects.
    pub(crate) fn hover_fn(&self, accepted: bool) -> Option<DropHoverFn> {
        if accepted { self.on_hover.clone() } else { self.on_reject_hover.clone() }
    }
}

impl El {
    /// Take files dropped on this element from another app (Finder, Explorer, a file manager).
    /// The innermost target under the drop that accepts any of the files gets the ones it
    /// accepts, all in one call; files it doesn't accept are left out. A drop no target takes
    /// is ignored: put `on_drop_files` on the root as well to catch drops anywhere. While a modal
    /// is open, its root stands in for the window's, and a drop outside it reaches nothing. A drop
    /// target takes no pointer events, so one laid over other elements leaves their clicks,
    /// context menus and help.
    ///
    /// ```
    /// # use rune::el::el;
    /// # use std::path::PathBuf;
    /// # struct App { attachments: Vec<PathBuf>, over: bool }
    /// # let s = App { attachments: Vec::new(), over: false };
    /// el().fill(if s.over { rune::Color::WHITE.alpha(0.1) } else { rune::Color::TRANSPARENT })
    ///     .on_drop_files(|s: &mut App, files, _| s.attachments.extend_from_slice(files))
    ///     .accepts_extensions(&["png", "jpg", "m4a"])
    ///     .on_drop_hover(|s: &mut App, over, _| s.over = over);
    /// ```
    ///
    /// [`Tx::drop_point`] says where on the element the files landed, to insert them there.
    ///
    /// macOS, Windows and X11. Wayland reports no file drags in winit 0.30, and the web build
    /// has no paths to give. Headless scripts drop files with `drop_files` events.
    pub fn on_drop_files<S: 'static>(mut self, f: impl Fn(&mut S, &[std::path::PathBuf], &mut Tx) + 'static) -> Self {
        self.handlers.drop.get_or_insert_default().on_drop = Some(Rc::new(move |s, files, tx| f(downcast::<S>(s), files, tx)));
        self
    }
    /// Only the files `accepts` returns true for reach this element's [`El::on_drop_files`]. A
    /// drag carrying none of them passes this element over: the next accepting ancestor takes
    /// the drop and shows the drop hover.
    pub fn accepts_files(mut self, accepts: impl Fn(&std::path::Path) -> bool + 'static) -> Self {
        self.handlers.drop.get_or_insert_default().accepts = Some(Rc::new(accepts));
        self
    }
    /// [`El::accepts_files`] for files with one of these extensions, compared without case.
    pub fn accepts_extensions(self, extensions: &[&str]) -> Self {
        let extensions: Vec<String> = extensions.iter().map(|e| e.trim_start_matches('.').to_ascii_lowercase()).collect();
        self.accepts_files(move |path| path.extension().and_then(|e| e.to_str())
            .is_some_and(|e| extensions.iter().any(|x| x.eq_ignore_ascii_case(e))))
    }
    /// Called with `true` when a file drag over the window makes this element the drop target
    /// (the innermost [`El::on_drop_files`] under the pointer that accepts the files), and with
    /// `false` when the drag moves off it, leaves the window, or drops, or when the element goes
    /// away under the drag. Draw a drop highlight from it. With `true`, [`Tx::drop_point`] says
    /// where the drag came in.
    pub fn on_drop_hover<S: 'static>(mut self, f: impl Fn(&mut S, bool, &mut Tx) + 'static) -> Self {
        self.handlers.drop.get_or_insert_default().on_hover = Some(Rc::new(move |s, over, tx| f(downcast::<S>(s), over, tx)));
        self
    }
    /// Follow a file drag this element takes ([`El::on_drop_files`]) while it's over it: `f`
    /// hears the drag's point on the element, in the element's own points as
    /// [`El::on_pointer_move_local`] gives them, when it becomes the target and then as the drag
    /// moves, at most once a frame. Draw a drop caret from it, cleared by
    /// [`El::on_drop_hover`]'s `false`, and insert where [`Tx::drop_point`] says on the drop.
    /// Only an element with this asks the window for frames as the drag moves over it, and a
    /// drag the platform gives no position for is never heard here. Scripts move a drag with
    /// further `drag_files` events.
    pub fn on_drop_move<S: 'static>(mut self, f: impl Fn(&mut S, PointerEvent, &mut Tx) + 'static) -> Self {
        self.handlers.drop.get_or_insert_default().on_move = Some(Rc::new(move |s, p, tx| f(downcast::<S>(s), p, tx)));
        self
    }
    /// Hear the files dropped on this element that it doesn't accept ([`El::accepts_files`]),
    /// to say why ("that isn't an export"). With this, or [`El::on_drop_rejected_hover`], the
    /// element stops a drag carrying none of the files it takes instead of passing it to the
    /// target around it, and a drop of only those files comes here. On a drop of some files it
    /// takes and some it doesn't, [`El::on_drop_files`] gets the first and this the rest.
    ///
    /// ```
    /// # use rune::el::el;
    /// # struct App { error: Option<String>, wrong: bool }
    /// el().on_drop_files(|s: &mut App, _, _| s.error = None)
    ///     .accepts_extensions(&["json"])
    ///     .on_drop_rejected(|s: &mut App, files, _| s.error = Some(format!("{} isn't an export", files[0].display())))
    ///     .on_drop_rejected_hover(|s: &mut App, over, _| s.wrong = over);
    /// ```
    pub fn on_drop_rejected<S: 'static>(mut self, f: impl Fn(&mut S, &[std::path::PathBuf], &mut Tx) + 'static) -> Self {
        self.handlers.drop.get_or_insert_default().on_reject = Some(Rc::new(move |s, files, tx| f(downcast::<S>(s), files, tx)));
        self
    }
    /// [`El::on_drop_hover`] for a drag this element stops but takes none of (see
    /// [`El::on_drop_rejected`]): `true` when the drag reaches it, `false` when it moves off,
    /// leaves the window or drops, or when the element goes away under it. Draw a "can't drop
    /// here" state from it.
    pub fn on_drop_rejected_hover<S: 'static>(mut self, f: impl Fn(&mut S, bool, &mut Tx) + 'static) -> Self {
        self.handlers.drop.get_or_insert_default().on_reject_hover = Some(Rc::new(move |s, over, tx| f(downcast::<S>(s), over, tx)));
        self
    }
}

impl Tx {
    /// In [`El::on_drop_files`], [`El::on_drop_rejected`], [`El::on_drop_move`] and
    /// [`El::on_drop_hover`]'s `true`: where the file drag is on the element, in its own points
    /// (`x`, `y`, following its scroll and transforms) and in the window's. `None` when the
    /// platform gave no position and in every other handler.
    pub fn drop_point(&self) -> Option<PointerEvent> { self.drop_point }
}
