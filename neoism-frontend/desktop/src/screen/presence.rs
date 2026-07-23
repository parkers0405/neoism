//! Wave 7A — desktop side of the multiplayer presence plane.
//!
//! Inbound: `Application::pump_daemon` fans every daemon `CrdtReply`
//! into [`Screen::apply_presence_crdt_message`], which folds presence
//! pushes into the per-window [`RemotePresenceStore`].
//!
//! Outbound: `pump_daemon` also drains
//! [`Screen::drain_daemon_presence_messages`] each pass; the
//! [`PresencePublisher`] inside coalesces the local markdown cursor to
//! ≤~13Hz, emits keep-alive heartbeats under the daemon's ~10s TTL,
//! and clears presence when the pane loses focus or switches files.
//!
//! Renderer contract — THE accessor: when drawing a markdown pane for
//! `path`, call [`Screen::remote_cursors_for_path`]. Notebook panes use
//! the virtual `notebook-render://<path>` buffer from `markdown_crdt`
//! instead, because their collaborative text is rendered markdown while
//! the file on disk remains JSON. The renderer-side bridge chooses the
//! correct buffer id once per frame and draws one caret + optional
//! selection per returned `PeerPresence` (`cursor.line` / `cursor.column`
//! are zero-based; `color` is the peer's stable color; `display_name` is
//! the label).

use neoism_protocol::crdt::{CrdtClientMessage, CrdtServerMessage};
use neoism_ui::editor::crdt::{
    presence_buffer_id_for_path, PeerCursor, PeerPresence, PresencePublisher,
};

use super::{markdown_crdt::buffer_id_for_notebook_render_path, Screen};

fn presence_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// Stable local identity for the presence plane: `peer_id` is
/// `user@host` (stable per device, so the stable-hashed cursor color
/// survives restarts). The display name other peers see resolves
/// `NEOISM_DISPLAY_NAME` → the `[neoism] display-name` config key →
/// the hostname (Wave 7G); overriding the name never touches the
/// peer id, so the peer's color stays stable.
pub(crate) fn local_presence_identity(
    config_display_name: Option<&str>,
) -> (String, String) {
    let host = gethostname::gethostname().to_string_lossy().into_owned();
    let host = if host.trim().is_empty() {
        "neoism".to_string()
    } else {
        host
    };
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "user".to_string());
    let env_override = std::env::var("NEOISM_DISPLAY_NAME").ok();
    let display_name = neoism_ui::editor::crdt::resolve_presence_display_name(
        env_override.as_deref(),
        config_display_name,
        &host,
    );
    (format!("{user}@{host}"), display_name)
}

impl Screen<'_> {
    /// Fold one daemon CRDT push into the remote-presence store.
    /// Returns `true` when remote presence changed (redraw is due).
    pub fn apply_presence_crdt_message(&mut self, message: &CrdtServerMessage) -> bool {
        let changed = self.remote_presence.apply_server_message(message);
        if changed {
            self.mark_dirty();
        }
        changed
    }

    /// THE renderer accessor: remote peer cursors for the file at
    /// `path`, already excluding the local peer. For notebook panes, use
    /// `buffer_id_for_notebook_render_path` and query the store directly.
    /// Cheap per-frame read — borrows the store, allocates nothing.
    #[allow(dead_code)]
    pub fn remote_cursors_for_path<'a>(
        &'a self,
        path: &std::path::Path,
    ) -> impl Iterator<Item = &'a PeerPresence> + 'a {
        let buffer_id = presence_buffer_id_for_path(path);
        self.remote_presence.cursors_for(&buffer_id)
    }

    /// Outbound presence pump: coalesce the focused markdown pane's
    /// cursor into at most a couple of `CrdtClientMessage`s. Called by
    /// the app's daemon pump; returns an empty vec almost always.
    pub fn drain_daemon_presence_messages(&mut self) -> Vec<CrdtClientMessage> {
        let now_ms = presence_now_ms();
        let current = self.context_manager.current();
        let active = current
            .markdown
            .as_ref()
            .map(|markdown| {
                (
                    presence_buffer_id_for_path(&markdown.path),
                    PeerCursor::new(
                        markdown.cursor_line as u32,
                        markdown.cursor_col as u32,
                    ),
                    markdown.mode == neoism_ui::editor::markdown::MarkdownMode::Insert,
                )
            })
            .or_else(|| {
                current.notebook.as_ref().map(|notebook| {
                    (
                        buffer_id_for_notebook_render_path(&notebook.path),
                        PeerCursor::new(
                            notebook.markdown.cursor_line as u32,
                            notebook.markdown.cursor_col as u32,
                        ),
                        notebook.markdown.mode
                            == neoism_ui::editor::markdown::MarkdownMode::Insert,
                    )
                })
            })
            .or_else(|| {
                current.code.as_ref().map(|code| {
                    // The wire column is UTF-16 (CRDT offset policy);
                    // the code buffer's cursor_col is a byte column —
                    // convert against the live line text.
                    let col_utf16 = code
                        .buffer
                        .lines
                        .get(code.buffer.cursor_line)
                        .map(|line| {
                            let col = code.buffer.cursor_col.min(line.len());
                            line.get(..col)
                                .map(|prefix| prefix.encode_utf16().count())
                                .unwrap_or(col)
                        })
                        .unwrap_or(0);
                    (
                        presence_buffer_id_for_path(&code.path),
                        PeerCursor::new(
                            code.buffer.cursor_line as u32,
                            col_utf16 as u32,
                        ),
                        code.buffer.mode == neoism_ui::editor::code::CodeMode::Insert,
                    )
                })
            });

        if self.presence_publisher.is_none() {
            let (peer_id, display_name) =
                local_presence_identity(self.presence_display_name_override.as_deref());
            self.remote_presence.set_local_peer_id(peer_id.clone());
            self.presence_publisher = Some(PresencePublisher::new(peer_id, display_name));
        }
        let publisher = self
            .presence_publisher
            .as_mut()
            .expect("publisher initialized above");
        // Broadcast the color my cursor ACTUALLY wears — the picked
        // cursor-color override when set, else the theme accent. The
        // rainbow preset rides as a flag so peers animate it locally
        // (heartbeats are far too slow to stream the animation).
        let cursor = self.renderer.named_colors.cursor;
        publisher.set_color(neoism_ui::editor::crdt::PresenceColor {
            r: (cursor[0].clamp(0.0, 1.0) * 255.0) as u8,
            g: (cursor[1].clamp(0.0, 1.0) * 255.0) as u8,
            b: (cursor[2].clamp(0.0, 1.0) * 255.0) as u8,
        });
        publisher.set_rainbow(self.renderer.cursor_is_animated());
        publisher.tick(
            active.as_ref().map(|(buffer_id, cursor, insert)| {
                (buffer_id.as_str(), *cursor, None, *insert)
            }),
            now_ms,
        )
    }
}
