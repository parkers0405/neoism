//! Shared desktop/wasm LSP policy, with I/O queued for the TypeScript daemon host.
use neoism_protocol::editor::{
    EditorClientMessage, EditorLspAction, EditorServerMessage,
};
use neoism_ui::{
    editor::code::{
        buffer::CodeTextEdit,
        lsp_session::{self as lsp, CodeLspUi, LspUiEvent},
        CodeDiagnosticSeverity, CodePane,
    },
    services::{IoError, LspRequest, LspService},
    panels::hover_popup::{self, HoverPopupLayout},
    primitives::{ide_theme::IdeTheme, paint_record::RecordingPainter},
};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
#[derive(Default)]
struct Requests(Mutex<Vec<LspRequest>>);
impl LspService for Requests {
    fn request(&self, request: LspRequest) -> Result<(), IoError> {
        self.0.lock().unwrap().push(request);
        Ok(())
    }
}
pub struct EditorLsp {
    pub session: CodeLspUi,
    requests: Arc<Requests>,
    pub surface: String,
    pub open_paths: Vec<PathBuf>,
    popup: neoism_ui::panels::completion_menu::CompletionMenu,
    viewport: [f32; 4],
    window: [f32; 2],
    scale: f32,
    hover_painter: Option<RecordingPainter>,
    hover_font: (String,u16),
    hover_theme: String,
    hover_pointer: (f32,f32),
    hover_effects: Vec<Value>,
    overlay_viewport: Option<[f32;4]>,
}
impl Default for EditorLsp {
    fn default() -> Self {
        Self {
            session: Default::default(),
            requests: Default::default(),
            surface: String::new(),
            open_paths: vec![],
            popup: neoism_ui::panels::completion_menu::CompletionMenu::new(),
            viewport: [0.0, 0.0, 800.0, 600.0],
            window: [1440.0, 900.0],
            scale: 1.0,
            hover_painter: None,
            hover_font: ("Cascadia Code NF".into(),400),
            hover_theme: "pastel_dark".into(),
            hover_pointer: (-1.0,-1.0),
            hover_effects: vec![],
            overlay_viewport: None,
        }
    }
}
impl EditorLsp {
    pub fn enable(&mut self, surface: String, open_paths: Vec<PathBuf>) {
        self.session = Default::default();
        self.surface = surface;
        self.open_paths = open_paths;
        self.session.install_service(self.requests.clone());
    }
    pub fn frame(&mut self, pane: &CodePane) -> Value {
        let requests: Vec<Value> = self
            .requests
            .0
            .lock()
            .unwrap()
            .drain(..)
            .filter_map(wire_message)
            .map(|mut wire| {
                match &mut wire {
                    EditorClientMessage::OpenBuffer { surface_id, .. }
                    | EditorClientMessage::DidSave { surface_id, .. } => {
                        *surface_id = Some(self.surface.clone())
                    }
                    EditorClientMessage::LspQueryAt {
                        surface_id,
                        buffer_text,
                        open_paths,
                        ..
                    }
                    | EditorClientMessage::ApplyLspCodeActionAt {
                        surface_id,
                        buffer_text,
                        open_paths,
                        ..
                    } => {
                        *surface_id = Some(self.surface.clone());
                        *buffer_text = Some(pane.buffer.text_for_disk());
                        if !open_paths.is_empty() {
                            *open_paths = if self.open_paths.is_empty() {
                                vec![pane.path.clone()]
                            } else {
                                self.open_paths.clone()
                            };
                        }
                    }
                    _ => {}
                };
                serde_json::to_value(wire).unwrap()
            })
            .collect();
        let mut events:Vec<Value>=self.session.take_events().into_iter().map(|event|match event {
      LspUiEvent::Toast{message,level}=>json!({"kind":"toast","message":message,"level":format!("{:?}",level)}),
      LspUiEvent::OpenLocation{path,line,col}=>json!({"kind":"location","path":path,"line":line,"col":col}),
      LspUiEvent::OpenReferences{root,rows}=>json!({"kind":"references","root":root,"rows":rows.into_iter().map(|r|json!({"path":r.path,"line":r.line,"column":r.column,"text":r.text})).collect::<Vec<_>>()}),
      LspUiEvent::ApplyEditsToFile{path,edits}=>json!({"kind":"edits","path":path,"edits":edits.into_iter().map(|e|json!({"start_line":e.start_line,"start_col":e.start_col,"end_line":e.end_line,"end_col":e.end_col,"text":e.text})).collect::<Vec<_>>()}),
      LspUiEvent::SaveAfterFormat=>json!({"kind":"save"}),
      LspUiEvent::OpenRenamePrompt{word}=>json!({"kind":"rename","word":word}),
    }).collect();
        events.append(&mut self.hover_effects);
        let area=self.overlay_viewport.unwrap_or([0.0,0.0,self.window[0],self.window[1]]);
        let global_rect=|mut rect:[f32;4]|{rect[0]+=area[0];rect[1]+=area[1];rect};
        let menu=self.session.actions.as_ref().filter(|s|!s.display.items.is_empty()).map(|s|(&s.display,"actions",s.line,s.col))
      .or_else(||self.session.completion.as_ref().filter(|s|!s.display.items.is_empty()).map(|s|(&s.display,"completion",s.line,s.anchor_col)))
      .and_then(|(m,kind,line,col)| {
       let (x,y)=position(pane,line,col);
       let mut menu=m.clone();menu.anchor_row=0;menu.anchor_col=0;
       let anchor=neoism_ui::panels::completion_menu::EditorAnchor{cell_w:pane.geometry.cell_w,cell_h:pane.geometry.row_h,
        panel_left_phys:self.viewport[0]+x-area[0],panel_top_phys:self.viewport[1]+y-area[1],
        panel_lines:((self.viewport[3]-y)/pane.geometry.row_h.max(1.0)).floor().max(1.0) as u32,editor_focused:true};
       let layout=self.popup.host_frame(&menu,&anchor,(area[2],area[3],1.0))?;
       Some(json!({"kind":kind,"selected":m.selected,"rect":global_rect(layout.rect),"first":layout.first,"visibleRows":layout.visible_rows,"offset":layout.offset,
        "rowHeight":layout.row_height,"fontSize":layout.font_size,"padX":layout.padding_x,"kindWidth":layout.kind_width,"menuWidth":layout.menu_width,"animating":layout.animating,
        "items":m.items.iter().map(|i|json!({"word":i.word,"kind":i.kind,"menu":i.menu,"info":i.info})).collect::<Vec<_>>()}))
      });
        let hover = if menu.is_none(){self.session.hover.as_mut().filter(|h| !h.lines.is_empty()&&h.path==pane.path&&(h.from_mouse||(h.line==pane.buffer.cursor_line&&h.col==pane.buffer.cursor_col))).and_then(|h| {
                let (x, y) = position(pane, h.line, h.col);
                let painter=self.hover_painter.get_or_insert_with(||RecordingPainter::new(&self.hover_font.0,self.hover_font.1,vec![]));
                painter.begin_frame();
                hover_popup::render(painter,&h.lines,HoverPopupLayout{
                    anchor_x:self.viewport[0]+x-area[0],anchor_y:self.viewport[1]+y-area[1],cell_h:pane.geometry.row_h,
                    window_w:area[2],window_h:area[3],scale:self.scale,
                    document_language:neoism_ui::syntax::Lang::from_path(&h.path.to_string_lossy()),
                },&IdeTheme::by_name(&self.hover_theme),&mut h.interaction);
                let rect=global_rect(h.interaction.bounds()?);
                Some(json!({"line":h.line,"col":h.col,"lines":h.lines,"x":x,"y":y,"rect":rect,
                    "viewport":global_rect(h.interaction.viewport()),"paintOrigin":[area[0],area[1]],"paintSize":[area[2],area[3]],"scrollY":h.interaction.scroll_y(),"maxScroll":h.interaction.max_scroll(),
                    "selectedText":h.interaction.selected_text(),"dragging":h.interaction.is_dragging(),
                    "hoveredLink":h.interaction.link_at(self.hover_pointer.0,self.hover_pointer.1),"commands":painter.commands()}))
            })}else{None};
        json!({"menu":menu,"hover":hover,"requests":requests,"events":events,"diagnostics":self.session.diagnostics.counts_for(&pane.path)})
    }
    pub fn key(
        &mut self,
        pane: &mut CodePane,
        key: &str,
        ch: &str,
        ctrl: bool,
        shift: bool,
    ) -> bool {
        if !self.session.has_service() {
            return false;
        }
        if pane.input_mode == neoism_ui::editor::code::CodeInputMode::Vim
            && pane.buffer.mode == neoism_ui::editor::code::CodeMode::Normal
            && !ctrl
        {
            if pane.buffer.vim.pending.is_empty() && ch == "K" {
                self.session.request_hover(pane);
                return true;
            }
            if pane.buffer.vim.pending.showcmd() == "g" && matches!(ch, "d" | "r") {
                pane.buffer.vim.clear_pending();
                if ch == "d" {
                    self.session.request_definition_at(
                        pane,
                        pane.buffer.cursor_line,
                        pane.buffer.cursor_col,
                    );
                } else {
                    self.session.request_references(pane);
                }
                return true;
            }
        }
        if key == "escape" && self.session.hover.is_some() {
            self.session.dismiss_popups();
            return true;
        }
        let actions = self.session.action_menu_open();
        let completion = self.session.completion_menu_open();
        if !actions && !completion {
            return false;
        }
        let delta = match key {
            "up" => Some(-1),
            "down" => Some(1),
            "p" if ctrl => Some(-1),
            "n" if ctrl => Some(1),
            _ => None,
        };
        if let Some(delta) = delta {
            if actions {
                self.session.move_action_selection(delta)
            } else {
                self.session.move_completion_selection(delta)
            };
            return true;
        }
        if key == "escape" {
            self.session.dismiss_popups();
            return true;
        }
        if key == "enter" || key == "tab" && !shift && completion && !actions {
            return if actions {
                self.session.apply_selected_action(pane)
            } else {
                self.session.accept_completion(pane)
            };
        }
        false
    }
    pub fn geometry(&mut self, v: &Value) {
        let n = |key: &str, default: f32| v[key].as_f64().map_or(default, |n| n as f32);
        self.viewport = [
            n("x", 0.0),
            n("y", 0.0),
            n("width", 800.0),
            n("height", 600.0),
        ];
        self.window = [n("windowWidth", 1440.0), n("windowHeight", 900.0)];
        self.scale = n("scale", 1.0);
        self.popup.set_scale(self.scale);
    }
    pub fn hover_configure(&mut self,v:&Value){
        let font=(v["fontFamily"].as_str().unwrap_or("Cascadia Code NF").to_owned(),v["fontWeight"].as_u64().unwrap_or(400) as u16);
        if self.hover_font!=font {self.hover_font=font;self.hover_painter=None;if let Some(card)=self.session.hover.as_mut(){card.interaction.invalidate_layout();}}
        self.hover_theme=v["theme"].as_str().unwrap_or("pastel_dark").to_owned();
        self.overlay_viewport=v["viewport"].as_array().filter(|a|a.len()==4).and_then(|a|Some([a[0].as_f64()? as f32,a[1].as_f64()? as f32,a[2].as_f64()? as f32,a[3].as_f64()? as f32])).filter(|a|a.iter().all(|n|n.is_finite())&&a[2]>0.0&&a[3]>0.0);
    }
    pub fn pointer(&mut self,pane:&CodePane,pos:Option<(f32,f32)>){
        let area=self.overlay_viewport.unwrap_or([0.0,0.0,self.window[0],self.window[1]]);
        self.session.note_mouse_move_with_popup_offset(pane,pos,(self.viewport[0]-area[0],self.viewport[1]-area[1]));
    }
    pub fn hover_input(&mut self,v:&Value)->Result<(),String>{
        let area=self.overlay_viewport.unwrap_or([0.0,0.0,self.window[0],self.window[1]]);
        let x=v["x"].as_f64().map(|n|n as f32-area[0]).unwrap_or(self.hover_pointer.0);
        let y=v["y"].as_f64().map(|n|n as f32-area[1]).unwrap_or(self.hover_pointer.1);
        self.hover_pointer=(x,y);
        if let Some(card)=self.session.hover.as_mut(){
            match v["phase"].as_str().unwrap_or(""){
                "down"=>{card.interaction.begin_selection(x,y);},
                "move"=>{card.interaction.pointer_move(x,y);},
                "up"=>{card.interaction.pointer_move(x,y);card.interaction.end_selection();},
                "wheel"=>{card.interaction.scroll(x,y,v["delta"].as_f64().unwrap_or(0.0) as f32);},
                phase=>return Err(format!("Unknown hover input {phase}")),
            }
            if let Some(url)=card.interaction.take_activated_link(){self.hover_effects.push(json!({"kind":"open-url","url":url}));}
        }
        Ok(())
    }
    pub fn action(&mut self, pane: &mut CodePane, v: &Value) -> Result<(), String> {
        match v["action"].as_str().unwrap_or("") {
            "completion" => self.session.request_completion(pane, None),
            "hover" => self.session.request_hover(pane),
            "signature" => self.session.request_signature_help(pane),
            "definition" => self.session.request_definition_at(
                pane,
                pane.buffer.cursor_line,
                pane.buffer.cursor_col,
            ),
            "references" => self.session.request_references(pane),
            "actions" => self.session.request_code_actions(pane),
            "rename" => self.session.open_rename_prompt(pane),
            "rename-submit" => self
                .session
                .submit_rename(pane, v["name"].as_str().unwrap_or("").to_owned()),
            "format" => {
                self.session.queue_format_then_save(pane);
            }
            "dismiss" => self.session.dismiss_popups(),
            _ => return Err("Unknown LSP action".into()),
        };
        Ok(())
    }
    pub fn wheel(&mut self, delta: f32, precise: bool) {
        use neoism_ui::panels::completion_menu::ScrollDelta;
        let delta = if precise {
            ScrollDelta::Pixels { x: 0.0, y: delta }
        } else {
            ScrollDelta::Lines { x: 0.0, y: delta }
        };
        let steps = self.popup.wheel_steps(&delta);
        if self.session.action_menu_open() {
            self.session.move_action_selection(steps as isize);
        } else {
            self.session.move_completion_selection(steps as isize);
        }
    }
    pub fn select(&mut self, pane: &mut CodePane, index: usize) {
        if let Some(menu) = self.session.actions.as_ref() {
            let current = menu.selected;
            self.session
                .move_action_selection(index as isize - current as isize);
            self.session.apply_selected_action(pane);
        } else if let Some(menu) = self.session.completion.as_ref() {
            let current = menu.selected;
            self.session
                .move_completion_selection(index as isize - current as isize);
            self.session.accept_completion(pane);
        }
    }
    pub fn reply(&mut self, pane: &mut CodePane, message: Value) -> Result<(), String> {
        let wire: EditorServerMessage = serde_json::from_value(message)
            .map_err(|e| format!("Invalid LSP reply: {e}"))?;
        match wire {
            EditorServerMessage::LspHoverResult { seq, contents, .. } => {
                if let Some(path) = self.session.hover.as_ref().map(|h| h.path.clone()) {
                    self.session.on_hover_result(seq, &path, &[contents]);
                }
            }
            EditorServerMessage::LspCompletions { seq, items, .. } => {
                let items = items
                    .into_iter()
                    .map(|i| lsp::LspCompletionData {
                        server_id: i.server_id,
                        label: i.label,
                        kind: i.kind,
                        detail: i.detail,
                        documentation: i.documentation,
                        insert_text: i.insert_text,
                        filter_text: i.filter_text,
                        sort_text: i.sort_text,
                        preselect: i.preselect,
                        payload: i.payload.unwrap_or(Value::Null),
                    })
                    .collect();
                self.session
                    .on_completion_result(pane, seq, &pane.path, items);
            }
            EditorServerMessage::LspQueryResult {
                seq,
                action,
                root,
                locations,
                references,
                code_actions,
                edits,
                applied_files,
                ran_command,
                title,
                ..
            } => {
                let edits = edits
                    .into_iter()
                    .map(|f| {
                        (
                            f.path,
                            f.edits
                                .into_iter()
                                .map(|e| CodeTextEdit {
                                    start_line: e.start_line as usize,
                                    start_col: e.start_col as usize,
                                    end_line: e.end_line as usize,
                                    end_col: e.end_col as usize,
                                    text: e.new_text,
                                })
                                .collect(),
                        )
                    })
                    .collect();
                if !self.session.on_workspace_edit_result(
                    pane,
                    seq,
                    &title,
                    edits,
                    applied_files.len(),
                    ran_command,
                ) {
                    match action {
                        EditorLspAction::Definition => {
                            self.session.on_definition_result(
                                seq,
                                locations
                                    .into_iter()
                                    .map(|l| lsp::LspLocationData {
                                        path: lsp::file_uri_to_path(&l.uri)
                                            .unwrap_or_else(|| PathBuf::from(l.uri)),
                                        line: l.line as usize,
                                        col: l.character as usize,
                                    })
                                    .collect(),
                            );
                        }
                        EditorLspAction::References => {
                            self.session.on_references_result(
                                seq,
                                root.unwrap_or_default(),
                                references
                                    .into_iter()
                                    .map(|r| neoism_ui::panels::finder::ReferenceRow {
                                        path: r.path,
                                        line: r.line,
                                        column: r.column,
                                        text: r.text,
                                    })
                                    .collect(),
                            );
                        }
                        EditorLspAction::CodeActions => {
                            self.session.on_code_actions_result(
                                seq,
                                &pane.path,
                                code_actions
                                    .into_iter()
                                    .map(|a| lsp::LspCodeActionData {
                                        server_id: a.server_id.clone(),
                                        title: a.title.clone(),
                                        kind: a.kind.clone().unwrap_or_default(),
                                        action: serde_json::json!({"gpuixWireAction":a}),
                                    })
                                    .collect(),
                            );
                        }
                        _ => {}
                    }
                }
            }
            EditorServerMessage::Diagnostics {
                file_path, items, ..
            } => {
                let path = file_path.unwrap_or_else(|| pane.path.clone());
                self.session.diagnostics.publish(
                    path,
                    "daemon".into(),
                    items
                        .into_iter()
                        .map(|d| lsp::LspStoredDiagnostic {
                            line: d.line as usize,
                            col: stored_utf16_col(pane, d.line as usize, d.col as usize),
                            end_line: d.end_line as usize,
                            end_col: stored_utf16_col(
                                pane,
                                d.end_line as usize,
                                d.end_col as usize,
                            ),
                            severity: match d.severity {
                                neoism_protocol::editor::DiagnosticSeverity::Error => {
                                    CodeDiagnosticSeverity::Error
                                }
                                neoism_protocol::editor::DiagnosticSeverity::Warn => {
                                    CodeDiagnosticSeverity::Warn
                                }
                                neoism_protocol::editor::DiagnosticSeverity::Info => {
                                    CodeDiagnosticSeverity::Info
                                }
                                _ => CodeDiagnosticSeverity::Hint,
                            },
                            message: d.message,
                            source: d.source,
                        })
                        .collect(),
                );
                self.session.diagnostics.fold_into_pane(pane);
            }
            EditorServerMessage::Error { message, .. } => {
                self.session.on_request_error(&message)
            }
            _ => {}
        }
        Ok(())
    }
}
pub fn text_edits(value: &Value) -> Result<Vec<CodeTextEdit>, String> {
    value
        .as_array()
        .ok_or_else(|| "Expected text edits".to_owned())?
        .iter()
        .map(|e| {
            let number = |key: &str| {
                e[key]
                    .as_u64()
                    .map(|n| n as usize)
                    .ok_or_else(|| format!("Invalid edit {key}"))
            };
            Ok(CodeTextEdit {
                start_line: number("start_line")?,
                start_col: number("start_col")?,
                end_line: number("end_line")?,
                end_col: number("end_col")?,
                text: e["text"].as_str().ok_or("Missing edit text")?.to_owned(),
            })
        })
        .collect()
}
// The daemon engine speaks UTF-8 byte columns. The existing shared
// diagnostics store predates that boundary and folds UTF-16 columns, so convert
// exactly once on ingress (query/edit/completion positions stay in bytes).
fn stored_utf16_col(pane: &CodePane, line: usize, byte: usize) -> usize {
    let Some(source) = pane.buffer.lines.get(line) else {
        return byte;
    };
    let mut end = byte.min(source.len());
    while !source.is_char_boundary(end) {
        end -= 1;
    }
    source[..end].encode_utf16().count()
}
fn position(pane: &CodePane, line: usize, col: usize) -> (f32, f32) {
    let source = pane.buffer.lines.get(line).map_or("", String::as_str);
    let (segment, column) = pane.geometry.wrap.visual_position(
        line,
        source,
        col.min(source.len()),
        neoism_ui::editor::code::layout::TAB_DISPLAY_WIDTH,
    );
    let row = pane.geometry.wrap.first_row_of_line(line) + segment;
    (
        pane.geometry.text_x + column as f32 * pane.geometry.cell_w - pane.scroll_x,
        row as f32 * pane.geometry.row_h - pane.scroll_y,
    )
}
fn wire_message(
    request: neoism_ui::services::LspRequest,
) -> Option<neoism_protocol::editor::EditorClientMessage> {
    use neoism_protocol::editor::{
        EditorClientMessage as Wire, EditorLspAction as Action, EditorLspCodeAction,
    };
    use neoism_ui::services::LspRequest;
    let query =
        |seq, action, path: std::path::PathBuf, line, character, text, open: bool| {
            Wire::LspQueryAt {
                buffer_text: None,
                seq,
                action,
                open_paths: if open { vec![path.clone()] } else { Vec::new() },
                path,
                line,
                character,
                text,
                surface_id: None,
            }
        };
    Some(match request {
        LspRequest::Sync { path, text, .. } => Wire::OpenBuffer {
            path,
            text: Some(text),
            line: None,
            character: None,
            surface_id: None,
        },
        LspRequest::SaveNotify { path } => Wire::DidSave {
            path,
            surface_id: None,
        },
        LspRequest::Completion {
            path,
            line,
            character,
            trigger,
            seq,
        } => query(
            seq,
            Action::Completion,
            path,
            line,
            character,
            trigger,
            false,
        ),
        LspRequest::Hover {
            path,
            line,
            character,
            seq,
        } => query(seq, Action::Hover, path, line, character, None, false),
        LspRequest::SignatureHelp {
            path,
            line,
            character,
            seq,
        } => query(
            seq,
            Action::SignatureHelp,
            path,
            line,
            character,
            None,
            false,
        ),
        LspRequest::Definition {
            path,
            line,
            character,
            seq,
        } => query(seq, Action::Definition, path, line, character, None, false),
        LspRequest::References {
            path,
            line,
            character,
            seq,
        } => query(seq, Action::References, path, line, character, None, false),
        LspRequest::CodeActions {
            path,
            line,
            character,
            seq,
        } => query(seq, Action::CodeActions, path, line, character, None, false),
        LspRequest::Rename {
            path,
            line,
            character,
            new_name,
            seq,
        } => query(
            seq,
            Action::Rename,
            path,
            line,
            character,
            Some(new_name),
            true,
        ),
        LspRequest::Format { path, seq, .. } => {
            query(seq, Action::Format, path, 0, 0, None, true)
        }
        LspRequest::ApplyCodeAction {
            path,
            server_id,
            title,
            action,
            seq,
        } => Wire::ApplyLspCodeActionAt {
            buffer_text: None,
            seq,
            open_paths: vec![path.clone()],
            action: if let Some(wire) = action.get("gpuixWireAction").cloned() {
                serde_json::from_value(wire).ok()?
            } else {
                EditorLspCodeAction {
                    server_id,
                    file_path: path,
                    document_revision: String::new(),
                    title,
                    kind: None,
                    preferred: false,
                    disabled_reason: None,
                    payload: action,
                }
            },
            surface_id: None,
        },
    })
}
