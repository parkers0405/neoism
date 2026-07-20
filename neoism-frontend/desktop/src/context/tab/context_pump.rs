use super::*;

impl<T: EventListener> Context<T> {
    #[inline]
    pub fn active_markdown(&self) -> Option<&MarkdownPane> {
        self.markdown
            .as_ref()
            .or_else(|| self.notebook.as_ref().map(|notebook| &notebook.markdown))
    }

    #[inline]
    pub fn active_markdown_mut(&mut self) -> Option<&mut MarkdownPane> {
        if self.markdown.is_some() {
            return self.markdown.as_mut();
        }
        self.notebook
            .as_mut()
            .map(|notebook| &mut notebook.markdown)
    }

    /// True when this pane mounts a non-terminal surface — a code editor,
    /// markdown preview, `.neodraw` sketch, agent pane, tags pane, or
    /// extensions pane. The Warp-style command composer (and its footer
    /// row reservation / scrollbar accounting) only belongs on a plain
    /// terminal pane, so every composer gate keys off
    /// `!has_non_terminal_surface()`. Mirrors the non-terminal field set
    /// the `Drop` impl uses to decide whether a `kill_pid` is safe — keep
    /// the two in sync when a new surface kind is added.
    #[inline]
    pub fn has_non_terminal_surface(&self) -> bool {
        self.markdown.is_some()
            || self.code.is_some()
            || self.draw.is_some()
            || self.notebook.is_some()
            || self.neoism_agent.is_some()
            || self.neoism_tags.is_some()
            || self.neoism_extensions.is_some()
    }
}
