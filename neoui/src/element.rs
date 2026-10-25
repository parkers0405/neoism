use crate::{Color, TextStyle};
use crate::input::{Key, Modifiers};
use crate::routing::{EventBinding, EventKind, EventPhase, KeyBinding};

/// Flex size. Fractions use `0.0..=1.0` (not `0..=100`). Percent heights in an
/// auto-height root are indefinite; prefer Auto or logical-pixel heights there.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Size {
    #[default]
    Auto,
    Px(f32),
    Percent(f32),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Insets {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Insets {
    pub const fn all(value: f32) -> Self {
        Self {
            top: value,
            right: value,
            bottom: value,
            left: value,
        }
    }
    pub const fn symmetric(horizontal: f32, vertical: f32) -> Self {
        Self {
            top: vertical,
            right: horizontal,
            bottom: vertical,
            left: horizontal,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    Start,
    Center,
    End,
    #[default]
    Stretch,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Justify {
    #[default]
    Start,
    Center,
    End,
    SpaceBetween,
    SpaceAround,
}

/// Deliberately small, renderer-independent subset of flex/grid style.
/// All sizes include padding (border-box). There is no pixel rounding.
#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    pub width: Size,
    pub height: Size,
    pub min_width: Size,
    pub min_height: Size,
    pub max_width: Size,
    pub max_height: Size,
    pub flex_grow: f32,
    pub flex_shrink: f32,
    pub flex_basis: Size,
    pub gap: f32,
    pub padding: Insets,
    pub align_items: Align,
    pub align_self: Option<Align>,
    pub justify_content: Justify,
    /// Clip descendants to this node's rectangular border box. This affects
    /// paints, hits and semantics together; it does not create a scroll container.
    pub clip: bool,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            width: Size::Auto,
            height: Size::Auto,
            // Zero minimum permits wrapping text inside shrinking flex children.
            min_width: Size::Px(0.0),
            min_height: Size::Px(0.0),
            max_width: Size::Auto,
            max_height: Size::Auto,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: Size::Auto,
            gap: 0.0,
            padding: Insets::default(),
            align_items: Align::Stretch,
            align_self: None,
            justify_content: Justify::Start,
            clip: false,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Role {
    #[default]
    Generic,
    Group,
    Text,
    Button,
    Checkbox,
    Switch,
    Separator,
    Custom(String),
}

/// Accessibility metadata, not a platform accessibility implementation. Disabled
/// is inherited by descendants and suppresses pointer actions and focus.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Semantics {
    pub role: Role,
    pub label: Option<String>,
    pub checked: Option<bool>,
    pub disabled: bool,
}

impl Semantics {
    pub fn new(role: Role) -> Self {
        Self {
            role,
            ..Self::default()
        }
    }
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Kind {
    Column,
    Row,
    Stack,
    Spacer,
    Text { text: String, style: TextStyle },
    Quad { color: Color, radius: f32 },
}

/// A disposable description. Keys must be nonempty and unique across the whole
/// root, not just siblings. Reuse keys for the same logical control on rebuild.
/// Leaves intentionally cannot contain children. Stacks overlap children in
/// insertion order, last painted/hit-tested on top, with intrinsic max sizing.
#[derive(Clone, Debug)]
pub struct Element<Action> {
    pub(crate) key: String,
    pub(crate) kind: Kind,
    pub(crate) style: Style,
    pub(crate) children: Vec<Self>,
    pub(crate) action: Option<Action>,
    pub(crate) semantics: Option<Semantics>,
    pub(crate) event_bindings: Vec<EventBinding<Action>>,
    pub(crate) key_bindings: Vec<KeyBinding<Action>>,
    pub(crate) focus_scope: Option<bool>,
    pub(crate) capture_pointer: bool,
    pub(crate) focusable: bool,
}

impl<Action> Element<Action> {
    fn new(key: impl Into<String>, kind: Kind, children: Vec<Self>) -> Self {
        Self {
            key: key.into(),
            kind,
            style: Style::default(),
            children,
            action: None,
            semantics: None,
            event_bindings: Vec::new(),
            key_bindings: Vec::new(),
            focus_scope: None,
            capture_pointer: false,
            focusable: false,
        }
    }

    pub fn column(
        key: impl Into<String>,
        children: impl IntoIterator<Item = Self>,
    ) -> Self {
        Self::new(key, Kind::Column, children.into_iter().collect())
    }
    pub fn row(key: impl Into<String>, children: impl IntoIterator<Item = Self>) -> Self {
        Self::new(key, Kind::Row, children.into_iter().collect())
    }
    pub fn stack(
        key: impl Into<String>,
        children: impl IntoIterator<Item = Self>,
    ) -> Self {
        Self::new(key, Kind::Stack, children.into_iter().collect())
    }
    pub fn spacer(key: impl Into<String>) -> Self {
        Self::new(key, Kind::Spacer, vec![])
    }
    pub fn text(
        key: impl Into<String>,
        text: impl Into<String>,
        style: TextStyle,
    ) -> Self {
        Self::new(
            key,
            Kind::Text {
                text: text.into(),
                style,
            },
            vec![],
        )
    }
    pub fn quad(key: impl Into<String>, color: Color, radius: f32) -> Self {
        Self::new(key, Kind::Quad { color, radius }, vec![])
    }

    pub fn key(&self) -> &str {
        &self.key
    }
    pub fn style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }
    pub fn width(mut self, width: Size) -> Self {
        self.style.width = width;
        self
    }
    pub fn height(mut self, height: Size) -> Self {
        self.style.height = height;
        self
    }
    pub fn min_width(mut self, width: Size) -> Self {
        self.style.min_width = width;
        self
    }
    pub fn max_width(mut self, width: Size) -> Self {
        self.style.max_width = width;
        self
    }
    pub fn grow(mut self, grow: f32) -> Self {
        self.style.flex_grow = grow;
        self
    }
    pub fn shrink(mut self, shrink: f32) -> Self {
        self.style.flex_shrink = shrink;
        self
    }
    pub fn basis(mut self, basis: Size) -> Self {
        self.style.flex_basis = basis;
        self
    }
    pub fn gap(mut self, gap: f32) -> Self {
        self.style.gap = gap;
        self
    }
    pub fn padding(mut self, padding: Insets) -> Self {
        self.style.padding = padding;
        self
    }
    pub fn align_items(mut self, align: Align) -> Self {
        self.style.align_items = align;
        self
    }
    pub fn align_self(mut self, align: Align) -> Self {
        self.style.align_self = Some(align);
        self
    }
    pub fn justify_content(mut self, justify: Justify) -> Self {
        self.style.justify_content = justify;
        self
    }
    pub fn clip(mut self, clip: bool) -> Self {
        self.style.clip = clip;
        self
    }
    /// Register a listener without retaining any application callback.
    pub fn on_event(self, kind: EventKind, phase: EventPhase, action: Action) -> Self {
        self.event_binding(EventBinding::new(kind, phase, action))
    }
    pub fn event_binding(mut self, binding: EventBinding<Action>) -> Self {
        self.event_bindings.push(binding);
        self
    }
    pub fn key_binding(self, key: Key, modifiers: Modifiers, action: Action) -> Self {
        self.bind_key(KeyBinding::new(key, modifiers, action))
    }
    pub fn bind_key(mut self, binding: KeyBinding<Action>) -> Self {
        self.key_bindings.push(binding);
        self
    }
    /// A trapped scope is modal: the last mounted enabled trap owns traversal
    /// and pointer input. Removing it restores the previous mounted identity.
    pub fn focus_scope(mut self, trap: bool) -> Self {
        self.focus_scope = Some(trap);
        self
    }
    pub fn capture_pointer(mut self, capture: bool) -> Self {
        self.capture_pointer = capture;
        self
    }
    /// Make raw text/IME/key listener targets keyboard-focusable without an
    /// artificial accessibility role or legacy activation action.
    pub fn focusable(mut self, focusable: bool) -> Self {
        self.focusable = focusable;
        self
    }

    pub fn on_pointer_down(mut self, action: Action) -> Self {
        self.action = Some(action);
        self
    }
    pub fn semantics(mut self, semantics: Semantics) -> Self {
        self.semantics = Some(semantics);
        self
    }
}
