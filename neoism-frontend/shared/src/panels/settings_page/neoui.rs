//! Settings presentation owners. The pane remains the canonical value model;
//! these comparable snapshots describe presentation, not an editable copy.
use neoui::widgets::{Toggle, ToggleStyle};
use neoui::{Component, ComponentContext, Element, Size, TextStyle};

use crate::primitives::ide_theme::IdeTheme;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct RowPalette {
    foreground: neoui::Color,
    muted: neoui::Color,
    background: neoui::Color,
    accent: neoui::Color,
    border: neoui::Color,
    divider: neoui::Color,
}
impl RowPalette {
    pub(super) fn new(theme: &IdeTheme) -> Self {
        Self {
            foreground: theme.f32(theme.fg),
            muted: theme.f32(theme.dim),
            background: theme.f32(theme.bg),
            accent: theme.f32(theme.accent),
            border: theme.f32(theme.border),
            divider: theme.f32_alpha(theme.border, 0.5),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct RowSnapshot {
    pub path: String,
    pub label: String,
    pub detail: String,
    /// None reserves the existing nonboolean control's measured height.
    pub checked: Option<bool>,
    pub legacy_height: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct SettingsRowsProps {
    pub rows: Vec<RowSnapshot>,
    pub width: f32,
    pub scale: f32,
    pub palette: RowPalette,
    pub focused_key: Option<String>,
}

pub(super) struct SettingsRows;
pub(super) enum SettingsRowsEvent {
    KeyboardFocusVisible(bool),
}
impl Component for SettingsRows {
    type Props = SettingsRowsProps;
    type State = bool;
    type Event = SettingsRowsEvent;
    type Action = String;

    fn mount(_: &Self::Props, _: &mut ComponentContext<'_, String>) -> bool {
        false
    }
    fn update(
        state: &mut bool,
        _: &Self::Props,
        event: Self::Event,
        _: &mut ComponentContext<'_, String>,
    ) -> Vec<String> {
        let SettingsRowsEvent::KeyboardFocusVisible(visible) = event;
        *state = visible;
        Vec::new()
    }
    fn render(
        keyboard_focus: &bool,
        props: &Self::Props,
        cx: &mut ComponentContext<'_, String>,
    ) -> Element<String> {
        Element::column(
            "settings/root",
            props.rows.iter().map(|row| {
                if let Some(checked) = row.checked {
                    cx.child::<BooleanRow>(
                        &row.path,
                        BooleanRowProps {
                            path: row.path.clone(),
                            label: row.label.clone(),
                            detail: row.detail.clone(),
                            checked,
                            keyboard_focused: *keyboard_focus
                                && props.focused_key.as_deref()
                                    == Some(toggle_key(&row.path).as_str()),
                            width: props.width,
                            scale: props.scale,
                            palette: props.palette.clone(),
                        },
                    )
                } else {
                    Element::spacer(row_key(&row.path))
                        .height(Size::Px(row.legacy_height))
                }
            }),
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct BooleanRowProps {
    path: String,
    label: String,
    detail: String,
    checked: bool,
    keyboard_focused: bool,
    width: f32,
    scale: f32,
    palette: RowPalette,
}
struct BooleanRow;
impl Component for BooleanRow {
    type Props = BooleanRowProps;
    type State = ();
    type Event = ();
    type Action = String;
    fn mount(_: &Self::Props, _: &mut ComponentContext<'_, String>) {}
    fn update(
        _: &mut (),
        _: &Self::Props,
        _: (),
        _: &mut ComponentContext<'_, String>,
    ) -> Vec<String> {
        Vec::new()
    }
    fn render(
        _: &(),
        props: &Self::Props,
        _: &mut ComponentContext<'_, String>,
    ) -> Element<String> {
        boolean_row(props)
    }
}

pub(super) fn row_key(path: &str) -> String {
    format!("setting:{path}")
}
pub(super) fn toggle_key(path: &str) -> String {
    format!("setting:{path}/toggle")
}

fn boolean_row(props: &BooleanRowProps) -> Element<String> {
    let key = row_key(&props.path);
    let child_key = |suffix: &str| format!("{key}/{suffix}");
    let scale = props.scale;
    let palette = &props.palette;
    // Drop one final empty paragraph, preserving interior/leading blank lines.
    let mut children = Vec::new();
    if !props.label.trim().is_empty() {
        children.push(Element::text(
            child_key("label"),
            props.label.strip_suffix('\n').unwrap_or(&props.label),
            TextStyle {
                size: 14.0 * scale,
                line_height: 20.0 * scale,
                color: palette.foreground,
            },
        ));
    }
    if !props.detail.trim().is_empty() {
        children
            .push(Element::spacer(child_key("detail-gap")).height(Size::Px(4.0 * scale)));
        children.push(Element::text(
            child_key("description"),
            props.detail.strip_suffix('\n').unwrap_or(&props.detail),
            TextStyle {
                size: 12.0 * scale,
                line_height: 18.0 * scale,
                color: palette.muted,
            },
        ));
    }
    children
        .push(Element::spacer(child_key("control-gap")).height(Size::Px(13.0 * scale)));
    let switch_width = (40.0 * scale).min(props.width);
    children.push(
        Toggle::new(
            toggle_key(&props.path),
            &props.label,
            props.checked,
            props.path.clone(),
        )
        .build(&ToggleStyle {
            width: switch_width,
            height: 22.0 * scale,
            inset: 3.0 * scale,
            on: palette.accent,
            off: palette.border,
            knob: palette.background,
        }),
    );
    children.push(Element::spacer(child_key("focus-gap")).height(Size::Px(4.0 * scale)));
    // Keep the kind and geometry stable: focus visibility is paint-only.
    children.push(
        Element::quad(
            child_key("focus"),
            if props.keyboard_focused {
                palette.accent
            } else {
                [0.0; 4]
            },
            0.0,
        )
        .width(Size::Px(switch_width))
        .height(Size::Px(scale))
        .align_self(neoui::Align::Start),
    );
    children
        .push(Element::spacer(child_key("divider-gap")).height(Size::Px(14.0 * scale)));
    children.push(
        Element::quad(child_key("divider"), palette.divider, 0.0).height(Size::Px(scale)),
    );
    children
        .push(Element::spacer(child_key("bottom-gap")).height(Size::Px(11.0 * scale)));
    Element::column(key, children)
}

// Preserve the existing geometry fixtures without maintaining a second control.
#[cfg(test)]
pub(super) fn toggle_row(
    row: &super::state::SettingRow,
    detail: &str,
    checked: bool,
    keyboard_focused: bool,
    width: f32,
    scale: f32,
    theme: &IdeTheme,
) -> Element<String> {
    boolean_row(&BooleanRowProps {
        path: row.path.clone(),
        label: row.label.clone(),
        detail: detail.to_owned(),
        checked,
        keyboard_focused,
        width,
        scale,
        palette: RowPalette::new(theme),
    })
}
