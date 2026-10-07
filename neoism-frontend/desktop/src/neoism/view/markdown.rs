pub(crate) use neoism_ui::panels::agent_pane::view::markdown::AssistantMarkdownBlock;

use crate::neoism::agent::NeoismAgentPane;
use neoism_ui::panels::agent_pane::selection_model::SelectableCaretStop;
use neoism_ui::panels::agent_pane::view::markdown::AgentMarkdownPane;

impl AgentMarkdownPane for NeoismAgentPane {
    fn retain_reveal_markdown_layout(
        &self,
        blocks: &[AssistantMarkdownBlock],
    ) -> Option<std::rc::Rc<Vec<AssistantMarkdownBlock>>> {
        NeoismAgentPane::retain_reveal_markdown_layout(self, blocks)
    }
    fn text_reveal_state(
        &mut self,
    ) -> Option<&mut neoism_ui::panels::agent_pane::text_reveal::TextRevealState> {
        Some(&mut self.text_reveal)
    }

    #[cfg(feature = "servo-artifacts")]
    fn cached_html_artifact_frame(
        &self,
        key: &str,
    ) -> Option<neoism_ui::panels::agent_pane::view::markdown::HtmlArtifactFrame> {
        if std::env::var("NEOISM_SERVO_ARTIFACTS").as_deref() != Ok("1") {
            return None;
        }
        let session = self.session_id_str().unwrap_or("draft");
        let source_key = format!("{}:{}:{key}", session.len(), session);
        self.html_artifact_frames.get(&source_key).copied()
    }
    #[cfg(feature = "servo-artifacts")]
    fn html_artifact_frame(
        &mut self,
        request: neoism_ui::panels::agent_pane::view::markdown::HtmlArtifactRequest,
    ) -> Option<neoism_ui::panels::agent_pane::view::markdown::HtmlArtifactFrame> {
        // Never instantiate an unsandboxed engine just because a model emitted HTML.
        if std::env::var("NEOISM_SERVO_ARTIFACTS").as_deref() != Ok("1") {
            return None;
        }
        if self.html_artifact_requests.len() >= 8 {
            return None;
        }
        let session = self.session_id_str().unwrap_or("draft");
        let source_key = format!("{}:{}:{}", session.len(), session, request.key);
        let frame = self.html_artifact_frames.get(&source_key).copied();
        self.html_artifact_requests.push(request);
        frame
    }

    fn cached_markdown_blocks_for(
        &self,
        text: &str,
        width: f32,
        scale: f32,
    ) -> Option<std::rc::Rc<Vec<AssistantMarkdownBlock>>> {
        let key = NeoismAgentPane::markdown_blocks_key(text, width, scale);
        self.cached_markdown_blocks(&key)
    }

    fn store_markdown_blocks_for(
        &self,
        text: &str,
        width: f32,
        scale: f32,
        blocks: std::rc::Rc<Vec<AssistantMarkdownBlock>>,
    ) {
        let key = NeoismAgentPane::markdown_blocks_key(text, width, scale);
        self.store_markdown_blocks(key, blocks);
    }

    fn register_selectable_line(&mut self, text: &str, rect: [f32; 4]) -> usize {
        NeoismAgentPane::register_selectable_line(self, text, rect)
    }

    fn register_selectable_line_with_caret_stops(
        &mut self,
        text: &str,
        rect: [f32; 4],
        caret_stops: &[SelectableCaretStop],
    ) -> usize {
        NeoismAgentPane::register_selectable_line_with_caret_stops(
            self,
            text,
            rect,
            caret_stops,
        )
    }

    fn selectable_line_highlight(&self, index: usize) -> Option<(f32, f32)> {
        NeoismAgentPane::selectable_line_highlight(self, index)
    }

    fn register_link_hit_rect(&mut self, target: String, rect: [f32; 4]) {
        NeoismAgentPane::register_link_hit_rect(self, target, rect);
    }

    fn link_hovered(&self, target: &str) -> bool {
        NeoismAgentPane::link_hovered(self, target)
    }

    fn mermaid_raw_mode(&self, key: u64) -> bool {
        NeoismAgentPane::mermaid_raw_mode(self, key)
    }

    fn markdown_horizontal_scroll_offset(&mut self, key: &str, max_scroll: f32) -> f32 {
        NeoismAgentPane::markdown_horizontal_scroll_offset(self, key, max_scroll)
    }

    fn register_markdown_horizontal_scroll_rect(
        &mut self,
        key: String,
        rect: [f32; 4],
        max_scroll: f32,
    ) {
        NeoismAgentPane::register_markdown_horizontal_scroll_rect(
            self, key, rect, max_scroll,
        );
    }

    fn register_markdown_horizontal_scrollbar(
        &mut self,
        key: String,
        track: [f32; 4],
        thumb: [f32; 4],
        max_scroll: f32,
    ) {
        NeoismAgentPane::register_markdown_horizontal_scrollbar(
            self, key, track, thumb, max_scroll,
        );
    }

    fn markdown_horizontal_scrollbar_visible(&self, key: &str) -> bool {
        NeoismAgentPane::markdown_horizontal_scrollbar_visible(self, key)
    }

    fn code_copy_feedback_progress(&self, target: &str) -> Option<f32> {
        NeoismAgentPane::code_copy_feedback_progress(self, target)
    }

    fn suppress_markdown_interactions(&self) -> bool {
        NeoismAgentPane::suppress_markdown_interactions(self)
    }
}
