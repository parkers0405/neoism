use sugarloaf::Sugarloaf;

use crate::panels::agent_pane::session_group::SESSION_PICKER_FOOTER;
use crate::panels::agent_pane::state::picker::{
    NeoismAgentPicker, NeoismAgentPickerKind,
};
use crate::panels::agent_pane::state::NeoismAgentPane;

use crate::primitives::ide_theme::IdeTheme;
use crate::widgets::inline_picker::{InlinePickerRow, InlinePickerView};

pub trait AgentPickerPane {
    fn picker_mut(&mut self) -> Option<&mut NeoismAgentPicker>;

    /// In-progress inline rename buffer for the `/sessions` picker, if the
    /// host is currently renaming the selected session. Defaults to `None`;
    /// only the desktop wires rename today.
    fn picker_rename_buffer(&self) -> Option<String> {
        None
    }
}

impl AgentPickerPane for NeoismAgentPane {
    fn picker_mut(&mut self) -> Option<&mut NeoismAgentPicker> {
        self.picker_mut()
    }
}

pub fn render_picker(
    sugarloaf: &mut Sugarloaf,
    pane: &mut impl AgentPickerPane,
    input_rect: [f32; 4],
    theme: &IdeTheme,
    s: f32,
) {
    let rename = pane.picker_rename_buffer();
    let Some(picker) = pane.picker_mut() else {
        return;
    };
    let is_session = picker.kind == NeoismAgentPickerKind::Session;
    let footer_hint = is_session.then_some(SESSION_PICKER_FOOTER);
    // Rename only applies while a session picker is open.
    let rename = rename.filter(|_| is_session);
    // Slash / @file / skill-mention pickers type into the composer (which
    // owns the caret), so suppress the search-row caret for them.
    let show_search_caret = !matches!(
        picker.kind,
        NeoismAgentPickerKind::Slash
            | NeoismAgentPickerKind::FileMention
            | NeoismAgentPickerKind::SkillMention
    );
    let list_scroll_offset = picker.tick_list_scroll();
    let cursor_offset = picker.tick_cursor();
    let rows = picker
        .options()
        .iter()
        .map(|option| InlinePickerRow {
            title: &option.title,
            description: &option.description,
            footer: &option.footer,
            is_header: option.is_header,
            is_current: option.is_current,
            is_pinned: option.pinned,
        })
        .collect::<Vec<_>>();
    if let Some(render_state) = crate::widgets::inline_picker::render(
        sugarloaf,
        InlinePickerView {
            title: &picker.title,
            query: &picker.query,
            selected: picker.selected,
            scroll_offset: picker.scroll_offset,
            list_scroll_offset,
            cursor_offset,
            rows: &rows,
            footer_hint,
            rename: rename.as_deref(),
            show_search_caret,
        },
        input_rect,
        theme,
        s,
    ) {
        picker.set_last_rect(render_state.rect);
        picker.set_footer_h(render_state.footer_h);
        // cursor rect is intentionally NOT updated here — the caret stays
        // in the input text area while the picker dropdown is visible.
    }
}
