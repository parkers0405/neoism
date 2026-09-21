// MenuKeyAction, with_icon, disabled, handle_key etc. are shared
// scaffolding — context_menu uses a subset today; the next migration
// (modal action list, palette) will consume the rest. Quiet the
// unused-API warnings until then.
#![allow(dead_code)]

// Generic vertical menu — selection state, arrow / shortcut handling,
// scrolling. The action payload is a generic type `A` so callers keep
// using their own enum (ContextMenuAction, PaletteAction, etc.) without
// boxing closures.
//
// Like the other widgets here, Menu doesn't paint; the consumer reads
// the items + selected index and renders them in whatever house style
// the panel uses (rounded row, accent strip, icon glyph). Menu only
// owns the bookkeeping.

use crate::widgets::overlay::{Key, NamedKey};

#[derive(Clone, Debug)]
pub struct MenuItem<A> {
    pub label: String,
    /// Optional icon glyph (codicon / nerdfont). The renderer decides
    /// whether to draw it.
    pub icon: Option<String>,
    /// Optional shortcut hint (e.g. "⌘P", "Esc"). Type-to-search
    /// matches the first character of the hint ASCII-insensitively, so
    /// "y"/"n" prompts work out of the box.
    pub shortcut: Option<String>,
    pub enabled: bool,
    pub action: A,
}

impl<A> MenuItem<A> {
    pub fn new(label: impl Into<String>, action: A) -> Self {
        Self {
            label: label.into(),
            icon: None,
            shortcut: None,
            enabled: true,
            action,
        }
    }

    pub fn with_icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    pub fn with_shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }
}

/// What the menu did with a key. The caller routes the result back to
/// the host: `Activated(action)` runs the action and closes the menu;
/// `Moved` triggers a redraw; `Ignored` falls through to the next
/// handler (so e.g. typing a printable letter that isn't a shortcut
/// can be handed to a search field).
pub enum MenuKeyAction<'a, A> {
    Activated(&'a A),
    Moved,
    Ignored,
}

#[derive(Clone, Debug)]
pub struct Menu<A> {
    items: Vec<MenuItem<A>>,
    selected: usize,
    scroll_offset: usize,
    max_visible: usize,
}

impl<A> Menu<A> {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            selected: 0,
            scroll_offset: 0,
            // Sensible default; consumers can override per-panel.
            max_visible: 10,
        }
    }

    pub fn with_max_visible(mut self, max: usize) -> Self {
        self.max_visible = max.max(1);
        self
    }

    pub fn set_max_visible(&mut self, max: usize) {
        let max = max.max(1);
        if self.max_visible == max {
            return;
        }
        self.max_visible = max;
        self.ensure_selection_visible();
    }

    pub fn max_visible(&self) -> usize {
        self.max_visible
    }

    pub fn items(&self) -> &[MenuItem<A>] {
        &self.items
    }

    pub fn set_items(&mut self, items: Vec<MenuItem<A>>) {
        self.items = items;
        self.selected = self.items.iter().position(|item| item.enabled).unwrap_or(0);
        self.scroll_offset = 0;
        self.ensure_selection_visible();
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.selected = 0;
        self.scroll_offset = 0;
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    pub fn selected_item(&self) -> Option<&MenuItem<A>> {
        self.items.get(self.selected).filter(|item| item.enabled)
    }

    pub fn selected_action(&self) -> Option<&A> {
        self.selected_item().map(|item| &item.action)
    }

    /// Visible row count, capped by `max_visible` and the total number
    /// of items.
    pub fn visible_count(&self) -> usize {
        self.items.len().min(self.max_visible.max(1))
    }

    /// Move selection by `delta`, skipping disabled rows. Wraps around
    /// when leaving the ends of the list.
    pub fn move_selection(&mut self, delta: i32) {
        if self.items.is_empty() {
            return;
        }
        let len = self.items.len() as i32;
        let mut next = self.selected as i32;
        for _ in 0..self.items.len() {
            next = (next + delta).rem_euclid(len);
            if self.items[next as usize].enabled {
                self.selected = next as usize;
                self.ensure_selection_visible();
                return;
            }
        }
    }

    pub fn set_hovered_index(&mut self, index: usize) -> bool {
        if self.items.get(index).is_some_and(|item| item.enabled) {
            self.selected = index;
            true
        } else {
            false
        }
    }

    pub fn set_selected_index(&mut self, idx: usize) -> bool {
        if idx < self.items.len() && self.items[idx].enabled {
            self.selected = idx;
            self.ensure_selection_visible();
            true
        } else {
            false
        }
    }

    /// Match a printable char against the first character of each
    /// item's shortcut hint. Returns the matched action when a row
    /// becomes selected.
    pub fn match_shortcut(&mut self, ch: char) -> Option<&A> {
        let ch = ch.to_ascii_lowercase();
        let index = self.items.iter().position(|item| {
            item.enabled
                && item.shortcut.as_deref().is_some_and(|hint| {
                    hint.chars()
                        .next()
                        .is_some_and(|h| h.to_ascii_lowercase() == ch)
                })
        })?;
        self.selected = index;
        self.ensure_selection_visible();
        self.items.get(self.selected).map(|item| &item.action)
    }

    pub fn handle_key(&mut self, key: &Key) -> MenuKeyAction<'_, A> {
        match key {
            Key::Named(NamedKey::ArrowDown) => {
                self.move_selection(1);
                MenuKeyAction::Moved
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.move_selection(-1);
                MenuKeyAction::Moved
            }
            Key::Named(NamedKey::PageDown) => {
                self.move_selection(5);
                MenuKeyAction::Moved
            }
            Key::Named(NamedKey::PageUp) => {
                self.move_selection(-5);
                MenuKeyAction::Moved
            }
            Key::Named(NamedKey::Tab) => {
                self.move_selection(1);
                MenuKeyAction::Moved
            }
            Key::Named(NamedKey::Enter) => {
                if let Some(item) = self.items.get(self.selected) {
                    if item.enabled {
                        return MenuKeyAction::Activated(&item.action);
                    }
                }
                MenuKeyAction::Ignored
            }
            _ => MenuKeyAction::Ignored,
        }
    }

    /// Pixel-based wheel scroll. `delta_pixels` positive = scroll down.
    /// `row_h` is the rendered row height (caller knows it).
    pub fn scroll_pixels(
        &mut self,
        delta_pixels: f32,
        row_h: f32,
        wheel_accumulator: &mut f32,
    ) {
        let visible = self.visible_count().max(1);
        if self.items.len() <= visible || delta_pixels == 0.0 || row_h <= 0.0 {
            return;
        }
        *wheel_accumulator += delta_pixels;
        let mut rows = 0i32;
        while wheel_accumulator.abs() >= row_h {
            let sign = wheel_accumulator.signum();
            *wheel_accumulator -= sign * row_h;
            rows += if sign > 0.0 { -1 } else { 1 };
        }
        if rows == 0 {
            return;
        }
        let max_offset = self.items.len().saturating_sub(visible);
        self.scroll_offset = if rows < 0 {
            self.scroll_offset
                .saturating_sub(rows.unsigned_abs() as usize)
        } else {
            self.scroll_offset
                .saturating_add(rows as usize)
                .min(max_offset)
        };
        let end = (self.scroll_offset + visible).min(self.items.len());
        if self.selected < self.scroll_offset || self.selected >= end {
            let rows = &self.items[self.scroll_offset..end];
            let candidate = if self.selected < self.scroll_offset {
                rows.iter().position(|item| item.enabled)
            } else {
                rows.iter().rposition(|item| item.enabled)
            };
            self.selected = self.scroll_offset + candidate.unwrap_or(0);
        }
    }

    /// Iterate the slice of items currently in view (post-scroll).
    pub fn visible_items(&self) -> impl Iterator<Item = (usize, &MenuItem<A>)> {
        let start = self.scroll_offset;
        let count = self.visible_count();
        self.items.iter().enumerate().skip(start).take(count)
    }

    fn ensure_selection_visible(&mut self) {
        if self.items.is_empty() {
            self.scroll_offset = 0;
            return;
        }
        let visible = self.visible_count().max(1);
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + visible {
            self.scroll_offset = self.selected + 1 - visible;
        }
        let max_offset = self.items.len().saturating_sub(visible);
        self.scroll_offset = self.scroll_offset.min(max_offset);
    }
}

impl<A> Default for Menu<A> {
    fn default() -> Self {
        Self::new()
    }
}
