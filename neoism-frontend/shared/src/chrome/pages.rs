//! Chrome helper-page hosting — Settings / Extensions / NeoWorld /
//! About on the web chrome.
//!
//! Desktop hosts these surfaces natively: Extensions and NeoWorld are
//! `ChromePageKind` buffer tabs whose panes live on the context grid,
//! Settings is the full-screen `renderer.settings` overlay, and About
//! is a `UniversalModal`. This module gives `Chrome` the same four
//! hosts so the shared panels become reachable from wasm:
//!
//! - `settings_page` — full-screen overlay, painted last (late
//!   overlay pass) and owning all input while active, mirroring
//!   `desktop/src/router/route.rs`'s settings arm.
//! - `modal` — the About dialog (and any future chrome-owned modal),
//!   also late-overlay + input-owning, mirroring the desktop modal
//!   arm.
//! - `extensions_page` / `neoworld_pane` — bodies for the
//!   `ChromePageKind::{Extensions, NeoWorld}` buffer tabs; they paint
//!   in the terminal rect when their tab is active and receive
//!   pointer/key/wheel input scoped to that rect (tab strip, top bar
//!   and status line keep their own hits so the user can switch away).
//!
//! Everything host-actionable (persist a setting, open a repository
//! URL, save a pet snapshot) is queued on `pending_*` vecs the host
//! bridge drains once per frame.

use super::*;

use crate::event::{
    KeyDescriptor, KeyState, LogicalKey, Modifiers, NamedKey, PointerButton, UiEvent,
    WheelMode,
};
use crate::layout::Rect;
use crate::panels::buffer_tabs::{BufferTabTarget, ChromePageKind};
use crate::panels::extensions_page::PaneAction;
use crate::panels::neoworld::NeoWorldPane;
use crate::panels::notifications::NotificationLevel;
use crate::panels::settings_page::SettingsAction;
use crate::widgets::modal::{
    ModalAction, ModalButton, ModalFormField, ModalFormSpec, ModalHostAction,
    ModalInputSpec, ModalSpec,
};

use sugarloaf::Sugarloaf;

impl<A: Send + Copy + 'static> Chrome<A> {
    pub fn open_file_browser(
        &mut self,
        mode: crate::panels::file_browser::FileBrowserMode,
        start: &str,
        recents: Vec<String>,
    ) {
        self.hide_focus_modals();
        self.file_browser.open(mode, start, recents);
    }

    pub fn file_browser_overlay_active(&self) -> bool {
        self.file_browser.is_active()
    }

    // ── Queries ────────────────────────────────────────────────────

    /// Which chrome helper page the ACTIVE buffer tab points at, if
    /// any (`None` for terminal / file / agent tabs).
    pub fn active_chrome_page(&self) -> Option<ChromePageKind> {
        match self.buffer_tabs.target_at(self.active_tab_index) {
            Some(BufferTabTarget::ChromePage(page)) => Some(page.kind),
            _ => None,
        }
    }

    /// True while a full-screen chrome overlay (settings page or the
    /// chrome-owned modal) covers the window and owns input.
    /// `is_terminal_tab_active` consults this so the host stops
    /// painting terminal cells underneath, the same way desktop's
    /// terminal compose gates on `settings.is_active()`.
    pub fn chrome_overlay_active(&self) -> bool {
        self.settings_page.is_active() || self.modal.is_active()
    }

    /// True when the chrome-page layer wants raw keyboard input routed
    /// into `Chrome::handle_event` (web `keyboard_capture_active`).
    pub fn chrome_page_wants_keyboard(&self) -> bool {
        self.chrome_overlay_active()
            || self.file_browser.is_active()
            || self.active_chrome_page().is_some()
    }

    /// A generic overlay/modal owns keyboard input independently of the
    /// active content surface.  Keep this predicate centralized: terminal
    /// composer layout, paint, prompt-row removal and cursor ownership must
    /// all make the same decision.
    pub fn generic_keyboard_overlay_active(&self) -> bool {
        self.command_palette.is_enabled()
            || self.finder.is_enabled()
            || self.git_diff.is_visible()
            || self.context_menu.is_visible()
            || self.share_sheet.is_visible()
            || self.chrome_page_wants_keyboard()
    }

    /// Effective terminal composer ownership. `CommandComposer::is_visible`
    /// is the configured/shell-state request; overlays temporarily suppress
    /// it without destroying that request, so closing an overlay restores the
    /// composer on the next layout automatically.
    pub fn terminal_composer_eligible(&self) -> bool {
        self.is_terminal_tab_active()
            && !self.is_neoism_agent_tab_active()
            && self.command_composer.is_visible()
            && !self.generic_keyboard_overlay_active()
    }

    /// Animation pump: the NeoWorld sim runs continuously while its
    /// page is visible; the modal's busy bar animates while shown.
    /// (The extensions pane's scroll glide feeds
    /// `editor_pane_animating` from the draw pass instead.)
    pub(crate) fn chrome_pages_animating(&self) -> bool {
        self.modal.needs_redraw()
            || (self.active_chrome_page() == Some(ChromePageKind::NeoWorld)
                && self.neoworld_pane.is_some())
    }

    // ── Open / close ───────────────────────────────────────────────

    /// Open the full-screen Settings overlay seeded with the config
    /// document (`config.json` as one JSON value — the web host
    /// fetches it from the daemon). Mirrors desktop
    /// `Screen::open_settings_panel`.
    pub fn open_settings_page(
        &mut self,
        values: serde_json::Value,
        font_families: Vec<String>,
    ) {
        self.hide_focus_modals();
        self.settings_page.set_values(values);
        if !font_families.is_empty() {
            self.settings_page.set_font_families(font_families);
        }
        self.settings_page.open();
        self.relayout();
    }

    /// Refresh the open (or next-opened) settings overlay with a newer
    /// config snapshot — used when the daemon fetch resolves after the
    /// overlay already opened.
    pub fn set_settings_values(&mut self, values: serde_json::Value) {
        self.settings_page.set_values(values);
    }

    pub fn close_settings_page(&mut self) {
        self.settings_page.close();
        self.relayout();
    }

    /// Open the About modal — name, version, and build commit.
    /// Mirrors desktop `Screen::open_about`.
    pub fn open_about_modal(&mut self, version: &str, commit: &str) {
        self.hide_focus_modals();
        let body = format!(
            "Neoism  v{version}\n\nA terminal-first workspace for code, notes,\nagents, and multiplayer editing.\n\nCommit\n{commit}"
        );
        self.modal.open(ModalSpec {
            title: "About Neoism".to_string(),
            body,
            meta: String::new(),
            input: None,
            buttons: vec![ModalButton::new("OK", "Enter", ModalAction::Close)],
            busy: false,
            blocking: true,
        });
        self.relayout();
    }

    // ── Spec-driven modal channel ─────────────────────────────────
    //
    // The generalized modal hosting: hosts open desktop's exact
    // `ModalSpec`s (file-tree create/rename/delete, the LSP rename
    // form, or an arbitrary spec), the chrome routes keys/clicks
    // through the shared `UniversalModal` exactly like desktop's
    // router, validates input with the same helpers/messages, and
    // queues confirmed outcomes as [`ModalHostAction`]s the host
    // drains (web: daemon `Files` ops / `editorLspRenameSubmit`).

    /// Open an arbitrary chrome-hosted modal. The generic entry the
    /// wasm bridge feeds from JSON specs; the typed openers below
    /// carry desktop's exact file-tree/LSP specs.
    pub fn open_chrome_modal(&mut self, spec: ModalSpec) {
        self.connection_gate_active = false;
        self.hide_focus_modals();
        self.modal.open(spec);
        self.relayout();
    }

    /// Show/update the web connection-loss gate without destroying the
    /// workspace canvas. It owns input and cannot be escaped or light-dismissed;
    /// only the host may close it after authenticated hydration succeeds.
    pub fn show_connection_gate(&mut self, body: String, meta: String) {
        self.hide_focus_modals();
        self.modal.open(ModalSpec {
            title: "Connection lost".to_string(),
            body,
            meta,
            input: None,
            buttons: vec![
                ModalButton::new(
                    "Retry now",
                    "Enter",
                    ModalAction::RunEditorCommand { command: "connection.retry".into() },
                ),
                ModalButton::new(
                    "Switch workplace",
                    "↓",
                    ModalAction::RunEditorCommand { command: "connection.switch".into() },
                ),
            ],
            busy: false,
            blocking: true,
        });
        self.modal.set_dismissible(false);
        self.connection_gate_active = true;
        self.relayout();
    }

    pub fn hide_connection_gate(&mut self) {
        if !self.connection_gate_active { return; }
        self.connection_gate_active = false;
        self.modal.close();
        self.relayout();
    }

    pub fn connection_gate_active(&self) -> bool {
        self.connection_gate_active
    }

    /// Open a chrome-hosted FORM modal (labelled fields + submit),
    /// desktop `UniversalModal::open_form` semantics.
    pub fn open_chrome_form_modal(&mut self, spec: ModalFormSpec) {
        self.hide_focus_modals();
        self.modal.open_form(spec);
        self.relayout();
    }

    /// Desktop `Screen::open_file_tree_new_file_prompt` — identical
    /// title/body/meta/placeholder/buttons.
    pub fn open_file_tree_new_file_modal(&mut self, dir: &str) {
        let label = self.chrome_display_path(dir);
        self.open_chrome_modal(ModalSpec {
            title: "New File".to_string(),
            body: format!("Create a file under `{label}`."),
            meta: "Relative paths are allowed; parent folders are created.".to_string(),
            input: Some(ModalInputSpec {
                value: String::new(),
                placeholder: "src/new_file.rs".to_string(),
            }),
            buttons: vec![
                ModalButton::new(
                    "Create File",
                    "Enter",
                    ModalAction::FileTreeNewFile {
                        dir: dir.to_string(),
                        name: String::new(),
                    },
                ),
                ModalButton::new("Cancel", "Esc", ModalAction::Close),
            ],
            busy: false,
            blocking: true,
        });
    }

    /// Desktop `Screen::open_file_tree_new_folder_prompt` parity.
    pub fn open_file_tree_new_folder_modal(&mut self, dir: &str) {
        let label = self.chrome_display_path(dir);
        self.open_chrome_modal(ModalSpec {
            title: "New Folder".to_string(),
            body: format!("Create a folder under `{label}`."),
            meta: "Relative paths are allowed.".to_string(),
            input: Some(ModalInputSpec {
                value: String::new(),
                placeholder: "new_folder".to_string(),
            }),
            buttons: vec![
                ModalButton::new(
                    "Create Folder",
                    "Enter",
                    ModalAction::FileTreeNewFolder {
                        dir: dir.to_string(),
                        name: String::new(),
                    },
                ),
                ModalButton::new("Cancel", "Esc", ModalAction::Close),
            ],
            busy: false,
            blocking: true,
        });
    }

    /// Desktop `Screen::open_file_tree_rename_prompt` parity —
    /// pre-filled with the current name, Enter renames, Esc cancels.
    pub fn open_file_tree_rename_modal(&mut self, path: &str) {
        let value = std::path::Path::new(path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let label = self.chrome_display_path(path);
        self.open_chrome_modal(ModalSpec {
            title: "Rename".to_string(),
            body: format!("Rename `{label}`."),
            meta: "Enter a new name in the same folder.".to_string(),
            input: Some(ModalInputSpec {
                value,
                placeholder: "new_name".to_string(),
            }),
            buttons: vec![
                ModalButton::new(
                    "Rename",
                    "Enter",
                    ModalAction::FileTreeRename {
                        path: path.to_string(),
                        name: String::new(),
                        notes: false,
                    },
                ),
                ModalButton::new("Cancel", "Esc", ModalAction::Close),
            ],
            busy: false,
            blocking: true,
        });
    }

    /// Desktop `Screen::confirm_delete_file_tree_path` parity — the
    /// destructive confirm (`d` or Enter deletes, Esc cancels).
    pub fn open_file_tree_delete_modal(&mut self, path: &str, is_dir: bool) {
        let label = self.chrome_display_path(path);
        let kind = if is_dir { "folder" } else { "file" };
        self.open_chrome_modal(ModalSpec {
            title: format!("Delete {kind}?"),
            body: format!("Delete `{label}` from disk?"),
            meta: "This cannot be undone. Press d or Enter to confirm.".to_string(),
            input: None,
            buttons: vec![
                ModalButton::new(
                    "× Delete",
                    "d",
                    ModalAction::FileTreeDelete {
                        path: path.to_string(),
                        notes: false,
                    },
                ),
                ModalButton::new("Cancel", "Esc", ModalAction::Close),
            ],
            busy: false,
            blocking: true,
        });
    }

    /// Desktop `Screen::open_code_rename_prompt` parity — the LSP
    /// rename form pre-filled with the symbol under the cursor
    /// (`code_rename_to` field, submit label "Rename").
    pub fn open_lsp_rename_modal(&mut self, word: &str) {
        self.open_chrome_form_modal(ModalFormSpec {
            title: format!("Rename `{word}`"),
            fields: vec![ModalFormField {
                id: "code_rename_to".into(),
                label: "New name".into(),
                value: word.to_string(),
                placeholder: "new_name".into(),
                secret: false,
            }],
            submit_label: "Rename".into(),
        });
    }

    /// Drain confirmed modal outcomes for the host bridge (web: the
    /// daemon `Files` plane and the LSP rename submit path).
    pub fn drain_modal_host_actions(&mut self) -> Vec<ModalHostAction> {
        self.modal.take_host_actions()
    }

    /// Chrome-scope twin of desktop `Screen::execute_modal_action`,
    /// covering the arms a chrome-hosted modal can produce. Prompt
    /// actions chain into the matching prompt modal (the shared
    /// policy's `OpenFollowupPrompt`), validated confirms queue a
    /// [`ModalHostAction`] and close (`CloseBeforeAction` /
    /// `CloseAfterValidatedInput`), and everything unrecognized
    /// closes honestly.
    pub(crate) fn execute_chrome_modal_action(&mut self, action: ModalAction) {
        match action {
            ModalAction::Close => {
                if self.connection_gate_active { return; }
                self.modal.close();
                self.relayout();
            }
            ModalAction::FileTreePromptNewFile { dir }
            | ModalAction::NotesPromptNewFile { dir } => {
                self.open_file_tree_new_file_modal(&dir);
            }
            ModalAction::FileTreePromptNewFolder { dir } => {
                self.open_file_tree_new_folder_modal(&dir);
            }
            ModalAction::FileTreePromptRename { path, .. } => {
                self.open_file_tree_rename_modal(&path);
            }
            ModalAction::FileTreePromptDelete { path, .. } => {
                let is_dir = self.file_tree_path_is_dir(&path);
                self.open_file_tree_delete_modal(&path, is_dir);
            }
            ModalAction::FileTreeNewFile { dir, name } => {
                self.confirm_file_tree_create(dir, name, false);
            }
            ModalAction::FileTreeNewFolder { dir, name } => {
                self.confirm_file_tree_create(dir, name, true);
            }
            ModalAction::FileTreeRename { path, name, .. } => {
                self.confirm_file_tree_rename(path, name);
            }
            ModalAction::FileTreeDelete { path, .. } => {
                self.modal
                    .queue_host_action(ModalHostAction::FileTreeDelete { path });
                self.modal.close();
                self.relayout();
            }
            ModalAction::ServerFormSubmit => {
                let values = self.modal.take_submitted_form().unwrap_or_default();
                // Code-rename form: `code_rename_to` is the
                // discriminator (present even when left empty) —
                // desktop `lifecycle/modal.rs` parity.
                if values.iter().any(|(id, _)| id == "code_rename_to") {
                    self.modal.close();
                    self.relayout();
                    let name = values
                        .iter()
                        .find(|(id, _)| id == "code_rename_to")
                        .map(|(_, value)| value.trim().to_string())
                        .filter(|value| !value.is_empty());
                    match name {
                        Some(name) => self
                            .modal
                            .queue_host_action(ModalHostAction::LspRename { name }),
                        None => self.notifications.push(
                            "Rename needs a non-empty name",
                            NotificationLevel::Warn,
                        ),
                    }
                    return;
                }
                // Any other chrome-hosted form is spec-driven: hand
                // every submitted field to the host as a generic
                // outcome (one action per field, field id as the id).
                self.modal.close();
                self.relayout();
                for (id, value) in values {
                    self.modal
                        .queue_host_action(ModalHostAction::Generic { id, value });
                }
            }
            // Spec-driven single-input submit: the generic JSON specs
            // encode host submits as `RunEditorCommandWithInput`
            // (input attached via `with_input`) and plain confirms as
            // `RunEditorCommand`. Desktop treats both as close-only,
            // so specs never behave destructively if they ever cross
            // hosts.
            ModalAction::RunEditorCommandWithInput { command, value } => {
                let trimmed = value.trim().to_string();
                if trimmed.is_empty() {
                    // `CloseAfterValidatedInput`: keep the modal up so
                    // the user can type a value (desktop `RenameTab`
                    // parity for empty input).
                    self.notifications
                        .push("Name required", NotificationLevel::Warn);
                    return;
                }
                self.modal.queue_host_action(ModalHostAction::Generic {
                    id: command,
                    value: trimmed,
                });
                self.modal.close();
                self.relayout();
            }
            ModalAction::RunEditorCommand { command } => {
                self.modal.queue_host_action(ModalHostAction::Generic {
                    id: command.clone(),
                    value: String::new(),
                });
                if command.starts_with("connection.") {
                    return;
                }
                self.modal.close();
                self.relayout();
            }
            // Anything else a spec could carry has no chrome-side
            // executor yet — close instead of wedging the modal open.
            _ => {
                self.modal.close();
                self.relayout();
            }
        }
    }

    /// Validated create — the chrome-side twin of desktop
    /// `child_path_for_input` + `create_file_tree_file/folder`: same
    /// messages, same keep-modal-open-on-warn behavior; on success the
    /// op is queued for the host (web is always the "remote" arm — the
    /// daemon owns the filesystem).
    fn confirm_file_tree_create(&mut self, dir: String, name: String, folder: bool) {
        let name = match validate_child_name(&name) {
            Ok(name) => name,
            Err(message) => {
                self.notifications.push(message, NotificationLevel::Warn);
                return;
            }
        };
        let action = if folder {
            ModalHostAction::FileTreeNewFolder { dir, name }
        } else {
            ModalHostAction::FileTreeNewFile { dir, name }
        };
        self.modal.queue_host_action(action);
        self.modal.close();
        self.relayout();
    }

    /// Validated rename — shares desktop's
    /// `rename_target_for_input` helper (Noop when the name didn't
    /// change closes silently, exactly like desktop).
    fn confirm_file_tree_rename(&mut self, path: String, name: String) {
        use crate::panels::file_tree::{rename_target_for_input, RenameTarget};
        match rename_target_for_input(std::path::Path::new(&path), &name) {
            Ok(RenameTarget::Noop) => {
                self.modal.close();
                self.relayout();
            }
            Ok(RenameTarget::Target(_)) => {
                self.modal
                    .queue_host_action(ModalHostAction::FileTreeRename {
                        path,
                        name: name.trim().to_string(),
                    });
                self.modal.close();
                self.relayout();
            }
            Err(message) => {
                self.notifications.push(message, NotificationLevel::Warn);
            }
        }
    }

    /// Workspace-relative display label — desktop
    /// `Screen::file_tree_display_path` parity, keyed on the chrome's
    /// workspace root.
    fn chrome_display_path(&self, path: &str) -> String {
        let path = std::path::Path::new(path);
        if let Some(root) = self.workspace_root_path.as_deref() {
            if let Ok(rel) = path.strip_prefix(root) {
                if !rel.as_os_str().is_empty() {
                    return rel.display().to_string();
                }
            }
        }
        path.display().to_string()
    }

    /// Whether `path` is a directory according to the file tree's
    /// current entries (the wasm chrome has no filesystem to stat).
    pub fn file_tree_path_is_dir(&self, path: &str) -> bool {
        use crate::panels::file_tree::NodeKind;
        let path = std::path::Path::new(path);
        self.file_tree
            .as_ref()
            .map(|tree| {
                tree.entries().iter().any(|entry| {
                    entry.path.as_deref() == Some(path)
                        && matches!(entry.kind, NodeKind::Dir { .. })
                })
            })
            .unwrap_or(false)
    }

    /// Open (or re-activate) the Extensions chrome-page tab. Desktop
    /// twin: `Screen::open_extensions_page` — singleton tab, search
    /// box auto-focused.
    pub fn open_extensions_page_tab(&mut self) -> usize {
        self.hide_focus_modals();
        let ix = self
            .buffer_tabs
            .open_chrome_page(ChromePageKind::Extensions, 0);
        self.set_active_tab_index(ix);
        self.extensions_page.focus_search();
        self.relayout();
        ix
    }

    /// Open (or re-activate) the NeoWorld chrome-page tab. Desktop
    /// twin: `Screen::open_neoworld_page`.
    pub fn open_neoworld_page_tab(&mut self) -> usize {
        self.hide_focus_modals();
        let ix = self
            .buffer_tabs
            .open_chrome_page(ChromePageKind::NeoWorld, 0);
        self.set_active_tab_index(ix);
        self.relayout();
        ix
    }

    /// Install the NeoWorld pane (host builds it from a persisted
    /// `StoredPet`-shaped snapshot, or fresh). Replaces any existing
    /// pane, so re-seeding after a persistence load is safe.
    pub fn install_neoworld_pane(&mut self, pane: NeoWorldPane) {
        self.neoworld_pane = Some(pane);
    }

    pub fn neoworld_pane(&self) -> Option<&NeoWorldPane> {
        self.neoworld_pane.as_ref()
    }

    pub fn neoworld_pane_mut(&mut self) -> Option<&mut NeoWorldPane> {
        self.neoworld_pane.as_mut()
    }

    // ── Host drains ────────────────────────────────────────────────

    /// Settings actions (Set / SetKeybind / OpenConfigFile /
    /// RunAction) queued since the last drain. The host persists Set /
    /// SetKeybind (web: daemon `Config` envelope → the same
    /// `neoism_backend::config::write_setting` desktop calls) and
    /// routes the rest.
    pub fn drain_settings_actions(&mut self) -> Vec<SettingsAction> {
        std::mem::take(&mut self.pending_settings_actions)
    }

    /// Extensions page intents that need a host (currently only
    /// `OpenRepository` — install toggles are absorbed with an honest
    /// "manage from desktop" toast because web is read-only).
    pub fn drain_extensions_actions(&mut self) -> Vec<PaneAction> {
        std::mem::take(&mut self.pending_extensions_actions)
    }

    /// Pet-state snapshots to persist (periodic + after interaction),
    /// newest last. The host stores the newest one (desktop:
    /// sqlite `NeoWorldStore`; web: localStorage mirroring the
    /// `StoredPet` shape).
    pub fn drain_neoworld_snapshots(&mut self) -> Vec<neoism_neoworld_core::PetState> {
        std::mem::take(&mut self.pending_neoworld_snapshots)
    }

    // ── Draw ───────────────────────────────────────────────────────

    /// Paint the active chrome page's body inside the terminal rect.
    /// Called from `Chrome::draw`'s non-terminal branch after the
    /// theme-bg backdrop is down.
    pub(crate) fn draw_chrome_page_body(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        kind: ChromePageKind,
        rect: Rect,
    ) {
        let theme = self.ide_theme;
        let scale = self.chrome_scale;
        match kind {
            ChromePageKind::Extensions => {
                let ticking = self.extensions_page.tick_scroll();
                self.extensions_page.set_viewport_height(rect.h);
                self.extensions_page.render(
                    sugarloaf,
                    [rect.x, rect.y, rect.w, rect.h],
                    &theme,
                    scale,
                    Some(self.last_pointer_pos),
                    &[],
                );
                self.editor_pane_animating |= ticking;
            }
            ChromePageKind::NeoWorld => {
                // Hosts normally install a persisted pet before (or
                // right after) opening the tab; fall back to a preview
                // pet so the page is never blank.
                if self.neoworld_pane.is_none() {
                    self.neoworld_pane = Some(NeoWorldPane::preview());
                }
                let mut snapshot = None;
                if let Some(pane) = self.neoworld_pane.as_mut() {
                    pane.render(
                        sugarloaf,
                        [rect.x, rect.y, rect.w, rect.h],
                        &theme,
                        scale,
                    );
                    snapshot = pane.take_periodic_snapshot();
                }
                if let Some(state) = snapshot {
                    self.pending_neoworld_snapshots.push(state);
                }
                // The sim never idles — keep the host's frame pump on.
                self.editor_pane_animating = true;
            }
        }
    }

    /// Paint the full-screen overlays (settings, modal) LAST, through
    /// sugarloaf's late-overlay pass so text from panels drawn earlier
    /// this frame can never bleed through the opaque surfaces (the
    /// same trick desktop uses for its modal material).
    pub(crate) fn draw_chrome_overlays(&mut self, sugarloaf: &mut Sugarloaf) {
        if !self.chrome_overlay_active() {
            return;
        }
        let Some(viewport) = self.last_viewport else {
            return;
        };
        let theme = self.ide_theme;
        if self.settings_page.is_active() {
            let scale = self.chrome_scale;
            sugarloaf.set_late_overlay_mode(true);
            self.settings_page
                .render(sugarloaf, viewport.w, viewport.h, &theme, scale, None);
            sugarloaf.set_late_overlay_mode(false);
        }
        if self.modal.is_active() {
            sugarloaf.set_late_overlay_mode(true);
            self.modal
                .render(sugarloaf, (viewport.w, viewport.h, 1.0), &theme);
            sugarloaf.set_late_overlay_mode(false);
        }
    }

    // ── Event routing ──────────────────────────────────────────────

    /// Route input to the chrome-page layer. Called FIRST from
    /// `Chrome::handle_event` — desktop parity: the settings overlay
    /// and modal own the keyboard/pointer outright while active
    /// (router/route.rs), and an active Extensions/NeoWorld tab owns
    /// input inside its page rect while the surrounding chrome strips
    /// keep theirs. Returns true when the event was consumed.
    pub(crate) fn handle_chrome_page_event(&mut self, event: &UiEvent) -> bool {
        let input_like = matches!(
            event,
            UiEvent::Key(_)
                | UiEvent::Text(_)
                | UiEvent::PointerDown { .. }
                | UiEvent::PointerUp { .. }
                | UiEvent::PointerMove { .. }
                | UiEvent::PointerLeave
                | UiEvent::Wheel { .. }
        );
        if !input_like {
            return false;
        }
        if self.modal.is_active() {
            return self.handle_chrome_modal_event(event);
        }
        if self.settings_page.is_active() {
            return self.handle_settings_overlay_event(event);
        }
        match self.active_chrome_page() {
            Some(ChromePageKind::Extensions) => self.handle_extensions_page_event(event),
            Some(ChromePageKind::NeoWorld) => self.handle_neoworld_page_event(event),
            None => false,
        }
    }

    /// Chrome-owned modal input — the web twin of the universal-modal
    /// arm in desktop `router/route.rs` (keys) and
    /// `lifecycle/modal.rs::handle_modal_click` (pointer). The modal
    /// owns the keyboard/pointer outright while active; typing edits
    /// the input/form field, Tab walks form fields, Up/Down move the
    /// button selection, Enter fires the selected action (attaching
    /// the input value, exactly like `UniversalModal::selected_action`
    /// does on desktop), Esc fires the Esc-hinted action or closes,
    /// and single-char presses trigger hint buttons on input-less
    /// modals (`d` on the delete confirm).
    fn handle_chrome_modal_event(&mut self, event: &UiEvent) -> bool {
        match event {
            UiEvent::Key(key) => {
                if key.state != KeyState::Pressed {
                    return true;
                }
                self.handle_chrome_modal_key(key);
                true
            }
            UiEvent::Text(text) => {
                if self.modal.has_input() {
                    let filtered: String =
                        text.chars().filter(|c| !c.is_control()).collect();
                    if !filtered.is_empty() {
                        self.modal.push_input(&filtered);
                    }
                } else if let Some(action) = self.modal.action_for_hint(text) {
                    self.execute_chrome_modal_action(action);
                }
                true
            }
            UiEvent::PointerDown { x, y, .. } => {
                self.handle_chrome_modal_click(*x, *y);
                true
            }
            UiEvent::Wheel { dy, mode, .. } => {
                let line_h = self.cell_h.max(14.0);
                let pixels = match mode {
                    WheelMode::Pixel => *dy,
                    WheelMode::Line => *dy * line_h,
                    WheelMode::Page => *dy * self.layout.terminal.h.max(line_h),
                };
                let (px, py) = self.last_pointer_pos;
                let width = self.last_viewport.map(|v| v.w).unwrap_or(0.0);
                // `scroll_at` uses the winit positive-up sign; DOM wheel
                // dy is positive scrolling down.
                let _ = self.modal.scroll_at(px, py, width, 1.0, -pixels);
                true
            }
            // Pointer move/up/leave are swallowed while the modal owns
            // input — nothing below may see them (desktop parity: the
            // blocking modal arm returns true for everything).
            _ => true,
        }
    }

    /// Key routing for the chrome modal — a UiEvent transliteration of
    /// desktop `router/route.rs`'s modal arm, one guard at a time.
    fn handle_chrome_modal_key(&mut self, key: &KeyDescriptor) {
        let blocking = self.modal.is_blocking();
        let has_input = self.modal.has_input();
        let mods = key.modifiers;
        let plain_mods = !mods.contains(Modifiers::CTRL)
            && !mods.contains(Modifiers::ALT)
            && !mods.contains(Modifiers::META);
        match &key.logical {
            LogicalKey::Named(NamedKey::Escape) => {
                if !self.modal.is_dismissible() {
                    return;
                }
                if let Some(action) = self.modal.escape_action() {
                    self.execute_chrome_modal_action(action);
                } else {
                    self.modal.close();
                    self.relayout();
                }
            }
            LogicalKey::Named(NamedKey::Backspace) if blocking && has_input => {
                self.modal.pop_input();
            }
            LogicalKey::Named(NamedKey::Delete) if blocking && has_input => {
                self.modal.delete_input();
            }
            LogicalKey::Named(NamedKey::ArrowLeft) if blocking && has_input => {
                self.modal.move_input_caret_left();
            }
            LogicalKey::Named(NamedKey::ArrowRight) if blocking && has_input => {
                self.modal.move_input_caret_right();
            }
            LogicalKey::Named(NamedKey::Home) if blocking && has_input => {
                self.modal.input_caret_to_start();
            }
            LogicalKey::Named(NamedKey::End) if blocking && has_input => {
                self.modal.input_caret_to_end();
            }
            LogicalKey::Named(NamedKey::Tab) if blocking && has_input => {
                let _ = self.modal.focus_next_form_field();
            }
            LogicalKey::Named(NamedKey::ArrowUp) if blocking => {
                // Forms walk fields ↔ buttons; plain modals keep the
                // button-list selection (desktop parity).
                if self.modal.has_markdown_input() {
                    self.modal.move_input_up();
                } else if !self.modal.form_focus_move(false) {
                    self.modal.move_selection_up();
                }
            }
            LogicalKey::Named(NamedKey::ArrowDown) if blocking => {
                if self.modal.has_markdown_input() {
                    self.modal.move_input_down();
                } else if !self.modal.form_focus_move(true) {
                    self.modal.move_selection_down();
                }
            }
            LogicalKey::Named(NamedKey::PageUp) if blocking => {
                self.modal.scroll_body_page(false);
            }
            LogicalKey::Named(NamedKey::PageDown) if blocking => {
                self.modal.scroll_body_page(true);
            }
            LogicalKey::Named(NamedKey::Enter) if blocking => {
                if has_input
                    && self.modal.has_markdown_input()
                    && !mods.contains(Modifiers::CTRL)
                    && !mods.contains(Modifiers::META)
                {
                    self.modal.insert_input_newline();
                    return;
                }
                let selected = self.modal.selected_action();
                let action = if matches!(selected, Some(ModalAction::ServerFormSubmit)) {
                    self.modal.submit_form()
                } else {
                    selected
                };
                if let Some(action) = action {
                    self.execute_chrome_modal_action(action);
                }
            }
            LogicalKey::Named(NamedKey::Space) if blocking && has_input => {
                if plain_mods {
                    self.modal.push_input(" ");
                }
            }
            LogicalKey::Character(text) if blocking && has_input => {
                // Typed text edits the focused input; chords fall
                // through (Ctrl+V paste arrives as a `Text` event from
                // the web host's paste plumbing).
                if plain_mods {
                    let filtered: String =
                        text.chars().filter(|c| !c.is_control()).collect();
                    if !filtered.is_empty() {
                        self.modal.push_input(&filtered);
                    }
                }
            }
            LogicalKey::Character(text) if blocking && !has_input => {
                if let Some(action) = self.modal.action_for_hint(text) {
                    self.execute_chrome_modal_action(action);
                }
            }
            _ => {}
        }
    }

    /// Pointer routing for the chrome modal — desktop
    /// `Screen::handle_modal_click` transliterated (minus the server
    /// modal's Join ↔ Create slider, which is not chrome-hosted yet).
    fn handle_chrome_modal_click(&mut self, x: f32, y: f32) {
        let blocking = self.modal.is_blocking();
        if self.modal.close_button_hit(x, y) {
            if let Some(action) = self.modal.escape_action() {
                self.execute_chrome_modal_action(action);
            } else {
                self.modal.close();
                self.relayout();
            }
            return;
        }
        if self.modal.click_markdown_input(x, y) {
            return;
        }
        let width = self.last_viewport.map(|v| v.w).unwrap_or(0.0);
        match self.modal.hit_test(x, y, width, 1.0) {
            Ok(Some(index)) => {
                if self.modal.focus_form_hit(index) {
                    return;
                }
                self.modal.set_selected_index(index);
                let selected = self.modal.selected_action();
                let action = if matches!(selected, Some(ModalAction::ServerFormSubmit)) {
                    self.modal.submit_form()
                } else {
                    selected
                };
                if let Some(action) = action {
                    self.execute_chrome_modal_action(action);
                }
            }
            Ok(None) => {}
            Err(()) => {
                if !self.modal.is_dismissible() {
                    return;
                }
                if !blocking {
                    let _ = self.modal.close_if_non_blocking();
                    self.relayout();
                    return;
                }
                // Outside click on a blocking modal cancels — the same
                // escape-action path desktop takes.
                if let Some(action) = self.modal.escape_action() {
                    self.execute_chrome_modal_action(action);
                } else {
                    self.modal.close();
                    self.relayout();
                }
            }
        }
    }

    /// Full-screen settings overlay input — the web twin of the
    /// desktop settings arm in `router/route.rs` (keybind capture →
    /// Esc chain → typed search) plus pointer/wheel routing from
    /// `bridges/top_bar.rs` and `window_event/scroll.rs`.
    fn handle_settings_overlay_event(&mut self, event: &UiEvent) -> bool {
        match event {
            UiEvent::Key(key) => {
                if key.state != KeyState::Pressed {
                    return true;
                }
                if self.settings_page.capturing().is_some() {
                    self.capture_settings_keybind(key);
                    return true;
                }
                match &key.logical {
                    LogicalKey::Named(NamedKey::Escape) => {
                        self.settings_page.on_escape();
                        if !self.settings_page.is_active() {
                            self.relayout();
                        }
                    }
                    LogicalKey::Named(NamedKey::Backspace) => {
                        self.settings_page.backspace();
                    }
                    LogicalKey::Named(NamedKey::Enter) => {
                        if let Some(action) = self.settings_page.commit_edit() {
                            self.queue_settings_action(action);
                        }
                    }
                    LogicalKey::Character(text) => {
                        for ch in text.chars().filter(|ch| !ch.is_control()) {
                            self.settings_page.input_char(ch);
                        }
                    }
                    _ => {}
                }
                true
            }
            UiEvent::Text(text) => {
                if self.settings_page.capturing().is_some() {
                    if let Some(ch) = text.chars().find(|ch| !ch.is_control()) {
                        self.capture_settings_keybind(&KeyDescriptor {
                            physical: crate::event::PhysicalKey(0),
                            logical: LogicalKey::Character(ch.to_string().into()),
                            state: KeyState::Pressed,
                            modifiers: Modifiers::empty(),
                            repeat: false,
                        });
                    }
                    return true;
                }
                for ch in text.chars().filter(|ch| !ch.is_control()) {
                    self.settings_page.input_char(ch);
                }
                true
            }
            UiEvent::PointerDown { x, y, .. } => {
                let outcome = self.settings_page.pointer_down(*x, *y);
                if let Some(action) = outcome.action {
                    self.queue_settings_action(action);
                }
                if !self.settings_page.is_active() {
                    self.relayout();
                }
                true
            }
            UiEvent::PointerMove { x, y, .. } => {
                self.settings_page.pointer_move(*x, *y);
                true
            }
            UiEvent::Wheel { dy, mode, .. } => {
                let line_h = self.cell_h.max(14.0);
                let pixels = match mode {
                    WheelMode::Pixel => *dy,
                    WheelMode::Line => *dy * line_h,
                    WheelMode::Page => *dy * self.layout.terminal.h.max(line_h),
                };
                // `scroll_by` uses the winit positive-up sign; DOM
                // wheel dy is positive scrolling down.
                self.settings_page.scroll_by(-pixels);
                true
            }
            _ => true,
        }
    }

    /// Finish (or abort) a keybind capture from a `KeyDescriptor` —
    /// the shared-event twin of desktop `capture_settings_keybind`.
    fn capture_settings_keybind(&mut self, key: &KeyDescriptor) {
        if matches!(key.logical, LogicalKey::Named(NamedKey::Escape)) {
            self.settings_page.cancel_capture();
            return;
        }
        let key_str = match &key.logical {
            LogicalKey::Named(NamedKey::Space) => "space".to_string(),
            LogicalKey::Named(NamedKey::Enter) => "return".to_string(),
            LogicalKey::Named(NamedKey::Tab) => "tab".to_string(),
            LogicalKey::Named(NamedKey::ArrowUp) => "up".to_string(),
            LogicalKey::Named(NamedKey::ArrowDown) => "down".to_string(),
            LogicalKey::Named(NamedKey::ArrowLeft) => "left".to_string(),
            LogicalKey::Named(NamedKey::ArrowRight) => "right".to_string(),
            LogicalKey::Named(NamedKey::Home) => "home".to_string(),
            LogicalKey::Named(NamedKey::End) => "end".to_string(),
            LogicalKey::Named(NamedKey::PageUp) => "pageup".to_string(),
            LogicalKey::Named(NamedKey::PageDown) => "pagedown".to_string(),
            LogicalKey::Named(NamedKey::Delete) => "delete".to_string(),
            LogicalKey::Named(NamedKey::Backspace) => "back".to_string(),
            LogicalKey::Character(c) => {
                let c = c.trim();
                if c.is_empty() {
                    return;
                }
                c.to_lowercase()
            }
            // Bare modifier / unsupported key: keep waiting.
            _ => return,
        };
        let mods = key.modifiers;
        let mut parts: Vec<&str> = Vec::new();
        if mods.contains(crate::event::Modifiers::META) {
            parts.push("super");
        }
        if mods.contains(crate::event::Modifiers::CTRL) {
            parts.push("control");
        }
        if mods.contains(crate::event::Modifiers::ALT) {
            parts.push("alt");
        }
        if mods.contains(crate::event::Modifiers::SHIFT) {
            parts.push("shift");
        }
        let with = parts.join(" | ");
        if let Some(action) = self.settings_page.finish_capture(key_str, with) {
            self.queue_settings_action(action);
        }
    }

    fn queue_settings_action(&mut self, action: SettingsAction) {
        self.pending_settings_actions.push(action);
    }

    /// Extensions page input, scoped to the page body (the terminal
    /// rect). The tab strip / top bar / status line keep their own
    /// hits so switching tabs away still works.
    fn handle_extensions_page_event(&mut self, event: &UiEvent) -> bool {
        let page_rect = self.layout.terminal;
        match event {
            UiEvent::PointerDown { button, x, y, .. } => {
                if !page_rect.contains(*x, *y) {
                    return false;
                }
                if let Some(action) =
                    self.extensions_page.on_pointer_down(*x, *y, *button)
                {
                    self.queue_extensions_action(action);
                }
                true
            }
            UiEvent::Wheel { dy, mode, .. } => {
                let (px, py) = self.last_pointer_pos;
                if !page_rect.contains(px, py) {
                    return false;
                }
                let line_h = self.cell_h.max(14.0);
                let pixels = match mode {
                    WheelMode::Pixel => *dy,
                    WheelMode::Line => *dy * line_h,
                    WheelMode::Page => *dy * page_rect.h.max(line_h),
                };
                // Positive = scrolling down — the pane's contract.
                self.extensions_page.scroll_pixels(pixels);
                true
            }
            UiEvent::Key(key) => {
                let response = self.extensions_page.on_key(key);
                if let Some(action) = response.action {
                    self.queue_extensions_action(action);
                }
                if response.consumed {
                    self.extensions_page
                        .ensure_selected_visible(page_rect.h.max(1.0));
                }
                response.consumed
            }
            UiEvent::Text(text) => self.extensions_page.on_text(text),
            _ => false,
        }
    }

    fn queue_extensions_action(&mut self, action: PaneAction) {
        match &action {
            PaneAction::InstallToggleRequested { .. } => {
                // Read-only surface — honest note instead of a dead
                // spinner. Install flows stay on the desktop host.
                self.notifications.push(
                    "Extensions are read-only on web — install and uninstall from the Neoism desktop app.",
                    NotificationLevel::Info,
                );
            }
            PaneAction::OpenRepository(_) => {
                self.pending_extensions_actions.push(action);
            }
        }
    }

    /// NeoWorld page input — grab/drag/poke the pet, desktop
    /// `handle_neoworld_pointer_*` parity. A consumed release queues a
    /// persistence snapshot like desktop's `neoworld_runtime::persist`
    /// call on pointer-up.
    fn handle_neoworld_page_event(&mut self, event: &UiEvent) -> bool {
        let page_rect = self.layout.terminal;
        let Some(pane) = self.neoworld_pane.as_mut() else {
            return false;
        };
        match event {
            UiEvent::PointerDown {
                button: PointerButton::Left,
                x,
                y,
                ..
            } => {
                if !page_rect.contains(*x, *y) {
                    return false;
                }
                pane.pointer_down(*x, *y)
            }
            UiEvent::PointerMove { x, y, .. } => pane.pointer_move(*x, *y),
            UiEvent::PointerUp {
                button: PointerButton::Left,
                x,
                y,
                ..
            } => {
                let consumed = pane.pointer_up(*x, *y);
                if consumed {
                    let state = *pane.pet();
                    self.pending_neoworld_snapshots.push(state);
                }
                consumed
            }
            _ => false,
        }
    }
}

/// Create-name validation — the byte-for-byte twin of desktop
/// `screen/mod.rs::child_path_for_input`'s checks and messages, minus
/// the join (the daemon owns the filesystem on web, so only the name
/// validation crosses over). Returns the trimmed name on success.
fn validate_child_name(input: &str) -> Result<String, String> {
    use std::path::{Component, Path};
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Enter a name first.".to_string());
    }
    let rel = Path::new(trimmed);
    if rel.is_absolute() {
        return Err("Use a relative name, not an absolute path.".to_string());
    }
    let mut has_name = false;
    for component in rel.components() {
        match component {
            Component::Normal(_) => has_name = true,
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("Names cannot escape the selected folder.".to_string());
            }
        }
    }
    if !has_name {
        return Err("Enter a name first.".to_string());
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod connection_gate_tests {
    use super::*;

    #[test]
    fn connection_gate_is_host_dismissed_and_retry_keeps_canvas_gate_open() {
        let mut chrome = Chrome::<()>::new();
        chrome.show_connection_gate(
            "The workspace is preserved while Neoism reconnects.".into(),
            "Attempt 3 · Retrying in 2s".into(),
        );
        assert!(chrome.connection_gate_active());
        assert!(chrome.modal.is_active());
        assert!(!chrome.modal.is_dismissible());

        chrome.execute_chrome_modal_action(ModalAction::RunEditorCommand {
            command: "connection.retry".into(),
        });
        assert!(chrome.modal.is_active(), "retry must not dismiss the gate");

        chrome.hide_connection_gate();
        assert!(!chrome.connection_gate_active());
        assert!(!chrome.modal.is_active());
    }
}
