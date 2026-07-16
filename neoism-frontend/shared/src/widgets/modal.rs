use web_time::Instant;

use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::editor::neodraw::{render_scene, Camera, Vec2};
use crate::primitives::IdeTheme;
use crate::syntax::{highlight_line, syn_color, Lang};
use crate::widgets::mermaid::{mermaid_scene, parse_mermaid_diagram, MermaidDiagram};
use crate::widgets::scrollbar;

const MODAL_WIDTH: f32 = 560.0;
const MODAL_CORNER_RADIUS: f32 = 8.0;
const MODAL_MARGIN_TOP: f32 = 92.0;
const MODAL_PADDING: f32 = 18.0;
const TITLE_FONT_SIZE: f32 = 15.0;
const BODY_FONT_SIZE: f32 = 12.0;
const META_FONT_SIZE: f32 = 11.0;
const ACTION_FONT_SIZE: f32 = 13.0;
const ACTION_ROW_HEIGHT: f32 = 32.0;
const INPUT_ROW_HEIGHT: f32 = 34.0;
const INPUT_FONT_SIZE: f32 = 13.0;
const INPUT_CARET_WIDTH: f32 = 1.25;
const BODY_LINE_HEIGHT: f32 = 19.0;
const TITLE_HEIGHT: f32 = 24.0;
const SEPARATOR_HEIGHT: f32 = 1.0;
const BUSY_BAR_HEIGHT: f32 = 4.0;
const BUSY_BAR_WIDTH: f32 = 120.0;
const MAX_BODY_LINES: usize = 14;
const MAX_VISIBLE_ACTIONS: usize = 8;
const WRAP_COLUMNS: usize = 72;

#[allow(dead_code)]
const DEPTH_BACKDROP: f32 = 0.0;
const DEPTH_BG: f32 = 0.1;
const DEPTH_ELEMENT: f32 = 0.2;
const ORDER: u8 = 24;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModalAction {
    Close,
    /// Open the notes graph view (footer settings menu → Graph).
    NotesOpenGraph,
    /// Open the notes create menu (footer settings menu → Add…).
    NotesOpenCreateMenu,
    InstallLsp {
        server: String,
    },
    InstallPythonKernel,
    InstallTreesitter {
        lang: String,
    },
    ApplyTheme {
        name: String,
    },
    ApplyShaderOverlay {
        path: Option<String>,
    },
    /// Apply a Mash Up Pack (theme + shader + fonts as one look).
    /// `None` deactivates the current pack, keeping the theme.
    ApplyMashupPack {
        id: Option<String>,
    },
    RunEditorCommand {
        command: String,
    },
    RunEditorCommandWithInput {
        command: String,
        value: String,
    },
    OpenLspLocation {
        uri: String,
        line: u32,
        character: u32,
    },
    /// Retry an agent CLI install (claude / codex / opencode).
    /// `kind` matches `AgentKind::id()`.
    InstallAgent {
        kind: String,
    },
    /// Once an agent install succeeds, the success modal offers a
    /// "Launch" button that fires this action — opens a new tab and
    /// runs the binary.
    RunAgent {
        kind: String,
    },
    AcpPermission {
        server_id: String,
        request_id: u64,
        option_id: Option<String>,
    },
    FileTreeEdit {
        path: String,
    },
    FileTreeCopy {
        path: String,
    },
    FileTreePaste {
        dest_dir: String,
    },
    FileTreePromptDelete {
        path: String,
    },
    FileTreeDelete {
        path: String,
    },
    FileTreePromptNewFile {
        dir: String,
    },
    NotesPromptNewFile {
        dir: String,
    },
    FileTreePromptNewFolder {
        dir: String,
    },
    FileTreePromptRename {
        path: String,
    },
    FileTreeNewFile {
        dir: String,
        name: String,
    },
    NotesNewFile {
        dir: String,
        name: String,
    },
    /// Create a fresh `.neodraw` in `dir` (the vault the notes sidebar is
    /// viewing) and open it — dispatched from the sidebar's ⋮ create menu.
    NotesNewDrawing {
        dir: String,
    },
    /// Prompt for a custom icon glyph for the notes entry at `path`.
    NotesPromptIcon {
        path: String,
    },
    /// Persist `icon` (emoji/any glyph; empty = reset to default) for the
    /// notes entry at `path` in the vault's `.neoism-icons.json`.
    NotesSetIcon {
        path: String,
        icon: String,
    },
    FileTreeNewFolder {
        dir: String,
        name: String,
    },
    FileTreeRename {
        path: String,
        name: String,
    },
    /// Rename the buffer tab at `index` to `name` (filled from the modal
    /// input box via [`ModalAction::with_input`]). When
    /// `agent_session_id` is `Some`, the host ALSO publishes the new
    /// title at the daemon level for that agent session (mirrors the
    /// agent `SetTitle` path); otherwise the rename is a local tab label.
    RenameTab {
        index: usize,
        agent_session_id: Option<String>,
        name: String,
    },
    NotesVaultPromptAdd,
    ServerFormSubmit,
    ServerRemoveConfirm {
        id: String,
    },
    NotesVaultAdd {
        name: String,
    },
    NotesVaultPromptRename,
    NotesVaultRename {
        name: String,
    },
    NotesVaultSwitch {
        name: String,
    },
    NotesVaultOpenVaultsRoot,
    NotesVaultLinkCurrentWorkspace,
    NotesVaultPromptLinkProject {
        vault: String,
    },
    NotesVaultLinkProject {
        vault: String,
        path: String,
    },
    /// Render every note in `vault` to a reMarkable document bundle and
    /// push it to the tablet (a "Neoism" folder of writable pages).
    NotesVaultShareWithRemarkable {
        vault: String,
    },
}

impl ModalAction {
    fn with_input(self, value: String) -> Self {
        match self {
            ModalAction::FileTreeNewFile { dir, .. } => {
                ModalAction::FileTreeNewFile { dir, name: value }
            }
            ModalAction::NotesNewFile { dir, .. } => {
                ModalAction::NotesNewFile { dir, name: value }
            }
            ModalAction::NotesSetIcon { path, .. } => {
                ModalAction::NotesSetIcon { path, icon: value }
            }
            ModalAction::FileTreeNewFolder { dir, .. } => {
                ModalAction::FileTreeNewFolder { dir, name: value }
            }
            ModalAction::FileTreeRename { path, .. } => {
                ModalAction::FileTreeRename { path, name: value }
            }
            ModalAction::RenameTab {
                index,
                agent_session_id,
                ..
            } => ModalAction::RenameTab {
                index,
                agent_session_id,
                name: value,
            },
            ModalAction::NotesVaultAdd { .. } => {
                ModalAction::NotesVaultAdd { name: value }
            }
            ModalAction::NotesVaultRename { .. } => {
                ModalAction::NotesVaultRename { name: value }
            }
            ModalAction::NotesVaultLinkProject { vault, .. } => {
                ModalAction::NotesVaultLinkProject { vault, path: value }
            }
            ModalAction::RunEditorCommandWithInput { command, .. } => {
                ModalAction::RunEditorCommandWithInput { command, value }
            }
            action => action,
        }
    }

    fn is_destructive(&self) -> bool {
        matches!(
            self,
            ModalAction::FileTreePromptDelete { .. } | ModalAction::FileTreeDelete { .. }
        )
    }
}

#[derive(Clone, Debug)]
pub struct ModalButton {
    pub label: String,
    pub hint: String,
    pub action: ModalAction,
}

impl ModalButton {
    pub fn new(
        label: impl Into<String>,
        hint: impl Into<String>,
        action: ModalAction,
    ) -> Self {
        Self {
            label: label.into(),
            hint: hint.into(),
            action,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ModalSpec {
    pub title: String,
    pub body: String,
    pub meta: String,
    pub input: Option<ModalInputSpec>,
    pub buttons: Vec<ModalButton>,
    pub busy: bool,
    pub blocking: bool,
}

#[derive(Clone, Debug)]
pub struct ModalInputSpec {
    pub value: String,
    pub placeholder: String,
}

#[derive(Clone, Debug)]
pub struct ModalFormField {
    pub id: String,
    pub label: String,
    pub value: String,
    pub placeholder: String,
    pub secret: bool,
}

#[derive(Clone, Debug)]
pub struct ModalFormSpec {
    pub title: String,
    pub fields: Vec<ModalFormField>,
    pub submit_label: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BodyLineKind {
    Text,
    Heading,
    Bullet,
    Code(Lang),
    Mermaid,
}

#[derive(Clone, Debug)]
struct BodyLine {
    text: String,
    kind: BodyLineKind,
    mermaid: Option<MermaidDiagram>,
    row_span: usize,
}

impl BodyLine {
    fn new(text: String, kind: BodyLineKind) -> Self {
        Self {
            text,
            kind,
            mermaid: None,
            row_span: 1,
        }
    }

    fn code(text: String, lang: Lang) -> Self {
        Self::new(text, BodyLineKind::Code(lang))
    }
}

#[derive(Clone, Debug)]
pub struct UniversalModal {
    active: bool,
    title: String,
    body: String,
    meta: String,
    buttons: Vec<ModalButton>,
    input: Option<ModalInputSpec>,
    form: Option<ModalFormSpec>,
    form_focus: usize,
    /// Caret position in CHARS within the focused input (single input or
    /// focused form field). Reset to end-of-value on open and focus change.
    input_caret: usize,
    submitted_form: Option<Vec<(String, String)>>,
    selected_index: usize,
    scroll_offset: usize,
    body_scroll_offset: usize,
    body_wheel_accumulator: f32,
    scale: f32,
    busy: bool,
    blocking: bool,
    opened_at: Instant,
}

impl Default for UniversalModal {
    fn default() -> Self {
        Self {
            active: false,
            title: String::new(),
            body: String::new(),
            meta: String::new(),
            buttons: Vec::new(),
            input: None,
            form: None,
            form_focus: 0,
            input_caret: 0,
            submitted_form: None,
            selected_index: 0,
            scroll_offset: 0,
            body_scroll_offset: 0,
            body_wheel_accumulator: 0.0,
            scale: 1.0,
            busy: false,
            blocking: true,
            opened_at: Instant::now(),
        }
    }
}

impl UniversalModal {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale.clamp(0.5, 3.0);
    }

    pub fn close(&mut self) {
        self.active = false;
        self.buttons.clear();
        self.input = None;
        self.form = None;
        self.submitted_form = None;
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.body_scroll_offset = 0;
        self.body_wheel_accumulator = 0.0;
        self.busy = false;
        self.blocking = true;
    }

    pub fn open(&mut self, spec: ModalSpec) {
        self.active = true;
        self.title = spec.title;
        self.body = spec.body;
        self.meta = spec.meta;
        self.input = spec.input;
        self.form = None;
        self.buttons = if spec.buttons.is_empty() {
            vec![ModalButton::new("Close", "Esc", ModalAction::Close)]
        } else {
            spec.buttons
        };
        self.busy = spec.busy;
        self.blocking = spec.blocking;
        self.opened_at = Instant::now();
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.body_scroll_offset = 0;
        self.body_wheel_accumulator = 0.0;
        self.input_caret = self.focused_input_chars();
    }

    pub fn open_form(&mut self, spec: ModalFormSpec) {
        self.open(ModalSpec {
            title: spec.title.clone(),
            body: String::new(),
            meta: String::new(),
            buttons: vec![
                ModalButton::new(
                    spec.submit_label.clone(),
                    "Enter",
                    ModalAction::ServerFormSubmit,
                ),
                ModalButton::new("Cancel", "Esc", ModalAction::Close),
            ],
            input: None,
            busy: false,
            blocking: true,
        });
        self.form = Some(spec);
        self.form_focus = self
            .form
            .as_ref()
            .and_then(|form| form.fields.iter().position(|field| !field.label.is_empty()))
            .unwrap_or(0);
        self.submitted_form = None;
        self.input_caret = self.focused_input_chars();
    }

    pub fn take_submitted_form(&mut self) -> Option<Vec<(String, String)>> {
        self.submitted_form.take()
    }

    pub fn open_message(&mut self, title: impl Into<String>, body: impl Into<String>) {
        self.open(ModalSpec {
            title: title.into(),
            body: body.into(),
            meta: String::new(),
            input: None,
            buttons: vec![ModalButton::new("Close", "Esc", ModalAction::Close)],
            busy: false,
            blocking: true,
        });
    }

    pub fn is_blocking(&self) -> bool {
        self.active && self.blocking
    }

    pub fn owns_editor_focus(&self) -> bool {
        self.is_blocking() || self.has_input()
    }

    pub fn close_if_non_blocking(&mut self) -> bool {
        if self.active && !self.blocking {
            self.close();
            true
        } else {
            false
        }
    }

    pub fn active_title(&self) -> Option<&str> {
        self.active.then_some(self.title.as_str())
    }

    pub fn has_input(&self) -> bool {
        self.active && (self.input.is_some() || self.form.is_some())
    }

    fn focused_input_value(&self) -> Option<&str> {
        if let Some(form) = self.form.as_ref() {
            return form
                .fields
                .get(self.form_focus)
                .map(|field| field.value.as_str());
        }
        self.input.as_ref().map(|input| input.value.as_str())
    }

    fn focused_input_value_mut(&mut self) -> Option<&mut String> {
        if let Some(form) = self.form.as_mut() {
            return form
                .fields
                .get_mut(self.form_focus)
                .map(|field| &mut field.value);
        }
        self.input.as_mut().map(|input| &mut input.value)
    }

    fn focused_input_chars(&self) -> usize {
        self.focused_input_value()
            .map(|value| value.chars().count())
            .unwrap_or(0)
    }

    fn byte_index_at(value: &str, caret_chars: usize) -> usize {
        value
            .char_indices()
            .nth(caret_chars)
            .map(|(index, _)| index)
            .unwrap_or(value.len())
    }

    pub fn input_caret_chars(&self) -> usize {
        self.input_caret
    }

    pub fn push_input(&mut self, text: &str) {
        let caret = self.input_caret.min(self.focused_input_chars());
        let inserted = text.chars().count();
        let Some(value) = self.focused_input_value_mut() else {
            return;
        };
        let at = Self::byte_index_at(value, caret);
        value.insert_str(at, text);
        self.input_caret = caret + inserted;
    }

    pub fn pop_input(&mut self) {
        let caret = self.input_caret.min(self.focused_input_chars());
        if caret == 0 {
            return;
        }
        let Some(value) = self.focused_input_value_mut() else {
            return;
        };
        let start = Self::byte_index_at(value, caret - 1);
        let end = Self::byte_index_at(value, caret);
        value.replace_range(start..end, "");
        self.input_caret = caret - 1;
    }

    pub fn delete_input(&mut self) {
        let caret = self.input_caret.min(self.focused_input_chars());
        let Some(value) = self.focused_input_value_mut() else {
            return;
        };
        let start = Self::byte_index_at(value, caret);
        let end = Self::byte_index_at(value, caret + 1);
        if start < end {
            value.replace_range(start..end, "");
        }
        self.input_caret = caret;
    }

    pub fn move_input_caret_left(&mut self) {
        let caret = self.input_caret.min(self.focused_input_chars());
        self.input_caret = caret.saturating_sub(1);
    }

    pub fn move_input_caret_right(&mut self) {
        self.input_caret = (self.input_caret + 1).min(self.focused_input_chars());
    }

    pub fn input_caret_to_start(&mut self) {
        self.input_caret = 0;
    }

    pub fn input_caret_to_end(&mut self) {
        self.input_caret = self.focused_input_chars();
    }

    pub fn needs_redraw(&self) -> bool {
        self.active && self.busy
    }

    pub fn focus_next_form_field(&mut self) -> bool {
        let Some(form) = self.form.as_ref() else {
            return false;
        };
        if form.fields.is_empty() {
            return false;
        }
        for offset in 1..=form.fields.len() {
            let index = (self.form_focus + offset) % form.fields.len();
            if !form.fields[index].label.is_empty() {
                self.form_focus = index;
                self.input_caret = self.focused_input_chars();
                return true;
            }
        }
        true
    }

    pub fn move_selection_up(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
            if self.selected_index < self.scroll_offset {
                self.scroll_offset = self.selected_index;
            }
        }
    }

    pub fn move_selection_down(&mut self) {
        if self.selected_index + 1 < self.buttons.len() {
            self.selected_index += 1;
            if self.selected_index >= self.scroll_offset + MAX_VISIBLE_ACTIONS {
                self.scroll_offset = self.selected_index - MAX_VISIBLE_ACTIONS + 1;
            }
        }
    }

    pub fn selected_action(&self) -> Option<ModalAction> {
        let action = self
            .buttons
            .get(self.selected_index)
            .map(|button| button.action.clone())?;
        Some(self.apply_input(action))
    }

    pub fn submit_form(&mut self) -> Option<ModalAction> {
        let form = self.form.as_ref()?;
        self.submitted_form = Some(
            form.fields
                .iter()
                .map(|field| (field.id.clone(), field.value.clone()))
                .collect(),
        );
        Some(ModalAction::ServerFormSubmit)
    }

    pub fn action_for_hint(&self, hint: &str) -> Option<ModalAction> {
        if hint.len() != 1 {
            return None;
        }
        self.buttons
            .iter()
            .find(|button| button.hint.eq_ignore_ascii_case(hint))
            .map(|button| self.apply_input(button.action.clone()))
    }

    pub fn escape_action(&self) -> Option<ModalAction> {
        self.buttons
            .iter()
            .find(|button| button.hint.eq_ignore_ascii_case("Esc"))
            .map(|button| self.apply_input(button.action.clone()))
    }

    fn apply_input(&self, action: ModalAction) -> ModalAction {
        if let Some(input) = self.input.as_ref() {
            action.with_input(input.value.clone())
        } else {
            action
        }
    }

    pub fn set_selected_index(&mut self, index: usize) {
        if index < self.buttons.len() {
            self.selected_index = index;
            if self.selected_index < self.scroll_offset {
                self.scroll_offset = self.selected_index;
            } else if self.selected_index >= self.scroll_offset + MAX_VISIBLE_ACTIONS {
                self.scroll_offset = self.selected_index - MAX_VISIBLE_ACTIONS + 1;
            }
        }
    }

    fn visible_action_count(&self) -> usize {
        self.buttons.len().min(MAX_VISIBLE_ACTIONS)
    }

    fn body_lines(&self) -> Vec<BodyLine> {
        let mut out = Vec::new();
        let mut in_code = false;
        let mut code_lang = Lang::Other;
        let mut code_info = String::new();
        let mut code_lines: Vec<String> = Vec::new();
        for raw in self.body.lines() {
            let trimmed = raw.trim();
            if let Some(info) = trimmed
                .strip_prefix("```")
                .or_else(|| trimmed.strip_prefix("~~~"))
            {
                if in_code {
                    if code_info.trim().eq_ignore_ascii_case("mermaid") {
                        let source = code_lines.join("\n");
                        if let Some(diagram) = parse_mermaid_diagram(&source) {
                            out.push(BodyLine {
                                text: source,
                                kind: BodyLineKind::Mermaid,
                                mermaid: Some(diagram),
                                row_span: 8,
                            });
                        } else {
                            for line in code_lines.drain(..) {
                                out.push(BodyLine::code(line, code_lang));
                            }
                        }
                    } else {
                        for line in code_lines.drain(..) {
                            for wrapped in wrap_preserving(&line, WRAP_COLUMNS) {
                                out.push(BodyLine::code(wrapped, code_lang));
                            }
                        }
                    }
                    code_lines.clear();
                    code_info.clear();
                    in_code = false;
                } else {
                    in_code = true;
                    code_info = info.trim().to_string();
                    code_lang = lang_from_fence(&code_info);
                    code_lines.clear();
                }
                continue;
            }

            if trimmed.is_empty() {
                out.push(BodyLine {
                    text: String::new(),
                    kind: BodyLineKind::Text,
                    mermaid: None,
                    row_span: 1,
                });
                continue;
            }

            if in_code {
                code_lines.push(raw.to_string());
                continue;
            }

            let (kind, content) = if let Some(heading) = markdown_heading(raw) {
                (BodyLineKind::Heading, heading)
            } else if let Some(bullet) = markdown_bullet(raw) {
                (BodyLineKind::Bullet, bullet)
            } else {
                (BodyLineKind::Text, trimmed)
            };

            let mut line = String::new();
            for word in content.split_whitespace() {
                let next_len = if line.is_empty() {
                    word.len()
                } else {
                    line.len() + 1 + word.len()
                };
                if next_len > WRAP_COLUMNS && !line.is_empty() {
                    out.push(BodyLine::new(line, kind));
                    line = word.to_string();
                } else {
                    if !line.is_empty() {
                        line.push(' ');
                    }
                    line.push_str(word);
                }
            }
            if !line.is_empty() {
                out.push(BodyLine::new(line, kind));
            }
        }
        if in_code {
            for line in code_lines {
                for wrapped in wrap_preserving(&line, WRAP_COLUMNS) {
                    out.push(BodyLine::code(wrapped, code_lang));
                }
            }
        }
        out
    }

    fn visible_body_lines(&self) -> Vec<BodyLine> {
        let mut rows = 0usize;
        let mut visible = Vec::new();
        for line in self.body_lines().into_iter().skip(self.body_scroll_offset) {
            if rows > 0 && rows + line.row_span > MAX_BODY_LINES {
                break;
            }
            rows += line.row_span;
            visible.push(line);
            if rows >= MAX_BODY_LINES {
                break;
            }
        }
        visible
    }

    fn visible_body_line_count(&self) -> usize {
        self.visible_body_lines()
            .iter()
            .map(|line| line.row_span)
            .sum::<usize>()
            .min(MAX_BODY_LINES)
            .max(1)
    }

    fn clamp_body_scroll(&mut self) {
        let max_offset = self.body_lines().len().saturating_sub(MAX_BODY_LINES);
        self.body_scroll_offset = self.body_scroll_offset.min(max_offset);
    }

    fn scroll_body_rows(&mut self, rows: i32) {
        if rows == 0 {
            return;
        }
        let max_offset = self.body_lines().len().saturating_sub(MAX_BODY_LINES);
        self.body_scroll_offset = if rows < 0 {
            self.body_scroll_offset
                .saturating_sub(rows.unsigned_abs() as usize)
        } else {
            self.body_scroll_offset
                .saturating_add(rows as usize)
                .min(max_offset)
        };
    }

    pub fn scroll_body_page(&mut self, down: bool) {
        let rows = if down {
            MAX_BODY_LINES as i32
        } else {
            -(MAX_BODY_LINES as i32)
        };
        self.scroll_body_rows(rows);
    }

    fn scroll_actions_rows(&mut self, rows: i32) {
        if rows == 0 || self.buttons.len() <= MAX_VISIBLE_ACTIONS {
            return;
        }
        let max_offset = self.buttons.len().saturating_sub(MAX_VISIBLE_ACTIONS);
        self.scroll_offset = if rows < 0 {
            self.scroll_offset
                .saturating_sub(rows.unsigned_abs() as usize)
        } else {
            self.scroll_offset
                .saturating_add(rows as usize)
                .min(max_offset)
        };
        if self.selected_index < self.scroll_offset {
            self.selected_index = self.scroll_offset;
        } else if self.selected_index >= self.scroll_offset + MAX_VISIBLE_ACTIONS {
            self.selected_index = self.scroll_offset + MAX_VISIBLE_ACTIONS - 1;
        }
    }

    pub fn scroll_at(
        &mut self,
        mouse_x: f32,
        mouse_y: f32,
        window_width: f32,
        scale_factor: f32,
        delta_pixels: f32,
    ) -> bool {
        if !self.active {
            return false;
        }
        let (x, y, w, h) = self.modal_rect(window_width, scale_factor);
        if mouse_x < x || mouse_x > x + w || mouse_y < y || mouse_y > y + h {
            return false;
        }
        if delta_pixels == 0.0 {
            return true;
        }

        let row_h = (BODY_LINE_HEIGHT * self.scale).max(1.0);
        self.body_wheel_accumulator += delta_pixels;
        let mut rows = 0i32;
        while self.body_wheel_accumulator.abs() >= row_h {
            let sign = self.body_wheel_accumulator.signum();
            self.body_wheel_accumulator -= sign * row_h;
            rows += if sign > 0.0 { -1 } else { 1 };
        }

        let actions_y = self.actions_y(y);
        if mouse_y >= actions_y {
            self.scroll_actions_rows(rows);
        } else {
            self.scroll_body_rows(rows);
        }
        true
    }

    fn body_scroll_normalized(&self) -> f32 {
        let max_offset = self.body_lines().len().saturating_sub(MAX_BODY_LINES);
        if max_offset == 0 {
            0.0
        } else {
            self.body_scroll_offset as f32 / max_offset as f32
        }
    }

    fn modal_rect(&self, window_width: f32, scale_factor: f32) -> (f32, f32, f32, f32) {
        let s = self.scale;
        let width = MODAL_WIDTH * s;
        let body_lines = self.visible_body_line_count() as f32;
        let meta_h = if self.meta.is_empty() { 0.0 } else { 20.0 * s };
        let busy_h = if self.busy {
            BUSY_BAR_HEIGHT * s + 12.0 * s
        } else {
            0.0
        };
        let input_h = if self.input.is_some() {
            INPUT_ROW_HEIGHT * s + 12.0 * s
        } else {
            0.0
        };
        let form_h = self
            .form
            .as_ref()
            .map(|form| {
                form.fields
                    .iter()
                    .filter(|field| !field.label.is_empty())
                    .count() as f32
                    * (INPUT_ROW_HEIGHT + 30.0)
                    * s
            })
            .unwrap_or(0.0);
        let actions_h = self.visible_action_count() as f32 * ACTION_ROW_HEIGHT * s;
        let height = MODAL_PADDING * s
            + TITLE_HEIGHT * s
            + 12.0 * s
            + body_lines * BODY_LINE_HEIGHT * s
            + meta_h
            + busy_h
            + input_h
            + form_h
            + 14.0 * s
            + SEPARATOR_HEIGHT
            + 4.0 * s
            + actions_h
            + MODAL_PADDING * s;
        let x = (window_width / scale_factor - width) / 2.0;
        let y = MODAL_MARGIN_TOP * s;
        (x, y, width, height)
    }

    fn actions_y(&self, modal_y: f32) -> f32 {
        let s = self.scale;
        let body_lines = self.visible_body_line_count() as f32;
        let meta_h = if self.meta.is_empty() { 0.0 } else { 20.0 * s };
        let busy_h = if self.busy {
            BUSY_BAR_HEIGHT * s + 12.0 * s
        } else {
            0.0
        };
        let input_h = if self.input.is_some() {
            INPUT_ROW_HEIGHT * s + 12.0 * s
        } else {
            0.0
        };
        let form_h = self
            .form
            .as_ref()
            .map(|form| {
                form.fields
                    .iter()
                    .filter(|field| !field.label.is_empty())
                    .count() as f32
                    * (INPUT_ROW_HEIGHT + 30.0)
                    * s
            })
            .unwrap_or(0.0);
        modal_y
            + MODAL_PADDING * s
            + TITLE_HEIGHT * s
            + 12.0 * s
            + body_lines * BODY_LINE_HEIGHT * s
            + meta_h
            + busy_h
            + input_h
            + form_h
            + 14.0 * s
            + SEPARATOR_HEIGHT
            + 4.0 * s
    }

    pub fn hit_test(
        &self,
        mouse_x: f32,
        mouse_y: f32,
        window_width: f32,
        scale_factor: f32,
    ) -> Result<Option<usize>, ()> {
        let (x, y, w, h) = self.modal_rect(window_width, scale_factor);
        if mouse_x < x || mouse_x > x + w || mouse_y < y || mouse_y > y + h {
            return Err(());
        }
        if let Some(form) = self.form.as_ref() {
            let mut field_y = y
                + MODAL_PADDING * self.scale
                + TITLE_HEIGHT * self.scale
                + 12.0 * self.scale;
            for (index, field) in form.fields.iter().enumerate() {
                if field.label.is_empty() {
                    continue;
                }
                field_y += 20.0 * self.scale;
                let input_h = INPUT_ROW_HEIGHT * self.scale;
                let field_x = x + MODAL_PADDING * self.scale;
                let field_w = w - MODAL_PADDING * self.scale * 2.0;
                if mouse_x >= field_x
                    && mouse_x <= field_x + field_w
                    && mouse_y >= field_y
                    && mouse_y <= field_y + input_h
                {
                    return Ok(Some(self.buttons.len() + index));
                }
                field_y += input_h + 10.0 * self.scale;
            }
        }

        let actions_y = self.actions_y(y);
        if mouse_y < actions_y {
            return Ok(None);
        }

        let row = ((mouse_y - actions_y) / (ACTION_ROW_HEIGHT * self.scale)) as usize;
        let actual = self.scroll_offset + row;
        if row < self.visible_action_count() && actual < self.buttons.len() {
            Ok(Some(actual))
        } else {
            Ok(None)
        }
    }

    pub fn active_rect(&self, window_width: f32, scale_factor: f32) -> Option<[f32; 4]> {
        self.active.then(|| {
            let (x, y, w, h) = self.modal_rect(window_width, scale_factor);
            [x, y, w, h]
        })
    }

    pub fn focus_form_hit(&mut self, hit: usize) -> bool {
        let Some(form) = self.form.as_ref() else {
            return false;
        };
        let index = hit.saturating_sub(self.buttons.len());
        if form
            .fields
            .get(index)
            .is_some_and(|field| !field.label.is_empty())
        {
            self.form_focus = index;
            self.input_caret = self.focused_input_chars();
            true
        } else {
            false
        }
    }

    pub fn render(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        dimensions: (f32, f32, f32),
        theme: &IdeTheme,
    ) {
        if !self.active {
            return;
        }
        self.clamp_body_scroll();

        let (window_width, window_height, scale_factor) = dimensions;
        let s = self.scale;
        let pad = MODAL_PADDING * s;
        let radius = MODAL_CORNER_RADIUS * s;
        let (x, y, w, h) = self.modal_rect(window_width, scale_factor);
        let body_lines = self.visible_body_lines();
        let text_clip = [
            x + pad,
            y + pad,
            (w - pad * 2.0).max(0.0),
            (h - pad * 2.0).max(0.0),
        ];

        // Backdrop intentionally elided — the modal body itself is
        // solid, so the user can still read what's behind the popup
        // (matches the `:` command palette pattern the user asked us
        // to match across overlays).
        let _ = (window_width, window_height, scale_factor);

        sugarloaf.rounded_rect(
            None,
            x,
            y,
            w,
            h,
            theme.f32_alpha(theme.panel_bg(), 0.99),
            DEPTH_BG,
            radius,
            ORDER,
        );

        let title_opts = DrawOpts {
            font_size: TITLE_FONT_SIZE * s,
            color: theme.u8(theme.fg),
            bold: true,
            clip_rect: Some(text_clip),
            ..DrawOpts::default()
        };
        let body_opts = DrawOpts {
            font_size: BODY_FONT_SIZE * s,
            color: theme.u8(theme.dim),
            clip_rect: Some(text_clip),
            ..DrawOpts::default()
        };
        let meta_opts = DrawOpts {
            font_size: META_FONT_SIZE * s,
            color: theme.u8(theme.muted),
            clip_rect: Some(text_clip),
            ..DrawOpts::default()
        };

        let text_x = x + pad;
        let mut text_y = y + pad;
        let text_w = (w - pad * 2.0).max(0.0);
        let title = truncate_to_fit(self.title.as_str(), text_w, sugarloaf, &title_opts);
        sugarloaf
            .text_mut()
            .draw(text_x, text_y, title.as_str(), &title_opts);

        text_y += TITLE_HEIGHT * s + 12.0 * s;
        let body_view_y = text_y;
        let body_view_h = self.visible_body_line_count() as f32 * BODY_LINE_HEIGHT * s;
        let body_clip = [text_x, body_view_y, text_w, body_view_h];
        if body_lines.is_empty() {
            sugarloaf.text_mut().draw(
                text_x,
                text_y,
                "No details available.",
                &body_opts,
            );
            text_y += BODY_LINE_HEIGHT * s;
        } else {
            for line in body_lines {
                self.draw_body_line(
                    sugarloaf, theme, &line, text_x, text_y, text_w, body_clip,
                );
                text_y += BODY_LINE_HEIGHT * s * line.row_span as f32;
            }
        }

        let total_body_lines = self.body_lines().len();
        if total_body_lines > MAX_BODY_LINES {
            if let Some((thumb_y, thumb_h)) = scrollbar::compute_thumb(
                MAX_BODY_LINES,
                total_body_lines,
                body_view_y,
                body_view_h,
                self.body_scroll_normalized(),
            ) {
                let bar_x = text_x + text_w - scrollbar::width();
                scrollbar::draw_track(
                    sugarloaf,
                    bar_x,
                    body_view_y,
                    body_view_h,
                    0.95,
                    DEPTH_ELEMENT + 0.05,
                    ORDER + 2,
                );
                scrollbar::draw_thumb(
                    sugarloaf,
                    bar_x,
                    thumb_y,
                    thumb_h,
                    0.95,
                    false,
                    DEPTH_ELEMENT + 0.05,
                    ORDER + 2,
                );
            }
        }

        if !self.meta.is_empty() {
            let meta = truncate_to_fit(self.meta.as_str(), text_w, sugarloaf, &meta_opts);
            sugarloaf
                .text_mut()
                .draw(text_x, text_y, meta.as_str(), &meta_opts);
            text_y += 20.0 * s;
        }

        if self.busy {
            let track_y = text_y + 4.0 * s;
            let track_w = w - pad * 2.0;
            sugarloaf.rounded_rect(
                None,
                text_x,
                track_y,
                track_w,
                BUSY_BAR_HEIGHT * s,
                theme.f32(theme.hover),
                DEPTH_ELEMENT,
                2.0 * s,
                ORDER,
            );

            let elapsed = Instant::now()
                .saturating_duration_since(self.opened_at)
                .as_millis() as f32;
            let segment_w = (BUSY_BAR_WIDTH * s).min(track_w * 0.6);
            let travel = (track_w - segment_w).max(1.0);
            let phase = ((elapsed / 1100.0) % 1.0) * travel;
            sugarloaf.rounded_rect(
                None,
                text_x + phase,
                track_y,
                segment_w,
                BUSY_BAR_HEIGHT * s,
                theme.f32(theme.accent),
                DEPTH_ELEMENT + 0.01,
                2.0 * s,
                ORDER + 1,
            );
            text_y += BUSY_BAR_HEIGHT * s + 12.0 * s;
        }

        if let Some(input) = self.input.as_ref() {
            let input_y = text_y + 6.0 * s;
            let input_h = INPUT_ROW_HEIGHT * s;
            sugarloaf.rounded_rect(
                None,
                text_x,
                input_y,
                w - pad * 2.0,
                input_h,
                theme.f32(theme.hover),
                DEPTH_ELEMENT,
                5.0 * s,
                ORDER,
            );
            sugarloaf.rounded_rect(
                None,
                text_x,
                input_y,
                2.0 * s,
                input_h,
                theme.f32(theme.accent),
                DEPTH_ELEMENT + 0.01,
                1.0 * s,
                ORDER + 1,
            );

            let is_placeholder = input.value.is_empty();
            let input_text = if is_placeholder {
                input.placeholder.as_str()
            } else {
                input.value.as_str()
            };
            let input_opts = DrawOpts {
                font_size: INPUT_FONT_SIZE * s,
                color: if is_placeholder {
                    theme.u8(theme.muted)
                } else {
                    theme.u8(theme.fg)
                },
                clip_rect: Some([
                    text_x + 12.0 * s,
                    input_y,
                    (w - pad * 2.0 - 24.0 * s).max(0.0),
                    input_h,
                ]),
                ..DrawOpts::default()
            };
            let input_fit = truncate_to_fit(
                input_text,
                (w - pad * 2.0 - 24.0 * s).max(0.0),
                sugarloaf,
                &input_opts,
            );
            let input_text_x = text_x + 12.0 * s;
            let input_text_y = input_y + (input_h - INPUT_FONT_SIZE * s) / 2.0;
            sugarloaf.text_mut().draw(
                input_text_x,
                input_text_y,
                input_fit.as_str(),
                &input_opts,
            );
            let caret_offset = if is_placeholder {
                0.0
            } else {
                // Measure only up to the caret within the fitted text so
                // arrow-key movement is visible mid-string.
                let caret_prefix = input_fit
                    .chars()
                    .take(self.input_caret)
                    .collect::<String>();
                sugarloaf
                    .text_mut()
                    .measure(caret_prefix.as_str(), &input_opts)
            };
            let caret_h = (INPUT_FONT_SIZE * s + 3.0 * s).min(input_h - 8.0 * s);
            sugarloaf.rect(
                None,
                input_text_x + caret_offset + 2.0 * s,
                input_y + (input_h - caret_h) / 2.0,
                INPUT_CARET_WIDTH * s,
                caret_h,
                theme.f32(theme.accent),
                DEPTH_ELEMENT + 0.02,
                ORDER + 2,
            );
        }

        if let Some(form) = self.form.as_ref() {
            for (index, field) in form.fields.iter().enumerate() {
                if field.label.is_empty() {
                    continue;
                }
                let label_opts = DrawOpts {
                    font_size: 12.0 * s,
                    color: theme.u8(theme.muted),
                    ..DrawOpts::default()
                };
                sugarloaf
                    .text_mut()
                    .draw(text_x, text_y, &field.label, &label_opts);
                text_y += 20.0 * s;
                let input_h = INPUT_ROW_HEIGHT * s;
                sugarloaf.rounded_rect(
                    None,
                    text_x,
                    text_y,
                    w - pad * 2.0,
                    input_h,
                    theme.f32(theme.surface),
                    DEPTH_ELEMENT,
                    5.0 * s,
                    ORDER,
                );
                let value = if field.value.is_empty() {
                    field.placeholder.clone()
                } else if field.secret {
                    "•".repeat(field.value.chars().count())
                } else {
                    field.value.clone()
                };
                let opts = DrawOpts {
                    font_size: INPUT_FONT_SIZE * s,
                    color: if field.value.is_empty() {
                        theme.u8(theme.muted)
                    } else {
                        theme.u8(theme.fg)
                    },
                    ..DrawOpts::default()
                };
                sugarloaf.text_mut().draw(
                    text_x + 12.0 * s,
                    text_y + (input_h - INPUT_FONT_SIZE * s) / 2.0,
                    &value,
                    &opts,
                );
                if index == self.form_focus {
                    sugarloaf.rect(
                        None,
                        text_x,
                        text_y,
                        2.0 * s,
                        input_h,
                        theme.f32(theme.accent),
                        DEPTH_ELEMENT + 0.02,
                        ORDER + 1,
                    );
                    // The caret sits after `input_caret` chars of the
                    // rendered value (masked for secret fields), not at
                    // the end — arrow keys move it through the text.
                    let caret_chars =
                        self.input_caret.min(field.value.chars().count());
                    let caret_text = if field.secret {
                        "•".repeat(caret_chars)
                    } else {
                        field.value.chars().take(caret_chars).collect::<String>()
                    };
                    let caret_x = text_x
                        + 12.0 * s
                        + sugarloaf.text_mut().measure(caret_text.as_str(), &opts)
                        + 2.0 * s;
                    let caret_h = (INPUT_FONT_SIZE + 3.0) * s;
                    sugarloaf.rect(
                        None,
                        caret_x,
                        text_y + (input_h - caret_h) * 0.5,
                        INPUT_CARET_WIDTH * s,
                        caret_h,
                        theme.f32(theme.accent),
                        DEPTH_ELEMENT + 0.03,
                        ORDER + 2,
                    );
                }
                text_y += input_h + 10.0 * s;
            }
        }

        let sep_y = self.actions_y(y) - 4.0 * s - SEPARATOR_HEIGHT;
        sugarloaf.rect(
            None,
            x + pad,
            sep_y,
            w - pad * 2.0,
            SEPARATOR_HEIGHT,
            theme.f32(theme.border),
            DEPTH_ELEMENT,
            ORDER,
        );

        let row_x = x + pad;
        let row_w = w - pad * 2.0;
        let row_h = ACTION_ROW_HEIGHT * s;
        let action_clip = [
            row_x,
            self.actions_y(y),
            row_w,
            self.visible_action_count() as f32 * row_h,
        ];
        let hint_opts = DrawOpts {
            font_size: META_FONT_SIZE * s,
            color: theme.u8(theme.muted),
            clip_rect: Some(action_clip),
            ..DrawOpts::default()
        };

        for (display_idx, button) in self
            .buttons
            .iter()
            .skip(self.scroll_offset)
            .take(MAX_VISIBLE_ACTIONS)
            .enumerate()
        {
            let actual_idx = self.scroll_offset + display_idx;
            let row_y = self.actions_y(y) + display_idx as f32 * row_h;
            let selected = actual_idx == self.selected_index;
            let destructive = button.action.is_destructive();
            if selected {
                sugarloaf.rounded_rect(
                    None,
                    row_x,
                    row_y,
                    row_w,
                    row_h,
                    if destructive {
                        theme.f32_alpha(theme.red, 0.14)
                    } else {
                        theme.f32(theme.hover)
                    },
                    DEPTH_ELEMENT,
                    4.0 * s,
                    ORDER,
                );
            }

            let label_x = row_x + 14.0 * s;
            let label_y = row_y + (row_h - ACTION_FONT_SIZE * s) / 2.0;
            let label_opts = DrawOpts {
                font_size: ACTION_FONT_SIZE * s,
                color: if destructive {
                    theme.u8(theme.red)
                } else {
                    theme.u8(theme.fg)
                },
                bold: selected || destructive,
                clip_rect: Some(action_clip),
                ..DrawOpts::default()
            };
            let hint_width = if button.hint.is_empty() {
                0.0
            } else {
                sugarloaf
                    .text_mut()
                    .measure(button.hint.as_str(), &hint_opts)
            };
            let label_budget = (row_w - 28.0 * s - hint_width - 12.0 * s).max(0.0);
            let label = truncate_to_fit(
                button.label.as_str(),
                label_budget,
                sugarloaf,
                &label_opts,
            );
            sugarloaf
                .text_mut()
                .draw(label_x, label_y, label.as_str(), &label_opts);

            if !button.hint.is_empty() {
                let hint_x = row_x + row_w - 14.0 * s - hint_width;
                let hint_y = row_y + (row_h - META_FONT_SIZE * s) / 2.0;
                sugarloaf.text_mut().draw(
                    hint_x,
                    hint_y,
                    button.hint.as_str(),
                    &hint_opts,
                );
            }
        }

        sugarloaf.rect(
            None,
            x,
            y,
            3.0 * s,
            h,
            theme.f32(theme.accent),
            DEPTH_ELEMENT,
            ORDER,
        );
    }

    fn draw_body_line(
        &self,
        sugarloaf: &mut Sugarloaf,
        theme: &IdeTheme,
        line: &BodyLine,
        x: f32,
        y: f32,
        w: f32,
        clip: [f32; 4],
    ) {
        let s = self.scale;
        let line_h = BODY_LINE_HEIGHT * s;
        match line.kind {
            BodyLineKind::Mermaid => {
                let Some(diagram) = line.mermaid.as_ref() else {
                    return;
                };
                let h = line_h * line.row_span as f32 - 8.0 * s;
                let rect = [x, y + 4.0 * s, w, h.max(line_h)];
                sugarloaf.rounded_rect(
                    None,
                    rect[0],
                    rect[1],
                    rect[2],
                    rect[3],
                    theme.f32_alpha(theme.black, 0.42),
                    DEPTH_ELEMENT,
                    10.0 * s,
                    ORDER,
                );
                let scene = mermaid_scene(diagram, theme, s);
                let Some(bounds) = scene.bounds() else {
                    return;
                };
                let pad = 10.0 * s;
                let avail_w = (rect[2] - pad * 2.0).max(1.0);
                let avail_h = (rect[3] - pad * 2.0).max(1.0);
                let zoom = (avail_w / bounds.width().max(1.0))
                    .min(avail_h / bounds.height().max(1.0))
                    .min(2.0);
                let center = bounds.center();
                let camera = Camera {
                    pan: Vec2::new(
                        rect[0] + rect[2] * 0.5 - center.x * zoom,
                        rect[1] + rect[3] * 0.5 - center.y * zoom,
                    ),
                    zoom,
                };
                render_scene(sugarloaf, &scene, &camera, clip, DEPTH_ELEMENT, ORDER + 1);
            }
            BodyLineKind::Heading => {
                let opts = DrawOpts {
                    font_size: BODY_FONT_SIZE * s,
                    color: theme.u8(theme.fg),
                    bold: true,
                    clip_rect: Some(clip),
                    ..DrawOpts::default()
                };
                let text = truncate_to_fit(&line.text, w, sugarloaf, &opts);
                sugarloaf.text_mut().draw(x, y, &text, &opts);
            }
            BodyLineKind::Bullet => {
                let bullet_opts = DrawOpts {
                    font_size: BODY_FONT_SIZE * s,
                    color: theme.u8(theme.accent),
                    bold: true,
                    clip_rect: Some(clip),
                    ..DrawOpts::default()
                };
                let text_opts = DrawOpts {
                    font_size: BODY_FONT_SIZE * s,
                    color: theme.u8(theme.dim),
                    clip_rect: Some(clip),
                    ..DrawOpts::default()
                };
                let bullet_w = sugarloaf.text_mut().draw(x, y, "•", &bullet_opts);
                let text_x = x + bullet_w + 8.0 * s;
                let text = truncate_to_fit(
                    &line.text,
                    (w - bullet_w - 8.0 * s).max(0.0),
                    sugarloaf,
                    &text_opts,
                );
                sugarloaf.text_mut().draw(text_x, y, &text, &text_opts);
            }
            BodyLineKind::Code(lang) => {
                sugarloaf.rounded_rect(
                    None,
                    x,
                    y - 2.0 * s,
                    w,
                    line_h,
                    theme.f32_alpha(theme.surface, 0.55),
                    DEPTH_ELEMENT,
                    3.0 * s,
                    ORDER,
                );
                let mut tx = x + 8.0 * s;
                for (tok, slice) in highlight_line(&line.text, lang) {
                    let opts = DrawOpts {
                        font_size: BODY_FONT_SIZE * s,
                        color: syn_color(tok, theme, false),
                        clip_rect: Some(clip),
                        ..DrawOpts::default()
                    };
                    tx += sugarloaf.text_mut().draw(tx, y, slice, &opts);
                    if tx > x + w {
                        break;
                    }
                }
            }
            BodyLineKind::Text => {
                let opts = DrawOpts {
                    font_size: BODY_FONT_SIZE * s,
                    color: theme.u8(theme.dim),
                    clip_rect: Some(clip),
                    ..DrawOpts::default()
                };
                let text = truncate_to_fit(&line.text, w, sugarloaf, &opts);
                sugarloaf.text_mut().draw(x, y, &text, &opts);
            }
        }
    }
}

fn truncate_to_fit(
    text: &str,
    available_w: f32,
    sugarloaf: &mut Sugarloaf,
    opts: &DrawOpts,
) -> String {
    if available_w <= 0.0 || text.is_empty() {
        return String::new();
    }
    if sugarloaf.text_mut().measure(text, opts) <= available_w {
        return text.to_string();
    }
    if sugarloaf.text_mut().measure("…", opts) >= available_w {
        return "…".to_string();
    }

    let chars: Vec<char> = text.chars().collect();
    let mut lo = 0usize;
    let mut hi = chars.len();
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let mut candidate: String = chars[..mid].iter().collect();
        candidate.push('…');
        if sugarloaf.text_mut().measure(&candidate, opts) <= available_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }

    let mut out: String = chars[..lo].iter().collect();
    out.push('…');
    out
}

fn markdown_heading(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let rest = trimmed
        .strip_prefix('#')?
        .trim_start_matches('#')
        .trim_start();
    (!rest.is_empty()).then_some(rest)
}

fn markdown_bullet(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("• "))
}

fn wrap_preserving(line: &str, max_chars: usize) -> Vec<String> {
    if max_chars == 0 || line.chars().count() <= max_chars {
        return vec![line.to_string()];
    }
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in line.chars() {
        current.push(ch);
        if current.chars().count() >= max_chars {
            out.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn lang_from_fence(label: &str) -> Lang {
    let label = label
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_start_matches('.')
        .to_ascii_lowercase();
    match label.as_str() {
        "rs" | "rust" => Lang::Rust,
        "js" | "javascript" | "mjs" | "cjs" => Lang::Javascript,
        "jsx" => Lang::Jsx,
        "ts" | "typescript" => Lang::Typescript,
        "tsx" => Lang::Tsx,
        "py" | "python" => Lang::Python,
        "go" | "golang" => Lang::Go,
        "lua" => Lang::Lua,
        "toml" => Lang::Toml,
        "json" | "jsonc" => Lang::Json,
        "md" | "markdown" => Lang::Markdown,
        _ => Lang::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_form_skips_hidden_id_and_submits_structured_values() {
        let mut modal = UniversalModal::new();
        modal.open_form(ModalFormSpec {
            title: "Edit server".into(),
            fields: vec![
                ModalFormField {
                    id: "server_id".into(),
                    label: String::new(),
                    value: "id-1".into(),
                    placeholder: String::new(),
                    secret: true,
                },
                ModalFormField {
                    id: "address".into(),
                    label: "Address".into(),
                    value: String::new(),
                    placeholder: "https://host".into(),
                    secret: false,
                },
                ModalFormField {
                    id: "token".into(),
                    label: "Token".into(),
                    value: String::new(),
                    placeholder: "token".into(),
                    secret: true,
                },
            ],
            submit_label: "Save".into(),
        });
        modal.push_input("https://work.example");
        assert!(modal.focus_next_form_field());
        modal.push_input("secret");
        assert_eq!(modal.submit_form(), Some(ModalAction::ServerFormSubmit));
        let values = modal.take_submitted_form().unwrap();
        assert!(values.contains(&("server_id".into(), "id-1".into())));
        assert!(values.contains(&("address".into(), "https://work.example".into())));
        assert!(values.contains(&("token".into(), "secret".into())));
    }

    #[test]
    fn server_form_hit_tests_fields_and_buttons() {
        let mut modal = UniversalModal::new();
        modal.open_form(ModalFormSpec {
            title: "Add server".into(),
            fields: vec![ModalFormField {
                id: "address".into(),
                label: "Server address".into(),
                value: String::new(),
                placeholder: "http://localhost:7878".into(),
                secret: false,
            }],
            submit_label: "Add server".into(),
        });
        let (x, y, _, _) = modal.modal_rect(1200.0, 1.0);
        let field_y = y + MODAL_PADDING + TITLE_HEIGHT + 12.0 + 20.0;
        let field_hit =
            modal.hit_test(x + MODAL_PADDING + 20.0, field_y + 10.0, 1200.0, 1.0);
        assert_eq!(field_hit, Ok(Some(modal.buttons.len())));

        let actions_y = modal.actions_y(y);
        assert_eq!(
            modal.hit_test(x + MODAL_PADDING + 20.0, actions_y + 10.0, 1200.0, 1.0),
            Ok(Some(0))
        );
        assert_eq!(
            modal.hit_test(
                x + MODAL_PADDING + 20.0,
                actions_y + ACTION_ROW_HEIGHT + 10.0,
                1200.0,
                1.0,
            ),
            Ok(Some(1))
        );
    }

    #[test]
    fn body_lines_upgrade_mermaid_fence_to_diagram_line() {
        let mut modal = UniversalModal::new();
        modal.open(ModalSpec {
            title: "Diagram".into(),
            body: "```mermaid\nflowchart LR\nA[Start] --> B{Done}\n```".into(),
            meta: String::new(),
            input: None,
            buttons: Vec::new(),
            busy: false,
            blocking: false,
        });

        let lines = modal.body_lines();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].kind, BodyLineKind::Mermaid);
        assert!(lines[0].mermaid.is_some());
        assert!(lines[0].row_span > 1);
    }
}
