//! What an element tells assistive technology beyond its role and label: description, value,
//! state, live updates and its place in a set. Screen readers, Voice Control and automation
//! tools read it through the OS accessibility tree (`crate::a11y`).

use crate::El;

/// How assistive technology announces changes to an element's text ([`El::live_region`]), as ARIA's
/// `aria-live`. Role `"status"` is polite and `"alert"` assertive unless set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LiveRegion {
    /// Changes aren't announced.
    #[default]
    Off,
    /// Announced when the user is idle: a status line, a saved notice.
    Polite,
    /// Announced at once, interrupting: an error.
    Assertive,
}

/// A checkbox's, switch's or toggle button's state for accessibility ([`El::checked`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Checked {
    False,
    True,
    Mixed,
}

/// A range control's value ([`El::range_value`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Range {
    pub value: f64,
    pub min: f64,
    pub max: f64,
    pub step: Option<f64>,
}

/// The rarer accessibility properties, boxed so an element without them stays small.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct A11yMore {
    pub description: Option<String>,
    pub value: Option<String>,
    pub range: Option<Range>,
    /// 1-based position and the set's size.
    pub set: Option<(usize, usize)>,
    pub shortcut: Option<String>,
    /// `El::active_descendant`'s key.
    pub active: Option<String>,
    pub invalid: bool,
    pub read_only: bool,
    /// `El::sorted`: ascending or descending.
    pub sorted: Option<bool>,
    /// `El::href`: the URL this element links to.
    pub href: Option<String>,
}

/// Accessibility state beyond role and label (`El::level`, `El::expanded`, `El::secure`, and
/// this file's builders).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct A11y {
    pub level: Option<u32>,
    pub expanded: Option<bool>,
    pub secure: bool,
    pub checked: Option<Checked>,
    pub disabled: bool,
    pub hidden: bool,
    pub live: Option<LiveRegion>,
    pub more: Option<Box<A11yMore>>,
}

impl A11y {
    pub(crate) const EMPTY: A11y = A11y {
        level: None, expanded: None, secure: false, checked: None, disabled: false, hidden: false, live: None, more: None,
    };

    fn more(&mut self) -> &mut A11yMore {
        self.more.get_or_insert_with(Default::default)
    }
}

impl El {
    /// Make this element a link to `url`, as an HTML `<a href>` is: its role is `"link"`, a
    /// press opens `url` with [`Tx::open_url`](crate::Tx::open_url), and on web the semantic
    /// mirror puts a real `<a href>` over it, so middle-click, ⌘-click, "Copy Link", the
    /// keyboard and crawlers treat it as one. A static page export (`--html`) writes it as one
    /// too. `url` may be relative to the page (`/docs`) or a fragment (`#install`); natively
    /// those don't open. Its label is its text unless [`El::label`] sets one.
    pub fn href(mut self, url: impl Into<String>) -> Self {
        let url = url.into();
        self.cold_mut().a11y.more().href = Some(url.clone());
        self.set_role_name("link");
        self.handlers.on_press = Some(std::rc::Rc::new(move |_, tx: &mut crate::Tx| {
            if url.contains(':') { let _ = tx.open_url(&url); }
        }));
        self
    }

    /// The URL [`El::href`] set.
    pub fn link_url(&self) -> Option<&str> {
        self.cold.a11y.more.as_ref()?.href.as_deref()
    }

    /// A longer explanation than the label, read after it (ARIA `aria-description`): what a
    /// setting does, or the shortcut a button has. The label stays short. Without one, an
    /// [`El::help`] tooltip describes the element; `description("")` turns that off.
    pub fn description(mut self, text: impl Into<String>) -> Self {
        self.cold_mut().a11y.more().description = Some(text.into());
        self
    }
    /// The element whose content is active while this one keeps keyboard focus (ARIA
    /// `aria-activedescendant`): the highlighted row of a tree, table or suggestion list whose
    /// container or search field holds focus. `key` names it as `Cx::layout` does.
    pub fn active_descendant(mut self, key: impl Into<String>) -> Self {
        self.cold_mut().a11y.more().active = Some(key.into());
        self
    }
    /// Whether the value entered here fails validation (ARIA `aria-invalid`).
    pub fn invalid(mut self, invalid: bool) -> Self {
        self.cold_mut().a11y.more().invalid = invalid;
        self
    }
    /// A control whose value can be read but not changed now (ARIA `aria-readonly`): a
    /// checklist shown in a finished document.
    pub fn read_only(mut self, read_only: bool) -> Self {
        self.cold_mut().a11y.more().read_only = read_only;
        self
    }
    /// How a column header sorts its table: `Some(true)` ascending, `Some(false)` descending
    /// (ARIA `aria-sort`).
    pub fn sorted(mut self, ascending: Option<bool>) -> Self {
        self.cold_mut().a11y.more().sorted = ascending;
        self
    }
    /// The value assistive technology reads for a control whose value isn't its text: a
    /// picker's chosen item, a colour well's colour, a step counter's "3 of 5". Text fields
    /// report their text and range controls their number without it.
    pub fn a11y_value(mut self, text: impl Into<String>) -> Self {
        self.cold_mut().a11y.more().value = Some(text.into());
        self
    }
    /// A slider's or progress bar's number, between `min` and `max` (ARIA `aria-valuenow`,
    /// `aria-valuemin` and `aria-valuemax`). Assistive technology increments it through
    /// [`El::on_step`].
    pub fn range_value(mut self, value: f64, min: f64, max: f64) -> Self {
        let step = self.cold.a11y.more.as_ref().and_then(|m| m.range).and_then(|r| r.step);
        self.cold_mut().a11y.more().range = Some(Range { value, min, max, step });
        self
    }
    /// The amount one step of [`El::on_step`] moves a [`range_value`](Self::range_value).
    pub fn range_step(mut self, step: f64) -> Self {
        let more = self.cold_mut().a11y.more();
        let range = more.range.get_or_insert(Range { value: 0.0, min: 0.0, max: 0.0, step: None });
        range.step = Some(step);
        self
    }
    /// Whether a checkbox, switch, radio button or toggle button is on (ARIA `aria-checked`
    /// or `aria-pressed`). Leave it unset on controls that can't be.
    pub fn checked(mut self, on: bool) -> Self {
        self.cold_mut().a11y.checked = Some(if on { Checked::True } else { Checked::False });
        self
    }
    /// A checkbox that is partly on: some of what it controls is (ARIA `aria-checked="mixed"`).
    pub fn checked_mixed(mut self) -> Self {
        self.cold_mut().a11y.checked = Some(Checked::Mixed);
        self
    }
    /// A control that can't be used now: assistive technology reads it as dimmed. It also
    /// stops it taking focus; leave its press handler off as well.
    pub fn disabled(mut self, disabled: bool) -> Self {
        if disabled || !self.cold.is_empty() {
            let cold = self.cold_mut();
            cold.a11y.disabled = disabled;
            if disabled { let f = &mut *cold.focus; f.enabled = false; f.opt_out = true; }
        }
        self
    }
    /// Leave this element and everything inside it out of the accessibility tree: decoration,
    /// or a duplicate of what another element already says. It still draws and takes input.
    pub fn a11y_hidden(mut self) -> Self {
        self.cold_mut().a11y.hidden = true;
        self
    }
    /// How changes to this element's text are announced (ARIA `aria-live`), for status text
    /// that updates while the user does something else.
    pub fn live_region(mut self, live: LiveRegion) -> Self {
        self.cold_mut().a11y.live = Some(live);
        self
    }
    /// This element's place in its set, 1-based, and the set's size (ARIA `aria-posinset` and
    /// `aria-setsize`): a row of a list that builds only some of its rows says where it is.
    /// Rows of `virtual_rows` and `virtualize` lists get it without this.
    pub fn position_in_set(mut self, position: usize, size: usize) -> Self {
        self.cold_mut().a11y.more().set = Some((position, size));
        self
    }
    /// The keyboard shortcut that does what this control does, as the platform writes it
    /// (`"Ctrl+S"`, `"⌘S"`), for assistive technology to read.
    pub fn shortcut_hint(mut self, keys: impl Into<String>) -> Self {
        self.cold_mut().a11y.more().shortcut = Some(keys.into());
        self
    }
}
