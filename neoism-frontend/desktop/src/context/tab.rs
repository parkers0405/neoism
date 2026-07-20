use crate::app::ime::Ime;
use crate::app::messenger::Messenger;
use crate::context::renderable::{Cursor, RenderableContent};
use crate::context::splash::SplashInjection;
use crate::editor::markdown::MarkdownPane;
use crate::editor::neodraw::DrawPane;
use crate::editor::notebook::NotebookPane;
use crate::event::sync::FairMutex;
use crate::event::Msg;
use crate::layout::ContextDimension;
use crate::neoism::agent::NeoismAgentPane;
use crate::performer::{self, Machine};
use crate::workspace::extensions::NeoismExtensionsPane;
use crate::workspace::tags_view::NeoismTagsPane;
use neoism_backend::event::EventListener;
use neoism_terminal_core::crosswords::Crosswords;
use neoism_terminal_core::selection::SelectionRange;
use neoism_ui::editor::code::CodePane;
use std::sync::Arc;
use std::thread::JoinHandle;

pub struct Context<T: EventListener> {
    pub route_id: usize,
    pub terminal: Arc<FairMutex<Crosswords>>,
    pub terminal_input: crate::terminal::blocks::TerminalInputBuffer,
    pub terminal_shell_kind: crate::terminal::blocks::TerminalShellKind,
    pub renderable_content: RenderableContent,
    pub messenger: Messenger,
    #[cfg(not(target_os = "windows"))]
    pub main_fd: Arc<i32>,
    #[cfg(not(target_os = "windows"))]
    pub shell_pid: u32,
    pub rich_text_id: usize,
    pub dimension: ContextDimension,
    pub pending_terminal_resize: bool,
    /// True until the NEOISM splash banner has been written into this
    /// pane's scrollback. Deferred to first render so we know the
    /// real terminal width and can horizontally center the wordmark
    /// — the dimension at `create_context` time can be smaller than
    /// the eventual rendered width once layout settles. Non-terminal
    /// panes are born with this `false` (no PTY scrollback to write
    /// to).
    pub pending_splash: bool,
    /// Counter of consecutive frames the pane's `(cols, rows)` has
    /// been stable. Used by the splash injector to wait for layout
    /// to settle before computing the centering pad — the
    /// dimension at the very first frame can lag the eventual
    /// rendered size.
    pub splash_dim_stable_frames: u8,
    /// `(cols, rows)` last seen by the splash injector. When this
    /// changes between frames the stable counter resets.
    pub splash_last_dim: (usize, usize),
    /// Cursor row observed on the previous render frame. Used by
    /// the splash dismiss trigger — when the live cursor row
    /// moves *down* between frames (`current > last`), the user
    /// has submitted a command (shell echoes `\r\n` on Enter)
    /// and we kick off the fade animation. Reset to the live
    /// cursor row whenever `history_size == 0`, so a `clear`
    /// that brings the splash back also resets the comparison.
    pub splash_last_cursor_row: i32,
    /// Pane geometry the splash was actually injected at — origin
    /// row in the live grid, column count, and the cell width/
    /// height in pixels at the moment of injection. The GPU
    /// overlay reads this so it can paint its pulse / ripple over
    /// the wordmark cells regardless of subsequent scroll state.
    pub splash_injection: Option<SplashInjection>,
    pub ime: Ime,
    /// Present when this pane renders a DAEMON-hosted shell (8A "one
    /// shell, many screens"): the feed pushes daemon `PtyOutput`
    /// frames into the machine's byte channel, and the shared slot
    /// carries the session id the pane's input sink resolves against.
    /// `None` for conventional local-PTY panes.
    pub remote_pty: Option<crate::context::remote_pty::RemotePtyBinding>,
    pub(super) _io_thread: Option<JoinHandle<(Machine<T>, performer::State)>>,
    /// Rust-rendered markdown document surface. Mutually exclusive with
    /// real PTY content for workspace markdown tabs.
    pub markdown: Option<MarkdownPane>,
    /// Native code editor surface (the nvim replacement). Mutually
    /// exclusive with `markdown`/PTY content, like the other Rust panes.
    pub code: Option<CodePane>,
    /// Rust-rendered `.neodraw` sketch surface. Mutually exclusive with
    /// terminal/markdown content, like the other Rust panes.
    pub draw: Option<DrawPane>,
    /// Rust-rendered `.ipynb` notebook surface. Owns notebook JSON while
    /// reusing the virtualized markdown renderer for cell presentation.
    pub notebook: Option<NotebookPane>,
    /// Rust-rendered Neoism agent chat surface. Mutually exclusive with
    /// terminal/markdown content.
    pub neoism_agent: Option<NeoismAgentPane>,
    /// Rust-rendered Neoism workspace tags surface. Mutually exclusive
    /// with terminal/markdown/agent content.
    pub neoism_tags: Option<NeoismTagsPane>,
    /// Rust-rendered Extensions browser surface. Mutually exclusive
    /// with terminal/markdown/agent/tags content.
    pub neoism_extensions: Option<NeoismExtensionsPane>,
}

impl<T: neoism_backend::event::EventListener> Drop for Context<T> {
    fn drop(&mut self) {
        // Shutdown the terminal's PTY.
        let _ = self.messenger.channel.send(Msg::Shutdown);

        // Non-terminal panes have no PTY and use a sentinel `shell_pid`;
        // killing a sentinel PID would target an unrelated process.
        // Daemon-backed panes (8A) report `shell_pid == 0` — `kill(0,
        // SIGHUP)` would HUP our OWN process group; their shell is owned
        // by the daemon and torn down via the `ClosePty` the machine's
        // shutdown emits.
        #[cfg(not(target_os = "windows"))]
        if self.remote_pty.is_none()
            && self.markdown.is_none()
            && self.code.is_none()
            && self.draw.is_none()
            && self.notebook.is_none()
            && self.neoism_agent.is_none()
            && self.neoism_tags.is_none()
            && self.neoism_extensions.is_none()
        {
            neoism_terminal_pty::kill_pid(self.shell_pid as i32);
        }
    }
}

mod context_pump;

impl<T: EventListener> Context<T> {
    #[inline]
    pub fn set_selection(&mut self, selection_range: Option<SelectionRange>) {
        let old_selection = self.renderable_content.selection_range;
        let has_updated = old_selection != selection_range;

        if has_updated {
            // Selection affects terminal line rendering, so use terminal damage
            self.renderable_content
                .pending_update
                .set_terminal_damage(neoism_terminal_core::damage::TerminalDamage::Full);
        }

        self.renderable_content.selection_range = selection_range;
    }

    #[inline]
    pub fn set_hyperlink_range(&mut self, hyperlink_range: Option<SelectionRange>) {
        let old_hyperlink = self.renderable_content.hyperlink_range;

        if old_hyperlink != hyperlink_range {
            // Hyperlinks affect terminal line rendering, so use terminal damage
            self.renderable_content
                .pending_update
                .set_terminal_damage(neoism_terminal_core::damage::TerminalDamage::Full);
        }

        self.renderable_content.hyperlink_range = hyperlink_range;
    }

    #[inline]
    pub fn has_hyperlink_range(&self) -> bool {
        self.renderable_content.hyperlink_range.is_some()
    }

    #[inline]
    pub fn cursor_from_ref(&self) -> Cursor {
        Cursor {
            state: self.renderable_content.cursor.state.new_from_self(),
            content: self.renderable_content.cursor.content_ref,
            content_ref: self.renderable_content.cursor.content_ref,
            is_ime_enabled: false,
        }
    }

    /// Re-home this context's live terminal PTY parser driver onto
    /// `window_id` after a workspace detach (via the messenger control
    /// channel). The session keeps running — only the host-window tag on
    /// emitted events changes.
    pub fn rebind_window(&self, window_id: neoism_backend::event::WindowId) {
        self.messenger.send_rebind_window(window_id);
    }
}
