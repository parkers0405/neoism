//! Inbound edit landing, shared by code and Markdown. Only live render properties change
//! after mounting; the clock never invalidates a row memo or requests an app commit.
use std::time::Instant;

pub const DURATION: f32 = 0.55;

pub struct Flash {
    pub first: usize,
    pub last: usize,
    pub revision: u64,
    pub started: Instant,
    pub opacity: rune::Live<f32>,
    pub offset: rune::Live<[f32; 2]>,
    /// Headless scene samples are held at an exact phase, independent of wall-clock GPU setup.
    fixture_age: Option<f32>,
}

pub fn frame(age: f32) -> (f32, [f32; 2]) {
    let remaining = 1.0 - (age / DURATION).clamp(0.0, 1.0);
    (remaining, [0.0, -6.0 * remaining.powi(3)])
}

impl Flash {
    pub fn new(first: usize, last: usize, revision: u64) -> Self {
        Self { first, last, revision, started: Instant::now(), opacity: rune::Live::new(1.0), offset: rune::Live::new([0.0, -6.0]), fixture_age: None }
    }

    pub fn tick(&self, visible: bool, revision: u64) -> bool {
        let age = self.fixture_age.unwrap_or_else(|| self.started.elapsed().as_secs_f32());
        let active = visible && revision == self.revision && age < DURATION;
        let (alpha, offset) = frame(if active { age } else { DURATION });
        self.opacity.set(alpha);
        self.offset.set(offset);
        active
    }
}

impl crate::app::App {
    pub fn external_edit_tick(&mut self) -> bool {
        let path = self.active_buffer().map(|b| b.path.clone());
        let mut moving = false;
        for buffer in self.buffers.values_mut() {
            if let Some(flash) = &buffer.external_edit {
                if flash.tick(path.as_ref() == Some(&buffer.path), buffer.doc.revision) {
                    moving = true;
                } else {
                    buffer.external_edit = None;
                }
            }
        }
        moving
    }
}

/// Deterministic headless frames of the same live properties, at start/mid/end. No daemon.
pub fn fixture(app: &mut crate::app::App, markdown: bool) {
    let path = if markdown { "README.md" } else { "src/example.rs" };
    let source = if markdown {
        "# Shared document\n\nAgent changes land here.\n\n- A new item\n- Another item\n\nThe reader stays where they were.\n"
    } else {
        "// Shared document\nfn main() {\n    let updated = 42;\n\n    println!(\"remote: {updated}\");\n}\n\n// The reader stays where they were.\n"
    };
    app.open_file(path, None);
    app.apply_opened_file(path, source.as_bytes());
    let age = std::env::var("NEOISM_RUNE_EDIT_AGE").ok().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
    if let Some(buffer) = app.buffers.get_mut(path) {
        let mut flash = Flash::new(2, 5, buffer.doc.revision);
        flash.fixture_age = Some(age);
        let (opacity, offset) = frame(age);
        flash.opacity.set(opacity);
        flash.offset.set(offset);
        buffer.external_edit = Some(flash);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn landing_fades_and_settles_without_overshoot() {
        assert_eq!(frame(0.0), (1.0, [0.0, -6.0]));
        assert_eq!(frame(DURATION / 2.0), (0.5, [0.0, -0.75]));
        assert_eq!(frame(DURATION), (0.0, [0.0, 0.0]));
        assert_eq!(frame(9.0), frame(DURATION));
    }
    #[test]
    fn hidden_changed_and_expired_flashes_stop() {
        let mut f = Flash::new(1, 3, 7);
        assert!(f.tick(true, 7));
        assert!(!f.tick(false, 7));
        assert!(!f.tick(true, 8));
        f.started -= std::time::Duration::from_secs(1);
        assert!(!f.tick(true, 7));
    }

    #[test]
    fn external_edit_only_lands_in_visible_tab_and_never_follows() {
        use crate::crdt::{CrdtTextBuffer, CrdtTextEdit};
        use neoism_protocol::crdt::{CrdtServerMessage as In, CrdtSyncEnvelope};
        let mut app = crate::app::App::new();
        for path in ["a.rs", "note.md"] {
            app.open_file(path, None);
            app.apply_opened_file(path, b"one\ntwo\nthree\n");
        }
        let hub = CrdtTextBuffer::with_text(9_000_000_000, "one\ntwo\nthree\n");
        let b = app.buffers.get_mut("note.md").unwrap();
        b.shared = Some(crate::collab::doc::Binding::new(1024, "test-note".into(), &b.doc));
        app.apply_crdt(0, In::Snapshot { buffer_id: "test-note".into(), update_v1: hub.encode_full_update_v1(), state_vector_v1: hub.state_vector_v1() });
        assert!(app.buffers["note.md"].external_edit.is_none());
        app.editor_follow = false;
        let active = app.active;
        let focus = app.focus;
        let send = |app: &mut crate::app::App, value: &str| {
            let update = hub.apply_local_edit(CrdtTextEdit::Insert { index: 4, content: value.into() }).unwrap();
            app.apply_crdt(0, In::Sync { envelope: CrdtSyncEnvelope { buffer_id: "test-note".into(), origin_client_id: update.origin_client_id, update_v1: update.update_v1, state_vector_v1: update.state_vector_v1 } });
        };
        send(&mut app, "remote ");
        assert!(app.buffers["note.md"].external_edit.is_some());
        assert!(app.external_edit_tick());
        let revision = app.buffers["note.md"].external_edit.as_ref().unwrap().revision;
        send(&mut app, "next ");
        assert!(app.buffers["note.md"].external_edit.as_ref().unwrap().revision > revision);
        assert_eq!(app.active, active);
        assert_eq!(app.focus, focus);
        assert!(!app.editor_follow);
        app.open_file("a.rs", None);
        assert!(!app.external_edit_tick());
        assert!(app.buffers["note.md"].external_edit.is_none());
        send(&mut app, "hidden ");
        assert!(app.buffers["note.md"].external_edit.is_none());
        app.open_file("note.md", None);
        assert!(!app.external_edit_tick());
        assert!(app.buffers["note.md"].external_edit.is_none());
    }
}