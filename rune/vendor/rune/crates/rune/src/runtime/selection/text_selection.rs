//! What code sees of the [`El::selectable`](crate::El::selectable) text selection: the
//! selection itself ([`TextSelection`], one [`SelectedText`] per text) and what an
//! [`El::on_selection_change`](crate::El::on_selection_change) element hears
//! ([`SelectionChange`]). The selection's state and behaviour are in `runtime/selection/selectable.rs`.
use std::ops::Range;
use std::rc::Rc;

/// A selection of [`El::selectable`](crate::El::selectable) text, as
/// [`Tx::text_selection`](crate::Tx::text_selection) reads it and an
/// [`El::selection_copy`](crate::El::selection_copy) element sees it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextSelection {
    pub(super) texts: Vec<SelectedText>,
    pub(super) text: String,
    pub(super) backward: bool,
}

impl TextSelection {
    /// The selected texts in tree order (for [`El::selection_copy`](crate::El::selection_copy),
    /// every text it could reach).
    pub fn texts(&self) -> &[SelectedText] { &self.texts }
    /// The selected text as copying takes it without
    /// [`El::selection_copy`](crate::El::selection_copy): as authored (before
    /// [`El::text_case`](crate::El::text_case)), without [`El::uncopied`](crate::El::uncopied)
    /// bytes, with a newline between texts and a tab between texts side by side (table cells).
    pub fn text(&self) -> &str { &self.text }
    /// The selection's focus (where a drag is, or ended) comes before its anchor (where it
    /// began): it was made dragging up or to the left. A toolbar over it can open at the end
    /// the pointer is at.
    pub fn backward(&self) -> bool { self.backward }
}

/// One [`El::selectable`](crate::El::selectable) text of a [`TextSelection`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectedText {
    pub(super) path: String,
    pub(super) key: Option<String>,
    pub(super) range: Range<usize>,
    pub(super) len: usize,
    pub(super) whole: bool,
    pub(super) text: String,
}

impl SelectedText {
    /// The text element's path ([`ElementInfo::path`](crate::ElementInfo)); for
    /// [`El::selection_copy`](crate::El::selection_copy), relative to the element that asks,
    /// empty for that element itself.
    pub fn path(&self) -> &str { &self.path }
    /// The text element's own key, if it has one.
    pub fn key(&self) -> Option<&str> { self.key.as_deref() }
    /// The selected bytes of the text's content, empty when none are.
    pub fn range(&self) -> Range<usize> { self.range.clone() }
    /// The length in bytes of the text's content.
    pub fn len(&self) -> usize { self.len }
    /// The text has no content.
    pub fn is_empty(&self) -> bool { self.len == 0 }
    /// Some of the text is selected.
    pub fn is_selected(&self) -> bool { !self.range.is_empty() }
    /// All of the text is selected.
    pub fn is_whole(&self) -> bool { self.whole }
    /// The selected part as copying takes it.
    pub fn text(&self) -> &str { &self.text }
}

/// What an [`El::on_selection_change`](crate::El::on_selection_change) element hears.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionChange {
    pub(super) selection: Option<Rc<TextSelection>>,
    pub(super) was: bool,
    pub(super) dragging: bool,
}

impl SelectionChange {
    /// The whole selection while any of it is inside the element, `None` otherwise.
    pub fn selection(&self) -> Option<&TextSelection> { self.selection.as_deref() }
    /// A selection appeared inside the element, where there was none.
    pub fn started(&self) -> bool { !self.was && self.selection.is_some() }
    /// The selection cleared or left the element.
    pub fn cleared(&self) -> bool { self.was && self.selection.is_none() }
    /// A press is still making the selection (a drag, or the press of a double-click). When it
    /// releases, an element holding some of the selection hears it once more with this false,
    /// even though the selection didn't change, so a toolbar can wait for the release to
    /// open. A selection made by code is never dragging.
    pub fn dragging(&self) -> bool { self.dragging }
}
