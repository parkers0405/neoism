use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::customization::{color_f32, color_u8};
use crate::primitives::IdeTheme;

pub struct CustomUiLayout {
    pub window: [f32; 4],
    pub top: [f32; 4],
    pub bottom: [f32; 4],
    pub left: [f32; 4],
    pub right: [f32; 4],
}

#[derive(Clone)]
pub struct CustomUiHitbox {
    pub rect: [f32; 4],
    pub command: String,
}

pub fn render(
    sugarloaf: &mut Sugarloaf,
    plugins: &neoism_lua::PluginSnapshot,
    layout: CustomUiLayout,
    theme: &IdeTheme,
) -> Vec<CustomUiHitbox> {
    let mut hitboxes = Vec::new();
    let mut items = plugins.contributions.iter().collect::<Vec<_>>();
    items.sort_by_key(|item| item.priority);
    let mut left_offsets = std::collections::HashMap::<&str, f32>::new();
    let mut right_offsets = std::collections::HashMap::<&str, f32>::new();
    for item in items {
        let Some(rect) = slot_rect(&item.slot, &layout) else { continue };
        if rect[2] <= 0.0 || rect[3] <= 0.0 {
            continue;
        }
        let selector = item.style.as_deref().unwrap_or(&item.slot);
        let style = plugins.styles.resolve(selector);
        if style.visible == Some(false) || (item.text.is_empty() && item.icon.is_none()) {
            continue;
        }
        let text = match item.icon.as_deref() {
            Some(icon) if !item.text.is_empty() => format!("{icon} {}", item.text),
            Some(icon) => icon.to_string(),
            None => item.text.clone(),
        };
        let font_size = style.font_size.unwrap_or(12.0);
        let padding = style.padding_x.or(style.padding).unwrap_or(8.0);
        let opts = DrawOpts {
            font_size,
            color: color_u8(style.foreground.as_deref(), theme, theme.u8(theme.fg)),
            clip_rect: Some(rect),
            ..DrawOpts::default()
        };
        let width = (sugarloaf.overlay_text_mut().measure(&text, &opts) + padding * 2.0)
            .min(rect[2]);
        let right_aligned = item.slot.ends_with(".right");
        let offset = if right_aligned {
            right_offsets.entry(&item.slot).or_default()
        } else {
            left_offsets.entry(&item.slot).or_default()
        };
        let x = if right_aligned {
            rect[0] + rect[2] - *offset - width
        } else {
            rect[0] + *offset
        };
        let y = rect[1] + ((rect[3] - font_size * 1.25) * 0.5).max(0.0);
        if style.background.is_some() {
            sugarloaf.overlay_rounded_rect(
                x,
                rect[1],
                width,
                rect[3],
                color_f32(style.background.as_deref(), theme, theme.f32(theme.surface)),
                0.0,
                style.radius.unwrap_or(4.0),
                176,
            );
        }
        sugarloaf.overlay_text_mut().draw(x + padding, y, &text, &opts);
        if let Some(command) = item.command.as_ref() {
            hitboxes.push(CustomUiHitbox {
                rect: [x, rect[1], width, rect[3]],
                command: command.clone(),
            });
        }
        *offset += width + style.gap.unwrap_or(4.0);
    }

    let mut panels = plugins.panels.iter().filter(|panel| panel.visible).collect::<Vec<_>>();
    panels.sort_by_key(|panel| plugins.styles.resolve(&format!("panel.{}", panel.id)).order.unwrap_or(0));
    for panel in panels {
        let style = plugins.styles.resolve(&format!("panel.{}", panel.id));
        let candidate_anchor = match panel.location {
            neoism_lua::PanelLocation::Left => layout.left,
            neoism_lua::PanelLocation::Right => layout.right,
            neoism_lua::PanelLocation::Bottom => layout.bottom,
            neoism_lua::PanelLocation::Center | neoism_lua::PanelLocation::Overlay => layout.window,
        };
        // A retained left/right/bottom panel remains a real overlay even when
        // the corresponding native sidebar/status slot is currently hidden.
        let anchor = if candidate_anchor[2] <= 0.0 || candidate_anchor[3] <= 0.0 {
            layout.window
        } else {
            candidate_anchor
        };
        let width = style.width.unwrap_or(320.0).min(anchor[2]);
        let height = style.height.unwrap_or(240.0).min(anchor[3]);
        let x = match panel.location {
            neoism_lua::PanelLocation::Right => anchor[0] + anchor[2] - width,
            neoism_lua::PanelLocation::Center | neoism_lua::PanelLocation::Overlay => anchor[0] + (anchor[2] - width) * 0.5,
            _ => anchor[0],
        };
        let y = match panel.location {
            neoism_lua::PanelLocation::Bottom => anchor[1] + anchor[3] - height,
            neoism_lua::PanelLocation::Center | neoism_lua::PanelLocation::Overlay => anchor[1] + (anchor[3] - height) * 0.5,
            _ => anchor[1],
        };
        sugarloaf.overlay_rounded_rect(
            x,
            y,
            width,
            height,
            color_f32(style.background.as_deref(), theme, theme.f32(theme.surface)),
            0.0,
            style.radius.unwrap_or(8.0),
            170,
        );
        let mut content_y = y + style.padding_y.or(style.padding).unwrap_or(12.0);
        let title_opts = DrawOpts { font_size: style.font_size.unwrap_or(13.0), color: color_u8(style.foreground.as_deref(), theme, theme.u8(theme.fg)), ..DrawOpts::default() };
        sugarloaf.overlay_text_mut().draw(x + 12.0, content_y, &panel.title, &title_opts);
        content_y += title_opts.font_size * 1.6;
        for item in &panel.content {
            let text = item.icon.as_deref().map_or_else(|| item.text.clone(), |icon| format!("{icon} {}", item.text));
            sugarloaf.overlay_text_mut().draw(x + 12.0, content_y, &text, &title_opts);
            if let Some(command) = item.command.as_ref() {
                hitboxes.push(CustomUiHitbox {
                    rect: [x + 8.0, content_y - 2.0, width - 16.0, title_opts.font_size * 1.5],
                    command: command.clone(),
                });
            }
            content_y += title_opts.font_size * 1.5;
        }
    }
    hitboxes
}

fn slot_rect(slot: &str, layout: &CustomUiLayout) -> Option<[f32; 4]> {
    match slot.split('.').next().unwrap_or(slot) {
        "top" | "chrome" => Some(layout.top),
        "status" | "bottom" => Some(layout.bottom),
        "file-tree" | "notes-tree" | "agent-sidebar" => Some(layout.left),
        "git" => Some(layout.right),
        _ => None,
    }
}