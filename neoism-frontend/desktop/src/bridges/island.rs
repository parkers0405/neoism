//! Bridge between the desktop's `ContextManager<EventProxy>` and the
//! shared `neoism_ui::widgets::island::IslandContexts` trait.
//!
//! The Island widget used to take a `&ContextManager<EventProxy>`
//! directly when it lived in the native frontend. After the cutover the
//! widget lives in the shared `neoism-ui` crate and reads its tab strip
//! through a POD trait (`IslandContexts`) so wasm + web hosts can drive
//! it without pulling in `neoism_backend`. This file is the desktop
//! adapter: a thin impl on `ContextManager<EventProxy>` that delegates
//! to the same accessors `tabs.rs` used to call (`len`, `current_index`,
//! `titles.titles`).

use neoism_backend::event::EventProxy;
use neoism_ui::widgets::island::{IslandContexts, IslandTabTitle};

use crate::context::ContextManager;

impl IslandContexts for ContextManager<EventProxy> {
    #[inline]
    fn len(&self) -> usize {
        ContextManager::len(self)
    }

    #[inline]
    fn current_index(&self) -> usize {
        ContextManager::current_index(self)
    }

    fn title(&self, index: usize) -> Option<IslandTabTitle> {
        let entry = self.titles.titles.get(&index);
        Some(IslandTabTitle {
            content: entry
                .map(|entry| entry.content.clone())
                .unwrap_or_else(|| "~".to_string()),
            program: entry.and_then(|entry| {
                entry.extra.as_ref().map(|extra| extra.program.clone())
            }),
            // Workspace identity must not disappear while its asynchronous
            // terminal title is absent during a tab switch.
            icon_kind: self.workspace_icon_kind_for_index(index),
        })
    }
}
