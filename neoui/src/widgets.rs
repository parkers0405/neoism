//! Composable controls. Styles are plain data, state belongs to the component
//! owner, and actions return to the application rather than invoking callbacks.

use crate::{Align, Color, Element, Insets, Justify, Role, Semantics, Size, TextStyle};

#[derive(Clone, Debug, PartialEq)]
pub struct ControlTheme {
    pub foreground: Color,
    pub muted: Color,
    pub background: Color,
    pub surface: Color,
    pub accent: Color,
    pub border: Color,
    pub disabled: Color,
    pub text_size: f32,
    pub line_height: f32,
    pub radius: f32,
}

impl Default for ControlTheme {
    fn default() -> Self {
        Self { foreground: [0.9, 0.91, 0.94, 1.0], muted: [0.55, 0.58, 0.64, 1.0],
            background: [0.08, 0.09, 0.11, 1.0], surface: [0.13, 0.15, 0.18, 1.0],
            accent: [0.46, 0.65, 0.96, 1.0], border: [0.25, 0.28, 0.33, 1.0],
            disabled: [0.35, 0.37, 0.42, 1.0], text_size: 14.0, line_height: 20.0, radius: 6.0 }
    }
}

impl ControlTheme {
    pub fn text(&self) -> TextStyle {
        TextStyle { size: self.text_size, line_height: self.line_height, color: self.foreground }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonVariant { #[default] Standard, Primary, Quiet, Danger }

pub struct Button<Action> {
    key: String,
    label: String,
    action: Action,
    disabled: bool,
    variant: ButtonVariant,
    padding: Insets,
}

impl<Action> Button<Action> {
    pub fn new(key: impl Into<String>, label: impl Into<String>, action: Action) -> Self {
        Self { key: key.into(), label: label.into(), action, disabled: false,
            variant: ButtonVariant::Standard, padding: Insets::symmetric(12.0, 7.0) }
    }
    pub fn disabled(mut self, disabled: bool) -> Self { self.disabled = disabled; self }
    pub fn variant(mut self, variant: ButtonVariant) -> Self { self.variant = variant; self }
    pub fn padding(mut self, padding: Insets) -> Self { self.padding = padding; self }
    pub fn build(self, theme: &ControlTheme) -> Element<Action> {
        let background = match self.variant {
            ButtonVariant::Primary => theme.accent,
            ButtonVariant::Quiet => [0.0; 4],
            ButtonVariant::Danger => [0.65, 0.16, 0.19, 1.0],
            ButtonVariant::Standard => theme.surface,
        };
        let foreground = if self.disabled { theme.disabled }
            else if self.variant == ButtonVariant::Primary { theme.background }
            else { theme.foreground };
        let content = Element::row(format!("{}/content", self.key), [
            Element::text(format!("{}/label", self.key), &self.label,
                TextStyle { color: foreground, ..theme.text() }),
        ]).padding(self.padding).justify_content(Justify::Center).align_items(Align::Center);
        Element::stack(&self.key, [
            Element::quad(format!("{}/background", self.key), background, theme.radius), content,
        ]).on_pointer_down(self.action)
            .semantics(Semantics::new(Role::Button).label(self.label).disabled(self.disabled))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToggleStyle {
    pub width: f32,
    pub height: f32,
    pub inset: f32,
    pub on: Color,
    pub off: Color,
    pub knob: Color,
}

impl Default for ToggleStyle {
    fn default() -> Self {
        let theme = ControlTheme::default();
        Self { width: 40.0, height: 22.0, inset: 3.0,
            on: theme.accent, off: theme.border, knob: theme.background }
    }
}

pub struct Toggle<Action> {
    key: String, label: String, checked: bool, disabled: bool, action: Action,
}

impl<Action> Toggle<Action> {
    pub fn new(key: impl Into<String>, label: impl Into<String>, checked: bool, action: Action) -> Self {
        Self { key: key.into(), label: label.into(), checked, disabled: false, action }
    }
    pub fn disabled(mut self, disabled: bool) -> Self { self.disabled = disabled; self }
    pub fn build(self, style: &ToggleStyle) -> Element<Action> {
        let knob_size = (style.height - 2.0 * style.inset).max(0.0);
        let track = Element::quad(format!("{}/track", self.key),
            if self.checked { style.on } else { style.off }, style.height * 0.5);
        let knob = Element::row(format!("{}/knob-layout", self.key), [
            Element::quad(format!("{}/knob", self.key), style.knob, knob_size * 0.5)
                .width(Size::Px(knob_size)).height(Size::Px(knob_size)).shrink(0.0),
        ]).padding(Insets::all(style.inset))
            .justify_content(if self.checked { Justify::End } else { Justify::Start })
            .align_items(Align::Center);
        Element::stack(self.key, [track, knob]).width(Size::Px(style.width))
            .height(Size::Px(style.height)).align_self(Align::Start)
            .on_pointer_down(self.action)
            .semantics(Semantics::new(Role::Switch).label(self.label)
                .checked(self.checked).disabled(self.disabled))
    }
}

pub fn label<Action>(key: impl Into<String>, text: impl Into<String>, theme: &ControlTheme) -> Element<Action> {
    Element::text(key, text, theme.text())
}

pub fn separator<Action>(key: impl Into<String>, color: Color, thickness: f32) -> Element<Action> {
    Element::quad(key, color, 0.0).height(Size::Px(thickness))
        .semantics(Semantics::new(Role::Separator))
}

/// A card supplies only composition and appearance. Focus trapping and modal
/// event ownership belong to the surface/router, not to a paint-only widget.
pub fn card<Action>(key: impl Into<String>, content: Element<Action>, theme: &ControlTheme, padding: Insets) -> Element<Action> {
    let key = key.into();
    Element::stack(&key, [
        Element::quad(format!("{key}/background"), theme.surface, theme.radius),
        Element::column(format!("{key}/body"), [content]).padding(padding),
    ]).semantics(Semantics::new(Role::Group))
}

pub struct MenuItem<Action> {
    pub key: String,
    pub label: String,
    pub action: Action,
    pub disabled: bool,
    pub selected: bool,
}

pub fn menu<Action>(key: impl Into<String>, items: impl IntoIterator<Item = MenuItem<Action>>, theme: &ControlTheme) -> Element<Action> {
    let key = key.into();
    let rows = items.into_iter().map(|item| {
        let style = if item.selected { ButtonVariant::Primary } else { ButtonVariant::Quiet };
        Button::new(item.key, item.label, item.action).variant(style)
            .disabled(item.disabled).build(theme)
    });
    card(&key, Element::column(format!("{key}/items"), rows).gap(2.0), theme, Insets::all(4.0))
}
